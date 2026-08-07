//! The scheduled-codegen statement model (§4 of `docs/scheduled-codegen-design.md`): enumerates,
//! from a normalized `ir::System`, the exact set of independently-schedulable statements — one
//! per ordinary output/local variable (§4.1), or a `<name>__init`/`<name>__reduce` pair for a
//! variable whose equation is a top-level `Reduce` (§4.2). Shared by target-mapping validation
//! (`crate::schedule`, §6) and `ScheduledC` codegen itself (§8) — both need the same statement
//! set, just for different purposes (naming/domains vs. body codegen).

use crate::error::{CodegenError, Result};
use alpha_transform::ir;
use isl::{MultiAff, Set};
use std::collections::HashMap;

/// What a statement's body actually computes, once schedule-independent of the `WriteC`-only
/// pieces (flag checks, per-role read dispatch) — see `crate::expr`/`crate::scheduledc` for how
/// each variant becomes C.
///
/// Several fields aren't read yet — `crate::schedule` (§6, this same phase) only needs each
/// statement's `name`/`domain`; `operator`/`body`/`body_context`/`projection`/`equations` are read
/// by the legality checker (§7, next phase) and `ScheduledC`'s own body codegen (§8, after that).
#[allow(dead_code)]
pub(crate) enum StatementKind<'a> {
    /// §4.1: one or more piecewise equations for an ordinary output/local variable, guarded by
    /// their own `SystemBody` domain — the same `(domain, equation)` pairs `WriteC`'s own
    /// `equations_by_var` groups (decision 2, §2).
    Ordinary {
        equations: Vec<(&'a Set, &'a ir::StandardEquation)>,
    },
    /// §4.2's `<name>__init`: `reduce_local[i...] = <neutral element>`. `reduce_local` is the
    /// underlying `R_NR<n>`-style variable name both halves of the pair share storage with (§4.2)
    /// — the same string as this statement's own name with the `__init` suffix stripped.
    /// `body_context` is the *ambient* (`a`-dim) prefix of the paired `ReduceStep`'s own full
    /// naming hint — purely cosmetic (§8.2's body codegen falls back to synthesized names either
    /// way), kept here only so `__init` and `__reduce` can render the same ambient index under
    /// the same C identifier when a real source name is available.
    ReduceInit {
        reduce_local: &'a str,
        operator: &'a ir::Operator,
        body_context: &'a [String],
    },
    /// §4.2's `<name>__reduce`: `reduce_local[i...] = combine(reduce_local[i...], <body>)`.
    /// `projection` is the reduce's own ambient-index projection (`(i,j) -> i`) — the RAW
    /// dependence edge to `<name>__init`, per §4.2's legality note and §7.2.
    ReduceStep {
        reduce_local: &'a str,
        operator: &'a ir::Operator,
        body: &'a ir::Expr,
        body_context: &'a [String],
        projection: &'a MultiAff,
    },
}

pub(crate) struct Statement<'a> {
    pub name: String,
    pub domain: Set,
    pub kind: StatementKind<'a>,
}

/// Enumerates every statement in `system` (already normalized — §5.1's precondition; this
/// function doesn't itself check that, callers must have already run `normalize_reduction::apply`
/// then `normalize::apply`, §1/§3). Errors if `system` contains a `UseEquation` — `ScheduledC` has
/// no codegen backend for subsystem calls, matching `WriteC` (§11).
pub(crate) fn statements(system: &ir::System) -> Result<Vec<Statement<'_>>> {
    let mut equations_by_var: HashMap<&str, Vec<(&Set, &ir::StandardEquation)>> = HashMap::new();
    for body in &system.bodies {
        for eq in &body.equations {
            match eq {
                ir::Equation::Standard(s) => {
                    equations_by_var
                        .entry(s.variable.as_str())
                        .or_default()
                        .push((&body.domain, s));
                }
                ir::Equation::Use(_) => {
                    return Err(CodegenError::Unsupported(
                        "UseEquation has no codegen backend — matches WriteC, which doesn't \
                         support subsystem calls either"
                            .to_string(),
                    ));
                }
            }
        }
    }

    let mut result = Vec::new();
    let mut seen_names: HashMap<String, ()> = HashMap::new();
    let ordered_vars: Vec<&ir::Variable> =
        system.outputs.iter().chain(system.locals.iter()).collect();
    for v in &ordered_vars {
        let Some(eqs) = equations_by_var.get(v.name.as_str()) else {
            continue;
        };
        push_statements_for_variable(v, eqs, &mut result)?;
    }

    for stmt in &result {
        if seen_names.insert(stmt.name.clone(), ()).is_some() {
            // Should be unreachable given `normalize_reduction`'s own collision avoidance (§4.3)
            // — checked here anyway since a naming collision silently corrupting a target
            // mapping's tuple-name keying would be a much worse failure mode than a clear error.
            return Err(CodegenError::InvalidSchedule(format!(
                "internal error: two statements both named '{}' — normalize_reduction's naming \
                 collision avoidance should make this unreachable",
                stmt.name
            )));
        }
    }

    Ok(result)
}

fn push_statements_for_variable<'a>(
    v: &'a ir::Variable,
    eqs: &[(&'a Set, &'a ir::StandardEquation)],
    out: &mut Vec<Statement<'a>>,
) -> Result<()> {
    if let [(_, eq)] = eqs {
        if let ir::ExprKind::Reduce {
            is_arg_reduce: false,
            operator,
            projection,
            body_context,
            body,
        } = &*eq.expr.kind
        {
            // §4.2: split into <name>__init (domain = the reduce's own ambient a-dim domain,
            // i.e. the variable's own declared domain — normalize_reduction already narrowed it
            // to exactly this, see that module's doc) and <name>__reduce (domain = the reduce
            // body's full (a+b)-dim domain, context_domain ∩ expression_domain).
            let a = v.domain.dim(isl::DimType::OutOrSet) as usize;
            let ambient_hint: &[String] = if body_context.len() >= a {
                &body_context[..a]
            } else {
                &[]
            };
            out.push(Statement {
                name: format!("{}__init", v.name),
                domain: v.domain.clone(),
                kind: StatementKind::ReduceInit {
                    reduce_local: v.name.as_str(),
                    operator,
                    body_context: ambient_hint,
                },
            });
            let context_domain = body.context_domain.clone().ok_or_else(|| {
                CodegenError::Unsupported(
                    "reduce body missing a context domain — codegen must run after Normalize's \
                     context refresh"
                        .to_string(),
                )
            })?;
            let full_domain = body.expression_domain.clone().intersect(context_domain)?;
            out.push(Statement {
                name: format!("{}__reduce", v.name),
                domain: full_domain,
                kind: StatementKind::ReduceStep {
                    reduce_local: v.name.as_str(),
                    operator,
                    body,
                    body_context,
                    projection,
                },
            });
            return Ok(());
        }
    }
    // Not a single top-level Reduce equation (a plain equation, a piecewise variable, or an
    // is_arg_reduce reduce left for §8's codegen to report as unsupported, matching WriteC) —
    // an ordinary §4.1 statement.
    out.push(Statement {
        name: v.name.clone(),
        domain: v.domain.clone(),
        kind: StatementKind::Ordinary {
            equations: eqs.to_vec(),
        },
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{normalized_system, PIECEWISE_EQUATION, PLAIN_COPY, PREFIX_SUM};
    use isl::DimType;

    #[test]
    fn reduce_equation_splits_into_init_and_reduce_pair() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let names: Vec<&str> = stmts.iter().map(|s| s.name.as_str()).collect();
        // `Y[i] = reduce(...)` is already the equation's own topmost node — already normal form,
        // so `normalize_reduction` (§4.3) leaves it in place rather than hoisting it behind a
        // pointless `Y = Y_NR` copy. `Y` itself becomes the `__init`/`__reduce` pair (§4.2).
        assert_eq!(names, vec!["Y__init", "Y__reduce"]);

        assert!(matches!(
            stmts[0].kind,
            StatementKind::ReduceInit {
                reduce_local: "Y",
                ..
            }
        ));
        assert!(matches!(
            stmts[1].kind,
            StatementKind::ReduceStep {
                reduce_local: "Y",
                ..
            }
        ));
        // __init's domain is 1-d (just `i`); __reduce's is 2-d (`i` and the reduce's own `j`).
        assert_eq!(stmts[0].domain.dim(DimType::OutOrSet), 1);
        assert_eq!(stmts[1].domain.dim(DimType::OutOrSet), 2);
    }

    #[test]
    fn ordinary_equation_stays_one_statement() {
        let system = normalized_system(PLAIN_COPY);
        let stmts = statements(&system).unwrap();
        let names: Vec<&str> = stmts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Y"]);
        assert!(matches!(stmts[0].kind, StatementKind::Ordinary { .. }));
    }

    /// §5.6 of `docs/codegen-test-design.md` / design decision 2 (§2 of
    /// `docs/scheduled-codegen-design.md`): a variable defined by equations across multiple
    /// `SystemBody`s (piecewise) stays merged into **one** statement, not split per source body.
    #[test]
    fn piecewise_equation_stays_one_statement_with_both_equations() {
        let system = normalized_system(PIECEWISE_EQUATION);
        let stmts = statements(&system).unwrap();
        let names: Vec<&str> = stmts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Y"]);
        let StatementKind::Ordinary { equations } = &stmts[0].kind else {
            panic!("expected an Ordinary statement");
        };
        assert_eq!(equations.len(), 2, "both source bodies' equations for Y");
    }
}
