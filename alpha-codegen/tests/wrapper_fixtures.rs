//! Wrapper-generation conformance (issue #23): `generate_wrapper` should succeed for every system
//! `generate_system` itself succeeds for, across the whole fixture corpus — plus two synthetic 3-D
//! and 4-D systems that exercise the general `k`-level pointer-chain construction the bundled
//! corpus alone never reaches (`CopyInput`/`PrefixScan` are 1-D, `LUDecomposition` is 2-D).
//!
//! Also compiles and runs the generated system + its wrapper together with `cc`, for `k = 1, 2, 3`
//! — proving the malloc/wiring/call/free path actually links and executes, not just that it parses
//! as valid Rust-generated text (mirrors `scheduledc_e2e.rs`'s own `compile_and_run` approach).

use alpha_model::Resolver;
use isl::Context;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
mod fixture_util;
use fixture_util::{
    all_alpha_files, all_systems, fixtures_root, is_known_scope_boundary, lowered_system,
};

#[test]
fn generates_for_every_system_write_c_itself_generates() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (bundled fixtures missing)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_wrapped = 0usize;
    let mut unexpected_failures = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        if !parse.errors.is_empty() {
            continue;
        }
        let tree = parse.tree();

        for system in all_systems(&tree) {
            let ctx = Context::new();
            let mut resolver = Resolver::new(ctx, &system);
            let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
            if !diagnostics.is_empty() {
                continue;
            }
            let Ok((mut ir_system, _lower_diags)) =
                alpha_transform::lower::lower_system(&mut resolver, &system)
            else {
                continue;
            };
            alpha_transform::normalize_reduction::apply(&mut ir_system);
            let normalized = alpha_transform::normalize::apply(ir_system, true);

            if let Err(alpha_codegen::CodegenError::Unsupported(msg)) =
                alpha_codegen::generate_system(&normalized)
            {
                assert!(
                    is_known_scope_boundary(&msg),
                    "{path:?}: unexpected generate_system failure: {msg}"
                );
                continue;
            }

            match alpha_codegen::generate_wrapper(&normalized) {
                Ok(_) => n_wrapped += 1,
                Err(e) => unexpected_failures.push((path.clone(), e.to_string())),
            }
        }
    }

    eprintln!(
        "wrapper: generated for {n_wrapped} systems across {} fixtures",
        files.len()
    );
    assert!(n_wrapped > 0, "generated a wrapper for zero systems");
    assert!(
        unexpected_failures.is_empty(),
        "{} systems failed wrapper generation unexpectedly:\n{:#?}",
        unexpected_failures.len(),
        unexpected_failures
    );
}

const COPY_3D_SRC: &str = "affine Copy3D [N]->{:N>0}
    inputs  X: {[i,j,k]: 0<=i<N and 0<=j<N and 0<=k<N}
    outputs Y: {[i,j,k]: 0<=i<N and 0<=j<N and 0<=k<N}
    let Y[i,j,k] = X[i,j,k];
.";

const COPY_4D_SRC: &str = "affine Copy4D [N]->{:N>0}
    inputs  X: {[i,j,k,l]: 0<=i<N and 0<=j<N and 0<=k<N and 0<=l<N}
    outputs Y: {[i,j,k,l]: 0<=i<N and 0<=j<N and 0<=k<N and 0<=l<N}
    let Y[i,j,k,l] = X[i,j,k,l];
.";

#[test]
fn generates_for_synthetic_3d_and_4d_systems_not_in_the_fixture_corpus() {
    for src in [COPY_3D_SRC, COPY_4D_SRC] {
        let normalized = lowered_system(src);
        alpha_codegen::generate_system(&normalized).expect("WriteC generation should succeed");
        alpha_codegen::generate_wrapper(&normalized).expect("wrapper generation should succeed");
    }
}

/// Compiles the generated system C and its wrapper C as two separate translation units (matching
/// how `alphac --wrapper` actually emits them) into one binary, runs it, and returns whether it
/// exited 0.
fn compile_and_run(generated_c: &str, wrapper_c: &str, tag: &str, run_args: &[&str]) -> bool {
    let dir = std::env::temp_dir();
    let c_path: PathBuf = dir.join(format!("alpha_rs_wrapper_e2e_{tag}.c"));
    let wrapper_path: PathBuf = dir.join(format!("alpha_rs_wrapper_e2e_{tag}_wrapper.c"));
    let bin_path: PathBuf = dir.join(format!("alpha_rs_wrapper_e2e_{tag}"));
    std::fs::write(&c_path, generated_c).expect("writing generated C to a temp file");
    std::fs::write(&wrapper_path, wrapper_c).expect("writing wrapper C to a temp file");

    let compile = Command::new("cc")
        .args(["-std=c99", "-o"])
        .arg(&bin_path)
        .arg(&c_path)
        .arg(&wrapper_path)
        .arg("-lm")
        .output()
        .expect("running cc — a C compiler is required to build this workspace at all");
    assert!(
        compile.status.success(),
        "cc failed to compile generated C + wrapper ({tag}):\n{}\n--- generated ---\n{generated_c}\n--- wrapper ---\n{wrapper_c}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&bin_path)
        .args(run_args)
        .output()
        .unwrap_or_else(|e| panic!("running compiled binary ({tag}): {e}"));
    if !run.status.success() {
        println!(
            "binary ({tag}) exited with {:?}\nsignal: {:?}\nstdout: {}\nstderr: {}",
            run.status.code(),
            run.status.signal(),
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
    }
    run.status.success()
}

const COPY_1D_SRC: &str = "affine Copy1D [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = X[i];
.";

const COPY_2D_SRC: &str = "affine Copy2D [N]->{:N>0}
    inputs  X: {[i,j]: 0<=i<N and 0<=j<N}
    outputs Y: {[i,j]: 0<=i<N and 0<=j<N}
    let Y[i,j] = X[i,j];
.";

#[test]
fn wrapper_compiles_and_runs_at_k1_k2_k3() {
    for (src, tag) in [
        (COPY_1D_SRC, "k1"),
        (COPY_2D_SRC, "k2"),
        (COPY_3D_SRC, "k3"),
    ] {
        let normalized = lowered_system(src);
        let generated =
            alpha_codegen::generate_system(&normalized).expect("WriteC generation should succeed");
        let wrapper = alpha_codegen::generate_wrapper(&normalized)
            .expect("wrapper generation should succeed");

        assert!(
            !compile_and_run(&generated, &wrapper, tag, &[]),
            "wrapper for {tag} did not throw an error when run without arguments"
        );
        assert!(
            !compile_and_run(
                &generated,
                &wrapper,
                &format!("{tag}_argv_illegal"),
                &["foo"]
            ),
            "wrapper for {tag} did not throw an error when given an illegal argument"
        );
        assert!(
            compile_and_run(&generated, &wrapper, &format!("{tag}_argv_legal"), &["7"]),
            "wrapper for {tag} did not run to completion when given legal arguments"
        );
    }
}
