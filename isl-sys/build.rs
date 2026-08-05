//! Locates and links libisl, then generates raw FFI bindings for the header subset in
//! `wrapper.h`.
//!
//! Currently: pkg-config only. A planned fallback (vendor + build isl from source when
//! pkg-config can't find a system install, for platforms/CI images that don't have it) isn't
//! implemented yet — this panics with a clear message instead of silently doing something more
//! complicated. Add the vendored path when that need actually arises (e.g. wiring up CI), rather
//! than building it in advance of a real requirement.
//!
//! Linking is dynamic by default. Setting the `ISL_STATIC` env var (a convention the `pkg-config`
//! crate already recognizes — see its own docs) links libisl/libgmp statically instead, so the
//! final binary/cdylib has no runtime dependency on them being installed on the system. This is
//! deliberately an env var rather than a Cargo feature: a Cargo feature is a property of the
//! dependency graph, so turning it on for one workspace member (e.g. the VS Code extension's
//! native addon) would silently turn it on for every other member built in the same `cargo build
//! --workspace` invocation too (e.g. the `alphac` CLI, in CI). An env var only affects the one
//! `cargo build`/`napi build` invocation it's set for.
//!
//! We emit our own `cargo:rustc-link-lib=static=...` directives for this rather than delegating to
//! `pkg_config::Config::statik(true)`: that built-in support refuses to treat a lib as statically
//! available whenever it lives under a "system root" (`/usr` on Linux, `/Library`/`/System` on
//! macOS) — which is exactly where `apt install libisl-dev libgmp-dev` puts it on Debian/Ubuntu, so
//! `.statik(true)` silently linked dynamically there instead (verified against a real Ubuntu 24.04
//! container; Homebrew's `/opt/homebrew` prefix on macOS isn't a "system root" by that check, so it
//! happened to work there by accident). We already independently verify the `.a` exists below, so
//! that system-root caution doesn't buy us anything here — it just breaks the one platform
//! (Debian/Ubuntu, i.e. the extension's Linux release build) where distro static archives are the
//! only ones available.

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=ISL_STATIC");

    let want_static = std::env::var_os("ISL_STATIC").is_some();

    let isl = pkg_config::Config::new()
        .atleast_version("0.18")
        // We print our own `cargo:rustc-*` directives below instead of relying on the crate's
        // built-in ones, so that we (not `Config::statik`'s system-root check) decide static vs.
        // dynamic — see the module doc comment above for why.
        .cargo_metadata(false)
        .probe("isl")
        .expect(
            "isl-sys: could not find libisl via pkg-config. Install it (e.g. `brew install isl \
             pkg-config` on macOS). A vendored-source fallback for environments without a \
             system isl is planned but not yet implemented.",
        );

    for dir in &isl.link_paths {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for lib in &isl.libs {
        let has_static_archive = isl
            .link_paths
            .iter()
            .any(|dir| dir.join(format!("lib{lib}.a")).is_file());
        assert!(
            !want_static || has_static_archive,
            "isl-sys: ISL_STATIC is set, but no static archive (lib{lib}.a) was found for \
             `-l{lib}` in any of pkg-config's reported search paths ({:?}). Install a static \
             build of {lib} (e.g. on Debian/Ubuntu, `apt-get install lib{lib}-dev` usually ships \
             both the shared library and the static archive — double check it's actually \
             installed).",
            isl.link_paths,
        );
        if want_static {
            println!("cargo:rustc-link-lib=static={lib}");
        } else {
            println!("cargo:rustc-link-lib={lib}");
        }
    }

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
