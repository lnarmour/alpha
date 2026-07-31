//! Phase 6 conformance: every `src-valid` fixture in the corpus is expected to produce zero
//! well-formedness diagnostics; `src-invalid` fixtures are, by directory convention, deliberately
//! abnormal in some way, so they're excluded from that regression check wholesale (unlike
//! `domain_fixtures.rs`/`function_fixtures.rs`'s narrow per-file allowlists — phase 6 catches
//! several *semantically* off patterns, most notably unbounded reductions, that several
//! `src-invalid/transformation-tests/*` fixtures use on purpose as transformation-precondition
//! test material — see the module-level doc comments in `higherOrderOperator1.alpha`/
//! `idempotence1.alpha`/etc., which explain their reductions are deliberately unbounded).
//!
//! Targeted unit tests below confirm the specific diagnostics fire on the specific fixtures known
//! to exercise them.

use alpha_model::completeness::{
    check_case_branches, check_reduce_bounded, check_standard_equation_completeness,
    check_system_bodies, check_undefined_variables, check_use_equation_recursion,
};
use alpha_model::{Diagnostic, Resolver};
use alpha_syntax::ast::{self, Equation};
use isl::Context;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alpha-language/tests")
}

fn all_alpha_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {dir:?}: {e}")) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            all_alpha_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "alpha") {
            out.push(path);
        }
    }
}

fn all_systems(root: &ast::Root) -> Vec<ast::System> {
    let mut out: Vec<ast::System> = root.systems().collect();
    fn walk_pkg(pkg: &ast::AlphaPackage, out: &mut Vec<ast::System>) {
        out.extend(pkg.systems());
        for sub in pkg.packages() {
            walk_pkg(&sub, out);
        }
    }
    for pkg in root.packages() {
        walk_pkg(&pkg, &mut out);
    }
    out
}

/// Every phase-6 diagnostic for one system, aggregating all the individual check functions —
/// mirrors what a real driver (`alphac`, eventually) would do once phase 6 has a single
/// entry point of its own.
fn check_all(resolver: &mut Resolver, system: &ast::System) -> Vec<Diagnostic> {
    let (domains, contexts, mut diags) = resolver.analyze_system(system);

    diags.extend(check_system_bodies(resolver, system));
    diags.extend(check_case_branches(system, &contexts));
    diags.extend(check_reduce_bounded(system, &contexts));
    diags.extend(check_undefined_variables(system));

    for body in system.bodies() {
        for eq in body.equations() {
            match &eq {
                Equation::Standard(s) => {
                    diags.extend(check_standard_equation_completeness(
                        resolver, s, &body, &domains,
                    ));
                }
                Equation::Use(u) => {
                    diags.extend(check_use_equation_recursion(resolver, u, system));
                }
            }
        }
    }

    diags
}

#[test]
fn every_src_valid_fixture_has_zero_completeness_diagnostics() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_systems = 0usize;
    let mut failures = Vec::new();

    for path in &files {
        if path.to_string_lossy().contains("/src-invalid/") {
            continue;
        }
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        if !parse.errors.is_empty() {
            continue;
        }
        let tree = parse.tree();

        for system in all_systems(&tree) {
            n_systems += 1;
            let ctx = Context::new();
            let mut resolver = Resolver::new(ctx, &system);
            // Convolution's own domain is a documented gap in `alpha_model::domain` (needs
            // isl vertex-enumeration this crate doesn't bind yet) — expected on the handful of
            // fixtures using `conv(...)`, not a phase-6 regression.
            let diags: Vec<_> = check_all(&mut resolver, &system)
                .into_iter()
                .filter(|d| {
                    !matches!(
                        d,
                        Diagnostic::UnsupportedCalculatorOp { operator, .. }
                            if operator.contains("convolution")
                    )
                })
                .collect();
            if !diags.is_empty() {
                failures.push((path.clone(), diags));
            }
        }
    }

    eprintln!(
        "checked phase-6 completeness for {n_systems} src-valid systems across {} fixtures",
        files.len()
    );
    assert!(n_systems > 0, "found zero systems across src-valid");
    assert!(
        failures.is_empty(),
        "{} src-valid fixtures unexpectedly reported completeness diagnostics:\n{:#?}",
        failures.len(),
        failures
    );
}

fn find_system(root: &ast::Root, name: &str) -> Option<ast::System> {
    fn walk_pkg(pkg: &ast::AlphaPackage, name: &str) -> Option<ast::System> {
        for s in pkg.systems() {
            if s.name().is_some_and(|t| t.text() == name) {
                return Some(s);
            }
        }
        for sub in pkg.packages() {
            if let Some(s) = walk_pkg(&sub, name) {
                return Some(s);
            }
        }
        None
    }
    for s in root.systems() {
        if s.name().is_some_and(|t| t.text() == name) {
            return Some(s);
        }
    }
    for pkg in root.packages() {
        if let Some(s) = walk_pkg(&pkg, name) {
            return Some(s);
        }
    }
    None
}

fn system_named(root: &ast::Root, name: &str) -> ast::System {
    find_system(root, name).unwrap_or_else(|| panic!("no system named {name} found"))
}

fn parse_fixture(rel: &str) -> ast::Root {
    let path = fixtures_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
    let parse = alpha_syntax::parse(&src);
    assert!(
        parse.errors.is_empty(),
        "{rel}: unexpected syntax errors: {:?}",
        parse.errors
    );
    parse.tree()
}

#[test]
fn unbounded_reduction_body_is_flagged() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: sibling alpha-language checkout not found");
        return;
    }
    let tree = parse_fixture(
        "alpha.model.tests/resources/src-invalid/unbounded-reduction/unboundedReduction.alpha",
    );
    let system = system_named(&tree, "unboundedBody");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let diags = check_all(&mut resolver, &system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::UnboundedReductionBody { .. })),
        "expected UnboundedReductionBody: {diags:#?}"
    );
}

#[test]
fn self_recursive_use_equation_is_flagged() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: sibling alpha-language checkout not found");
        return;
    }
    let tree =
        parse_fixture("alpha.model.tests/resources/src-invalid/syntax-tests/subsystem1.alpha");
    let system = system_named(&tree, "subsystem1c");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let diags = check_all(&mut resolver, &system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::InfinitelyRecursiveUseEquation { .. })),
        "expected InfinitelyRecursiveUseEquation: {diags:#?}"
    );
}

#[test]
fn incomplete_and_empty_system_bodies_are_flagged() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: sibling alpha-language checkout not found");
        return;
    }
    let tree =
        parse_fixture("alpha.model.tests/resources/src-invalid/syntax-tests/systemBody1.alpha");

    let empty_body_system = system_named(&tree, "systemBody1b");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &empty_body_system);
    let diags = check_all(&mut resolver, &empty_body_system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::EmptySystemBody { .. })),
        "expected EmptySystemBody for systemBody1b: {diags:#?}"
    );

    let incomplete_system = system_named(&tree, "systemBody1c");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &incomplete_system);
    let diags = check_all(&mut resolver, &incomplete_system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::IncompleteSystem { .. })),
        "expected IncompleteSystem for systemBody1c: {diags:#?}"
    );
}

#[test]
fn undefined_locals_and_outputs_are_flagged() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: sibling alpha-language checkout not found");
        return;
    }

    // `A` is defined (`A[i] = B[i];`); `B` is a local it references but that's never itself
    // defined in this body.
    let tree = parse_fixture(
        "alpha.model.tests/resources/src-invalid/undefined-locals-outputs/undefinedLocals.alpha",
    );
    let system = system_named(&tree, "undefinedLocals");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let diags = check_all(&mut resolver, &system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::UndefinedVariable { name, .. } if name == "B")),
        "expected UndefinedVariable('B'): {diags:#?}"
    );

    // `Z` is an output with no defining equation at all.
    let tree = parse_fixture(
        "alpha.model.tests/resources/src-invalid/undefined-locals-outputs/undefinedOutputs.alpha",
    );
    let system = system_named(&tree, "undefinedOutputs");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let diags = check_all(&mut resolver, &system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::UndefinedVariable { name, .. } if name == "Z")),
        "expected UndefinedVariable('Z'): {diags:#?}"
    );

    // `Z` is an output with no defining equation, even though it (unlike `undefinedLocals`'s `B`)
    // is never referenced anywhere — outputs are checked unconditionally, unlike locals.
    let tree = parse_fixture(
        "alpha.model.tests/resources/src-invalid/undefined-locals-outputs/undefinedUnusedOutputs.alpha",
    );
    let system = system_named(&tree, "undefinedUnusedOutputs");
    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let diags = check_all(&mut resolver, &system);
    assert!(
        diags
            .iter()
            .any(|d| matches!(d, Diagnostic::UndefinedVariable { name, .. } if name == "Z")),
        "expected UndefinedVariable('Z'): {diags:#?}"
    );
}
