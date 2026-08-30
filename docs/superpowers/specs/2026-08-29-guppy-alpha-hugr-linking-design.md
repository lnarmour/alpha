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

1. Resolve a unique function symbol and use HUGR's supported static-edge replacement machinery
    instead of rewriting individual call nodes.
2. Use HUGR's supported module linker to redirect static function edges.
3. Require exact function-signature equality except for explicit ordinary-array/borrow-array
    boundary conversions with identical sizes and element types; do not reorder ports.
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

The selected symbol must identify exactly one monomorphic module child that is either:

- a public `FuncDecl`, typically produced by `@guppy.declare`; or
- a public or private `FuncDefn`, including a no-op or dummy implementation.

Callers can use Guppy's `@link_name("foo")` to make the HUGR symbol independent of the Python
function name. The utility permits a unique private definition target for packages produced by
other valid HUGR workflows. Private declarations remain unsupported.

Guppy's direct `main.compile()` path may inline or eliminate a referenced dummy definition, leaving
no `foo` node for any linker to replace. To use a concrete dummy, compile both `foo` and `main` as
members of a `GuppyLibrary`, give them stable `@link_name` values, and select the resulting `main`
function as the module entry point. A Guppy declaration remains present when referenced by direct
entry-point compilation and does not require this retention step.

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
3. Compare each Alpha port with its Guppy counterpart and derive the Guppy-visible signature.
4. Create a module with one public `FuncDefn` named `symbol` and the Guppy-visible signature.
5. Embed the Alpha DFG beneath the function body, connecting each function input to the
    corresponding DFG input and each DFG output to the corresponding function output. Insert the
    explicit array conversion operations described below where required.
6. Validate the temporary module before linking.

The DFG is embedded without changing port order, types, operation semantics, or metadata within
the copied subtree. The temporary module has no executable entry point of its own; the Guppy
package remains the owner of execution.

## Linking Semantics

Before linking, the utility compares the Alpha function signature with the selected Guppy target's
signature. Arity and port order must match. Corresponding port types must either be exactly equal,
or be concrete `collections.array.array<N, T>` and
`collections.borrow_arr.borrow_array<N, T>` types with equal `N` and `T`. This exception is needed
because Guppy exposes source `array[T, N]` values as borrow arrays while Alpha's kernel ABI exports
ordinary arrays.

The promoted implementation has the Guppy target's signature. For inputs, `BArrayToArray` converts
a Guppy borrow array before an Alpha ordinary-array port, and `BArrayFromArray` performs the reverse
case. Outputs use the corresponding inverse direction after the Alpha DFG. No conversion is added
for equal types. Parametric arrays, different lengths or element types, non-array differences,
different arities, and any other mismatch are rejected.

The temporary Alpha module is then linked into the Guppy module with HUGR's name-linking policy:

- same-name declaration: replace it with the Alpha definition;
- same-name public dummy definition: retain the source Alpha definition with
    `OnMultiDefn::UseSource`;
- same-name private dummy definition: use `NodeLinkingDirective::replace` for that exact node and
    expose the replacement as the public Alpha symbol;
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
- missing, duplicated, or unsupported wrapper target, including a private declaration;
- polymorphic wrapper target;
- signature mismatch outside the explicit ordinary-array/borrow-array conversion rule;
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
- replace a private dummy when the input HUGR actually contains one;
- insert ordinary-array/borrow-array conversions on compatible inputs and outputs;
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
- structural type conversion beyond concrete ordinary-array/borrow-array boundaries, argument
    reordering, or general wrapper synthesis;
- inlining the Alpha body at Guppy call sites;
- linking arbitrary non-function module children;
- mutating the caller's input `Package` in place.

These can be added later without changing the core model: Alpha implementations are named module
functions, and standard HUGR linking resolves Guppy calls to them.