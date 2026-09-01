//! `NormalizeReduction`: hoists every *nested* `Reduce` out of a `StandardEquation`'s expression
//! and into its own fresh local variable + equation — giving later passes (reduction
//! simplification, out of this port's scope, but also the demand-driven
//! codegen's own per-variable memoization, and scheduled codegen's own per-statement naming, see
//! `docs/scheduled-codegen-design.md` §4.3) an equation boundary to work with directly, without
//! needing to dig a reduction out of an arbitrary surrounding expression.
//!
//! A `Reduce` that is *already* the equation's own expression (`eq.expr`'s own kind, not merely
//! reachable from it) needs no such boundary — it already has one, the equation itself — and is
//! left untouched. Hervé's own proof that nested reductions can't be normalized (the inverse of a
//! reduce's projection isn't unique) only forces the reduction to be the tree's topmost node; a
//! reduction that already *is* the topmost node satisfies that trivially. Nothing about being a
//! `Reduce` demands its own separate equation on top of that — this pass exists purely to
//! *establish* topmost-ness, not to always relocate every reduction it finds. This is also what
//! `alpha-codegen/src/stmt.rs`'s §4.2 statement split expects: it looks for a variable whose own
//! `eq.expr.kind` is directly `Reduce` and treats that as the reduce statement pair's shape — an
//! extraction that fired here unconditionally would leave that variable a pointless identity
//! copy of a freshly invented `_NR` local instead.
//!
//! Ported from `NormalizeReduction.xtend`. Things carried over verbatim from the source:
//! - `UseEquation`s are skipped outright — the source system's own doc comment says reductions
//!   aren't expected in `UseEquation` inputs.
//! - A `Reduce` directly attached to an equation (`are.eContainer instanceof Equation` in the
//!   source) is exactly the "nothing to do" case above.
//! - A `Reduce` nested inside another `Reduce`'s body is left alone even when hoisting a
//!   different, non-reduce-nested reduction elsewhere in the same equation (the source system's
//!   own doc: "does not fully normalize nested reductions" on one pass — matches `Normalize`'s
//!   own similar acknowledged imperfection, and matches `alpha-codegen/src/scheduledc.rs`'s own
//!   documented gap for a `Reduce` nested directly in another `Reduce`'s body).
//!
//! **Naming** (§4.3 of the scheduled-codegen design): an extracted reduce is named after its
//! *enclosing equation's target variable* — `B` → `B_NR`, with a numeric suffix only on an actual
//! collision (`B_NR0`, `B_NR1`, ...) — matching the upstream Java system's own convention
//! (`defineNormalizeReductionEquationName`), which this port previously diverged from only because
//! scheduled codegen didn't exist yet to need predictable names. A candidate is checked against
//! not just its own bare name but its `__init`/`__reduce` derived forms too (`docs/scheduled-
//! codegen-design.md` §4.2's reduce-statement-pair naming), since a real Alpha variable literally
//! named e.g. `B_NR0__init` is syntactically legal (identifiers permit `_` freely) and would
//! otherwise collide with a name this pass never itself constructs but scheduled codegen does.

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
            let target_var = s.variable.clone();
            extract_from_equation(
                s,
                &target_var,
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

/// Finds the equation's own *nested* `Reduce` nodes (a `Reduce` reachable from `eq.expr` without
/// passing through another `Reduce` first — matches the source system's
/// `visitAbstractReduceExpression` bailing out of recursion as soon as it collects one) and
/// extracts each into a new local + equation, replacing it in place with a reference to that new
/// variable.
///
/// If `eq.expr` itself is directly a `Reduce`, it's already the topmost node of its own
/// equation — already normal form, and exactly the shape `alpha-codegen/src/stmt.rs`'s §4.2
/// reduce-statement split expects — so this is a no-op (matches the source's own
/// `are.eContainer instanceof Equation` check short-circuiting before `targetREs.add`).
fn extract_from_equation(
    eq: &mut StandardEquation,
    target_var: &str,
    enclosing_domain: Option<&isl::Set>,
    existing_names: &mut HashSet<String>,
    new_equations: &mut Vec<StandardEquation>,
    new_locals: &mut Vec<Variable>,
    extracted: &mut usize,
) {
    if matches!(&*eq.expr.kind, ExprKind::Reduce { .. }) {
        return;
    }
    // The extracted equation's own ambient index names are exactly the original equation's own —
    // `extract_top_level` never descends past a name-changing boundary (it stops at the first
    // `Reduce` found, per the module doc, and doesn't itself know about `Select`/explicit-tuple
    // `Restrict`, the only other name-changing constructs — see `alpha_model::domain`'s module
    // doc), so no ambient name ever actually changes on the path to a nested `Reduce`.
    let index_names = eq.index_names.clone();
    extract_top_level(
        &mut eq.expr,
        target_var,
        &index_names,
        enclosing_domain,
        existing_names,
        new_equations,
        new_locals,
        extracted,
    );
}

#[allow(clippy::too_many_arguments)]
fn extract_top_level(
    e: &mut Expr,
    target_var: &str,
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
        let name = fresh_name(target_var, existing_names);
        let placeholder = Expr::new(
            ExprKind::Variable(name.clone()),
            domain.clone(),
            e.context_domain.clone(),
        );
        let reduce_expr = std::mem::replace(e, placeholder);
        new_locals.push(Variable {
            name: name.clone(),
            domain,
            multiplicity: alpha_model::Multiplicity::Unrestricted,
            element_type: alpha_model::ElementType::Unspecified,
        });
        new_equations.push(StandardEquation {
            variable: name,
            index_names: index_names.to_vec(),
            expr: reduce_expr,
        });
        *extracted += 1;
        // The source system doesn't wrap the replacement `VariableExpression` in an identity
        // `Dependence` itself — `Normalize` (specifically `outVariableExpression`) is what
        // establishes that invariant, and is expected to run again after this pass (matches this
        // port's own required pipeline order — see `alphac/src/main.rs`'s module doc and
        // `docs/scheduled-codegen-design.md` §3 for why, unlike upstream, that order is not
        // optional here).
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
                target_var,
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
                target_var,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                then_branch,
                target_var,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                else_branch,
                target_var,
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
                    target_var,
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
                target_var,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                data_expr,
                target_var,
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
                    target_var,
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
                target_var,
                index_names,
                enclosing_domain,
                existing_names,
                new_equations,
                new_locals,
                extracted,
            );
            extract_top_level(
                rhs,
                target_var,
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

/// A fresh, unused name for the extracted reduction's new local variable: `<target_var>_NR`, with
/// a numeric suffix appended only on an actual collision (`_NR0`, `_NR1`, ...) — matching the
/// upstream Java system's `defineNormalizeReductionEquationName`. A candidate collides if its own
/// bare name, `<candidate>__init`, or `<candidate>__reduce` is already taken — the latter two
/// account for names scheduled codegen's own statement model derives from this one (`docs/
/// scheduled-codegen-design.md` §4.2) but that this pass never itself constructs, so a real Alpha
/// variable already named e.g. `B_NR0__init` is never silently shadowed by a later reduce that
/// picks `B_NR0`.
fn fresh_name(target_var: &str, existing_names: &mut HashSet<String>) -> String {
    let base = format!("{target_var}_NR");
    let mut n: Option<u32> = None;
    loop {
        let candidate = match n {
            None => base.clone(),
            Some(k) => format!("{base}{k}"),
        };
        let collides = existing_names.contains(&candidate)
            || existing_names.contains(&format!("{candidate}__init"))
            || existing_names.contains(&format!("{candidate}__reduce"));
        if !collides {
            existing_names.insert(candidate.clone());
            return candidate;
        }
        n = Some(n.map_or(0, |k| k + 1));
    }
}

#[cfg(test)]
mod tests {
    use super::fresh_name;
    use std::collections::HashSet;

    #[test]
    fn first_reduce_on_a_variable_gets_the_bare_name() {
        let mut existing = HashSet::new();
        assert_eq!(fresh_name("B", &mut existing), "B_NR");
        assert!(existing.contains("B_NR"));
    }

    #[test]
    fn same_target_variable_collision_falls_back_to_a_numeric_suffix() {
        let mut existing = HashSet::new();
        let first = fresh_name("B", &mut existing);
        let second = fresh_name("B", &mut existing);
        let third = fresh_name("B", &mut existing);
        assert_eq!(
            (first.as_str(), second.as_str(), third.as_str()),
            ("B_NR", "B_NR0", "B_NR1")
        );
    }

    #[test]
    fn a_real_variable_named_after_a_derived_init_form_is_not_shadowed() {
        // A user could legally declare a variable named `B_NR__init` (Alpha identifiers permit
        // `_` freely) — the candidate `B_NR` must be skipped even though `fresh_name` itself never
        // constructs the `__init` suffix, because scheduled codegen's own statement model (§4.2)
        // would derive `B_NR__init` from whatever bare name this pass picks.
        let mut existing: HashSet<String> = ["B_NR__init".to_string()].into_iter().collect();
        let name = fresh_name("B", &mut existing);
        assert_eq!(name, "B_NR0");
    }

    #[test]
    fn a_real_variable_named_after_a_derived_reduce_form_is_not_shadowed() {
        let mut existing: HashSet<String> = ["B_NR__reduce".to_string()].into_iter().collect();
        let name = fresh_name("B", &mut existing);
        assert_eq!(name, "B_NR0");
    }

    #[test]
    fn distinct_target_variables_never_collide_with_each_other() {
        let mut existing = HashSet::new();
        let a = fresh_name("A", &mut existing);
        let b = fresh_name("B", &mut existing);
        assert_eq!((a.as_str(), b.as_str()), ("A_NR", "B_NR"));
    }
}
