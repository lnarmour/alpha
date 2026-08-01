# isl-sys

Raw FFI bindings to [libisl](https://libisl.sourceforge.io/) (the Integer Set Library), generated
at build time by `bindgen` from `wrapper.h`.

This crate is intentionally just the `bindgen` output plus a couple of re-exports — no safe
abstractions live here. The safe, idiomatic wrapper is the sibling [`isl`](../isl) crate; use that
one unless you specifically need raw `unsafe` access to the C API.

## What's bound

`wrapper.h` includes a deliberately bounded subset of isl's headers — not "all of isl", just what
the Alpha compiler actually needs:

- `ctx`, `options`, `id`, `val`
- `space`, `local_space`
- `set`, `map`, `union_set`, `union_map`
- `aff`, `constraint`
- `polynomial` (piecewise quasipolynomials, for cardinality counting)
- `ast`, `ast_build` (isl's loop-generation AST builder)
- `printer` (including isl's own C-format pretty-printer)

Every `isl_*` type, function, and constant (including the uppercase `ISL_*` `#define`s like the
printer's output-format selector) is allowlisted and bound. isl's own C enums (`isl_dim_type`,
`isl_ast_*_type`, `isl_error`, ...) are bound as module-scoped constants rather than Rust `enum`s,
since C code can synthesize values outside the declared variants and `bindgen`'s default enum
codegen doesn't tolerate that.

## Build requirements

- A system install of **libisl** (>= 0.18) discoverable via **pkg-config**. On macOS:
  ```
  brew install isl pkg-config
  ```
  `pkg-config` installs to `/opt/homebrew/bin`, which isn't always on `PATH` in non-interactive
  shells — prefix build commands with `export PATH="/opt/homebrew/bin:$PATH"` if `cargo build`
  reports it can't find isl.
- **libclang**, for `bindgen` itself. On macOS this ships with the Xcode Command Line Tools.

There is currently no vendored-source fallback — `build.rs` panics with a clear message if
pkg-config can't find isl, rather than trying to build one from source. A vendored path (build isl
from source when pkg-config can't find a system install) is planned but not implemented; add it
if a target environment without a system isl install actually shows up.

## Status

Done: the full bounded header set above is bound and builds clean. Covered by one runtime smoke
test (`tests/smoke.rs`) exercising the FFI directly.

## License

MIT (isl itself is MIT-licensed). Unlike `barvinok-sys`, this crate has no GPL entanglement.
