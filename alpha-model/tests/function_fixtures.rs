//! Phase-2 (partial) conformance check: for every equation in every system in the real fixture
//! corpus, walk its body looking for `DependenceExpression`s (`f@expr` or `X[expr]` array
//! notation) and resolve their function into a real isl `MultiAff`, using the equation's own
//! ambient index names (`StandardEquation`'s `[i,j]`, or `UseEquation`'s `with [i,j]`) as
//! `ArrayFunction`'s implicit input tuple.

use alpha_model::Resolver;
use alpha_syntax::ast::{self, AstNode, CalcExpr, Equation, Expr};
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

/// The index names `UseEquation`'s `over` clause introduces (in scope alongside `with`'s names —
/// see the call site). Handles the two shapes real fixtures use: `RectangularDomain`'s `as
/// [k]` names, and a raw `Domain`'s own leading `[i,j]` tuple (read directly off its text, since
/// that's simpler here than a full calculator-expression evaluation for a test that's only
/// gathering names).
fn instantiation_domain_index_names(u: &ast::UseEquation) -> Vec<String> {
    match u.instantiation_domain() {
        Some(CalcExpr::RectangularDomain(rect)) => {
            rect.index_names().map(|t| t.text().to_string()).collect()
        }
        Some(CalcExpr::Domain(d)) => domain_tuple_names(&d),
        _ => Vec::new(),
    }
}

/// A `Domain`'s (`{[i,j]:...}`) own leading tuple names, read directly off its raw text —
/// simpler here than a full calculator-expression evaluation for a test that's only gathering
/// names for context-tracking purposes.
fn domain_tuple_names(d: &ast::Domain) -> Vec<String> {
    let text = d.syntax().text().to_string();
    let inner = text
        .trim_start_matches('{')
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if inner.is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    }
}

/// A `Relation`'s (`{[i,j]->[x]:...}`) range-side (output) tuple names.
fn relation_range_names(r: &ast::Relation) -> Vec<String> {
    let text = r.syntax().text().to_string();
    let Some(after_arrow) = text.split("->").nth(1) else {
        return Vec::new();
    };
    let inner = after_arrow
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if inner.is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    }
}

/// Reads an `ArrayFunction`'s raw comma-separated elements, keeping only the ones that are bare
/// identifiers (vs. a general expression) — see `collect_functions`'s `Expr::Reduce` arm for why.
fn bare_identifier_elements(af: &ast::ArrayFunction) -> Vec<String> {
    af.raw_elements()
        .into_iter()
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect()
}

/// Extends `base` with `new`, skipping any name already present — a construct that
/// re-introduces an already-in-scope name (e.g. a `RestrictExpression`'s own domain tuple
/// happening to reuse the outer equation's own index name, `{[i]:i>=0} : A[i-N]` inside an
/// equation whose LHS is already `X[i]`) isn't shadowing, it's the *same* name; without this a
/// naive extend would produce a tuple with a duplicate name, which isl rejects.
fn extend_unique(base: &[String], new: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = base.to_vec();
    for n in new {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// Collects every `DependenceExpression`'s function (and, for `ReduceExpression`, its
/// projection — same `CalcExpr::Function`/`ArrayFunction` shape) found anywhere in `expr`,
/// alongside the index-name context in scope at that point. Reduction projections extend the
/// ambient context with their own declared names for their *body*, matching the source system's
/// context-stack behavior for `AbstractReduceExpression` — but the projection function itself
/// (what we're collecting here) resolves in the *outer* context, since it's the projection that
/// introduces the new names, not something already having them in scope.
fn collect_functions(expr: &Expr, context: &[String], out: &mut Vec<(CalcExpr, Vec<String>)>) {
    match expr {
        Expr::Dependence(d) => {
            if let Some(f) = d.function() {
                if matches!(f, CalcExpr::Function(_) | CalcExpr::ArrayFunction(_)) {
                    out.push((f, context.to_vec()));
                }
            }
            if let Some(inner) = d.applied_expr() {
                collect_functions(&inner, context, out);
            }
        }
        Expr::Reduce(r) => {
            // Deliberately *not* collecting the projection itself here: a reduce's projection
            // function has a genuinely different shape than a plain `ArrayFunction` — per the
            // source system's `ExpressionDomainCalculator`, it implicitly extends the ambient
            // context with its own new bound variable(s) before projecting them back out (e.g.
            // `reduce(+, [k], A[i,k]*B[k,j])`'s `[k]` really means "the body's domain is
            // `[i,j,k]`, project out `k`", not "`[i,j] -> [k]`" as a bare `ArrayFunction` would
            // read). That's expression-domain-inference territory (phases 3/4), not phase 2's
            // "resolve a function literal" scope — `eval_function` isn't the right tool for it,
            // so this test doesn't misuse it here.
            //
            // The body *does* need the right index names in its own ambient context, though
            // (`L[i,k]` inside the reduce needs `k` in scope) — this test approximates that
            // (without the full expression-domain-inference machinery above) two different ways
            // depending on which form the projection takes:
            // - Bare `[k,...]` (`ArrayFunction`) sugar *extends* the ambient context with those
            //   names (it has no input tuple of its own — it relies on the names already being
            //   in scope, same as `DependenceExpression`'s array notation always does).
            // - A full `(i,j->...)` (`Function`) *replaces* the ambient context outright: it's
            //   self-declaring, and its own input names are the body's *entire* index space,
            //   independent of whatever the enclosing equation's LHS did or didn't declare
            //   (real fixtures rely on exactly this — e.g. a scalar-LHS equation whose only
            //   content is `reduce(+, (i,j->i), A[i,j])`, where `i`/`j` come purely from the
            //   reduce's own function, not from the (absent) LHS index list).
            if let Some(body) = r.body() {
                let extended = match r.projection() {
                    Some(CalcExpr::ArrayFunction(af)) => {
                        extend_unique(context, bare_identifier_elements(&af))
                    }
                    Some(CalcExpr::Function(f)) => {
                        f.index_names().map(|t| t.text().to_string()).collect()
                    }
                    _ => context.to_vec(),
                };
                collect_functions(&body, &extended, out);
            }
        }
        Expr::If(e) => {
            for sub in [e.cond(), e.then_branch(), e.else_branch()]
                .into_iter()
                .flatten()
            {
                collect_functions(&sub, context, out);
            }
        }
        Expr::Restrict(e) => {
            // `{[x]:x>=0} : A[x]` — the restrict domain's own tuple (`x`) is in scope for the
            // restricted sub-expression, alongside the outer context.
            let extended = match e.domain_source() {
                Some(CalcExpr::Domain(d)) => extend_unique(context, domain_tuple_names(&d)),
                _ => context.to_vec(),
            };
            if let Some(sub) = e.expr() {
                collect_functions(&sub, &extended, out);
            }
        }
        Expr::AutoRestrict(e) => {
            if let Some(sub) = e.expr() {
                collect_functions(&sub, context, out);
            }
        }
        Expr::Case(e) => {
            for branch in e.branches() {
                collect_functions(&branch, context, out);
            }
        }
        Expr::Convolution(e) => {
            // `[2] as [x]` (a `RectangularDomain`) introduces `x` as a new bound name in scope
            // for *both* the kernel weight expression and the data expression (alongside the
            // outer context, unlike a reduce's `Function`-form projection, which replaces it) —
            // e.g. `conv([2] as [x], W[x], A[i+x])` needs `x` in scope for `W[x]` and `i`+`x`
            // for `A[i+x]`.
            let extended = match e.kernel_domain() {
                Some(CalcExpr::RectangularDomain(rect)) => {
                    extend_unique(context, rect.index_names().map(|t| t.text().to_string()))
                }
                _ => context.to_vec(),
            };
            for sub in [e.kernel_expr(), e.data_expr()].into_iter().flatten() {
                collect_functions(&sub, &extended, out);
            }
        }
        Expr::Select(e) => {
            // `select {[i,j]->[x]:} from A[x]` — the select relation's *range*-side tuple (`x`)
            // is in scope for the selected sub-expression.
            let extended = match e.relation() {
                Some(CalcExpr::Relation(r)) => extend_unique(context, relation_range_names(&r)),
                _ => context.to_vec(),
            };
            if let Some(sub) = e.expr() {
                collect_functions(&sub, &extended, out);
            }
        }
        Expr::MultiArg(e) => {
            for a in e.args() {
                collect_functions(&a, context, out);
            }
        }
        Expr::Binary(e) => {
            for sub in [e.lhs(), e.rhs()].into_iter().flatten() {
                collect_functions(&sub, context, out);
            }
        }
        Expr::Unary(e) => {
            if let Some(sub) = e.operand() {
                collect_functions(&sub, context, out);
            }
        }
        Expr::Paren(e) => {
            if let Some(sub) = e.inner() {
                collect_functions(&sub, context, out);
            }
        }
        Expr::Variable(_) | Expr::Index(_) | Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => {}
    }
}

/// `src-invalid/syntax-tests/array1.alpha`'s `B = A[i];` and `array2.alpha`'s `B = A[i,j];` are
/// deliberate negative fixtures for exactly the rule this test's `eval_function` call ends up
/// enforcing as a side effect: array notation (`A[i]`) needs its index names declared somewhere
/// in scope (here, the LHS `B` has none — no `[i]`), and using an undeclared name isn't
/// resolvable to an affine expression. Confirmed by reading both files: their own comments say
/// "incorrect dimensions"/"unnamed indices". So `eval_function` correctly rejecting them here is
/// the right outcome, not a gap — this is the one place in this test where a resolution
/// *failure* is the expected result.
fn is_expected_invalid_fixture(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("src-invalid/syntax-tests/array1.alpha")
        || s.contains("src-invalid/syntax-tests/array2.alpha")
}

#[test]
fn dependence_and_reduce_functions_resolve() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_functions = 0usize;
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
            let resolver = Resolver::new(ctx, &system);

            for body in system.bodies() {
                for eq in body.equations() {
                    let (context, expr): (Vec<String>, Option<Expr>) = match &eq {
                        Equation::Standard(s) => (
                            s.index_names().map(|t| t.text().to_string()).collect(),
                            s.expr(),
                        ),
                        Equation::Use(u) => {
                            // `over {[i,j]:...}`'s own index tuple *and* `with [k]`'s names are
                            // both in scope for the equation's output/input expressions (e.g.
                            // FFT's `over[2] as [k] with [i] : (q[k,i]) = FFT[N/2](p[k,i])` needs
                            // both `k` (from `over`) and `i` (from `with`)).
                            let ctx_names = extend_unique(
                                &instantiation_domain_index_names(u),
                                u.subsystem_dims().map(|t| t.text().to_string()),
                            );
                            // Walk both output and input expressions for a UseEquation.
                            let mut found = Vec::new();
                            for e in u.output_exprs().chain(u.input_exprs()) {
                                collect_functions(&e, &ctx_names, &mut found);
                            }
                            for (calc, names) in found {
                                n_functions += 1;
                                if let Err(d) = resolver.eval_function(&calc, &names) {
                                    if !is_expected_invalid_fixture(path) {
                                        failures.push((
                                            path.clone(),
                                            format!(
                                                "{d} | text={:?} ctx={:?}",
                                                calc.syntax().text().to_string(),
                                                names
                                            ),
                                        ));
                                    }
                                }
                            }
                            continue;
                        }
                    };
                    let Some(expr) = expr else { continue };
                    let mut found = Vec::new();
                    collect_functions(&expr, &context, &mut found);
                    for (calc, names) in found {
                        n_functions += 1;
                        if let Err(d) = resolver.eval_function(&calc, &names) {
                            if is_expected_invalid_fixture(path) {
                                continue;
                            }
                            failures.push((
                                path.clone(),
                                format!(
                                    "{d} | text={:?} ctx={:?}",
                                    calc.syntax().text().to_string(),
                                    names
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "resolved {n_functions} dependence/reduce functions across {} fixtures",
        files.len()
    );
    assert!(
        n_functions > 0,
        "found zero functions to resolve across the whole corpus"
    );
    assert!(
        failures.is_empty(),
        "{} unexpected function-resolution failures:\n{:#?}",
        failures.len(),
        failures
    );
}
