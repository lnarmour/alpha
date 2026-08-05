# alpha-language-rs

A Rust implementation of the [Alpha language](https://github.com/CSU-CS-Melange/alpha-language)
polyhedral compilation toolchain: parse → semantic analysis → normalize → generate C.

`alphac` is the CLI this workspace builds: `alphac file.alpha -o file.c`.

For the full design rationale (why isl via FFI, workspace layout, licensing, scope) see
[`docs/design.md`](docs/design.md). For current status, known bugs, and next steps see
[`docs/progress.md`](docs/progress.md). Each crate also has its own README.

## Dependencies

- **[uv](https://docs.astral.sh/uv/)**, to run `prek` for pre-commit hooks (see
  [Pre-commit hooks](#pre-commit-hooks) below):
  ```
  curl -LsSf https://astral.sh/uv/install.sh | sh
  ```
- **Rust**, via [rustup](https://rustup.rs/) (not a system package manager):
  ```
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  ```
- **isl** (>= 0.18) and **pkg-config**. On macOS:
  ```
  brew install isl pkg-config
  ```
  Homebrew installs `pkg-config` to `/opt/homebrew/bin`, which isn't always on `PATH` in
  non-interactive shells — prefix commands with `export PATH="/opt/homebrew/bin:$PATH"` if
  `cargo build` reports it can't find isl (the Makefile below already does this for you).
- **libclang**, for `bindgen`. On macOS this ships with the Xcode Command Line Tools.
- **Barvinok**: not required. `barvinok`/`barvinok-sys` are stub crates gated behind the
  off-by-default `barvinok` Cargo feature.

Linux/macOS only — no Windows support (isl depends on GMP, which complicates static linking on Windows).

## Building

```
make build            # debug build, whole workspace
make release          # optimized build; alphac lands at target/release/alphac
```

Equivalent plain-cargo commands (see the Makefile for the exact `PATH` handling):

```
cargo build --workspace
cargo build --workspace --release
```

## Testing

```
make test
```

or `cargo test --workspace`. Conformance tests read `.alpha` fixtures bundled in this repo under
[`tests/alpha-language-fixtures/`](tests/alpha-language-fixtures/) (copied from the upstream
[`alpha-language`](https://github.com/CSU-CS-Melange/alpha-language) repo's `tests/**`).

## Running `alphac`

```
cargo run -p alphac -- path/to/file.alpha -o path/to/file.c
```

Without `-o`, generated C is printed to stdout.

## Python bindings (`alpha-py`)

Interactive, scheduled codegen — `parse`/`normalize`/`schedule`/`generate` from Python, plus
`%%alpha`/`%%schedule` Jupyter cell magics. See [`alpha-py/README.md`](alpha-py/README.md) for
details; quickstart:

```
uv sync && source .venv/bin/activate   # once
uv pip install maturin                 # once
cd alpha-py && maturin develop
```

```python
import alpha

sys = alpha.parse("""
affine PrefixScan [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.
""")
norm = alpha.normalize(sys)
sched = norm.schedule(
    "{ Y_NR__init[i] -> [i, 0, 0]; Y_NR__reduce[i,j] -> [i, 1, j]; Y[i] -> [i, 2, 0]; }"
)
print(alpha.generate(sched))
```

## Other make targets

```
make check    # cargo check --workspace
make clippy   # cargo clippy --workspace --all-targets
make fmt      # cargo fmt --all
make lint     # uv run prek run --all-files (all pre-commit hooks)
make clean    # cargo clean
```

## Pre-commit hooks

This repo uses [`prek`](https://github.com/j178/prek) (a fast, Rust-based reimplementation of
`pre-commit`) to run formatting and lint checks before each commit, managed as a `uv` dev
dependency (see [`pyproject.toml`](pyproject.toml)):

```
uv sync               # one-time: creates .venv, installs prek
uv run prek install   # one-time: installs the git hook
```

Hooks then run automatically on `git commit`. To run them manually against all files:

```
make lint              # or: uv run prek run --all-files
```

Configured hooks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo check --workspace`, plus standard whitespace/YAML/TOML hygiene checks. See
[`.pre-commit-config.yaml`](.pre-commit-config.yaml).

## License

MIT by default (see each crate's `Cargo.toml`), matching the upstream `isl` library. The optional
`barvinok`/`barvinok-sys` crates are **GPL-licensed** and feature-gated off by default — nothing
in a default build pulls in GPL code. See [`docs/design.md`](docs/design.md) for the full
licensing rationale.
