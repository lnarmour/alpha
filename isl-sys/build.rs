//! Locates and links libisl, then generates raw FFI bindings for the header subset in
//! `wrapper.h`. See `docs/rust-port-design.md` §5 in the workspace root.
//!
//! Currently: pkg-config only. The design doc's planned fallback (vendor + build isl from
//! source when pkg-config can't find a system install, for platforms/CI images that don't have
//! it) isn't implemented yet — this panics with a clear message pointing here instead of
//! silently doing something more complicated. Add the vendored path when that need actually
//! arises (e.g. wiring up CI), rather than building it in advance of a real requirement.

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");

    let isl = pkg_config::Config::new()
        .atleast_version("0.18")
        .probe("isl")
        .expect(
            "isl-sys: could not find libisl via pkg-config. Install it (e.g. `brew install isl \
             pkg-config` on macOS) — see docs/rust-port-design.md §5 in the workspace root. A \
             vendored-source fallback for environments without a system isl is planned but not \
             yet implemented.",
        );

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_type("isl_.*")
        .allowlist_function("isl_.*")
        // Most constants follow the lowercase `isl_*` convention, but a handful of `#define`d
        // ones (notably `ISL_FORMAT_*`, isl's printer output-format selector) use the uppercase
        // `ISL_*` convention instead.
        .allowlist_var("(isl|ISL)_.*")
        // isl's own enums (isl_dim_type, isl_ast_*_type, isl_error, ...) are meant to be used as
        // plain C ints across the FFI boundary, not Rust `enum`s (bindgen's default `enum`
        // codegen doesn't handle values C code might synthesize outside the declared variants
        // gracefully) — module-scoped constants match how the C API itself treats them.
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .derive_debug(true)
        .generate_comments(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    for path in &isl.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let bindings = builder
        .generate()
        .expect("isl-sys: bindgen failed to generate bindings");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    bindings
        .write_to_file(std::path::Path::new(&out_dir).join("bindings.rs"))
        .expect("isl-sys: could not write bindings.rs");
}
