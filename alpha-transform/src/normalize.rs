//! `Normalize`: the term-rewriting pass bringing every `StandardEquation` (and, for its own
//! input/output expressions only — see [`crate::ir::Equation::Use`]'s doc — every `UseEquation`)
//! into the source system's normal form (`docs/rust-port-design.md` §7):
//! - the parent of a `Case` must be a `StandardEquation` or `Reduce`,
//! - the parent of a `Restrict` must be a `StandardEquation`, `Reduce`, or `Case`,
//! - the parent of a `Variable` must be a `Dependence`,
//! - the child of a `Dependence` must be a `Variable` or a constant.
//!
//! Ported from `Normalize.xtend`'s two objectives — dependences propagate *down* towards leaves,
//! restricts propagate *up* towards the equation root (except into `Case`, where they propagate
//! down into branches instead) — as a set of local rewrite rules applied to a fixpoint.
//!
//! **Architecture note** (see [`crate::ir`]'s doc for the full rationale): the source system
//! mutates a live EMF graph in place and calls `AlphaInternalStateConstructor.recomputeContextDomain`
//! at specific points after specific rewrites. This module instead (a) always recomputes a
//! rewritten node's `expression_domain` immediately, bottom-up, from its (already-normalized)
//! children's domains via [`expr_from_kind`] — the same formula `alpha_model::domain` uses, just
//! applied to isl objects already sitting in the tree instead of re-deriving them — and (b)
//! recomputes every node's `context_domain` in one dedicated top-down pass
//! ([`refresh_context_domains`]) run between rounds of structural rewriting, rather than
//! incrementally patching it inline. [`apply`] alternates the two (structural fixpoint, then a
//! context refresh) for a bounded number of rounds — mirroring the source system's own admission
//! that its single bottom-up pass "may create situations where multiple calls are required to
//! reach the normal form."
//!
//! **Two small, deliberate departures from `Normalize.xtend`'s literal behavior**, both making
//! this port strictly *more* complete rather than differently correct:
//! - The binary-operator restrict-hoist rule (`(D:A) op B -> D:(A op B)` and `A op (D:B) ->
//!   D:(A op B)`) is applied symmetrically to either operand. Tracing the source's own Xtend
//!   multi-dispatch resolution order suggests its fallback path only ever inspects the left
//!   operand for this rule in one dispatch branch — its own doc comment states the rule as
//!   symmetric ("and"), so this is read as the intent, not a deliberate left-only restriction.
//! - Redundant-restrict removal (`D:E -> E` when `D` doesn't actually narrow `E`'s domain) is
//!   applied unconditionally, where the source system skips it when the restrict is a direct
//!   child of a `Case` or another `Restrict` (a readability-preservation nicety — keeping an
//!   explicit-but-redundant domain marker visible on a case branch). This crate's `Expr` has no
//!   parent back-reference to check that condition cheaply, and losing the marker doesn't affect
//!   correctness (it's exactly what the source system's own "deep" normalize mode already does
//!   unconditionally).

use crate::ir::{Equation, Expr, ExprKind, Operator, System};
use isl::Set;

/// Recomputes `kind`'s expression domain bottom-up from its (already-normalized) children's
/// domains, using the same formula as `alpha_model::domain`'s phase-3 inference for that node
/// kind, and wraps it into a fresh [`Expr`] with `context_domain: None` (filled in later by
/// [`refresh_context_domains`]). Every rewrite rule below builds its replacement node through
/// this, so a node's `expression_domain` is always correct immediately, never stale.
fn expr_from_kind(kind: ExprKind) -> Expr {
    let domain = match &kind {
        ExprKind::Variable(_) | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Real(_) => {
            unreachable!("leaf nodes are never reconstructed by a rewrite rule")
        }
        ExprKind::Dependence { function, operand } => operand
            .expression_domain
            .clone()
            .preimage_multi_aff(function.clone())
            .expect("Normalize: preimage of an already-valid dependence function"),
        ExprKind::IndexFunction { function } => Set::universe(function.domain_space())
            .expect("Normalize: universe of an already-valid function's domain space"),
        ExprKind::IndexPolynomial { polynomial } => Set::universe(polynomial.domain_space())
            .expect("Normalize: universe of an already-valid polynomial's domain space"),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => cond
            .expression_domain
            .clone()
            .intersect(then_branch.expression_domain.clone())
            .and_then(|s| s.intersect(else_branch.expression_domain.clone()))
            .expect("Normalize: intersecting already-compatible if-branch domains"),
        ExprKind::Restrict { domain, operand } => operand
            .expression_domain
            .clone()
            .intersect(domain.clone())
            .expect("Normalize: intersecting an already-valid restrict domain"),
        ExprKind::AutoRestrict { operand } => operand.expression_domain.clone(),
        ExprKind::Case { branches, .. } => branches
            .iter()
            .map(|b| b.expression_domain.clone())
            .reduce(|a, b| a.union(b).expect("Normalize: unioning case-branch domains"))
            .expect("Normalize: a Case always has at least one branch"),
        ExprKind::Reduce {
            projection, body, ..
        } => body
            .expression_domain
            .clone()
            .apply(
                projection
                    .clone()
                    .into_map()
                    .expect("Normalize: an already-valid projection converts to a map"),
            )
            .expect("Normalize: applying an already-valid reduce projection"),
        ExprKind::Convolution { .. } => {
            unreachable!(
                "Convolution's own expression domain is an unresolved gap in alpha_model::domain \
                 (see that module's doc) — lowering already excludes any equation containing one, \
                 so Normalize never constructs or rewrites this variant"
            )
        }
        ExprKind::Select { relation, operand } => operand
            .expression_domain
            .clone()
            .apply(
                relation
                    .clone()
                    .reverse()
                    .expect("Normalize: reversing an already-valid select relation"),
            )
            .expect("Normalize: applying an already-valid select relation"),
        ExprKind::MultiArg { args, .. } => args
            .iter()
            .map(|a| a.expression_domain.clone())
            .reduce(|a, b| {
                a.intersect(b)
                    .expect("Normalize: intersecting multi-arg operand domains")
            })
            .expect("Normalize: a MultiArg always has at least one argument"),
        ExprKind::Binary { lhs, rhs, .. } => lhs
            .expression_domain
            .clone()
            .intersect(rhs.expression_domain.clone())
            .expect("Normalize: intersecting already-compatible binary operand domains"),
        ExprKind::Unary { operand, .. } => operand.expression_domain.clone(),
    };
    Expr::new(kind, domain, None)
}

fn is_variable(e: &Expr) -> bool {
    matches!(&*e.kind, ExprKind::Variable(_))
}

/// `f @ E = E` when `f` is the identity (or a 0-in-0-out no-op) — skipped when `E` is already a
/// bare `Variable`, since unwrapping it would violate the "every `Variable` needs a `Dependence`
/// parent" invariant (the source system's own `outDependenceExpression` has this same guard).
fn maybe_strip_identity_dependence(function: isl::MultiAff, operand: Expr) -> Result<Expr, Expr> {
    let identity_like = function.is_none_to_none() || function.is_identity().unwrap_or(false);
    if identity_like && !is_variable(&operand) {
        Ok(operand)
    } else {
        Err(expr_from_kind(ExprKind::Dependence { function, operand }))
    }
}

// ---------------------------------------------------------------------------------------------
// DependenceExpression rules
// ---------------------------------------------------------------------------------------------

fn try_dependence_rules(function: isl::MultiAff, operand: Expr) -> Result<Expr, Expr> {
    match *operand.kind {
        // f1 @ f2 @ E -> (f2 o f1) @ E, then re-check identity on the composed function (mirrors
        // the source system falling through to its identity check after this specific rewrite,
        // since this is the one dispatch branch that mutates the outer node in place rather than
        // replacing it outright).
        ExprKind::Dependence {
            function: inner_function,
            operand: inner_operand,
        } => {
            let composed = inner_function
                .pullback(function)
                .expect("Normalize: composing two already-valid dependence functions");
            maybe_strip_identity_dependence(composed, inner_operand)
        }
        // f1 @ val(f2) -> val(f2 o f1)
        ExprKind::IndexFunction {
            function: inner_function,
        } => {
            let composed = inner_function
                .pullback(function)
                .expect("Normalize: composing an already-valid dependence into an index function");
            Ok(expr_from_kind(ExprKind::IndexFunction {
                function: composed,
            }))
        }
        // f @ D:E -> f^-1(D) : (f @ E)
        ExprKind::Restrict {
            domain,
            operand: inner_operand,
        } => {
            let preimage = domain
                .preimage_multi_aff(function.clone())
                .expect("Normalize: preimage of an already-valid restrict domain");
            let new_operand = expr_from_kind(ExprKind::Dependence {
                function,
                operand: inner_operand,
            });
            Ok(expr_from_kind(ExprKind::Restrict {
                domain: preimage,
                operand: new_operand,
            }))
        }
        // f @ (A op B) -> (f@A) op (f@B)
        ExprKind::Binary { operator, lhs, rhs } => {
            let new_lhs = expr_from_kind(ExprKind::Dependence {
                function: function.clone(),
                operand: lhs,
            });
            let new_rhs = expr_from_kind(ExprKind::Dependence {
                function,
                operand: rhs,
            });
            Ok(expr_from_kind(ExprKind::Binary {
                operator,
                lhs: new_lhs,
                rhs: new_rhs,
            }))
        }
        // f @ (op E) -> op (f@E)
        ExprKind::Unary {
            operator,
            operand: inner_operand,
        } => {
            let new_operand = expr_from_kind(ExprKind::Dependence {
                function,
                operand: inner_operand,
            });
            Ok(expr_from_kind(ExprKind::Unary {
                operator,
                operand: new_operand,
            }))
        }
        // f @ op(E1, E2, ...) -> op(f@E1, f@E2, ...)
        ExprKind::MultiArg { operator, args } => {
            let new_args = args
                .into_iter()
                .map(|a| {
                    expr_from_kind(ExprKind::Dependence {
                        function: function.clone(),
                        operand: a,
                    })
                })
                .collect();
            Ok(expr_from_kind(ExprKind::MultiArg {
                operator,
                args: new_args,
            }))
        }
        // f @ case{E1, E2, ...} -> case{f@E1, f@E2, ...}
        ExprKind::Case { name, branches } => {
            let new_branches = branches
                .into_iter()
                .map(|b| {
                    expr_from_kind(ExprKind::Dependence {
                        function: function.clone(),
                        operand: b,
                    })
                })
                .collect();
            Ok(expr_from_kind(ExprKind::Case {
                name,
                branches: new_branches,
            }))
        }
        // f @ if(C, T, E) -> if(f@C, f@T, f@E)
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let new_cond = expr_from_kind(ExprKind::Dependence {
                function: function.clone(),
                operand: cond,
            });
            let new_then = expr_from_kind(ExprKind::Dependence {
                function: function.clone(),
                operand: then_branch,
            });
            let new_else = expr_from_kind(ExprKind::Dependence {
                function,
                operand: else_branch,
            });
            Ok(expr_from_kind(ExprKind::If {
                cond: new_cond,
                then_branch: new_then,
                else_branch: new_else,
            }))
        }
        // f @ conv(...) -> conv(kernel, f'@weight, f'@data): deferred (see module doc / the
        // design doc's convolution gap) — `f'` needs `AlphaExpressionUtil.extendMultiAffWithIdentityDimensions`,
        // and lowering already excludes any equation with a Convolution, so this never actually
        // fires today; left as a documented no-op rather than implemented against an unreachable
        // input.
        other => maybe_strip_identity_dependence(
            function,
            Expr::new(other, operand.expression_domain, operand.context_domain),
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// RestrictExpression rules
// ---------------------------------------------------------------------------------------------

fn try_restrict_rules(domain: Set, operand: Expr) -> Result<Expr, Expr> {
    match *operand.kind {
        // D1 : D2 : E -> (D1 and D2) : E
        ExprKind::Restrict {
            domain: inner_domain,
            operand: inner_operand,
        } => {
            let merged = domain
                .intersect(inner_domain)
                .expect("Normalize: intersecting two already-valid restrict domains");
            Ok(expr_from_kind(ExprKind::Restrict {
                domain: merged,
                operand: inner_operand,
            }))
        }
        // D : auto : E -> auto : E (the outer domain is simply discarded — `auto`'s own meaning
        // is entirely relative to sibling case branches, independent of any wrapping restrict).
        ExprKind::AutoRestrict { .. } => Ok(operand),
        // D : case{E1, E2, ...} -> case{D:E1, D:E2, ...}
        ExprKind::Case { name, branches } => {
            let new_branches = branches
                .into_iter()
                .map(|b| {
                    expr_from_kind(ExprKind::Restrict {
                        domain: domain.clone(),
                        operand: b,
                    })
                })
                .collect();
            Ok(expr_from_kind(ExprKind::Case {
                name,
                branches: new_branches,
            }))
        }
        other => try_redundant_restrict(
            domain,
            Expr::new(other, operand.expression_domain, operand.context_domain),
        ),
    }
}

fn try_redundant_restrict(domain: Set, operand: Expr) -> Result<Expr, Expr> {
    let restrict = expr_from_kind(ExprKind::Restrict {
        domain,
        operand: operand.clone(),
    });
    // D : E -> E if D doesn't actually narrow E's domain (see module doc for the one place this
    // departs from the source system's literal behavior: applied unconditionally here).
    if restrict
        .expression_domain
        .is_equal(&operand.expression_domain)
        .unwrap_or(false)
    {
        Ok(operand)
    } else {
        Err(restrict)
    }
}

// ---------------------------------------------------------------------------------------------
// BinaryExpression rules
// ---------------------------------------------------------------------------------------------

fn try_binary_rules(operator: String, lhs: Expr, rhs: Expr) -> Result<Expr, Expr> {
    match (&*lhs.kind, &*rhs.kind) {
        (ExprKind::Case { .. }, ExprKind::Case { .. }) => cross_product_cases(operator, lhs, rhs),
        (ExprKind::Case { .. }, _) => Ok(distribute_case_binary(operator, lhs, rhs, true)),
        (_, ExprKind::Case { .. }) => Ok(distribute_case_binary(operator, lhs, rhs, false)),
        (ExprKind::Restrict { .. }, _) => {
            let ExprKind::Restrict { domain, operand } = *lhs.kind else {
                unreachable!()
            };
            let new_binary = expr_from_kind(ExprKind::Binary {
                operator,
                lhs: operand,
                rhs,
            });
            Ok(expr_from_kind(ExprKind::Restrict {
                domain,
                operand: new_binary,
            }))
        }
        (_, ExprKind::Restrict { .. }) => {
            let ExprKind::Restrict { domain, operand } = *rhs.kind else {
                unreachable!()
            };
            let new_binary = expr_from_kind(ExprKind::Binary {
                operator,
                lhs,
                rhs: operand,
            });
            Ok(expr_from_kind(ExprKind::Restrict {
                domain,
                operand: new_binary,
            }))
        }
        _ => Err(expr_from_kind(ExprKind::Binary { operator, lhs, rhs })),
    }
}

/// `case{L1,L2,...} op case{R1,R2,...} -> case{...}`, one new branch per pair of left/right
/// branches whose context domains actually overlap — each new branch restricted to exactly that
/// overlap. Branches on either side that are themselves bare `Restrict`s are unwrapped first
/// (the fresh, exact intersection domain replaces whatever coarser domain marker they carried).
fn cross_product_cases(operator: String, lhs: Expr, rhs: Expr) -> Result<Expr, Expr> {
    let ExprKind::Case {
        branches: left_branches,
        ..
    } = *lhs.kind
    else {
        unreachable!()
    };
    let ExprKind::Case {
        branches: right_branches,
        ..
    } = *rhs.kind
    else {
        unreachable!()
    };

    let mut new_branches = Vec::new();
    for l in &left_branches {
        for r in &right_branches {
            let (Some(l_ctx), Some(r_ctx)) = (&l.context_domain, &r.context_domain) else {
                // Context domains aren't available yet (e.g. first structural pass before the
                // initial refresh) — skip pairing for now; the next round, after a context
                // refresh, will see them.
                continue;
            };
            let overlap = match l_ctx.clone().intersect(r_ctx.clone()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if overlap.is_empty().unwrap_or(true) {
                continue;
            }
            let l_inner = unwrap_restrict(l.clone());
            let r_inner = unwrap_restrict(r.clone());
            let combined = expr_from_kind(ExprKind::Binary {
                operator: operator.clone(),
                lhs: l_inner,
                rhs: r_inner,
            });
            new_branches.push(expr_from_kind(ExprKind::Restrict {
                domain: overlap,
                operand: combined,
            }));
        }
    }
    if new_branches.is_empty() {
        // No context domains were available at all (e.g. the first structural pass before the
        // initial refresh) — signal "no progress" rather than `Ok` with an unchanged shape, which
        // would make the caller's fixpoint loop retry this forever. A later round, after a
        // context refresh, will see them.
        return Err(expr_from_kind(ExprKind::Binary {
            operator,
            lhs: expr_from_kind(ExprKind::Case {
                name: None,
                branches: left_branches,
            }),
            rhs: expr_from_kind(ExprKind::Case {
                name: None,
                branches: right_branches,
            }),
        }));
    }
    Ok(expr_from_kind(ExprKind::Case {
        name: None,
        branches: new_branches,
    }))
}

fn unwrap_restrict(e: Expr) -> Expr {
    let expression_domain = e.expression_domain.clone();
    let context_domain = e.context_domain.clone();
    match *e.kind {
        ExprKind::Restrict { operand, .. } => operand,
        other => Expr::new(other, expression_domain, context_domain),
    }
}

/// `case{E1,E2,...} op E -> case{E1 op E, E2 op E, ...}` (or the mirror image if the case is on
/// the right) — `case_is_left` picks which side gets distributed into.
fn distribute_case_binary(operator: String, lhs: Expr, rhs: Expr, case_is_left: bool) -> Expr {
    let (case_expr, other) = if case_is_left { (lhs, rhs) } else { (rhs, lhs) };
    let ExprKind::Case { branches, .. } = *case_expr.kind else {
        unreachable!()
    };
    let new_branches = branches
        .into_iter()
        .map(|b| {
            let (l, r) = if case_is_left {
                (b, other.clone())
            } else {
                (other.clone(), b)
            };
            expr_from_kind(ExprKind::Binary {
                operator: operator.clone(),
                lhs: l,
                rhs: r,
            })
        })
        .collect();
    expr_from_kind(ExprKind::Case {
        name: None,
        branches: new_branches,
    })
}

// ---------------------------------------------------------------------------------------------
// UnaryExpression rules
// ---------------------------------------------------------------------------------------------

fn try_unary_rules(operator: String, operand: Expr) -> Result<Expr, Expr> {
    match *operand.kind {
        // (op D:E) -> D:(op E)
        ExprKind::Restrict {
            domain,
            operand: inner,
        } => {
            let new_unary = expr_from_kind(ExprKind::Unary {
                operator,
                operand: inner,
            });
            Ok(expr_from_kind(ExprKind::Restrict {
                domain,
                operand: new_unary,
            }))
        }
        // op case{E1,E2,...} -> case{op E1, op E2, ...}
        ExprKind::Case { name, branches } => {
            let new_branches = branches
                .into_iter()
                .map(|b| {
                    expr_from_kind(ExprKind::Unary {
                        operator: operator.clone(),
                        operand: b,
                    })
                })
                .collect();
            Ok(expr_from_kind(ExprKind::Case {
                name,
                branches: new_branches,
            }))
        }
        other => Err(expr_from_kind(ExprKind::Unary {
            operator,
            operand: Expr::new(other, operand.expression_domain, operand.context_domain),
        })),
    }
}

// ---------------------------------------------------------------------------------------------
// MultiArgExpression rules
// ---------------------------------------------------------------------------------------------

fn try_multi_arg_rules(operator: Operator, args: Vec<Expr>) -> Result<Expr, Expr> {
    // f(op, ..., case{E1,E2,...}, ...) -> case{f(op, ..., E1, ...), f(op, ..., E2, ...), ...} —
    // handles (and replaces) the first Case argument found; a further Case argument, if any, is
    // picked up on the next fixpoint iteration (mirrors the source system's one-at-a-time loop).
    if let Some(idx) = args
        .iter()
        .position(|a| matches!(&*a.kind, ExprKind::Case { .. }))
    {
        let mut args = args;
        let case_arg = args.remove(idx);
        let ExprKind::Case { branches, .. } = *case_arg.kind else {
            unreachable!()
        };
        let new_branches = branches
            .into_iter()
            .map(|b| {
                let mut new_args = args.clone();
                new_args.insert(idx, b);
                expr_from_kind(ExprKind::MultiArg {
                    operator: operator.clone(),
                    args: new_args,
                })
            })
            .collect();
        return Ok(expr_from_kind(ExprKind::Case {
            name: None,
            branches: new_branches,
        }));
    }

    // f(op, D1:E1, D2:E2, X, ...) -> (D1 and D2 and ...) : f(op, E1, E2, X, ...)
    if args
        .iter()
        .any(|a| matches!(&*a.kind, ExprKind::Restrict { .. }))
    {
        let mut combined_domain: Option<Set> = None;
        let new_args = args
            .into_iter()
            .map(|a| {
                let expression_domain = a.expression_domain.clone();
                let context_domain = a.context_domain.clone();
                match *a.kind {
                    ExprKind::Restrict { domain, operand } => {
                        combined_domain = Some(match combined_domain.take() {
                            None => domain,
                            Some(d) => d
                                .intersect(domain)
                                .expect("Normalize: intersecting multi-arg restrict domains"),
                        });
                        operand
                    }
                    other => Expr::new(other, expression_domain, context_domain),
                }
            })
            .collect();
        let domain = combined_domain.expect("at least one Restrict arg was found above");
        let new_multi_arg = expr_from_kind(ExprKind::MultiArg {
            operator,
            args: new_args,
        });
        return Ok(expr_from_kind(ExprKind::Restrict {
            domain,
            operand: new_multi_arg,
        }));
    }

    Err(expr_from_kind(ExprKind::MultiArg { operator, args }))
}

// ---------------------------------------------------------------------------------------------
// CaseExpression rules
// ---------------------------------------------------------------------------------------------

fn try_case_rules(name: Option<String>, branches: Vec<Expr>, deep: bool) -> Result<Expr, Expr> {
    let can_flatten = |b: &Expr| match &*b.kind {
        ExprKind::Case { name, .. } => deep || name.is_none(),
        _ => false,
    };

    // case{E1, case{E2,E3,...}, E4, ...} -> case{E1, E2, E3, ..., E4, ...}
    if branches.iter().any(can_flatten) {
        let mut flattened = Vec::with_capacity(branches.len());
        for b in branches {
            if can_flatten(&b) {
                let ExprKind::Case {
                    branches: inner, ..
                } = *b.kind
                else {
                    unreachable!()
                };
                flattened.extend(inner);
            } else {
                flattened.push(b);
            }
        }
        return Ok(expr_from_kind(ExprKind::Case {
            name,
            branches: flattened,
        }));
    }

    // Remove branches whose context domain is known to be empty (unreachable).
    let had_context = branches.iter().all(|b| b.context_domain.is_some());
    if had_context {
        let pruned: Vec<Expr> = branches
            .into_iter()
            .filter(|b| {
                !b.context_domain
                    .as_ref()
                    .is_some_and(|d| d.is_empty().unwrap_or(false))
            })
            .collect();
        // Collapse a single-branch case to its lone branch.
        if pruned.len() == 1 {
            return Ok(pruned.into_iter().next().unwrap());
        }
        return Err(expr_from_kind(ExprKind::Case {
            name,
            branches: pruned,
        }));
    }

    if branches.len() == 1 {
        return Ok(branches.into_iter().next().unwrap());
    }

    Err(expr_from_kind(ExprKind::Case { name, branches }))
}

// ---------------------------------------------------------------------------------------------
// IfExpression rules
// ---------------------------------------------------------------------------------------------

/// Tries, in order, `cond` then `then_branch` then `else_branch`: if that slot is a `Restrict`,
/// pull it out (`if(D:C,T,E) -> D:if(C,T,E)`, and the `then`/`else` mirrors); if it's a `Case`,
/// distribute over its branches. Stops at the first slot that matches, mirroring the source
/// system's own priority order (`outIfExpression` tries `cond`, then `then`, then `else`, each via
/// a single-slot dispatch that returns as soon as one fires).
fn try_if_rules(cond: Expr, then_branch: Expr, else_branch: Expr) -> Result<Expr, Expr> {
    let cond_domain = (cond.expression_domain.clone(), cond.context_domain.clone());
    let then_domain = (
        then_branch.expression_domain.clone(),
        then_branch.context_domain.clone(),
    );
    let else_domain = (
        else_branch.expression_domain.clone(),
        else_branch.context_domain.clone(),
    );
    match *cond.kind {
        ExprKind::Restrict { domain, operand } => {
            let new_if = expr_from_kind(ExprKind::If {
                cond: operand,
                then_branch,
                else_branch,
            });
            Ok(expr_from_kind(ExprKind::Restrict {
                domain,
                operand: new_if,
            }))
        }
        ExprKind::Case { branches, .. } => {
            let new_branches = branches
                .into_iter()
                .map(|c| {
                    expr_from_kind(ExprKind::If {
                        cond: c,
                        then_branch: then_branch.clone(),
                        else_branch: else_branch.clone(),
                    })
                })
                .collect();
            Ok(expr_from_kind(ExprKind::Case {
                name: None,
                branches: new_branches,
            }))
        }
        other => {
            let cond = Expr::new(other, cond_domain.0, cond_domain.1);
            match *then_branch.kind {
                ExprKind::Restrict { domain, operand } => {
                    let new_if = expr_from_kind(ExprKind::If {
                        cond,
                        then_branch: operand,
                        else_branch,
                    });
                    Ok(expr_from_kind(ExprKind::Restrict {
                        domain,
                        operand: new_if,
                    }))
                }
                ExprKind::Case { branches, .. } => {
                    let new_branches = branches
                        .into_iter()
                        .map(|t| {
                            expr_from_kind(ExprKind::If {
                                cond: cond.clone(),
                                then_branch: t,
                                else_branch: else_branch.clone(),
                            })
                        })
                        .collect();
                    Ok(expr_from_kind(ExprKind::Case {
                        name: None,
                        branches: new_branches,
                    }))
                }
                other_then => {
                    let then_branch = Expr::new(other_then, then_domain.0, then_domain.1);
                    match *else_branch.kind {
                        ExprKind::Restrict { domain, operand } => {
                            let new_if = expr_from_kind(ExprKind::If {
                                cond,
                                then_branch,
                                else_branch: operand,
                            });
                            Ok(expr_from_kind(ExprKind::Restrict {
                                domain,
                                operand: new_if,
                            }))
                        }
                        ExprKind::Case { branches, .. } => {
                            let new_branches = branches
                                .into_iter()
                                .map(|e| {
                                    expr_from_kind(ExprKind::If {
                                        cond: cond.clone(),
                                        then_branch: then_branch.clone(),
                                        else_branch: e,
                                    })
                                })
                                .collect();
                            Ok(expr_from_kind(ExprKind::Case {
                                name: None,
                                branches: new_branches,
                            }))
                        }
                        other_else => {
                            let else_branch = Expr::new(other_else, else_domain.0, else_domain.1);
                            Err(expr_from_kind(ExprKind::If {
                                cond,
                                then_branch,
                                else_branch,
                            }))
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AutoRestrictExpression rules
// ---------------------------------------------------------------------------------------------

fn try_auto_restrict_rules(operand: Expr) -> Result<Expr, Expr> {
    let saved = (
        operand.expression_domain.clone(),
        operand.context_domain.clone(),
    );
    match *operand.kind {
        // auto : D : E -> auto : E
        ExprKind::Restrict { operand: inner, .. } => {
            Ok(expr_from_kind(ExprKind::AutoRestrict { operand: inner }))
        }
        // auto : auto : E -> auto : E
        ExprKind::AutoRestrict { operand: inner } => {
            Ok(expr_from_kind(ExprKind::AutoRestrict { operand: inner }))
        }
        other => Err(expr_from_kind(ExprKind::AutoRestrict {
            operand: Expr::new(other, saved.0, saved.1),
        })),
    }
}

// ---------------------------------------------------------------------------------------------
// ReduceExpression rules
// ---------------------------------------------------------------------------------------------

/// The one rule touching a reduction itself: `reduce(op1,f1, D:reduce(op2,f2,E)) ->
/// reduce(op1,f1, reduce(op2,f2, f2^-1(D):E))`, exposing a nested reduction that would otherwise
/// be hidden behind a `Restrict`. The source system triggers this from the *inner* `Restrict`'s
/// own rule (checking `re.eContainer instanceof AbstractReduceExpression`); this crate's `Expr`
/// has no parent back-reference, so it's checked from the outer `Reduce`'s side instead — the
/// same pattern-match, initiated from the other end of the same parent-child relationship.
fn try_reduce_rules(
    is_arg_reduce: bool,
    operator: Operator,
    projection: isl::MultiAff,
    body_context: Vec<String>,
    body: Expr,
) -> Result<Expr, Expr> {
    let body_saved = (body.expression_domain.clone(), body.context_domain.clone());
    match *body.kind {
        ExprKind::Restrict { domain, operand } => {
            let operand_saved = (
                operand.expression_domain.clone(),
                operand.context_domain.clone(),
            );
            match *operand.kind {
                ExprKind::Reduce {
                    is_arg_reduce: inner_arg,
                    operator: inner_op,
                    projection: inner_proj,
                    body_context: inner_body_context,
                    body: inner_body,
                } => {
                    let preimage = domain
                        .preimage_multi_aff(inner_proj.clone())
                        .expect("Normalize: preimage for nested-reduction fusion");
                    let new_inner_body = expr_from_kind(ExprKind::Restrict {
                        domain: preimage,
                        operand: inner_body,
                    });
                    let new_inner_reduce = expr_from_kind(ExprKind::Reduce {
                        is_arg_reduce: inner_arg,
                        operator: inner_op,
                        projection: inner_proj,
                        body_context: inner_body_context,
                        body: new_inner_body,
                    });
                    Ok(expr_from_kind(ExprKind::Reduce {
                        is_arg_reduce,
                        operator,
                        projection,
                        body_context,
                        body: new_inner_reduce,
                    }))
                }
                other_inner => {
                    let body = expr_from_kind(ExprKind::Restrict {
                        domain,
                        operand: Expr::new(other_inner, operand_saved.0, operand_saved.1),
                    });
                    Err(expr_from_kind(ExprKind::Reduce {
                        is_arg_reduce,
                        operator,
                        projection,
                        body_context,
                        body,
                    }))
                }
            }
        }
        other => Err(expr_from_kind(ExprKind::Reduce {
            is_arg_reduce,
            operator,
            projection,
            body_context,
            body: Expr::new(other, body_saved.0, body_saved.1),
        })),
    }
}

// ---------------------------------------------------------------------------------------------
// Top-level dispatch and drivers
// ---------------------------------------------------------------------------------------------

fn try_rewrite(e: Expr, deep: bool) -> Result<Expr, Expr> {
    let saved = (e.expression_domain.clone(), e.context_domain.clone());
    match *e.kind {
        ExprKind::Dependence { function, operand } => try_dependence_rules(function, operand),
        ExprKind::Restrict { domain, operand } => try_restrict_rules(domain, operand),
        ExprKind::Binary { operator, lhs, rhs } => try_binary_rules(operator, lhs, rhs),
        ExprKind::Unary { operator, operand } => try_unary_rules(operator, operand),
        ExprKind::MultiArg { operator, args } => try_multi_arg_rules(operator, args),
        ExprKind::Case { name, branches } => try_case_rules(name, branches, deep),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => try_if_rules(cond, then_branch, else_branch),
        ExprKind::AutoRestrict { operand } => try_auto_restrict_rules(operand),
        ExprKind::Reduce {
            is_arg_reduce,
            operator,
            projection,
            body_context,
            body,
        } => try_reduce_rules(is_arg_reduce, operator, projection, body_context, body),
        other => Err(Expr::new(other, saved.0, saved.1)),
    }
}

/// Normalizes every child slot of `e`, bottom-up, before `e`'s own rules are tried. Every slot
/// uses [`normalize_expr`] (which enforces the "every `Variable` needs a `Dependence` parent"
/// invariant on its result) except a `Dependence`'s own operand, which uses
/// [`normalize_dependence_operand`] instead — that's the one position a bare `Variable` is
/// exactly the required shape, not a violation to fix.
fn normalize_children(e: Expr, deep: bool) -> Expr {
    let Expr {
        kind,
        expression_domain,
        context_domain,
    } = e;
    let kind = match *kind {
        leaf @ (ExprKind::Variable(_)
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Real(_)
        | ExprKind::IndexFunction { .. }
        | ExprKind::IndexPolynomial { .. }) => leaf,
        ExprKind::Dependence { function, operand } => ExprKind::Dependence {
            function,
            operand: normalize_dependence_operand(operand, deep),
        },
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ExprKind::If {
            cond: normalize_expr(cond, deep),
            then_branch: normalize_expr(then_branch, deep),
            else_branch: normalize_expr(else_branch, deep),
        },
        ExprKind::Restrict { domain, operand } => ExprKind::Restrict {
            domain,
            operand: normalize_expr(operand, deep),
        },
        ExprKind::AutoRestrict { operand } => ExprKind::AutoRestrict {
            operand: normalize_expr(operand, deep),
        },
        ExprKind::Case { name, branches } => ExprKind::Case {
            name,
            branches: branches
                .into_iter()
                .map(|b| normalize_expr(b, deep))
                .collect(),
        },
        ExprKind::Reduce {
            is_arg_reduce,
            operator,
            projection,
            body_context,
            body,
        } => ExprKind::Reduce {
            is_arg_reduce,
            operator,
            projection,
            body_context,
            body: normalize_expr(body, deep),
        },
        ExprKind::Convolution {
            kernel_domain,
            kernel_expr,
            data_expr,
        } => ExprKind::Convolution {
            kernel_domain,
            kernel_expr: normalize_expr(kernel_expr, deep),
            data_expr: normalize_expr(data_expr, deep),
        },
        ExprKind::Select { relation, operand } => ExprKind::Select {
            relation,
            operand: normalize_expr(operand, deep),
        },
        ExprKind::MultiArg { operator, args } => ExprKind::MultiArg {
            operator,
            args: args.into_iter().map(|a| normalize_expr(a, deep)).collect(),
        },
        ExprKind::Binary { operator, lhs, rhs } => ExprKind::Binary {
            operator,
            lhs: normalize_expr(lhs, deep),
            rhs: normalize_expr(rhs, deep),
        },
        ExprKind::Unary { operator, operand } => ExprKind::Unary {
            operator,
            operand: normalize_expr(operand, deep),
        },
    };
    // Domain fields may now be stale relative to the (possibly rewritten) children — harmless:
    // `try_rewrite` immediately either reconstructs this node via `expr_from_kind` (fresh,
    // correct `expression_domain`) or, if untouched, its `Err` fallback arm does the same.
    Expr::new(kind, expression_domain, context_domain)
}

/// Bottom-up normalize-to-fixpoint of one expression: normalize children first, then apply this
/// node's own rules repeatedly (each successful rewrite's result is itself re-normalized from
/// scratch — mirroring the source system's `reapply`, since a rewrite can both change this node's
/// own shape *and* introduce fresh child nodes, like a newly pushed-down `Dependence`, that
/// themselves need full bottom-up treatment) until none apply. Ensures the result isn't a bare
/// `Variable` unless it's already wrapped by a `Dependence` one level up (checked by the caller
/// via the recursive structure — see [`normalize_children`]'s doc).
pub fn normalize_expr(e: Expr, deep: bool) -> Expr {
    let original_context = e.context_domain.clone();
    let e = normalize_children(e, deep);
    match try_rewrite(e, deep) {
        Ok(new_e) => normalize_expr(new_e, deep),
        Err(mut unchanged) => {
            // `try_rewrite` returning `Err` means this node's own top-level shape didn't change
            // (only, possibly, its children did — handled by their own recursive calls). Its own
            // context domain is therefore still valid; every `try_*_rules` "no rule matched"
            // fallback reconstructs its result via `expr_from_kind`, which always sets
            // `context_domain: None` (correct for a genuinely *new* node, wrong here) — restore
            // it rather than leaving it wiped, or a parent relying on fresh context right after a
            // `refresh_context_domains` call (case-branch pruning, the binary-case cross-product)
            // would never actually see it: children are always normalized before their parent's
            // own rules run, so by the time the parent checks, an unwritten child would already
            // have "unchanged-but-wiped" its own context on the way back up.
            if unchanged.context_domain.is_none() {
                unchanged.context_domain = original_context;
            }
            ensure_variable_wrapped(unchanged)
        }
    }
}

/// Same as [`normalize_expr`], but for a `Dependence`'s own operand slot: a bare `Variable` result
/// here is exactly the required normal form, not a violation to wrap.
fn normalize_dependence_operand(e: Expr, deep: bool) -> Expr {
    let original_context = e.context_domain.clone();
    let e = normalize_children(e, deep);
    match try_rewrite(e, deep) {
        Ok(new_e) => normalize_dependence_operand(new_e, deep),
        Err(mut unchanged) => {
            if unchanged.context_domain.is_none() {
                unchanged.context_domain = original_context;
            }
            unchanged
        }
    }
}

/// `V -> I @ V`: every bare `Variable` gets wrapped in an identity dependence, reusing its own
/// (already correct) domains verbatim on both the wrapper and the unchanged inner variable —
/// mirroring the source system's `outVariableExpression`, which copies `ve`'s existing
/// `contextDomain`/`expressionDomain` onto both rather than recomputing (recomputing via
/// `preimage` of an identity function over its own domain would be mathematically equivalent, but
/// copying is simpler and avoids any isl-representation drift).
fn ensure_variable_wrapped(e: Expr) -> Expr {
    if !matches!(&*e.kind, ExprKind::Variable(_)) {
        return e;
    }
    let space = e.expression_domain.space();
    let identity = isl::MultiAff::identity_on_domain_space(space)
        .expect("Normalize: identity function over an already-valid domain space");
    let expression_domain = e.expression_domain.clone();
    let context_domain = e.context_domain.clone();
    Expr::new(
        ExprKind::Dependence {
            function: identity,
            operand: e,
        },
        expression_domain,
        context_domain,
    )
}

/// Top-down: (re)establishes `context_domain` on every node in `e`'s subtree, given `e`'s own
/// (already-correct) context — mirrors `alpha_model::domain`'s phase-4 `context_domain`, just
/// operating on this crate's owned tree with the function/relation/projection objects already in
/// hand instead of re-deriving them from syntax.
fn refresh_context(e: &mut Expr, parent_context: Set) {
    let own_context = parent_context.intersect(e.expression_domain.clone()).ok();
    e.context_domain = own_context.clone();
    let Some(own_context) = own_context else {
        return;
    };
    match e.kind.as_mut() {
        ExprKind::Variable(_)
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Real(_)
        | ExprKind::IndexFunction { .. }
        | ExprKind::IndexPolynomial { .. } => {}
        ExprKind::Dependence { function, operand } => {
            if let Ok(map) = function.clone().into_map() {
                if let Ok(processed) = own_context.apply(map) {
                    refresh_context(operand, processed);
                }
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            refresh_context(cond, own_context.clone());
            refresh_context(then_branch, own_context.clone());
            refresh_context(else_branch, own_context);
        }
        ExprKind::Restrict { operand, .. } => refresh_context(operand, own_context),
        ExprKind::AutoRestrict { operand } => refresh_context(operand, own_context),
        ExprKind::Case { branches, .. } => refresh_case_branches(branches, own_context),
        ExprKind::Reduce {
            projection, body, ..
        } => {
            if let Ok(processed) = own_context.preimage_multi_aff(projection.clone()) {
                refresh_context(body, processed);
            }
        }
        ExprKind::Convolution { .. } => {
            // Unreachable in practice (see `expr_from_kind`'s doc), kept as a harmless no-op
            // rather than a panic in case that ever changes.
        }
        ExprKind::Select { relation, operand } => {
            if let Ok(processed) = own_context.apply(relation.clone()) {
                refresh_context(operand, processed);
            }
        }
        ExprKind::MultiArg { args, .. } => {
            for a in args.iter_mut() {
                refresh_context(a, own_context.clone());
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            refresh_context(lhs, own_context.clone());
            refresh_context(rhs, own_context);
        }
        ExprKind::Unary { operand, .. } => refresh_context(operand, own_context),
    }
}

/// `Case`'s branches need one special case within the generic top-down walk: an `AutoRestrict`
/// branch's context isn't `parent_context ∩ own_domain` like every other node — it's `parent_context`
/// minus the union of every *other* branch's expression domain (or, if it's the only branch, just
/// `parent_context`), intersected with its own domain. Mirrors
/// `alpha_model::domain::Resolver`'s `auto_restrict_context`, but with direct sibling access
/// (this crate's `Case` holds its branches as a plain `Vec`, unlike the syntax layer, which has no
/// parent back-reference and has to walk up to find siblings).
fn refresh_case_branches(branches: &mut [Expr], parent_context: Set) {
    let n = branches.len();
    for branch in branches.iter_mut() {
        if !matches!(&*branch.kind, ExprKind::AutoRestrict { .. }) {
            refresh_context(branch, parent_context.clone());
        }
    }
    for i in 0..n {
        if !matches!(&*branches[i].kind, ExprKind::AutoRestrict { .. }) {
            continue;
        }
        let own_domain = branches[i].expression_domain.clone();
        let inferred = if n == 1 {
            parent_context.clone().intersect(own_domain)
        } else {
            let mut union: Option<Set> = None;
            for (j, b) in branches.iter().enumerate() {
                if i == j {
                    continue;
                }
                union = Some(match union {
                    None => b.expression_domain.clone(),
                    Some(u) => u
                        .union(b.expression_domain.clone())
                        .expect("Normalize: unioning sibling case-branch domains"),
                });
            }
            parent_context
                .clone()
                .subtract(union.expect("n > 1 implies at least one sibling"))
                .and_then(|s| s.intersect(own_domain))
        };
        if let Ok(inferred) = inferred {
            branches[i].context_domain = Some(inferred.clone());
            if let ExprKind::AutoRestrict { operand } = branches[i].kind.as_mut() {
                refresh_context(operand, inferred);
            }
        }
    }
}

/// Alternates structural rewriting with a context refresh for a bounded number of rounds —
/// several of `Normalize`'s own rules (case-branch pruning by empty context, the binary-case
/// cross-product) need *current* context domains to fire correctly, and a structural rewrite can
/// change what those should be. `MAX_ROUNDS` is a pragmatic bound, not a proof of convergence —
/// the source system's own doc comment admits its single bottom-up pass "may create situations
/// where multiple calls are required to reach the normal form," and real Alpha programs don't
/// nest `case`/`restrict`/`reduce` deeply enough for this to matter in practice.
const MAX_ROUNDS: u32 = 8;

fn normalize_to_fixpoint(expr: Expr, top_context: Set, deep: bool) -> Expr {
    let mut expr = expr;
    for _ in 0..MAX_ROUNDS {
        refresh_context(&mut expr, top_context.clone());
        expr = normalize_expr(expr, deep);
    }
    refresh_context(&mut expr, top_context);
    expr
}

/// Normalizes every `StandardEquation` in `system` (context-aware — see [`normalize_to_fixpoint`])
/// and every `UseEquation`'s input/output expressions (structural rules only; no context domain
/// is available there — see [`crate::ir`]'s module doc for that gap).
pub fn apply(system: System, deep: bool) -> System {
    let System {
        name,
        inputs,
        outputs,
        locals,
        bodies,
    } = system;

    let find_domain = |var_name: &str| -> Set {
        inputs
            .iter()
            .chain(outputs.iter())
            .chain(locals.iter())
            .find(|v| v.name == var_name)
            .map(|v| v.domain.clone())
            .unwrap_or_else(|| panic!("Normalize: equation variable '{var_name}' not declared"))
    };

    let bodies = bodies
        .into_iter()
        .map(|body| {
            let crate::ir::SystemBody { domain, equations } = body;
            let equations = equations
                .into_iter()
                .map(|eq| match eq {
                    Equation::Standard(s) => {
                        let crate::ir::StandardEquation { variable, index_names, expr } = s;
                        let top_context = find_domain(&variable)
                            .intersect_params(domain.clone())
                            .expect("Normalize: intersecting an already-valid variable domain with its body's parameter domain");
                        let expr = normalize_to_fixpoint(expr, top_context, deep);
                        Equation::Standard(crate::ir::StandardEquation { variable, index_names, expr })
                    }
                    Equation::Use(u) => {
                        let crate::ir::UseEquation {
                            callee,
                            output_exprs,
                            input_exprs,
                        } = u;
                        let output_exprs =
                            output_exprs.into_iter().map(|e| normalize_expr(e, deep)).collect();
                        let input_exprs =
                            input_exprs.into_iter().map(|e| normalize_expr(e, deep)).collect();
                        Equation::Use(crate::ir::UseEquation {
                            callee,
                            output_exprs,
                            input_exprs,
                        })
                    }
                })
                .collect();
            crate::ir::SystemBody { domain, equations }
        })
        .collect();

    System {
        name,
        inputs,
        outputs,
        locals,
        bodies,
    }
}
