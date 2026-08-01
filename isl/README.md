# isl

Safe, idiomatic Rust wrapper over [`isl-sys`](../isl-sys) — `Set`/`BasicSet`, `Map`, `Aff`/
`MultiAff`, `Constraint`/`LocalSpace`, `PwQPolynomial`, `AstBuild`/`AstNode`/`AstExpr`, `Space`/
`DimType`, all under one `Context`.

This is a *bounded* wrapper — it covers the operation inventory the Alpha compiler actually uses
(set/map algebra, affine functions, constraints, piecewise quasipolynomials, the AST builder), not
the whole of isl's API surface.

## Design

- **Ownership mirrors isl's C API.** isl's C functions are consumption-oriented: most operations
  free their input and return a new object. This crate encodes that directly in the type system —
  every isl call that consumes an argument takes it by value (`self`, not `&self`), so the borrow
  checker rejects reuse of a consumed isl object at compile time. `Clone` (backed by isl's real
  `_copy` functions, cheap since isl objects are internally refcounted/copy-on-write) is the only
  way to reuse a value across two operations.
- **Every fallible call returns `Result<T, IslError>`.** isl's null-on-error convention is
  converted via `Context::check`, mirroring the source Java system's own
  `callISLwithErrorHandling` pattern. `Context::new()` also sets `ISL_ON_ERROR_CONTINUE` so isl
  doesn't additionally spam stderr on every invalid input — useful for an IDE-facing tool getting
  fed invalid text on every keystroke.
- **Two output formats** (`Format::Isl` / `Format::C`) on the types that support it —
  `to_string_fmt` leans on isl's own C pretty-printer for affine/constraint/polynomial-to-C
  conversion, so codegen never needs a hand-rolled affine-to-C converter.

## Modules

| Module | Covers |
|---|---|
| `ctx` | `Context`, `IslError`/`Result`, the null-on-error → `Result` bridge |
| `space` | `Space`, `DimType` |
| `set` | `Set`, `BasicSet`, `Format` |
| `map` | `Map` |
| `aff` | `Aff`, `MultiAff`, `PwAff` |
| `constraint` | `Constraint`, `LocalSpace` |
| `polynomial` | `PwQPolynomial` (piecewise quasipolynomials) |
| `ast` | `AstBuild`, `AstNode`/`AstNodeKind`, `AstExpr`/`AstExprKind`, `UnionMap` — isl's loop-generation AST builder |

## Status

Done. Covered by 9 integration tests (`tests/smoke.rs`): set/map algebra, hulls, gist, dependence
image/preimage, constraint construction, and the AST builder generating real nested `for`/`if` C
code.

## Notes for callers

- Must never depend on `barvinok` (GPL) — cardinality/Ehrhart counting lives in the separate
  [`barvinok`](../barvinok) crate, feature-gated in `alpha-codegen`, precisely so this crate can
  stay MIT-only.
- `AstBuild::expr_from_set` (etc.) renders using **the build's own context set's dim names**,
  positionally — the set argument's own dim names are ignored. See `docs/progress.md` in the workspace
  root (bug #12) if you're relying on this: callers construct a small "universe" context set with
  the exact names they want to render, rather than trying to get dim names to agree between sets
  before printing.

## License

MIT.
