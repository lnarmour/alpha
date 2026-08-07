# alpha-rs

A Rust implementation of the [Alpha language](https://github.com/CSU-CS-Melange/alpha-language)
polyhedral compilation toolchain: parse → semantic analysis → normalize → generate C.

This is a from-scratch Rust port of `alpha-language`'s compiler core (not a wrapper around the
existing Java/Eclipse system), replacing Xtext/ANTLR/EMF/OSGi with ordinary Rust parsing tooling
(`logos` + a hand-written parser + `rowan`) and hand-ported semantic analysis / codegen. It binds
the polyhedral engine, [isl](https://libisl.sourceforge.io/), via native FFI rather than
reimplementing polyhedral algebra in Rust — isl's own textual parser, set/map algebra, and AST
builder are what the source system actually depends on for correctness, and every serious
polyhedral tool (Polly/LLVM, PPCG, Pluto's ISL mode) makes the same call.

**Out of scope for this port**: the `alphaz`/GeCoS automatic scheduling *search*, tiling, and
memory-mapping. `alpha-codegen` now has two backends: `WriteC`, the original demand-driven
generator (loop order falls out of memoized recursive calls, no explicit schedule), and
`ScheduledC` (`docs/scheduled-codegen-design.md`), which takes an explicit, user-supplied target
mapping and emits a flat loop nest in exactly the order it specifies — real polyhedral codegen
*given* a schedule, still not a scheduler that derives one. `alphalang`, a PyO3 binding crate, and
an interactive Jupyter notebook workflow (`%%alpha`/`%%schedule` cell magics, `read`/`normalize`/
`schedule`/`generate` as plain Python calls) are what actually drive `ScheduledC` day to day; see
`docs/scheduled-codegen-design.md` for the full design and implementation-phasing history.

## Pipeline

```
.alpha file
  │  alpha-syntax   (lexer, lossless CST, resilient parser, typed AST)
  ▼
parsed AST
  │  alpha-model    (name/domain resolution, six-phase well-formedness checker)
  ▼
analyzed AST + diagnostics
  │  alpha-transform (Normalize, NormalizeReduction)
  ▼
normalized IR
  │  alpha-codegen  (simpleC model; WriteC demand-driven generator, or ScheduledC + a target mapping)
  ▼
generated C
```

`alphac` is the CLI that wires the `WriteC` path together end to end: `alphac file.alpha -o file.c`.
`ScheduledC` has no CLI of its own — it's driven interactively through `alphalang` (see
`docs/scheduled-codegen-design.md` §5, §10).

## Workspace layout

| Crate | Purpose | Status |
|---|---|---|
| [`isl-sys`](../isl-sys) | Raw `bindgen` FFI to libisl | Done |
| [`isl`](../isl) | Safe, idiomatic wrapper over `isl-sys` | Done |
| [`barvinok-sys`](../barvinok-sys) | Raw FFI to libbarvinok (GPL) | Stub, deferred |
| [`barvinok`](../barvinok) | Safe wrapper over `barvinok-sys` (GPL) | Stub, deferred |
| [`alpha-syntax`](../alpha-syntax) | Lexer, CST, parser, typed AST | Done |
| [`alpha-model`](../alpha-model) | Semantic model, six-phase checker | Done (documented scope boundaries) |
| [`alpha-transform`](../alpha-transform) | `Normalize`, `NormalizeReduction` | Done for scope |
| [`alpha-codegen`](../alpha-codegen) | `simpleC` model + `WriteC`/`ScheduledC` generators | Done for scope |
| [`alphac`](../alphac) | CLI driver (`WriteC` only) | Done for scope |
| [`alphalang`](../alphalang) | PyO3 bindings + IPython magics for the `ScheduledC` notebook workflow | Done for scope |

Each crate has its own README with module-level detail. See [`docs/progress.md`](progress.md) for the
detailed, up-to-date crate-by-crate status, known bugs found and fixed, and next steps, meant to
let a new session pick up cold.

**Editor support**: a VS Code extension (`editors/vscode`, native addon via `napi-rs` — `alpha-syntax`
+ `alpha-model` compiled into an in-process `cdylib` the extension host calls directly, rather than
a generic LSP server — plus a static TextMate grammar, `editors/vscode/syntaxes/alpha.tmLanguage.json`,
for syntax highlighting) is built and released. No JupyterLab syntax highlighting — a prototype was
built and then deliberately dropped; see `docs/scheduled-codegen-design.md` §10.2/§12 step 8 for why
(JupyterLab 4 has no extension point mapping a cell-magic prefix to a highlighted language, so even
working it would only ever have covered standalone `.alpha` files, not notebook cells — not worth a
JS/TypeScript toolchain for that). The interactive notebook workflow itself (`%%alpha`/`%%schedule`
cell magics, `read`/`normalize`/`schedule`/`generate`) is unaffected — it's plain Python, via
`alphalang`, and stays plain-text-highlighted in a notebook.

## Prerequisites

- **Rust**, installed via [rustup](https://rustup.rs/) (not a system package manager):
  ```
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  ```
- **isl** (>= 0.18) and **pkg-config**. On macOS:
  ```
  brew install isl pkg-config
  ```
  `isl-sys`'s `build.rs` uses `pkg-config` to find isl. Homebrew installs `pkg-config` to
  `/opt/homebrew/bin`, which isn't always on `PATH` in non-interactive shells — prefix cargo
  invocations with `export PATH="/opt/homebrew/bin:$PATH"` if `cargo build` reports it can't find
  isl.
- **libclang**, for `bindgen`. On macOS this ships with the Xcode Command Line Tools
  (`/Library/Developer/CommandLineTools/usr/lib/libclang.dylib`).
- **Barvinok**: not required. `barvinok`/`barvinok-sys` are stub crates gated behind the
  `barvinok` Cargo feature (off by default) — nothing in a default build needs it.

Linux/macOS only — no Windows support (isl depends on GMP, which complicates static linking on
Windows; this matches the constraint the existing Eclipse-based system already has).

## Building and testing

```
cd alpha-rs
export PATH="/opt/homebrew/bin:$PATH"   # macOS, if pkg-config isn't already on PATH
cargo build --workspace
cargo test --workspace
```

### Fixture corpus

Every conformance test reads `.alpha` fixtures bundled in this repo under
[`tests/alpha-language-fixtures/`](../tests/alpha-language-fixtures/) (copied from the upstream
`alpha-language` repo's `tests/**`), via a relative path (`../tests/alpha-language-fixtures` from
each crate's `CARGO_MANIFEST_DIR`).

82 `.alpha` fixtures total, across both `src-valid` and `src-invalid` subtrees.

One finding worth knowing: despite one subdirectory being named `src-invalid/syntax-tests`,
*every* fixture in the corpus is syntactically well-formed Alpha, including that one — they all
test *semantic* violations (dimension mismatches, duplicate definitions, ...), not grammar errors.
Don't assume a `src-invalid` path means "should fail to parse."

## Using `alphac`

```
cargo run -p alphac -- path/to/file.alpha -o path/to/file.c
```

Without `-o`, generated C is printed to stdout. See [`alphac`'s README](../alphac/README.md) for CLI
details and the exact pipeline it runs.

## License

MIT by default (see each crate's `Cargo.toml`), matching the upstream `isl` library. **isl is
MIT; Barvinok is GPL** (barvinok vendors isl internally since 0.30, but the standalone isl project
itself stays MIT). Since Barvinok is only needed for one thing — cardinality/Ehrhart-polynomial
counting for `malloc` sizing — its bindings live in their own crate pair (`barvinok-sys`/
`barvinok`), separate from `isl-sys`/`isl`, and are only pulled in via `alpha-codegen`'s optional,
off-by-default `barvinok` Cargo feature. `alphac`'s default build and any future VS Code extension
build ship MIT-only, with no GPL surface, unless that feature is explicitly enabled.

## Further reading

- [`docs/progress.md`](progress.md) — current status, crate-by-crate detail, non-obvious bugs found and fixed,
  and next steps. The place to look for what's actually implemented right now versus planned.
- Each crate's own `README.md` — module-level design rationale specific to that crate.
