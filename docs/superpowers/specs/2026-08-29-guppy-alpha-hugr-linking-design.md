# Guppy and Alpha HUGR Linking

## Status

Approved design. Target branch: `louis/hugr`.

## Goal

Allow an arbitrary Guppy program to call a function implemented by a compiled Alpha system. The
Guppy source contains either a declaration or a dummy definition, conventionally named `foo`.
After both programs are compiled, a Python `alphalang` utility replaces that Guppy symbol with the
Alpha implementation while preserving the Guppy entry point and every existing call site.

The initial workflow is:

```python
wrapper = guppy_program.compile()
implementation = alphalang.generate_hugr(alpha_system, parameters)
linked = alphalang.link_alpha_function(wrapper, implementation, symbol="foo")
```

The result is a `hugr.package.Package` suitable for normal HUGR serialization, validation, and
execution.

## Design Principles

1. Link by public function symbol instead of rewriting individual call nodes.
2. Use HUGR's supported module linker to redirect static function edges.
3. Require exact function-signature equality; do not insert implicit adapters or reorder ports.
4. Preserve the Guppy package as the executable artifact, including its entry point and bundled
   extensions.
5. Keep the Rust implementation independent of Guppy. Guppy is one producer of standard HUGR
   packages, not a new compiler dependency for Alpha.
6. Return a new package and leave both inputs unchanged.

## Public Python API

The first public surface lives in `alphalang`:

```python
def link_alpha_function(
    wrapper: hugr.package.Package,
    implementation: str,
    symbol: str = "foo",
) -> hugr.package.Package:
    ...
```

`wrapper` is normally the `Package` returned by Guppy compilation. `implementation` is the text
envelope returned by `alphalang.generate_hugr`. `symbol` is the public HUGR link name to replace.

The Python function serializes `wrapper` to bytes and delegates to a private PyO3 function. It
deserializes the returned bytes as a `Package`. This small Python adapter gives callers an
object-oriented API without requiring Rust to inspect foreign Python classes.

The private native boundary is equivalent to:

```python
def _link_alpha_function(
    wrapper_package: bytes,
    implementation: str,
    symbol: str,
) -> bytes:
    ...
```

This native function is not part of the stable public API.

## Accepted Wrapper Shape

The first version accepts a package containing exactly one HUGR module. That module may have any
valid Guppy entry point and arbitrary functions besides the selected target.

The selected symbol must identify exactly one public, monomorphic module child that is either:

- a `FuncDecl`, typically produced by `@guppy.declare`; or
- a `FuncDefn`, including a no-op or dummy implementation.

Callers can use Guppy's `@link_name("foo")` to make the HUGR symbol independent of the Python
function name. Private functions are not link targets because HUGR name linking operates on public
module symbols.

Zero matching targets is an error. Multiple matching targets is also an error, even if HUGR
validation would reject the duplicate exports later. Diagnosing this before mutation produces a
clearer boundary error.

Multiple wrapper modules and polymorphic target signatures are deferred. They require an explicit
module-selection or instantiation API rather than an arbitrary first-match rule.

## Alpha Implementation Packaging

`alphalang.generate_hugr` currently returns a HUGR whose entry point is a standalone `DFG`. The
module linker requires a module-level function definition. The linker utility therefore promotes
the Alpha HUGR into a temporary implementation module:

1. Deserialize and validate the Alpha envelope.
2. Require its entry point to be a `DFG` with a monomorphic function signature.
3. Create a module with one public `FuncDefn` named `symbol` and that signature.
4. Embed the Alpha DFG beneath the function body, connecting each function input to the
   corresponding DFG input and each DFG output to the corresponding function output.
5. Validate the temporary module before linking.

The DFG is embedded without changing port order, types, operation semantics, or metadata within
the copied subtree. The temporary module has no executable entry point of its own; the Guppy
package remains the owner of execution.

## Linking Semantics

Before linking, the utility compares the Alpha function signature with the selected Guppy target's
signature. Equality is exact at the HUGR type level, including linearity, extension types, input
order, output order, and extension requirements.

The temporary Alpha module is then linked into the Guppy module with HUGR's name-linking policy:

- same-name declaration: replace it with the Alpha definition;
- same-name dummy definition: retain the source Alpha definition with `OnMultiDefn::UseSource`;
- same-name conflicting signature: fail;
- unrelated Guppy symbols: retain them unchanged;
- new public symbols from the Alpha module: reject them.

The linker redirects all static function edges that referred to the old declaration or definition
to the inserted Alpha `FuncDefn`, then removes the replaced node and its subtree. Consequently, one
or many Guppy call sites continue to work without per-call rewriting.

The Guppy module is the link target, so its entry point is retained. The output package contains
the linked module and the wrapper package's bundled extensions. Any extension requirements used by
the Alpha implementation remain recorded in the linked HUGR and are checked by normal HUGR
validation and resolution.

## Errors

The Python function raises `ValueError` for caller-contract and linking failures, with messages
that identify the symbol and relevant signatures where possible. Error categories include:

- invalid or empty symbol;
- malformed wrapper package envelope;
- wrapper package with other than one module;
- malformed or invalid Alpha HUGR envelope;
- Alpha artifact whose entry point is not a standalone DFG;
- missing, private, duplicated, or unsupported wrapper target;
- polymorphic wrapper target;
- exact signature mismatch;
- implementation-module construction failure;
- HUGR name-linking failure;
- final package validation failure.

No partially linked package is returned after an error. Input objects remain unchanged because the
native implementation operates on deserialized copies.

## Validation

Validation occurs at three boundaries:

1. Validate the deserialized Alpha DFG before packaging it.
2. Validate the temporary one-function implementation module.
3. Validate the final linked Guppy module before serialization.

The Python adapter then deserializes the returned bytes, providing a final serialization
round-trip check through the Python HUGR model.

## Tests

Rust tests cover the representation boundary and linker behavior:

- package a standalone Alpha DFG as a public function;
- replace a matching `FuncDecl`;
- replace a matching dummy `FuncDefn`;
- redirect multiple static call edges to the Alpha definition;
- preserve the original wrapper entry point;
- reject missing and duplicate symbols;
- reject signature mismatches and polymorphic targets;
- reject malformed or non-DFG Alpha artifacts;
- validate and round-trip the linked package.

Python integration tests compile real Guppy source and use the public `alphalang` API. They cover
both `@guppy.declare` and a dummy `@guppy` definition using `@link_name("foo")`, verify that the
returned value is a `Package`, and inspect or execute the result sufficiently to prove calls target
the Alpha implementation rather than the dummy body.

## Non-goals

The first version does not provide:

- source-level Guppy compilation inside `alphalang`;
- file-path or CLI linking APIs;
- multiple symbol replacements in one call;
- selecting among multiple modules in either package;
- polymorphic Alpha functions or target instantiation;
- structural type conversion, argument reordering, or wrapper synthesis;
- inlining the Alpha body at Guppy call sites;
- linking arbitrary non-function module children;
- mutating the caller's input `Package` in place.

These can be added later without changing the core model: Alpha implementations are named module
functions, and standard HUGR linking resolves Guppy calls to them.