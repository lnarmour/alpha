//! Phase 3/4 conformance check: for every `StandardEquation` in the real fixture corpus, compute
//! its expression domain (phase 3, bottom-up) and context domain (phase 4, top-down), and for
//! every `UseEquation`, its expression domain only (see `alpha_model::domain`'s module doc for
//! why `UseEquation` context domains are a deliberate, not-yet-implemented gap). Also checks the
//! convolution carve-out reports the expected diagnostic rather than a wrong answer.

use alpha_model::domain::Domains;
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

/// Same two negative fixtures `function_fixtures.rs` carves out (undeclared array-notation
/// indices), plus `systemBody1.alpha` — its own header comment says "All cases are expected to
/// throw AlphaIssues", and `systemBody1a`'s two unguarded `SystemBody`s are exactly
/// `Diagnostic::MultipleUnrestrictedSystemBody`'s trigger.
fn is_expected_invalid_fixture(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("src-invalid/syntax-tests/array1.alpha")
        || s.contains("src-invalid/syntax-tests/array2.alpha")
        || s.contains("src-invalid/syntax-tests/systemBody1.alpha")
}

/// `alpha_model::domain`'s two documented scope boundaries: convolution's own expression/context
/// domain (needs kernel-domain vertex enumeration, not yet bound — see the module doc) surfaces
/// as this specific `UnsupportedCalculatorOp`, not a wrong answer.
fn is_expected_unsupported(d: &Diagnostic) -> bool {
    matches!(
        d,
        Diagnostic::UnsupportedCalculatorOp { operator, .. }
            if operator.contains("convolution")
    )
}

#[test]
fn equation_domains_resolve_across_fixture_corpus() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_equations = 0usize;
    let mut n_context_domains = 0usize;
    let mut failures = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        if !parse.errors.is_empty() {
            continue; // covered by alpha-syntax's own fixture tests
        }
        let tree = parse.tree();

        for system in all_systems(&tree) {
            let ctx = Context::new();
            let mut resolver = Resolver::new(ctx, &system);

            for body in system.bodies() {
                for eq in body.equations() {
                    n_equations += 1;
                    let mut domains: Domains = Domains::new();
                    let result = resolver.equation_expression_domains(&eq, &mut domains);
                    match result {
                        Ok(()) => {}
                        Err(_) if is_expected_invalid_fixture(path) => continue,
                        Err(d) if is_expected_unsupported(&d) => continue,
                        Err(d) => {
                            failures.push((path.clone(), "expression domain".to_string(), d));
                            continue;
                        }
                    }

                    let Equation::Standard(s) = &eq else { continue };
                    let mut contexts: Domains = Domains::new();
                    match resolver.equation_context_domains(s, &body, &domains, &mut contexts) {
                        Ok(()) => {
                            n_context_domains += contexts.len();
                        }
                        Err(_) if is_expected_invalid_fixture(path) => {}
                        Err(d) if is_expected_unsupported(&d) => {}
                        Err(d) => {
                            failures.push((path.clone(), "context domain".to_string(), d));
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "computed expression domains for {n_equations} equations, {n_context_domains} \
         context-domain entries, across {} fixtures",
        files.len()
    );
    assert!(
        n_equations > 0,
        "found zero equations across the whole corpus"
    );
    assert!(
        failures.is_empty(),
        "{} unexpected domain-inference failures:\n{:#?}",
        failures.len(),
        failures
    );
}
