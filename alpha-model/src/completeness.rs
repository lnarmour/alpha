//! Phase 6 of the six-phase pipeline (`docs/rust-port-design.md` §6): the well-formedness
//! catalog (`UniquenessAndCompletenessCheck` in the source Java). Per that class's own doc
//! comment, the properties checked are:
//! - the expression domain of an equation's root expression must cover its variable's domain
//!   ([`check_standard_equation_completeness`]),
//! - branches of `case` expressions must have disjoint context domains
//!   ([`check_case_branches`]),
//! - `SystemBody`s must have pairwise-disjoint parameter domains and must jointly cover the
//!   system's whole parameter domain, and no body's own domain may be empty
//!   ([`check_system_bodies`]),
//! - `UseEquation`s targeting the same variable must have disjoint instantiation domains and must
//!   jointly cover it ([`check_use_equation_outputs`]),
//! - a `UseEquation` may not call its own enclosing system with call parameters that are the
//!   identity on the caller's parameters (unconditional infinite recursion —
//!   [`check_use_equation_recursion`]),
//! - a `ReduceExpression`'s body must range over a boundable index space
//!   ([`check_reduce_bounded`]),
//! - every output variable, and every local variable that's referenced anywhere, must have a
//!   defining equation in each `SystemBody` ([`check_undefined_variables`]).
//!
//! Like phase 5, these checks don't fail fast — a program can have several unrelated
//! well-formedness problems, and the source system reports all of them in one pass — so every
//! public function here returns `Vec<Diagnostic>` directly. Each is implemented internally as a
//! `Result`-returning helper for the isl-heavy plumbing (mirroring phases 3–4's style), then
//! flattened to a `Vec` at the public boundary, pushing the one `Diagnostic::IslError` (or
//! similar) if the plumbing itself failed rather than aborting silently.
//!
//! **Deliberately dormant, not broken, until phase 4 grows a `UseEquation` context domain**
//! (see [`crate::domain`]'s module doc for that gap): [`check_use_equation_outputs`] is a
//! faithful port of the source system's `outSystemBody` logic, including its own graceful
//! "if a `VariableExpression`'s context domain isn't computed yet, skip this variable's check
//! entirely" behavior (`if (vexpr.getContextDomain() == null) break;`) — since this port doesn't
//! populate context domains for expressions inside a `UseEquation` yet, every variable always
//! hits that skip path today. The check is still written now (not deferred) so it needs no
//! changes once that gap closes.
//!
//! **Known scope-narrowing** (documented rather than silently guessed at, per this project's
//! practice): [`check_use_equation_recursion`]'s self-recursion detection identifies "calls its
//! own system" by comparing the callee's bare name against the enclosing system's own name,
//! rather than resolving the callee to an actual system object — this port has no whole-program
//! symbol table yet (see [`crate::uniqueness`]'s `check_program_uniqueness` for the closest thing,
//! built for a different purpose). Sound for the overwhelmingly common case (a system recursing
//! via its own bare name, typically from within the same file), but would miss a self-call
//! written with a full package-qualified path, or (vanishingly unlikely in practice) misfire on a
//! call to an unrelated system that happens to share a bare name with its caller in a different
//! package.

use crate::diagnostic::Diagnostic;
use crate::domain::Domains;
use crate::resolve::Resolver;
use crate::walk::walk_expr;
use alpha_syntax::ast::{self, AstNode, CalcExpr, Equation, Expr};
use alpha_syntax::syntax_kind::{SyntaxNode, SyntaxToken};
use isl::{DimType, Set};
use std::collections::{HashMap, HashSet};

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn token_range(t: &SyntaxToken) -> (u32, u32) {
    let r = t.text_range();
    (r.start().into(), r.end().into())
}

fn isl_err(e: isl::IslError, node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: e.message,
        start,
        end,
    }
}

/// `checkSystemBodyConsistency`: `SystemBody`s must have pairwise-disjoint parameter domains, no
/// body's own domain may be empty, and their union must equal the system's whole parameter
/// domain.
pub fn check_system_bodies(resolver: &Resolver, system: &ast::System) -> Vec<Diagnostic> {
    match check_system_bodies_inner(resolver, system) {
        Ok(diags) => diags,
        Err(d) => vec![d],
    }
}

fn check_system_bodies_inner(
    resolver: &Resolver,
    system: &ast::System,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut diags = Vec::new();
    let bodies: Vec<ast::SystemBody> = system.bodies().collect();
    if bodies.is_empty() {
        return Ok(diags);
    }

    let Ok(param_domain) = resolver.param_domain() else {
        return Ok(diags);
    };

    let mut domains = Vec::with_capacity(bodies.len());
    for body in &bodies {
        // Mirrors the source system's own early-return: if a body's parameter domain failed to
        // resolve, an earlier phase already reported it — don't compound the diagnostic.
        let Ok(dom) = resolver.system_body_domain(body) else {
            return Ok(diags);
        };
        if dom.is_empty().unwrap_or(false) {
            let (start, end) = range_of(body.syntax());
            diags.push(Diagnostic::EmptySystemBody { start, end });
        }
        // Intersected with the system's own parameter domain before the disjointness/coverage
        // checks below — a `when` guard's raw text can be satisfiable outside the system's own
        // declared domain without that being a real conflict (confirmed against `FFT.alpha` in
        // the real fixture corpus: its `N%2=1`/`N<=2` guards both admit `N=1`, which is moot
        // since the system itself only declares `N>=2` — the guards only need to partition the
        // parameter values that are actually reachable).
        domains.push(
            dom.intersect(param_domain.clone())
                .map_err(|e| isl_err(e, body.syntax()))?,
        );
    }

    let mut union = domains[0].clone();
    let mut intersections: Option<Set> = None;
    for dom in &domains[1..] {
        if !union
            .is_disjoint(dom)
            .map_err(|e| isl_err(e, system.syntax()))?
        {
            let inter = union
                .clone()
                .intersect(dom.clone())
                .map_err(|e| isl_err(e, system.syntax()))?;
            intersections = Some(match intersections {
                None => inter,
                Some(i) => i.union(inter).map_err(|e| isl_err(e, system.syntax()))?,
            });
        }
        union = union
            .union(dom.clone())
            .map_err(|e| isl_err(e, system.syntax()))?;
    }

    if let Some(inter) = intersections {
        for body in &bodies {
            let (start, end) = range_of(body.syntax());
            diags.push(Diagnostic::OverlappingSystemBodies {
                detail: inter.to_string(),
                start,
                end,
            });
        }
    }

    if !union
        .is_equal(&param_domain)
        .map_err(|e| isl_err(e, system.syntax()))?
    {
        let missing = param_domain
            .subtract(union)
            .map_err(|e| isl_err(e, system.syntax()))?;
        let (start, end) = system
            .name()
            .map(|t| token_range(&t))
            .unwrap_or_else(|| range_of(system.syntax()));
        diags.push(Diagnostic::IncompleteSystem {
            name: system
                .name()
                .map(|t| t.text().to_string())
                .unwrap_or_default(),
            detail: missing.to_string(),
            start,
            end,
        });
    }

    Ok(diags)
}

/// `inStandardEquation`: an equation's expression domain must cover its variable's declared
/// domain (intersected with the enclosing `SystemBody`'s parameter domain). `domains` must
/// already hold this equation's expression domains (see
/// [`crate::domain::Resolver::analyze_system`]).
pub fn check_standard_equation_completeness(
    resolver: &mut Resolver,
    eq: &ast::StandardEquation,
    body: &ast::SystemBody,
    domains: &Domains,
) -> Vec<Diagnostic> {
    match check_standard_equation_completeness_inner(resolver, eq, body, domains) {
        Ok(diags) => diags,
        Err(d) => vec![d],
    }
}

fn check_standard_equation_completeness_inner(
    resolver: &mut Resolver,
    eq: &ast::StandardEquation,
    body: &ast::SystemBody,
    domains: &Domains,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut diags = Vec::new();
    let Some(expr) = eq.expr() else {
        return Ok(diags);
    };
    let Some(def_dom) = domains.get(expr.syntax()) else {
        return Ok(diags);
    };
    let Some(var_name) = eq.variable_name() else {
        return Ok(diags);
    };
    let Ok(var_dom) = resolver.variable_domain(var_name.text()) else {
        return Ok(diags);
    };
    // Space-equality check (cheap dimension-count substitute — see `domain.rs`'s `context_domain`
    // doc for why this crate doesn't have a real isl space-equality query bound): mirrors the
    // source system's own early-return ("already checked at ContextDomainCalculator").
    if var_dom.dim(DimType::OutOrSet) != def_dom.dim(DimType::OutOrSet) {
        return Ok(diags);
    }
    let Ok(body_dom) = resolver.system_body_domain(body) else {
        return Ok(diags);
    };

    let var_dom_context = var_dom
        .clone()
        .intersect_params(body_dom)
        .map_err(|e| isl_err(e, eq.syntax()))?;
    let undef_dom = var_dom_context
        .clone()
        .subtract(def_dom.clone())
        .map_err(|e| isl_err(e, eq.syntax()))?;
    if !undef_dom.is_empty().map_err(|e| isl_err(e, eq.syntax()))? {
        let system_param = resolver.param_domain()?;
        let undef_dom_param = undef_dom
            .clone()
            .params()
            .and_then(|p| p.gist(system_param))
            .map_err(|e| isl_err(e, eq.syntax()))?;
        let undef_dom_gist = undef_dom
            .gist(var_dom_context)
            .map_err(|e| isl_err(e, eq.syntax()))?;
        let (start, end) = range_of(expr.syntax());
        diags.push(Diagnostic::IncompleteEquation {
            name: var_name.text().to_string(),
            domain_detail: undef_dom_gist.to_string(),
            param_detail: undef_dom_param.to_string(),
            start,
            end,
        });
    }
    Ok(diags)
}

/// `inCaseExpression`: every `CaseExpression` in every `StandardEquation` in the system must have
/// pairwise-disjoint branch context domains. `contexts` must already hold every equation's
/// context domains (see [`crate::domain::Resolver::analyze_system`]).
pub fn check_case_branches(system: &ast::System, contexts: &Domains) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for_each_standard_equation_expr(system, |expr| {
        walk_expr(expr, &mut |e| {
            if let Expr::Case(case) = e {
                diags.extend(check_one_case(case, contexts));
            }
        });
    });
    diags
}

fn check_one_case(case: &ast::CaseExpr, contexts: &Domains) -> Vec<Diagnostic> {
    match check_one_case_inner(case, contexts) {
        Ok(diags) => diags,
        Err(d) => vec![d],
    }
}

fn check_one_case_inner(
    case: &ast::CaseExpr,
    contexts: &Domains,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut diags = Vec::new();
    let branches: Vec<Expr> = case.branches().collect();
    let mut doms = Vec::with_capacity(branches.len());
    for b in &branches {
        // Mirrors the source system's `testNonNullContextDomain` guard: skip gracefully if any
        // branch's context domain wasn't computed (an earlier, already-reported error).
        let Some(d) = contexts.get(b.syntax()) else {
            return Ok(diags);
        };
        doms.push(d.clone());
    }
    if doms.is_empty() {
        return Ok(diags);
    }

    let mut union = doms[0].clone();
    for (branch, dom) in branches.iter().zip(doms.iter()).skip(1) {
        if !union
            .is_disjoint(dom)
            .map_err(|e| isl_err(e, branch.syntax()))?
        {
            let inter = dom
                .clone()
                .intersect(union.clone())
                .map_err(|e| isl_err(e, branch.syntax()))?;
            let (start, end) = range_of(branch.syntax());
            diags.push(Diagnostic::OverlappingCaseBranch {
                detail: inter.to_string(),
                start,
                end,
            });
        }
        union = union
            .union(dom.clone())
            .map_err(|e| isl_err(e, branch.syntax()))?;
    }
    Ok(diags)
}

/// `inReduceExpression`: a reduction's body must range over an index space isl can establish as
/// bounded (both directions) in every dimension — an unbounded reduction wouldn't terminate.
/// `contexts` must already hold every equation's context domains.
pub fn check_reduce_bounded(system: &ast::System, contexts: &Domains) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for_each_standard_equation_expr(system, |expr| {
        walk_expr(expr, &mut |e| {
            if let Expr::Reduce(r) = e {
                diags.extend(check_one_reduce(r, contexts));
            }
        });
    });
    diags
}

fn check_one_reduce(r: &ast::ReduceExpr, contexts: &Domains) -> Vec<Diagnostic> {
    match check_one_reduce_inner(r, contexts) {
        Ok(diags) => diags,
        Err(d) => vec![d],
    }
}

fn check_one_reduce_inner(
    r: &ast::ReduceExpr,
    contexts: &Domains,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut diags = Vec::new();
    let Some(body) = r.body() else {
        return Ok(diags);
    };
    let Some(body_ctx) = contexts.get(body.syntax()) else {
        return Ok(diags);
    };
    let mut dom = body_ctx
        .clone()
        .convex_hull()
        .map_err(|e| isl_err(e, r.syntax()))?
        .into_set();
    let n = dom.dim(DimType::OutOrSet);
    for i in 0..n {
        let has_upper = dom.has_upper_bound(DimType::OutOrSet, i).unwrap_or(false);
        let has_lower = dom.has_lower_bound(DimType::OutOrSet, i).unwrap_or(false);
        if !has_upper || !has_lower {
            let (start, end) = range_of(r.syntax());
            diags.push(Diagnostic::UnboundedReductionBody { start, end });
        }
        dom = dom
            .eliminate(DimType::OutOrSet, i, 1)
            .map_err(|e| isl_err(e, r.syntax()))?;
    }
    Ok(diags)
}

/// `inUseEquation`'s self-recursion check: a `UseEquation` calling its own enclosing system with
/// call parameters that are the identity on the caller's parameters would recurse unconditionally
/// forever. See the module doc for this check's scope-narrowing (bare-name comparison, not full
/// symbol resolution).
pub fn check_use_equation_recursion(
    resolver: &Resolver,
    ue: &ast::UseEquation,
    system: &ast::System,
) -> Vec<Diagnostic> {
    match check_use_equation_recursion_inner(resolver, ue, system) {
        Ok(Some(d)) => vec![d],
        Ok(None) => Vec::new(),
        Err(d) => vec![d],
    }
}

fn check_use_equation_recursion_inner(
    resolver: &Resolver,
    ue: &ast::UseEquation,
    system: &ast::System,
) -> Result<Option<Diagnostic>, Diagnostic> {
    let Some(callee) = ue.callee() else {
        return Ok(None);
    };
    let Some(callee_name) = callee.segments().last().map(|t| t.text().to_string()) else {
        return Ok(None);
    };
    let Some(system_name) = system.name().map(|t| t.text().to_string()) else {
        return Ok(None);
    };
    if callee_name != system_name {
        return Ok(None);
    }
    let Some(call_params) = ue.call_params() else {
        return Ok(None);
    };
    let calc = CalcExpr::ArrayFunction(call_params);
    let maff = resolver.eval_function(&calc, &[])?;
    let n_params = maff.space().dim(DimType::Param);
    let moved = maff
        .move_dims(DimType::In, 0, DimType::Param, 0, n_params)
        .map_err(|e| isl_err(e, ue.syntax()))?;
    let is_identity = moved
        .into_map()
        .and_then(|m| m.is_identity())
        .map_err(|e| isl_err(e, ue.syntax()))?;
    Ok(is_identity.then(|| {
        let (start, end) = range_of(ue.syntax());
        Diagnostic::InfinitelyRecursiveUseEquation { start, end }
    }))
}

/// `outSystemBody`'s `UseEquation` output-consistency check: every variable written by one or
/// more `UseEquation`s in a `SystemBody` must have pairwise-disjoint, jointly-complete
/// instantiation domains. See the module doc for why this is dormant until phase 4 grows a
/// `UseEquation` context domain.
pub fn check_use_equation_outputs(
    resolver: &mut Resolver,
    system: &ast::System,
    contexts: &Domains,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for body in system.bodies() {
        diags.extend(check_use_equation_outputs_in_body(
            resolver, &body, contexts,
        ));
    }
    diags
}

fn check_use_equation_outputs_in_body(
    resolver: &mut Resolver,
    body: &ast::SystemBody,
    contexts: &Domains,
) -> Vec<Diagnostic> {
    match check_use_equation_outputs_in_body_inner(resolver, body, contexts) {
        Ok(diags) => diags,
        Err(d) => vec![d],
    }
}

fn check_use_equation_outputs_in_body_inner(
    resolver: &mut Resolver,
    body: &ast::SystemBody,
    contexts: &Domains,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut diags = Vec::new();

    // For each variable name, every `Expr::Variable` reference to it found in any UseEquation's
    // output exprs in this body, paired with the owning output expr (the diagnostic's anchor,
    // matching the source system's `findAncestorOutputExpression`).
    let mut by_var: HashMap<String, Vec<(Expr, ast::VariableExpr)>> = HashMap::new();
    for eq in body.equations() {
        let Equation::Use(u) = &eq else { continue };
        for out in u.output_exprs() {
            walk_expr(&out, &mut |e| {
                if let Expr::Variable(v) = e {
                    if let Some(name) = v.name() {
                        by_var
                            .entry(name.text().to_string())
                            .or_default()
                            .push((out.clone(), v.clone()));
                    }
                }
            });
        }
    }

    for (name, refs) in &by_var {
        let mut union: Option<Set> = None;
        let mut intersections: Option<Set> = None;
        let mut all_present = true;
        for (_, vexpr) in refs {
            let Some(d) = contexts.get(vexpr.syntax()) else {
                all_present = false;
                break;
            };
            union = Some(match union {
                None => d.clone(),
                Some(u) => {
                    if !u.is_disjoint(d).map_err(|e| isl_err(e, vexpr.syntax()))? {
                        let inter = d
                            .clone()
                            .intersect(u.clone())
                            .map_err(|e| isl_err(e, vexpr.syntax()))?;
                        intersections = Some(match intersections {
                            None => inter,
                            Some(i) => i.union(inter).map_err(|e| isl_err(e, vexpr.syntax()))?,
                        });
                    }
                    u.union(d.clone()).map_err(|e| isl_err(e, vexpr.syntax()))?
                }
            });
        }
        if !all_present {
            continue;
        }
        let Some(union) = union else { continue };

        if let Some(inter) = &intersections {
            for (out_expr, _) in refs {
                let (start, end) = range_of(out_expr.syntax());
                diags.push(Diagnostic::OverlappingUseEquations {
                    name: name.clone(),
                    detail: inter.to_string(),
                    start,
                    end,
                });
            }
        }

        let Ok(var_dom) = resolver.variable_domain(name) else {
            continue;
        };
        let Ok(body_dom) = resolver.system_body_domain(body) else {
            continue;
        };
        let v_dom_ctx = var_dom
            .clone()
            .intersect_params(body_dom)
            .map_err(|e| isl_err(e, body.syntax()))?;
        if !union
            .is_equal(&v_dom_ctx)
            .map_err(|e| isl_err(e, body.syntax()))?
        {
            let diff = var_dom
                .subtract(union)
                .map_err(|e| isl_err(e, body.syntax()))?;
            for (out_expr, _) in refs {
                let (start, end) = range_of(out_expr.syntax());
                diags.push(Diagnostic::IncompleteUseEquation {
                    name: name.clone(),
                    detail: diff.to_string(),
                    start,
                    end,
                });
            }
        }
    }

    Ok(diags)
}

/// `outSystemBody`'s undefined-variable check: every output variable, and every local variable
/// referenced anywhere in a `SystemBody`, must have a defining equation (a `StandardEquation`, or
/// appear in a `UseEquation`'s output) in that same body. Purely syntactic (existence, not
/// domain math) — computable regardless of whether earlier phases succeeded.
pub fn check_undefined_variables(system: &ast::System) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for body in system.bodies() {
        diags.extend(check_undefined_variables_in_body(system, &body));
    }
    diags
}

fn check_undefined_variables_in_body(
    system: &ast::System,
    body: &ast::SystemBody,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    let mut used: HashSet<String> = HashSet::new();
    let mut standard_defined: HashSet<String> = HashSet::new();
    let mut use_defined: HashSet<String> = HashSet::new();

    let mark_used = |e: &Expr, used: &mut HashSet<String>| {
        if let Expr::Variable(v) = e {
            if let Some(name) = v.name() {
                used.insert(name.text().to_string());
            }
        }
    };

    for eq in body.equations() {
        match &eq {
            Equation::Standard(s) => {
                if let Some(name) = s.variable_name() {
                    standard_defined.insert(name.text().to_string());
                }
                if let Some(expr) = s.expr() {
                    walk_expr(&expr, &mut |e| mark_used(e, &mut used));
                }
            }
            Equation::Use(u) => {
                for out in u.output_exprs() {
                    walk_expr(&out, &mut |e| {
                        if let Expr::Variable(v) = e {
                            if let Some(name) = v.name() {
                                use_defined.insert(name.text().to_string());
                                used.insert(name.text().to_string());
                            }
                        }
                    });
                }
                for inp in u.input_exprs() {
                    walk_expr(&inp, &mut |e| mark_used(e, &mut used));
                }
            }
        }
    }

    let is_defined = |name: &str| standard_defined.contains(name) || use_defined.contains(name);

    let check_variable =
        |name: Option<SyntaxToken>, must_check: bool, diags: &mut Vec<Diagnostic>| {
            let Some(name) = name else { return };
            if must_check && !is_defined(name.text()) {
                let (start, end) = token_range(&name);
                diags.push(Diagnostic::UndefinedVariable {
                    name: name.text().to_string(),
                    start,
                    end,
                });
            }
        };

    if let Some(locals) = system.locals() {
        for v in locals.variables() {
            let name = v.name();
            let is_used = name.as_ref().is_some_and(|n| used.contains(n.text()));
            check_variable(name, is_used, &mut diags);
        }
        for v in locals.fuzzy_variables() {
            let name = v.name();
            let is_used = name.as_ref().is_some_and(|n| used.contains(n.text()));
            check_variable(name, is_used, &mut diags);
        }
    }
    if let Some(outputs) = system.outputs() {
        for v in outputs.variables() {
            check_variable(v.name(), true, &mut diags);
        }
        for v in outputs.fuzzy_variables() {
            check_variable(v.name(), true, &mut diags);
        }
    }

    diags
}

/// Walks every `StandardEquation`'s root expression in `system`, calling `visit` on each — the
/// shared traversal [`check_case_branches`]/[`check_reduce_bounded`] both need (they only ever
/// look inside `StandardEquation` bodies, since [`crate::domain`] doesn't compute context domains
/// inside `UseEquation`s yet either).
fn for_each_standard_equation_expr(system: &ast::System, mut visit: impl FnMut(&Expr)) {
    for body in system.bodies() {
        for eq in body.equations() {
            let Equation::Standard(s) = &eq else { continue };
            if let Some(expr) = s.expr() {
                visit(&expr);
            }
        }
    }
}
