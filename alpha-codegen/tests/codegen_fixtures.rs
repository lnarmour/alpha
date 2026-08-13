//! Codegen conformance: for every real `.alpha` fixture whose systems fully resolve (parse
//! cleanly, produce zero `alpha_model::analyze_system` diagnostics), running the full
//! `lower → NormalizeReduction → Normalize → generate_system` pipeline should never produce an
//! *isl* error or an internal-error `Unsupported` message — those indicate a real bug in this
//! crate. A `CodegenError::Unsupported` for one of this session's documented, deliberate scope
//! boundaries (`UseEquation`, `Select`, `IndexPolynomial`, `argreduce` — see `writec`'s module
//! doc) is expected and tracked separately, not a failure.

use alpha_model::Resolver;
use isl::Context;
mod fixture_util;
use fixture_util::{all_alpha_files, all_systems, fixtures_root, is_known_scope_boundary};

#[test]
fn generates_or_reports_a_known_scope_boundary_across_fixture_corpus() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (bundled fixtures missing)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_generated = 0usize;
    let mut n_known_scope_boundary = 0usize;
    let mut n_skipped_diagnostics = 0usize;
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
                n_skipped_diagnostics += 1;
                continue;
            }

            let Ok((mut ir_system, _lower_diags)) =
                alpha_transform::lower::lower_system(&mut resolver, &system)
            else {
                continue;
            };
            alpha_transform::normalize_reduction::apply(&mut ir_system);
            let normalized = alpha_transform::normalize::apply(ir_system, true);

            match alpha_codegen::generate_system(&normalized) {
                Ok(_) => n_generated += 1,
                Err(alpha_codegen::CodegenError::Unsupported(msg))
                    if is_known_scope_boundary(&msg) =>
                {
                    n_known_scope_boundary += 1;
                }
                Err(e) => unexpected_failures.push((path.clone(), e.to_string())),
            }
        }
    }

    eprintln!(
        "codegen: {n_generated} systems generated, {n_known_scope_boundary} hit a known scope \
         boundary, {n_skipped_diagnostics} systems skipped (non-zero analyze_system diagnostics) \
         across {} fixtures",
        files.len()
    );
    assert!(
        n_generated > 0,
        "generated code for zero systems across the whole corpus"
    );
    assert!(
        unexpected_failures.is_empty(),
        "{} systems failed codegen unexpectedly:\n{:#?}",
        unexpected_failures.len(),
        unexpected_failures
    );
}

/// The `ScheduledC`-side counterpart of the test above (`docs/codegen-test-design.md` §5.10):
/// `generate_scheduled_system` was never run against the bundled fixture corpus at all before this
/// — only `generate_system` (`WriteC`) was. Uses the empty/identity-default schedule text (§6);
/// a fixture with a real reduce dependency is *expected* to reject that as
/// `CodegenError::IllegalSchedule` (§7 — the identity default has no reason to happen to interleave
/// a reduce's accumulation correctly), so that's treated as a recognized, non-bug outcome here, the
/// same way a documented `Unsupported` scope boundary is — only an *unexpected* error variant or
/// message should fail this test.
#[test]
fn scheduled_c_generates_or_reports_a_known_outcome_across_fixture_corpus() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (bundled fixtures missing)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_generated = 0usize;
    let mut n_illegal_schedule = 0usize;
    let mut n_known_scope_boundary = 0usize;
    let mut n_skipped_diagnostics = 0usize;
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
                n_skipped_diagnostics += 1;
                continue;
            }

            let Ok((mut ir_system, _lower_diags)) =
                alpha_transform::lower::lower_system(&mut resolver, &system)
            else {
                continue;
            };
            alpha_transform::normalize_reduction::apply(&mut ir_system);
            let normalized = alpha_transform::normalize::apply(ir_system, true);

            match alpha_codegen::generate_scheduled_system(&normalized, "") {
                Ok(_) => n_generated += 1,
                Err(alpha_codegen::CodegenError::IllegalSchedule(_)) => {
                    n_illegal_schedule += 1;
                }
                Err(alpha_codegen::CodegenError::Unsupported(msg))
                    if is_known_scope_boundary(&msg) =>
                {
                    n_known_scope_boundary += 1;
                }
                Err(e) => unexpected_failures.push((path.clone(), e.to_string())),
            }
        }
    }

    eprintln!(
        "ScheduledC: {n_generated} systems generated, {n_illegal_schedule} rejected the identity \
         default as illegal, {n_known_scope_boundary} hit a known scope boundary, \
         {n_skipped_diagnostics} systems skipped (non-zero analyze_system diagnostics) across {} \
         fixtures",
        files.len()
    );
    assert!(
        n_generated + n_illegal_schedule > 0,
        "generated code or got a recognized illegal-schedule rejection for zero systems across \
         the whole corpus"
    );
    assert!(
        unexpected_failures.is_empty(),
        "{} systems failed ScheduledC codegen unexpectedly:\n{:#?}",
        unexpected_failures.len(),
        unexpected_failures
    );
}
