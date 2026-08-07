//! With the `extension-module` feature, pyo3 deliberately doesn't link libpython (the compiled
//! module is loaded by an interpreter via `dlopen` at runtime, not linked against one) — see
//! README.md's "Plain `cargo build -p alpha-py` fails to link either way" note. maturin papers
//! over this for real wheel builds by injecting `-undefined dynamic_lookup` (macOS) itself; this
//! calls pyo3's own helper for the same flag so bare `cargo build`/`cargo test --workspace` (and
//! rust-analyzer) also link, without maturin in the loop.

fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
