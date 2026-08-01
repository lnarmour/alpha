//! Exercises the typed `ast::` layer over the real fixture corpus — walks every system,
//! variable, equation, and expression via the typed accessors (not just `parse()`), so a bug
//! like "wrong child index" or "missing accessor" surfaces here rather than only when
//! `alpha-model` is built on top of it.

use alpha_syntax::ast::{self, AstNode, CalcExpr, Equation, Expr, FnExpr};
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

fn walk_calc_expr(c: &CalcExpr) {
    match c {
        CalcExpr::Unary(u) => {
            if let Some(inner) = u.operand() {
                walk_calc_expr(&inner);
            }
        }
        CalcExpr::Binary(b) => {
            if let Some(l) = b.lhs() {
                walk_calc_expr(&l);
            }
            if let Some(r) = b.rhs() {
                walk_calc_expr(&r);
            }
        }
        CalcExpr::Paren(p) => {
            if let Some(inner) = p.inner() {
                walk_calc_expr(&inner);
            }
        }
        CalcExpr::Function(f) => {
            for e in f.exprs() {
                walk_fn_expr(&e);
            }
        }
        _ => {}
    }
}

fn walk_fn_expr(e: &FnExpr) {
    match e {
        FnExpr::Literal(_) => {}
        FnExpr::Floor(f) => {
            if let Some(inner) = f.operand() {
                walk_fn_expr(&inner);
            }
        }
        FnExpr::Binary(b) => {
            if let Some(l) = b.lhs() {
                walk_fn_expr(&l);
            }
            if let Some(r) = b.rhs() {
                walk_fn_expr(&r);
            }
        }
    }
}

fn walk_expr(e: &Expr) {
    match e {
        Expr::If(it) => {
            it.cond().as_ref().map(walk_expr);
            it.then_branch().as_ref().map(walk_expr);
            it.else_branch().as_ref().map(walk_expr);
        }
        Expr::Restrict(it) => {
            if let Some(d) = it.domain_source() {
                walk_calc_expr(&d);
            }
            it.expr().as_ref().map(walk_expr);
        }
        Expr::AutoRestrict(it) => {
            it.expr().as_ref().map(walk_expr);
        }
        Expr::Case(it) => {
            for b in it.branches() {
                walk_expr(&b);
            }
        }
        Expr::Variable(_) => {}
        Expr::Dependence(it) => {
            if let Some(f) = it.function() {
                walk_calc_expr(&f);
            }
            it.applied_expr().as_ref().map(walk_expr);
        }
        Expr::Index(it) => {
            if let Some(s) = it.source() {
                walk_calc_expr(&s);
            }
        }
        Expr::Reduce(it) => {
            if let Some(p) = it.projection() {
                walk_calc_expr(&p);
            }
            it.body().as_ref().map(walk_expr);
        }
        Expr::Convolution(it) => {
            if let Some(d) = it.kernel_domain() {
                walk_calc_expr(&d);
            }
            it.kernel_expr().as_ref().map(walk_expr);
            it.data_expr().as_ref().map(walk_expr);
        }
        Expr::Select(it) => {
            if let Some(r) = it.relation() {
                walk_calc_expr(&r);
            }
            it.expr().as_ref().map(walk_expr);
        }
        Expr::MultiArg(it) => {
            for a in it.args() {
                walk_expr(&a);
            }
        }
        Expr::Binary(it) => {
            it.lhs().as_ref().map(walk_expr);
            it.rhs().as_ref().map(walk_expr);
        }
        Expr::Unary(it) => {
            it.operand().as_ref().map(walk_expr);
        }
        Expr::Paren(it) => {
            it.inner().as_ref().map(walk_expr);
        }
        Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => {}
    }
}

#[test]
fn walk_every_fixture_via_typed_ast() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut total_systems = 0usize;
    let mut total_equations = 0usize;

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        assert!(
            parse.errors.is_empty(),
            "{path:?} unexpectedly failed to parse: {:?}",
            parse.errors
        );
        let tree = parse.tree();

        fn walk_systems(systems: impl Iterator<Item = ast::System>, path: &Path) -> (usize, usize) {
            let mut n_systems = 0;
            let mut n_equations = 0;
            for system in systems {
                n_systems += 1;
                assert!(
                    system.name().is_some(),
                    "{path:?}: system with no name token"
                );
                assert!(
                    system.param_domain().is_some(),
                    "{path:?}: system {:?} with no parameter domain",
                    system.name().map(|t| t.text().to_string())
                );
                for section in [system.inputs().map(|s| s.syntax().clone())]
                    .into_iter()
                    .flatten()
                {
                    let _ = section; // just confirming it casts; deeper checks below
                }
                for v in system.inputs().into_iter().flat_map(|s| {
                    s.variables()
                        .map(|v| (v.name(), v.domain()))
                        .collect::<Vec<_>>()
                }) {
                    assert!(v.0.is_some(), "{path:?}: input variable with no name");
                }
                for body in system.bodies() {
                    for eq in body.equations() {
                        n_equations += 1;
                        match eq {
                            Equation::Standard(s) => {
                                assert!(
                                    s.variable_name().is_some(),
                                    "{path:?}: standard equation with no LHS variable name"
                                );
                                if let Some(e) = s.expr() {
                                    walk_expr(&e);
                                } else {
                                    panic!("{path:?}: standard equation with no RHS expression");
                                }
                            }
                            Equation::Use(u) => {
                                assert!(
                                    u.callee().is_some(),
                                    "{path:?}: use equation with no callee system reference"
                                );
                                assert!(
                                    u.call_params().is_some(),
                                    "{path:?}: use equation with no call-params array function"
                                );
                                for e in u.output_exprs() {
                                    walk_expr(&e);
                                }
                                for e in u.input_exprs() {
                                    walk_expr(&e);
                                }
                            }
                        }
                    }
                }
            }
            (n_systems, n_equations)
        }

        let (s1, e1) = walk_systems(tree.systems(), path);
        let mut s2 = 0;
        let mut e2 = 0;
        for pkg in tree.packages() {
            let (a, b) = walk_systems(pkg.systems(), path);
            s2 += a;
            e2 += b;
        }
        total_systems += s1 + s2;
        total_equations += e1 + e2;
    }

    assert!(
        total_systems > 0,
        "walked zero systems across the whole corpus"
    );
    eprintln!(
        "walked {total_systems} systems / {total_equations} equations across {} fixtures",
        files.len()
    );
}
