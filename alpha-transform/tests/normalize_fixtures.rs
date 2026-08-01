//! `Normalize` conformance: for every `StandardEquation` that successfully lowers (see
//! `lower.rs`'s module doc for why some don't — convolution/fuzzy gaps inherited from
//! `alpha_model::domain`) across the whole real fixture corpus, running `Normalize` should
//! produce a tree satisfying all four of the source system's own documented normal-form
//! invariants:
//! - the parent of a `Case` must be the equation root or a `Reduce`,
//! - the parent of a `Restrict` must be the equation root, a `Reduce`, or a `Case`,
//! - the parent of a `Variable` must be a `Dependence`,
//! - the child of a `Dependence` must be a `Variable` or a constant.

use alpha_model::Resolver;
use alpha_syntax::ast::{self};
use alpha_transform::ir::{Equation, Expr, ExprKind};
use alpha_transform::{lower, normalize, normalize_reduction};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent {
    EquationRoot,
    Reduce,
    Case,
    Dependence,
    Other,
}

fn check_normal_form(e: &Expr, parent: Parent, path: &str, violations: &mut Vec<String>) {
    match &*e.kind {
        ExprKind::Case { branches, .. } => {
            if !matches!(parent, Parent::EquationRoot | Parent::Reduce) {
                violations.push(format!(
                    "{path}: Case under {parent:?} (want EquationRoot/Reduce)"
                ));
            }
            for (i, b) in branches.iter().enumerate() {
                check_normal_form(b, Parent::Case, &format!("{path}/Case[{i}]"), violations);
            }
        }
        ExprKind::Restrict { operand, .. } => {
            if !matches!(parent, Parent::EquationRoot | Parent::Reduce | Parent::Case) {
                violations.push(format!(
                    "{path}: Restrict under {parent:?} (want EquationRoot/Reduce/Case)"
                ));
            }
            check_normal_form(
                operand,
                Parent::Other,
                &format!("{path}/Restrict"),
                violations,
            );
        }
        ExprKind::Variable(name) => {
            if parent != Parent::Dependence {
                violations.push(format!(
                    "{path}: Variable({name}) under {parent:?} (want Dependence)"
                ));
            }
        }
        ExprKind::Dependence { operand, .. } => {
            if !matches!(
                &*operand.kind,
                ExprKind::Variable(_) | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Real(_)
            ) {
                violations.push(format!(
                    "{path}: Dependence child is {} (want Variable or constant)",
                    operand.kind_tag()
                ));
            }
            check_normal_form(
                operand,
                Parent::Dependence,
                &format!("{path}/Dependence"),
                violations,
            );
        }
        ExprKind::AutoRestrict { operand } => {
            check_normal_form(
                operand,
                Parent::Other,
                &format!("{path}/AutoRestrict"),
                violations,
            );
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            check_normal_form(cond, Parent::Other, &format!("{path}/If.cond"), violations);
            check_normal_form(
                then_branch,
                Parent::Other,
                &format!("{path}/If.then"),
                violations,
            );
            check_normal_form(
                else_branch,
                Parent::Other,
                &format!("{path}/If.else"),
                violations,
            );
        }
        ExprKind::Reduce { body, .. } => {
            check_normal_form(body, Parent::Reduce, &format!("{path}/Reduce"), violations);
        }
        ExprKind::Select { operand, .. } => {
            check_normal_form(
                operand,
                Parent::Other,
                &format!("{path}/Select"),
                violations,
            );
        }
        ExprKind::Convolution {
            kernel_expr,
            data_expr,
            ..
        } => {
            check_normal_form(
                kernel_expr,
                Parent::Other,
                &format!("{path}/Conv.kernel"),
                violations,
            );
            check_normal_form(
                data_expr,
                Parent::Other,
                &format!("{path}/Conv.data"),
                violations,
            );
        }
        ExprKind::MultiArg { args, .. } => {
            for (i, a) in args.iter().enumerate() {
                check_normal_form(
                    a,
                    Parent::Other,
                    &format!("{path}/MultiArg[{i}]"),
                    violations,
                );
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            check_normal_form(
                lhs,
                Parent::Other,
                &format!("{path}/Binary.lhs"),
                violations,
            );
            check_normal_form(
                rhs,
                Parent::Other,
                &format!("{path}/Binary.rhs"),
                violations,
            );
        }
        ExprKind::Unary { operand, .. } => {
            check_normal_form(operand, Parent::Other, &format!("{path}/Unary"), violations);
        }
        ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Real(_)
        | ExprKind::IndexFunction { .. }
        | ExprKind::IndexPolynomial { .. } => {}
    }
}

#[test]
fn normalize_reaches_normal_form_across_fixture_corpus() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_equations = 0usize;
    let mut n_skipped = 0usize;
    let mut failures = Vec::new();

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
            let Ok((mut ir_system, lower_diags)) = lower::lower_system(&mut resolver, &system)
            else {
                continue;
            };
            n_skipped += lower_diags.len();

            // The real pipeline order (see `normalize_reduction.rs`'s module doc): extract
            // top-level reductions into their own equations first, *then* normalize — a
            // `Dependence` directly wrapping a `Reduce` (as in `normalizeReductionDeep.alpha`)
            // only reaches normal form once the reduction has somewhere else to live.
            normalize_reduction::apply(&mut ir_system);
            // `deep = true`: the four invariants below are the source system's "deep" normal
            // form (full flattening even of named `case`s) — shallow mode deliberately keeps
            // named cases intact for readability (see `normalize.rs`'s module doc), which on its
            // own does not satisfy invariant #1 for a case nested inside a named case.
            let normalized = normalize::apply(ir_system, true);
            for body in &normalized.bodies {
                for eq in &body.equations {
                    let Equation::Standard(s) = eq else { continue };
                    n_equations += 1;
                    let mut violations = Vec::new();
                    check_normal_form(&s.expr, Parent::EquationRoot, &s.variable, &mut violations);
                    if !violations.is_empty() {
                        failures.push((path.clone(), s.variable.clone(), violations));
                    }
                }
            }
        }
    }

    eprintln!(
        "normalized {n_equations} equations ({n_skipped} equations skipped at lowering) across {} fixtures",
        files.len()
    );
    assert!(
        n_equations > 0,
        "found zero equations across the whole corpus"
    );
    assert!(
        failures.is_empty(),
        "{} equations failed to reach normal form:\n{:#?}",
        failures.len(),
        failures
    );
}
