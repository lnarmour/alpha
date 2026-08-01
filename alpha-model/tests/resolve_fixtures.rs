//! Phase-1 (interface resolution) conformance check against the real `.alpha` corpus: for every
//! system in every fixture, resolve its parameter domain and every declared variable's domain,
//! and confirm it succeeds (or, for the small number of fixtures that use constructs this port
//! doesn't resolve yet — function literals outside equation bodies, fuzzy variables — that the
//! failure is one of those *known* gaps, not a surprise).

use alpha_model::{Diagnostic, Resolver};
use alpha_syntax::ast;
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

/// Is this failure one of phase 1's documented, deliberate gaps (see `resolve.rs`'s module doc)
/// rather than a real bug?
fn is_known_gap(d: &Diagnostic) -> bool {
    matches!(d, Diagnostic::UnsupportedCalculatorOp { .. })
}

#[test]
fn every_system_interface_resolves() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_systems = 0usize;
    let mut n_variables = 0usize;
    let mut known_gaps = 0usize;
    let mut failures = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        assert!(
            parse.errors.is_empty(),
            "{path:?} unexpectedly failed to parse"
        );
        let tree = parse.tree();

        for system in all_systems(&tree) {
            n_systems += 1;
            let ctx = Context::new();
            let mut resolver = Resolver::new(ctx, &system);

            if let Err(d) = resolver.param_domain() {
                if is_known_gap(&d) {
                    known_gaps += 1;
                } else {
                    failures.push((path.clone(), system.name().map(|t| t.text().to_string()), d));
                }
                continue;
            }

            for section_vars in [
                system.inputs().map(|s| s.variables().collect::<Vec<_>>()),
                system.outputs().map(|s| s.variables().collect::<Vec<_>>()),
                system.locals().map(|s| s.variables().collect::<Vec<_>>()),
            ]
            .into_iter()
            .flatten()
            {
                for v in section_vars {
                    let Some(name) = v.name() else { continue };
                    n_variables += 1;
                    if let Err(d) = resolver.variable_domain(name.text()) {
                        if is_known_gap(&d) {
                            known_gaps += 1;
                        } else {
                            failures.push((
                                path.clone(),
                                system.name().map(|t| t.text().to_string()),
                                d,
                            ));
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "resolved {n_systems} systems / {n_variables} variables across {} fixtures ({known_gaps} known gaps)",
        files.len()
    );
    assert!(n_systems > 0, "walked zero systems across the whole corpus");
    assert!(
        failures.is_empty(),
        "{} unexpected resolution failures:\n{:#?}",
        failures.len(),
        failures
    );
}
