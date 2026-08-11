# alpha-codegen

The `simpleC` model and the `WriteC` demand-driven C code generator — turns a normalized
[`alpha-transform`](../alpha-transform) IR into compilable C source.

**Fidelity target: semantic equivalence, not bit-for-bit output matching** — the generated C
computes the same result with the same overall strategy (memoized demand-driven evaluation) as
the source project, but takes Rust-idiomatic liberties in naming, statement ordering, and
formatting.

## Modules

- **`simplec`**: the `simpleC` C AST — deliberately minimal (`Stmt`/`Expr`/`Function`/`CType` as
  plain enums), with `Expr::Raw(String)` as the escape hatch every affine/boolean condition goes
  through (isl's own C-format printer already produces valid C text for those — there's no
  hand-written affine-to-C converter to maintain).
- **`layout`**: storage layout for a system's variables. Two schemes: **interface variables** (a
  system's own `inputs`/`outputs`) are a plain pointer chain indexed directly — memory for these is
  the caller's responsibility. **Flat variables** (locals, generated flag arrays) are allocated and
  sized by this generator itself, via a row-major linearization over each dimension's own bounding
  box (`Set::dim_min`/`dim_max`) — a deliberately sanctioned isl-only fallback in place of the
  source system's Barvinok/Ehrhart-derived exact linearization. For a rectangular
  domain the two coincide exactly; for a non-rectangular one this allocates strictly more, never
  less, so it stays correct.
- **`writec`**: the generator itself. Every equation becomes a memoized `eval_<Var>(indices...)`
  function guarded by the `'N'`/`'I'`/`'F'` flag convention; every `Reduce` becomes its own
  synthesized `reduce<N>(...)` function; loops are generated via `isl_ast_build` over an identity
  schedule, never hand-rolled; `case` branches become a right-nested ternary.
- **`error`**: `CodegenError`/`Result` — every unsupported construct raises a named error, never a
  panic or a bare isl error.
- **`wrapper`**: `generate_wrapper`, a `*_wrapper.c`-style test harness generator for a system's
  public entry point — allocates memory for every parameter, calls the generated function, and
  frees it, so a generated system can be compiled and run without hand-written boilerplate. Reads
  concrete values for a system's own symbolic scalar parameters (e.g. `N`) from `argv` at runtime,
  falling back to a default when absent, matching the reference AlphaZ wrapper's own convention of
  running the same binary across several sizes. Exposed on the CLI via `alphac`'s `--wrapper` flag
  (see [that crate's README](../alphac)).

## Cardinality counting (`barvinok` feature)

Off by default. Enables Ehrhart/cardinality-based `malloc` sizing via the GPL-licensed
[`barvinok`](../barvinok) crate — not wired up yet (`barvinok`/`barvinok-sys` are still stubs), so
this feature currently has no effect beyond the dependency edge. Never enable it in the default
`alphac` build or a future VS Code extension build — see the root README's licensing section for
why.

## Scope (relative to the source project's own `WriteC`)

Every boundary below raises a named `CodegenError::Unsupported`, never a panic:

- **`UseEquation`** — no codegen backend, matching the source system exactly (a known limitation
  there too, not a port regression).
- **`Convolution`** — unreachable in practice; already excluded at lowering.
- **`Select`** / **`IndexPolynomial`** (`val{...}`) / **`argreduce`** — real features, genuinely
  not implemented. The whole 82-fixture corpus has zero `argreduce` uses and only a handful of
  `select`/`val{...}` uses.
- A `case`'s `auto` branch whose true domain isn't independently bounded once combined with only
  the enclosing reduce/equation's own ambient context — a real, rare tree shape (see
  `docs/progress.md`'s
  bug #14), not a crash to hide.

## Status

Done for scope. `codegen_fixtures` runs the whole pipeline over all 82 fixtures: 199 systems
generate successfully, 21 hit a named scope boundary, zero unexpected failures. Separately,
generated C for the three `alpha.codegen.tests` reference programs (`CopyInput`, `PrefixScan`,
`LUDecomposition`) was compiled and linked against the sibling Java system's own
`*-wrapper.c`/`*_verify.c` files and **passes real numeric verification** against the original
AlphaZ-generated output across a range of `N`. Wrapper generation (`generate_wrapper`) is also
implemented and fixture-tested (`tests/wrapper_fixtures.rs`).
