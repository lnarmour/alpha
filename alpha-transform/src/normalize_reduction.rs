//! `NormalizeReduction`: moves every top-level `Reduce` out of a `StandardEquation`'s expression
//! and into its own fresh local variable + equation — giving later passes (reduction
//! simplification, out of this port's scope, but also the demand-driven
//! codegen's own per-variable memoization) an equation boundary to work with directly, without
//! needing to dig a reduction out of an arbitrary surrounding expression.
//!
//! Ported from `NormalizeReduction.xtend`. Two things carried over verbatim from the source:
//! - `UseEquation`s are skipped outright — the source system's own doc comment says reductions
//!   aren't expected in `UseEquation` inputs.
//! - Only *top-level* reductions are extracted: a `Reduce` nested inside another `Reduce`'s body
//!   is left alone (the source system's own doc: "does not fully normalize nested reductions" on
//!   one pass — matches `Normalize`'s own similar acknowledged imperfection).

use crate::ir::{Equation, Expr, ExprKind, StandardEquation, System, Variable};
use std::collections::{HashMap, HashSet};

/// Applies the transformation across every `StandardEquation` in `system`, returning the number
/// of reductions extracted (mirrors the source system's `apply(AlphaVisitable)` return value).
pub fn apply(system: &mut System) -> usize {
    let mut existing_names: HashSet<String> = system
        .inputs
        .iter()
        .chain(system.outputs.iter())
        .chain(system.locals.iter())
        .map(|v| v.name.clone())
        .collect();
    // Every variable's own declared domain, looked up once before mutating `system` — see
    // `extract_top_level`'s doc for why the extracted `Reduce`'s own bottom-up `expression_domain`
    // isn't, on its own, a tight enough domain for the new local.
    let var_domains: HashMap<String, isl::Set> = system
        .inputs
        .iter()
        .chain(system.outputs.iter())
        .chain(system.locals.iter())
        .map(|v| (v.name.clone(), v.domain.clone()))
        .collect();

    let mut extracted = 0usize;
    let mut new_locals = Vec::new();
    for body in &mut system.bodies {
        let mut new_equations = Vec::new();
        for eq in &mut body.equations {
            let Equation::Standard(s) = eq else { continue };
            let enclosing_domain = var_domains.get(&s.variable).cloned();
            extract_from_equation(
                s,
                enclosing_domain.as_ref(),
                &mut existing_names,
                &mut new_equations,
                &mut new_locals,
                &mut extracted,
            );
        }
        body.equations
            .extend(new_equations.into_iter().map(Equation::Standard));
    }
    system.locals.extend(new_locals);
    extracted
}

/// Finds the equation's own *top-level* `Reduce` nodes (a `Reduce` reachable without passing
/// through another `Reduce` first — matches the source system's `visitAbstractReduceExpression`
/// bailing out of recursion as soon as it collects one) and extracts each into a new local +
/// equation, replacing it in place with a reference to that new variable.
fn extract_from_equation(
    eq: &mut StandardEquation,
    enclosing_domain: Option<&isl::Set>,
    existing_names: &mut HashSet<String>,
    new_equations: &mut Vec<StandardEquation>,
    new_locals: &mut Vec<Variable>,
    extracted: &mut usize,
) {
    // The extracted equation's own ambient index names are exactly the original equation's own —
    // `extract_top_level` never descends past a name-changing boundary (it stops at the first
    // `Reduce` found, per the module doc, and doesn't itself know about `Select`/explicit-tuple
    // `Restrict`, the only other name-changing constructs — see `alpha_model::domain`'s module
    // doc), so no ambient name ever actually changes on the path to a top-level `Reduce`.
    let index_names = eq.index_names.clone();
    extract_top_level(
        &mut eq.expr,
        &index_names,
        enclosing_domain,
        existing_names,
        new_equations,
        new_locals,
        extracted,
    );
}

fn extract_top_level(
    e: &mut Expr,
    index_names: &[String],
    enclosing_domain: Option<&isl::Set>,
    existing_names: &mut HashSet<String>,
    new_equations: &mut Vec<StandardEquation>,
    new_locals: &mut Vec<Variable>,
    extracted: &mut usize,
) {
    if matches!(&*e.kind, ExprKind::Reduce { .. }) {
        // The `Reduce` node's own `expression_domain` is a purely bottom-up derivation — it has
        // no reason to be bounded in the *ambient* dims (e.g. a `reduce(+, [j], {:j<=i}: X[j])`
        // only ever bounds `j` relative to `i`, never bounds `i` itself). Intersecting with the
        // enclosing equation's own target-variable domain (properly bounded, since that's a real
        // declared domain) is what keeps the new local's domain actually finite — caught by a
        // real `alphac` run against `PrefixScan.alpha` producing an isl "unbounded optimum" error
        // straight out of `alpha-codegen`'s array-sizing, not by inspection.
        let mut domain = e.expression_domain.clone();
        if let Some(enclosing) = enclosing_domain {
            if let Ok(narrowed) = domain.clone().intersect(enclosing.clone()) {
                domain = narrowed;
            }
        }
        let name = fresh_name("R", existing_names);
        let placeholder = Expr::new(
            ExprKind::Variable(name.clone()),
            domain.clone(),
            e.context_domain.clone(),
        );
        let reduce_expr = std::mem::replace(e, placeholder);
        new_locals.push(Variable {
            name: name.clone(),
            domain,
        });
        new_equations.push(StandardEquation {
            variable: name,
            index_names: index_names.to_vec(),
            expr: reduce_expr,
        });
        *extracted += 1;
        // The source system doesn't wrap the replacement `VariableExpression` in an identity
        // `Dependence` itself — `Normalize` (specifically `outVariableExpression`) is what
        // establishes that invariant, and is expected to run again after this pass (matches the
        // source system's own phase ordering: `NormalizeReduction` runs, then `Normalize` cleans
        // up the result).
        return;
    }
    // Not a Reduce at this node — recurse into children, but stop descending the moment a child
    // *is* a Reduce (don't reach into nested reductions; matches the source's "only top level").
    match e.kind.as_mut() {
        ExprKind::Dependence { operand, .. }
        | ExprKind::Restrict { operand, .. }
        | ExprKind::AutoRestrict { operand }
        | ExprKind::Select { operand, .. }
        | ExprKind::Unary { operand, .. } => {
            extract_top_level(
                operand,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            extract_top_level(
                cond,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                then_branch,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                else_branch,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
        }
        ExprKind::Case { branches, .. } => {
            for b in branches.iter_mut() {
                extract_top_level(
                    b,
                    index_names,
                    enclosing_domain,
                    existing_names,
                    new_equations,
                    new_locals,
                    extracted,
                );
            }
        }
        ExprKind::Convolution {
            kernel_expr,
            data_expr,
            ..
        } => {
            extract_top_level(
                kernel_expr,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                data_expr,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
        }
        ExprKind::MultiArg { args, .. } => {
            for a in args.iter_mut() {
                extract_top_level(
                    a,
                    index_names,
                    enclosing_domain,
                    existing_names,
                    new_equations,
                    new_locals,
                    extracted,
                );
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            extract_top_level(
                lhs,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                rhs,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
        }
        ExprKind::Variable(_)
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Real(_)
        | ExprKind::IndexFunction { .. }
        | ExprKind::IndexPolynomial { .. }
        | ExprKind::Reduce { .. } => {}
    }
}

/// `defineNormalizeReductionEquationName`: a fresh, unused name for the extracted reduction's new
/// local variable — the source system's `AlphaUtil.duplicateNameResolverWithCounter`, naming it
/// `<target variable>_NR` with a numeric suffix on collision. Unlike the source (named after the
/// equation's own target variable, since it processes one reduction at a time from a known
/// equation), this just uses a short counter-based name since this pass runs across every
/// equation in the system at once and codegen never surfaces these names to a user.
fn fresh_name(prefix: &str, existing_names: &mut HashSet<String>) -> String {
    let mut n = 0u32;
    loop {
        let candidate = format!("{prefix}_NR{n}");
        if !existing_names.contains(&candidate) {
            existing_names.insert(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}
