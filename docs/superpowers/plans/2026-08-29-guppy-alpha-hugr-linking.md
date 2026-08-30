# Guppy and Alpha HUGR Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `alphalang.link_alpha_function`, which replaces a public Guppy declaration or dummy function with a signature-compatible Alpha-generated HUGR function while preserving the Guppy entry point and call sites.

**Architecture:** A focused Rust module in the PyO3 crate deserializes the two artifacts, promotes Alpha's standalone DFG to a public module-level `FuncDefn`, and invokes HUGR's name linker with `OnMultiDefn::UseSource`. A thin Python adapter accepts and returns a standard `hugr.package.Package` through its serialization protocol, so Alpha does not depend on Guppy and importing `alphalang` does not require the HUGR Python package.

**Tech Stack:** Rust 2021, HUGR `=0.29.3`, PyO3 `0.23`, Python 3.9+, HUGR Python `0.18.5`, Guppylang `1.0.1`, pytest, Cargo.

**Spec:** `docs/superpowers/specs/2026-08-29-guppy-alpha-hugr-linking-design.md`

## Global Constraints

- Work directly on branch `louis/hugr`; do not create a worktree.
- The public first surface is Python: `alphalang.link_alpha_function(wrapper, implementation, symbol="foo")`.
- Accept a package containing exactly one wrapper module and an Alpha text envelope containing exactly one DFG-rooted HUGR.
- Replace exactly one monomorphic public `FuncDecl` or public/private `FuncDefn`; reject zero,
    duplicate, private declarations, and polymorphic targets.
- Require exact HUGR signature equality except for concrete ordinary-array/borrow-array boundaries
    with equal sizes and element types; do not reorder ports or instantiate type parameters.
- Preserve the Guppy module's entry point, unrelated definitions, call sites, and packaged extensions.
- Return a new package; do not mutate the caller's Python object.
- Keep Rust and runtime Python code independent of Guppy.
- Keep `alphalang` importable on Python 3.9; gate Guppy integration dependencies and tests to Python 3.12+.
- Use HUGR's module linker rather than manually reconnecting each call.

## File Structure

- `alpha-codegen/src/hugr.rs`: make generated Alpha text envelopes self-contained so a later process can deserialize their extension operations.
- `alpha-codegen/tests/hugr_scheduled.rs`: prove generated envelopes deserialize and validate with the default registry.
- `alphalang/src/linking.rs`: own DFG promotion, target selection, exact signature checking, package linking, serialization, and Rust tests.
- `alphalang/src/lib.rs`: expose the private PyO3 byte boundary and register it in `_alpha`.
- `alphalang/Cargo.toml`: add the existing workspace HUGR dependency used by `linking.rs`.
- `alphalang/python/alphalang/__init__.py`: expose the object-level `link_alpha_function` adapter without importing HUGR at module import time.
- `alphalang/tests/test_linking.py`: cover Python protocol behavior, errors, declarations, dummy definitions, call redirection, and entry-point preservation with real Guppy source.
- `alphalang/README.md`: document the wrapper workflow and its exact constraints.
- `pyproject.toml`, `uv.lock`: add Guppy only as a Python 3.12-gated development dependency for source-level integration tests.

---

### Task 1: Make Alpha HUGR Envelopes Self-Contained

**Files:**
- Modify: `alpha-codegen/src/hugr.rs`
- Modify: `alpha-codegen/tests/hugr_scheduled.rs`

**Interfaces:**
- Consumes: `generate_hugr(system, schedule_text, bindings) -> Result<hugr::Hugr>`.
- Produces: `generate_hugr_system(...) -> Result<String>` whose envelope can be loaded by `Hugr::load_str(&text, None)` in a fresh process.

- [ ] **Step 1: Strengthen the serialization test.**

In `emits_scheduled_cx_boundaries`, replace the string-marker-only assertion with a real round trip:

```rust
let envelope = alpha_codegen::generate_hugr_system(&system, "", &bindings).unwrap();
let decoded = hugr::Hugr::load_str(&envelope, None).unwrap();
decoded.validate().unwrap();
assert_eq!(
    decoded
        .get_optype(decoded.entrypoint())
        .dataflow_signature(),
    hugr
        .get_optype(hugr.entrypoint())
        .dataflow_signature()
);
assert_eq!(count_tket(&decoded, TketOp::CX), 1);
```

- [ ] **Step 2: Run the focused test and confirm the extension-registry failure.**

Run:

```bash
cargo test -p alpha-codegen --test hugr_scheduled emits_scheduled_cx_boundaries -- --exact
```

Expected: FAIL while loading the text envelope because `generate_hugr_system` does not currently embed the non-standard extension definitions used by the graph. If it already loads, retain the stronger regression test and continue; no serialization change is needed beyond removing the marker-only assertion.

- [ ] **Step 3: Serialize with the generated HUGR's extension registry.**

Change `generate_hugr_system` to retain the graph's registry in the envelope:

```rust
pub fn generate_hugr_system(
    system: &ir::System,
    schedule_text: &str,
    bindings: &ParameterBindings,
) -> Result<String> {
    let hugr = generate_hugr(system, schedule_text, bindings)?;
    let extensions = hugr.extensions().clone();
    hugr.store_str_with_exts(EnvelopeConfig::text(), &extensions)
        .map_err(build_error)
}
```

Import `HugrView` is already present and supplies `extensions()`.

- [ ] **Step 4: Run the scheduled HUGR integration tests.**

Run:

```bash
cargo test -p alpha-codegen --test hugr_scheduled
```

Expected: all five tests PASS, including deserialization and validation of the generated CX envelope.

- [ ] **Step 5: Commit the serialization contract.**

```bash
git add alpha-codegen/src/hugr.rs alpha-codegen/tests/hugr_scheduled.rs
git commit -m "fix: embed Alpha HUGR extensions"
```

---

### Task 2: Promote an Alpha DFG to a Named Function

**Files:**
- Modify: `alphalang/Cargo.toml`
- Create: `alphalang/src/linking.rs`
- Modify: `alphalang/src/lib.rs`

**Interfaces:**
- Consumes: a validated, standalone DFG-rooted `hugr::Hugr` and a non-empty symbol.
- Produces: `implementation_module(implementation: Hugr, symbol: &str) -> Result<Hugr, LinkError>`, a module with one public monomorphic `FuncDefn` containing the original DFG subtree.
- Produces: `LinkError`, whose `Display` text is passed unchanged to Python `ValueError` in Task 4.

- [ ] **Step 1: Add the HUGR dependency and module shell.**

Add the existing workspace-pinned dependency:

```toml
hugr.workspace = true
```

Declare the private module near the imports in `alphalang/src/lib.rs`:

```rust
mod linking;
```

Create `alphalang/src/linking.rs` with a small string-backed error that avoids adding another
dependency:

```rust
use std::fmt::{self, Display};

use hugr::builder::{Dataflow, HugrBuilder, ModuleBuilder};
use hugr::ops::OpType;
use hugr::{Hugr, HugrView, Visibility};

#[derive(Debug)]
pub(crate) struct LinkError(String);

impl LinkError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LinkError {}
```

- [ ] **Step 2: Write failing DFG-promotion tests.**

Add a `#[cfg(test)] mod tests` that builds an identity DFG with
`DFGBuilder::new(Signature::new_endo([bool_t()]))`. Assert:

```rust
#[test]
fn promotes_dfg_to_public_named_function() {
    let implementation = identity_dfg();
    let module = implementation_module(implementation, "foo").unwrap();
    module.validate().unwrap();

    let functions = module
        .children(module.module_root())
        .filter_map(|node| match module.get_optype(node) {
            OpType::FuncDefn(definition) => Some(definition),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].func_name(), "foo");
    assert_eq!(functions[0].visibility(), &Visibility::Public);
    assert!(functions[0].signature().params().is_empty());
    assert_eq!(
        functions[0].signature().body(),
        &Signature::new_endo([bool_t()])
    );
}

#[test]
fn rejects_empty_symbol_and_non_dfg_entrypoint() {
    assert!(implementation_module(identity_dfg(), "   ")
        .unwrap_err()
        .to_string()
        .contains("symbol must not be empty"));
    assert!(implementation_module(ModuleBuilder::new().finish_hugr().unwrap(), "foo")
        .unwrap_err()
        .to_string()
        .contains("entry point must be a DFG"));
}
```

Also attach a metadata value to the identity DFG root and assert it is present on the nested DFG
after promotion. This proves `add_hugr_with_wires` copies the subtree rather than rebuilding its
operations.

- [ ] **Step 3: Run the unit tests and confirm the helper is absent.**

Run:

```bash
cargo test -p alphalang linking::tests::promotes_dfg_to_public_named_function
```

Expected: compilation FAILS because `implementation_module` is not defined.

- [ ] **Step 4: Implement DFG promotion with builder wiring.**

Implement the helper with the checked-in HUGR builder APIs:

```rust
fn implementation_module(implementation: Hugr, symbol: &str) -> Result<Hugr, LinkError> {
    if symbol.trim().is_empty() {
        return Err(LinkError::new("link symbol must not be empty"));
    }
    implementation
        .validate()
        .map_err(|error| LinkError::new(format!("invalid Alpha HUGR: {error}")))?;

    let signature = match implementation.entrypoint_optype() {
        OpType::DFG(dfg) => dfg.signature.clone(),
        operation => {
            return Err(LinkError::new(format!(
                "Alpha HUGR entry point must be a DFG, found {}",
                operation.name()
            )));
        }
    };

    let output_count = signature.output_count();
    let mut module = ModuleBuilder::new();
    let mut function = module
        .define_function_vis(symbol, signature, Visibility::Public)
        .map_err(|error| LinkError::new(format!("cannot create Alpha function: {error}")))?;
    let inputs = function.input_wires().collect::<Vec<_>>();
    let nested = function
        .add_hugr_with_wires(implementation, inputs)
        .map_err(|error| LinkError::new(format!("cannot embed Alpha DFG: {error}")))?;
    function
        .finish_with_outputs((0..output_count).map(|port| nested.out_wire(port)))
        .map_err(|error| LinkError::new(format!("cannot finish Alpha function: {error}")))?;
    module
        .finish_hugr()
        .map_err(|error| LinkError::new(format!("invalid Alpha function module: {error}")))
}
```

Bring the traits required for `name()`, `input_wires()`, and `output_count()` into scope according
to compiler diagnostics (`NamedOp`, `Dataflow`, and `Signature`'s existing methods); do not replace
the builder with direct graph mutation.

- [ ] **Step 5: Run the complete linking module unit tests.**

Run:

```bash
cargo test -p alphalang linking::tests
```

Expected: the promotion, metadata, empty-symbol, and non-DFG tests PASS.

- [ ] **Step 6: Commit DFG promotion.**

```bash
git add alphalang/Cargo.toml alphalang/src/lib.rs alphalang/src/linking.rs Cargo.lock
git commit -m "feat: package Alpha DFGs as functions"
```

---

### Task 3: Replace a Wrapper Symbol Through HUGR Linking

**Files:**
- Modify: `alphalang/src/linking.rs`

**Interfaces:**
- Consumes: binary wrapper package bytes, a self-contained Alpha text envelope, and a symbol.
- Produces: `pub(crate) fn link_alpha_function_bytes(wrapper: &[u8], implementation: &str, symbol: &str) -> Result<Vec<u8>, LinkError>`.
- Guarantees: exactly one supported monomorphic target, exact or explicitly array-adaptable
    signatures, source-definition replacement, preserved wrapper entry point and extensions, and a
    validated binary package result.

- [ ] **Step 1: Add wrapper builders and failing success-path tests.**

Inside the existing Rust test module, build a wrapper module with a public target and a `main`
function that calls it. Use `ModuleBuilder::declare` for one fixture and
`define_function_vis(..., Visibility::Public)` for the dummy fixture. Set the finished module's
entry point to the returned `main` function node with `HugrMut::set_entrypoint`.

The declaration fixture follows this concrete shape:

```rust
fn wrapper_with_declaration() -> (Package, Node) {
    let signature = Signature::new_endo([bool_t()]);
    let mut module = ModuleBuilder::new();
    let target = module.declare("foo", signature.clone().into()).unwrap();
    let mut main = module.define_function("main", signature).unwrap();
    let call = main.call(&target, &[], main.input_wires()).unwrap();
    let main = main.finish_with_outputs(call.outputs()).unwrap();
    let mut module = module.finish_hugr().unwrap();
    module.set_entrypoint(main.node());
    (Package::from_hugr(module), main.node())
}
```

Serialize helpers with `Package::store(..., EnvelopeConfig::binary())`. Then add:

```rust
#[test]
fn replaces_declaration_and_preserves_entrypoint() {
    let (wrapper, old_entrypoint) = wrapper_with_declaration();
    let bytes = package_bytes(&wrapper);
    let linked = link_alpha_function_bytes(&bytes, &identity_dfg_text(), "foo").unwrap();
    let linked = Package::load(linked.as_slice(), None).unwrap();
    linked.validate().unwrap();

    let module = &linked.modules[0];
    assert_eq!(module.entrypoint(), old_entrypoint);
    let foo = public_functions(module, "foo");
    assert_eq!(foo.len(), 1);
    assert!(matches!(module.get_optype(foo[0]), OpType::FuncDefn(_)));
    let call = module
        .nodes()
        .find(|node| matches!(module.get_optype(*node), OpType::Call(_)))
        .unwrap();
    assert_eq!(module.static_source(call), Some(foo[0]));
}

#[test]
fn replaces_dummy_definition() {
    let wrapper = wrapper_with_dummy_false_body();
    let linked = linked_module(wrapper, identity_dfg_text(), "foo");
    assert_eq!(public_functions(&linked, "foo").len(), 1);
    assert_eq!(linked.nodes().filter(|node| linked.get_optype(*node).is_dfg()).count(), 2);
}
```

The final DFG count is the `main` body plus the promoted Alpha body; the removed dummy body's DFG
must not remain.

- [ ] **Step 2: Add failing contract-error tests.**

Add focused tests asserting stable message fragments:

```rust
#[test]
fn rejects_missing_duplicate_private_and_polymorphic_targets() {
    assert_link_error(wrapper_without_foo(), "foo", "public symbol 'foo' was not found");
    assert_link_error(wrapper_with_two_foos(), "foo", "more than one public symbol 'foo'");
    assert_link_error(wrapper_with_private_foo(), "foo", "symbol 'foo' is private");
    assert_link_error(wrapper_with_polymorphic_foo(), "foo", "symbol 'foo' is polymorphic");
}

#[test]
fn rejects_signature_mismatch_and_multiple_modules() {
    assert_link_error(wrapper_with_wrong_signature(), "foo", "signature mismatch for 'foo'");
    assert_link_error(package_with_two_modules(), "foo", "exactly one module");
}
```

Also cover malformed wrapper bytes, malformed Alpha text, and an invalid final graph. Match the
category and symbol rather than HUGR's full nested diagnostic.

- [ ] **Step 3: Run the linking tests and confirm the byte API is absent.**

Run:

```bash
cargo test -p alphalang linking::tests
```

Expected: compilation FAILS because `link_alpha_function_bytes` and target-selection helpers are
not defined.

- [ ] **Step 4: Implement strict target selection and signature comparison.**

Add an owned target descriptor so the immutable scan ends before linking mutates the module:

```rust
struct Target {
    node: Node,
    signature: PolyFuncType,
}

fn find_target(module: &Hugr, symbol: &str) -> Result<Target, LinkError> {
    let mut public = Vec::new();
    let mut has_private = false;
    for node in module.children(module.module_root()) {
        let Some((name, visibility, signature)) = (match module.get_optype(node) {
            OpType::FuncDecl(function) => Some((
                function.func_name(),
                function.visibility(),
                function.signature(),
            )),
            OpType::FuncDefn(function) => Some((
                function.func_name(),
                function.visibility(),
                function.signature(),
            )),
            _ => None,
        }) else {
            continue;
        };
        if name != symbol {
            continue;
        }
        if visibility == &Visibility::Public {
            public.push(Target {
                node,
                signature: signature.clone(),
            });
        } else {
            has_private = true;
        }
    }

    let target = match public.len() {
        0 if has_private => {
            return Err(LinkError::new(format!("symbol '{symbol}' is private")));
        }
        0 => {
            return Err(LinkError::new(format!(
                "public symbol '{symbol}' was not found"
            )));
        }
        1 => public.pop().unwrap(),
        _ => {
            return Err(LinkError::new(format!(
                "more than one public symbol '{symbol}' was found"
            )));
        }
    };
    if !target.signature.params().is_empty() {
        return Err(LinkError::new(format!(
            "symbol '{symbol}' is polymorphic"
        )));
    }
    Ok(target)
}
```

Implement both `FuncDecl` and `FuncDefn` arms using `func_name()`, `visibility()`, and
`signature()`. Clone the selected `PolyFuncType` and record its node and whether it is a public
declaration, public definition, or private definition. Reject a private declaration. Reject more
than one same-name function before HUGR's duplicate-export validator obscures the boundary error.

Compare the target with the Alpha DFG before constructing the implementation module. Permit each
port to differ only by concrete `array<N, T>` versus `borrow_array<N, T>` and insert
`BArrayFromArray`/`BArrayToArray` operations around the nested Alpha DFG. Retain this error for all
other mismatches:

```rust
let alpha_signature = match alpha.entrypoint_optype() {
    OpType::DFG(dfg) => dfg.signature.clone(),
    _ => return Err(LinkError::new("Alpha HUGR entry point must be a DFG")),
};
    if !signatures_are_equal_or_array_adaptable(target.signature.body(), &alpha_signature) {
    return Err(LinkError::new(format!(
        "signature mismatch for '{symbol}': wrapper has {}, Alpha has {}",
        target.signature.body(),
        alpha_signature
    )));
}
```

- [ ] **Step 5: Implement package deserialization, linking, validation, and serialization.**

Use the exact policy required by the spec:

```rust
pub(crate) fn link_alpha_function_bytes(
    wrapper: &[u8],
    implementation: &str,
    symbol: &str,
) -> Result<Vec<u8>, LinkError> {
    if symbol.trim().is_empty() {
        return Err(LinkError::new("link symbol must not be empty"));
    }
    let mut package = Package::load(wrapper, None)
        .map_err(|error| LinkError::new(format!("invalid wrapper package: {error}")))?;
    package
        .validate()
        .map_err(|error| LinkError::new(format!("invalid wrapper package: {error}")))?;
    if package.modules.len() != 1 {
        return Err(LinkError::new(format!(
            "wrapper package must contain exactly one module, found {}",
            package.modules.len()
        )));
    }

    let (alpha, alpha_extensions) = Hugr::load_with_exts(implementation.as_bytes(), None)
        .map_err(|error| LinkError::new(format!("invalid Alpha HUGR: {error}")))?;
    package.extensions.extend(&alpha_extensions);
    let mut wrapper_module = package.modules.pop().unwrap();
    let target = find_target(&wrapper_module, symbol)?;
    ensure_matching_signature(&target, &alpha, symbol)?;
    let implementation_module = implementation_module(alpha, symbol)?;
    let old_entrypoint = wrapper_module.entrypoint();
    let policy = NameLinkingPolicy::new_keep_both_invalid()
        .on_new_names(OnNewFunc::RaiseError)
        .on_signature_conflict(OnNewFunc::RaiseError)
        .on_multiple_defn(OnMultiDefn::UseSource);
    if target.is_private_definition {
        let source = implementation_module
            .children(implementation_module.module_root())
            .exactly_one()
            .unwrap();
        wrapper_module
            .insert_link_hugr_by_node(
                None,
                implementation_module,
                HashMap::from([(source, NodeLinkingDirective::replace([target.node]))]),
            )
            .map_err(|error| LinkError::new(format!("cannot replace '{symbol}': {error}")))?;
    } else {
        wrapper_module
            .link_module(implementation_module, &policy)
            .map_err(|error| LinkError::new(format!("cannot link '{symbol}': {error}")))?;
    }
    if wrapper_module.entrypoint() != old_entrypoint {
        return Err(LinkError::new("HUGR linker changed the wrapper entry point"));
    }

    package.modules.push(wrapper_module);
    package
        .validate()
        .map_err(|error| LinkError::new(format!("invalid linked package: {error}")))?;
    let mut bytes = Vec::new();
    package
        .store(&mut bytes, EnvelopeConfig::binary())
        .map_err(|error| LinkError::new(format!("cannot serialize linked package: {error}")))?;
    Ok(bytes)
}
```

Keep `package.extensions` in the same `Package` value while moving only its module out and back,
and extend it with the registry returned by `Hugr::load_with_exts`. Do not construct
`Package::from_hugr`, which would discard both the wrapper's and Alpha's packaged extension
registries. In the round-trip test, load the returned package with `Package::load(bytes, None)`;
that assertion proves the merged envelope is self-contained.

- [ ] **Step 6: Run the complete Rust linking suite.**

Run:

```bash
cargo test -p alphalang linking::tests
cargo test -p alpha-codegen --test hugr_scheduled
```

Expected: all success-path and contract-error tests PASS; the Alpha envelope regression remains
green.

- [ ] **Step 7: Commit symbol replacement.**

```bash
git add alphalang/src/linking.rs
git commit -m "feat: replace wrapper functions with Alpha HUGRs"
```

---

### Task 4: Expose the Python Package API

**Files:**
- Modify: `alphalang/src/lib.rs`
- Modify: `alphalang/python/alphalang/__init__.py`
- Create: `alphalang/tests/test_linking.py`

**Interfaces:**
- Consumes: any standard HUGR `Package` object implementing `to_bytes()` and class method `from_bytes(bytes)`.
- Produces: `alphalang.link_alpha_function(wrapper, implementation, symbol="foo") -> Package` of the same concrete class as `wrapper`.
- Produces: private native `_link_alpha_function(wrapper_package: bytes, implementation: str, symbol: str) -> bytes`.

- [ ] **Step 1: Write failing adapter protocol tests without requiring Guppy.**

Create `alphalang/tests/test_linking.py`. A minimal probe proves object conversion and immutability
without importing HUGR:

```python
class ProbePackage:
    loaded = None

    def __init__(self, payload: bytes):
        self.payload = payload

    def to_bytes(self) -> bytes:
        return self.payload

    @classmethod
    def from_bytes(cls, payload: bytes):
        cls.loaded = payload
        return cls(payload)


def test_link_alpha_function_uses_package_serialization_protocol(monkeypatch):
    wrapper = ProbePackage(b"wrapper")
    calls = []

    def native(wrapper_bytes, implementation, symbol):
        calls.append((wrapper_bytes, implementation, symbol))
        return b"linked"

    monkeypatch.setattr(alphalang, "_link_alpha_function", native)
    linked = alphalang.link_alpha_function(wrapper, "alpha", symbol="kernel")

    assert calls == [(b"wrapper", "alpha", "kernel")]
    assert isinstance(linked, ProbePackage)
    assert linked.payload == b"linked"
    assert wrapper.payload == b"wrapper"
```

Add tests that a missing `to_bytes` naturally raises `AttributeError`, and that a native
`ValueError("signature mismatch for 'foo'")` is propagated unchanged.

- [ ] **Step 2: Run the Python test and confirm the public function is absent.**

Run:

```bash
uv run pytest alphalang/tests/test_linking.py -q
```

Expected: FAIL with `AttributeError: module 'alphalang' has no attribute 'link_alpha_function'`.

- [ ] **Step 3: Add and register the private PyO3 byte function.**

Import `pyo3::types::PyBytes` and add:

```rust
#[pyfunction]
fn _link_alpha_function(
    py: Python<'_>,
    wrapper_package: &[u8],
    implementation: &str,
    symbol: &str,
) -> PyResult<Py<PyBytes>> {
    let linked = linking::link_alpha_function_bytes(wrapper_package, implementation, symbol)
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
    Ok(PyBytes::new(py, &linked).unbind())
}
```

Register it in `_alpha`:

```rust
m.add_function(wrap_pyfunction!(_link_alpha_function, m)?)?;
```

Use the PyO3 0.23 constructor spelling accepted by the compiler. The returned Python value must be
`bytes`, not a list of integers.

- [ ] **Step 4: Add the public lazy Package adapter.**

At the start of `alphalang/python/alphalang/__init__.py`, enable deferred annotations and import
the native function:

```python
from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from hugr.package import Package

from ._alpha import (
    # existing names
    _link_alpha_function,
)
```

Add the wrapper and export only the public name:

```python
def link_alpha_function(
    wrapper: Package,
    implementation: str,
    symbol: str = "foo",
) -> Package:
    """Replace a Guppy package symbol with a compiled Alpha HUGR function."""
    linked = _link_alpha_function(wrapper.to_bytes(), implementation, symbol)
    return type(wrapper).from_bytes(linked)
```

Append `"link_alpha_function"` to `__all__`. Do not append `_link_alpha_function`, import
`guppylang`, or import `hugr.package` at runtime.

- [ ] **Step 5: Rebuild the extension and run focused tests.**

Run:

```bash
uv run maturin develop --manifest-path alphalang/Cargo.toml
uv run pytest alphalang/tests/test_linking.py -q
uv run pytest alphalang/tests/test_alpha.py -q
```

Expected: all adapter tests and the existing Alpha binding tests PASS.

- [ ] **Step 6: Commit the Python API.**

```bash
git add alphalang/src/lib.rs alphalang/python/alphalang/__init__.py \
  alphalang/tests/test_linking.py
git commit -m "feat: expose Alpha function linking in Python"
```

---

### Task 5: Prove the Workflow with Real Guppy Source

**Files:**
- Modify: `pyproject.toml`
- Modify: `uv.lock`
- Modify: `alphalang/tests/test_linking.py`
- Modify: `alphalang/README.md`

**Interfaces:**
- Consumes: Guppy source decorators, `GuppyLibrary.from_members(...).compile()`, Alpha source and schedule text, and `alphalang.generate_hugr`.
- Produces: an end-to-end source-level workflow for both `FuncDecl` and dummy `FuncDefn` targets.

- [ ] **Step 1: Add the Python 3.12-gated Guppy test dependency.**

Run from the repository root:

```bash
uv add --group dev 'guppylang==1.0.1; python_version >= "3.12"'
```

Confirm the only intentional dependency changes are the new marked development requirement and
its lockfile closure. Do not add Guppy or HUGR to `alphalang`'s runtime dependencies; Guppy callers
already own the `Package` object passed to the protocol adapter.

- [ ] **Step 2: Add shared Alpha and Guppy fixtures.**

In `test_linking.py`, gate source integration on the interpreter and package:

```python
import sys

import pytest

guppylang = pytest.importorskip("guppylang") if sys.version_info >= (3, 12) else None

ALPHA_BOOL_ARRAY = """affine kernel [N] -> {:N>0}
outputs M : {[i] : 0 <= i < N} of bool;
locals linear Q : {[i] : 0 <= i < N} of qubit;
let
    with [i] : (Q[i]) = qalloc();
    with [i] : (M[i]) = measure(Q[i]);
.
"""

ALPHA_BOOL_ARRAY_SCHEDULE = """[N] -> {
Q__call0[i] -> [0,i]; M__call0[i] -> [1,i]
}"""


def alpha_bool_array_hugr() -> str:
    normalized = alphalang.normalize(alphalang.parse(ALPHA_BOOL_ARRAY))
    scheduled = normalized.schedule(ALPHA_BOOL_ARRAY_SCHEDULE)
    return alphalang.generate_hugr(scheduled, {"N": 4})
```

Import Guppy APIs inside each Python-3.12-only test so collection remains valid on Python 3.9-3.11:

```python
from guppylang import guppy
from guppylang.library import GuppyLibrary, link_name
from guppylang.std.builtins import array
```

- [ ] **Step 3: Add a declaration replacement integration test.**

Compile arbitrary Guppy source containing an entry point and two calls:

```python
@pytest.mark.skipif(sys.version_info < (3, 12), reason="Guppy requires Python 3.12")
def test_links_alpha_into_guppy_declaration_and_preserves_calls():
    from guppylang import guppy
    from guppylang.library import GuppyLibrary, link_name
    from guppylang.std.builtins import array

    @guppy.declare
    @link_name("foo")
    def alpha_decl() -> array[bool, 4]: ...

    @guppy
    def main() -> tuple[array[bool, 4], array[bool, 4]]:
        return alpha_decl(), alpha_decl()

    wrapper = GuppyLibrary.from_members(alpha_decl, main).compile()
    old_entrypoint_op = wrapper.modules[0][wrapper.modules[0].entrypoint].op
    linked = alphalang.link_alpha_function(wrapper, alpha_bool_array_hugr(), "foo")

    assert linked is not wrapper
    assert linked.modules[0][linked.modules[0].entrypoint].op.f_name == old_entrypoint_op.f_name
    targets = named_functions(linked.modules[0], "foo")
    assert len(targets) == 1
    assert isinstance(targets[0].op, FuncDefn)
    assert count_calls_to(linked.modules[0], targets[0]) == 2
```

Implement `named_functions` by scanning direct `module.children(module.module_root)` for Python
`hugr.ops.FuncDecl`/`FuncDefn` objects whose `f_name` matches. Inspect call targets through the
function input port, which follows all value inputs:

```python
from hugr.ops import Call, FuncDecl, FuncDefn


def named_functions(module, symbol):
    return [
        module[node]
        for node in module.children(module.module_root)
        if isinstance(module[node].op, (FuncDecl, FuncDefn))
        and module[node].op.f_name == symbol
    ]


def count_calls_to(module, target):
    count = 0
    for node, node_data in module.items():
        if not isinstance(node_data.op, Call):
            continue
        function_port = node.inp(len(node_data.op.signature.body.input))
        sources = list(module.linked_ports(function_port))
        assert len(sources) == 1
        count += sources[0].node == target.node
    return count
```

- [ ] **Step 4: Add a dummy-definition replacement integration test.**

Use a visibly different no-op body:

```python
@pytest.mark.skipif(sys.version_info < (3, 12), reason="Guppy requires Python 3.12")
def test_replaces_guppy_dummy_definition():
    from guppylang import guppy
    from guppylang.library import GuppyLibrary, link_name
    from guppylang.std.builtins import array

    @guppy
    @link_name("foo")
    def dummy() -> array[bool, 4]:
        return array(False, False, False, False)

    @guppy
    def main() -> array[bool, 4]:
        return dummy()

    wrapper = GuppyLibrary.from_members(dummy, main).compile()
    linked = alphalang.link_alpha_function(wrapper, alpha_bool_array_hugr())

    targets = named_functions(linked.modules[0], "foo")
    assert len(targets) == 1
    assert isinstance(targets[0].op, FuncDefn)
    assert any(isinstance(node_data.op, DFG) for node_data in linked.modules[0].values())
```

Also add public Python error tests using real packages for a missing symbol and a mismatched
`array[bool, 3]` declaration. Match `public symbol 'missing' was not found` and
`signature mismatch for 'foo'`.

- [ ] **Step 5: Run the source-level integration tests.**

Run:

```bash
uv run maturin develop --manifest-path alphalang/Cargo.toml
uv run pytest alphalang/tests/test_linking.py -q
```

Expected on Python 3.12+: declaration, dummy, two-call, entry-point, protocol, and error tests all
PASS. On Python 3.9-3.11, protocol tests PASS and Guppy tests are reported as skipped.

- [ ] **Step 6: Document the public workflow and constraints.**

Add a `Linking a Guppy wrapper` section to `alphalang/README.md` with this executable shape:

```python
from guppylang import guppy
from guppylang.library import GuppyLibrary, link_name
from guppylang.std.builtins import array
import alphalang

@guppy.declare
@link_name("foo")
def foo() -> array[bool, 4]: ...

@guppy
def main() -> array[bool, 4]:
    return foo()

wrapper = GuppyLibrary.from_members(foo, main).compile()
alpha_hugr = alphalang.generate_hugr(scheduled_alpha_system, {"N": 4})
linked = alphalang.link_alpha_function(wrapper, alpha_hugr, symbol="foo")
```

State that the wrapper must contain exactly one module and one public monomorphic declaration or
definition with an exactly matching HUGR signature. Mention `@link_name` as the stable way to
control the symbol and note that the original package object is unchanged.

- [ ] **Step 7: Run final validation.**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
uv run maturin develop --manifest-path alphalang/Cargo.toml
uv run pytest alphalang/tests -q
```

Expected: formatting, strict workspace clippy, all Rust tests, extension rebuild, and all Python
tests PASS. Inspect `git status --short` and leave unrelated files, including `run-narsil.sh`,
untouched.

- [ ] **Step 8: Commit integration and documentation.**

```bash
git add pyproject.toml uv.lock alphalang/tests/test_linking.py alphalang/README.md
git commit -m "test: cover Guppy Alpha HUGR linking"
```