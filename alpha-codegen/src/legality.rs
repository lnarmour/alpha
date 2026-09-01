//! Legality checking (§7 of `docs/scheduled-codegen-design.md`): rejects a schedule (already
//! parsed/validated by `crate::schedule`, §6) that reorders a real dependence, rather than
//! silently miscompiling. Alpha is single-assignment, so every true dependence is already
//! explicit in the IR as a `Dependence` node — this is a direct translation of already-explicit
//! data through isl's set/map algebra (§7.1), not a new analysis.

use crate::error::{CodegenError, Result};
use crate::stmt::{Statement, StatementKind};
use alpha_transform::ir;
use isl::{DimType, Map, MultiAff, UnionMap};
use std::collections::HashMap;

/// Checks every dependence edge implied by `statements` against `schedule` (the fully-fused,
/// already-validated `UnionMap` `crate::schedule::build_schedule` produced). Returns the first
/// violation found as a diagnostic naming both statements — extracting a concrete counterexample
/// point is deferred past v1 (§7.2, §13).
pub(crate) fn check_legality(statements: &[Statement], schedule: &UnionMap) -> Result<()> {
    let producer_of = producer_of_map(statements);
    let by_name = statements_by_name(statements);
    let schedule_maps = schedule_fragments_by_name(schedule)?;
    let lex_ge = lex_ge_relation(&schedule_maps)?;

    for stmt in statements {
        let Some(consumer_schedule) = schedule_maps.get(stmt.name.as_str()) else {
            continue; // no instances to check (e.g. a statement with an empty domain)
        };
        for edge in dependence_edges(stmt) {
            // The synthetic `__reduce -> __init` edge (§4.2) already names its producer directly
            // — it must *not* go through `producer_of`, which resolves a read of the shared
            // variable to `__reduce` itself (correct for every *other* consumer, but wrong for
            // `__reduce`'s own self-referential ordering edge to its paired `__init`).
            let producer_stmt = match edge.producer {
                Producer::Named(name) => by_name.get(name.as_str()).copied(),
                Producer::Variable(var) => producer_of.get(var).copied(),
            };
            let Some(producer_stmt) = producer_stmt else {
                continue; // an unresolved variable is an input — always available, §7.3
            };
            let Some(producer_schedule) = schedule_maps.get(producer_stmt.name.as_str()) else {
                continue;
            };
            let dep_map = dep_fn_to_producer_domain(edge.dep_fn, &stmt.name, producer_stmt)?;
            check_edge(
                &lex_ge,
                &stmt.name,
                consumer_schedule,
                dep_map,
                &producer_stmt.name,
                producer_schedule,
            )?;
        }
    }
    Ok(())
}

/// Where a dependence edge's producer comes from: a real `Dependence` node names an IR *variable*
/// that still needs resolving (via `producer_of_map`) to whichever statement actually produces its
/// final value; the synthetic `__reduce -> __init` edge (§4.2) already names its producer
/// statement directly, since that resolution would otherwise be wrong (see `check_legality`'s own
/// comment).
enum Producer<'a> {
    Variable(&'a str),
    Named(String),
}

struct Edge<'a> {
    dep_fn: MultiAff,
    producer: Producer<'a>,
}

/// Every dependence edge a statement participates in as a consumer: one per real `Dependence`
/// node in its own contributing expression(s) (§7.2), plus — for a `__reduce` statement — the one
/// synthetic edge to its paired `__init` (§4.2's RAW dependence on the shared array cell).
fn dependence_edges<'a>(stmt: &'a Statement<'a>) -> Vec<Edge<'a>> {
    let mut raw = Vec::new();
    let mut edges: Vec<Edge<'a>> = match &stmt.kind {
        StatementKind::Ordinary { equations } => {
            for (_, eq) in equations {
                find_dependences(&eq.expr, &mut raw);
            }
            Vec::new()
        }
        StatementKind::ReduceInit { .. } => Vec::new(),
        StatementKind::ReduceStep {
            reduce_local,
            body,
            projection,
            ..
        } => {
            find_dependences(body, &mut raw);
            vec![Edge {
                dep_fn: (**projection).clone(),
                producer: Producer::Named(format!("{reduce_local}__init")),
            }]
        }
        StatementKind::OperationCall(call) => call
            .inputs
            .iter()
            .map(|access| Edge {
                dep_fn: access.function.clone(),
                producer: Producer::Variable(access.variable.as_str()),
            })
            .collect(),
    };
    edges.extend(raw.into_iter().map(|(dep_fn, producer_var)| Edge {
        dep_fn,
        producer: Producer::Variable(producer_var),
    }));
    edges
}

fn statements_by_name<'a>(statements: &'a [Statement<'a>]) -> HashMap<&'a str, &'a Statement<'a>> {
    statements.iter().map(|s| (s.name.as_str(), s)).collect()
}

/// Finds every `Dependence` node reachable in `e` without passing through a `Reduce` (only
/// reachable here as an unextracted arg-reduce, §4.3/§11), `Select`, `IndexPolynomial`, or
/// `Convolution` boundary — none of those have a codegen backend (§7.3, §11), so deriving precise
/// edges through them is moot; `ScheduledC`'s own codegen (§8) independently rejects them.
fn find_dependences<'a>(e: &'a ir::Expr, out: &mut Vec<(MultiAff, &'a str)>) {
    match &*e.kind {
        ir::ExprKind::Dependence { function, operand } => {
            if let ir::ExprKind::Variable(name) = &*operand.kind {
                out.push((function.clone(), name.as_str()));
            }
            // A constant operand contributes no edge; any other operand kind would already be an
            // internal-error per `gen_dependence`'s own invariant (Normalize guarantees Variable
            // or a constant) — not re-checked here.
        }
        ir::ExprKind::Bool(_)
        | ir::ExprKind::Int(_)
        | ir::ExprKind::Real(_)
        | ir::ExprKind::Variable(_)
        | ir::ExprKind::IndexFunction { .. }
        | ir::ExprKind::IndexPolynomial { .. }
        | ir::ExprKind::Select { .. }
        | ir::ExprKind::Reduce { .. }
        | ir::ExprKind::Convolution { .. } => {}
        ir::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            find_dependences(cond, out);
            find_dependences(then_branch, out);
            find_dependences(else_branch, out);
        }
        ir::ExprKind::Restrict { operand, .. }
        | ir::ExprKind::AutoRestrict { operand }
        | ir::ExprKind::Unary { operand, .. } => find_dependences(operand, out),
        ir::ExprKind::Case { branches, .. } => {
            for b in branches {
                find_dependences(b, out);
            }
        }
        ir::ExprKind::MultiArg { args, .. } => {
            for a in args {
                find_dependences(a, out);
            }
        }
        ir::ExprKind::Binary { lhs, rhs, .. } => {
            find_dependences(lhs, out);
            find_dependences(rhs, out);
        }
    }
}

/// Maps an underlying IR variable name to the statement that actually produces its final value —
/// the identity for an ordinary variable, but a reduce-hoisted variable's *only* real producer is
/// its `__reduce` half (`__init` only ever precedes it, never anything else — §4.2's own legality
/// note is exactly the edge from `__reduce` back to `__init`, not the other direction).
fn producer_of_map<'a>(statements: &'a [Statement]) -> HashMap<&'a str, &'a Statement<'a>> {
    let mut m = HashMap::new();
    for stmt in statements {
        match &stmt.kind {
            StatementKind::Ordinary { .. } => {
                m.insert(stmt.name.as_str(), stmt);
            }
            StatementKind::ReduceStep { reduce_local, .. } => {
                m.insert(*reduce_local, stmt);
            }
            StatementKind::ReduceInit { .. } => {}
            StatementKind::OperationCall(call) => {
                for output in &call.outputs {
                    m.insert(output.variable.as_str(), stmt);
                }
            }
        }
    }
    m
}

/// Reconciles `dep_fn`'s range (the underlying variable's own `a`-dimensional declared domain)
/// against `producer`'s *real* domain — the identity when `producer` is ordinary (the two already
/// coincide), but a genuine one-to-many *relation* (not a function) when `producer` is a
/// `__reduce` statement: `dep_fn`'s range is only the reduce's `a`-dim ambient slice, while
/// `__reduce`'s own domain is the full `(a+b)`-dim one (§4.2), related by the reduce's own
/// `projection: MultiAff` (`(a+b)-dim -> a-dim`, generally many-to-one — e.g. every `j` for a
/// given `i` projects to that same `i`). The correct dependence is "the consumer instance depends
/// on *every* `__reduce` instance whose projection lands on the point `dep_fn` names" — built by
/// composing `dep_fn` with `projection`'s own *reverse* (one-to-many) rather than trying to invert
/// a many-to-one function pointwise.
///
/// Every isl object feeding into this (`dep_fn`, `projection`) comes from `alpha_model`/
/// `alpha_transform`'s own analysis, with no relation to `crate::stmt`'s statement-naming scheme —
/// their domain/range tuples are anonymous. The result must be explicitly (re)tagged with
/// `consumer_name`/`producer.name` before it can compose against anything keyed by statement name
/// (`Map::apply_range`/`Map::intersect` both require exact tuple-id equality on the composing
/// side, `isl_space_tuple_is_equal` — confirmed the hard way).
fn dep_fn_to_producer_domain(
    dep_fn: MultiAff,
    consumer_name: &str,
    producer: &Statement,
) -> Result<Map> {
    let untagged = match &producer.kind {
        StatementKind::ReduceStep { projection, .. } => {
            let reversed_projection = (**projection).clone().into_map()?.reverse()?;
            dep_fn.into_map()?.apply_range(reversed_projection)?
        }
        _ => dep_fn.into_map()?,
    };
    let tagged = untagged
        .set_tuple_name(DimType::In, consumer_name)?
        .set_tuple_name(DimType::OutOrSet, &producer.name)?;
    Ok(tagged)
}

fn schedule_fragments_by_name(schedule: &UnionMap) -> Result<HashMap<String, Map>> {
    let mut by_name = HashMap::new();
    schedule
        .for_each_map(|m| {
            if let Some(name) = m.space().tuple_name(DimType::In) {
                by_name.insert(name, m);
            }
        })
        .map_err(CodegenError::Isl)?;
    Ok(by_name)
}

/// The universal `{ [x] -> [y] : x >=_lex y }` relation on the schedule space every fragment in
/// `schedule_maps` shares (they're already uniformly padded by `crate::schedule`, §6) — built
/// once and reused for every edge, rather than per-edge. Derived from an existing schedule
/// fragment's own *range* space (rather than building one from scratch) so it already carries the
/// right parameter space to compose against every other fragment via `apply_range`.
fn lex_ge_relation(schedule_maps: &HashMap<String, Map>) -> Result<Map> {
    let Some(sample) = schedule_maps.values().next() else {
        return Err(CodegenError::InvalidSchedule(
            "internal error: no statements to check legality for".to_string(),
        ));
    };
    let range_space = sample.clone().range()?.space();
    Ok(Map::lex_ge_on_space(range_space)?)
}

fn check_edge(
    lex_ge: &Map,
    consumer_name: &str,
    consumer_schedule: &Map,
    dep_map: Map,
    producer_name: &str,
    producer_schedule: &Map,
) -> Result<()> {
    // producer_side: consumer.domain -> schedule_space, i.e. every S_p(q) reachable via the
    // dependence relation from a given consumer point.
    let producer_side = dep_map.apply_range(producer_schedule.clone())?;
    // dominated: consumer.domain -> schedule_space, every y with some reachable S_p(q) >=_lex y.
    let dominated = producer_side.apply_range(lex_ge.clone())?;
    // violation: pairs (p, S_c(p)) present in both `consumer_schedule` and `dominated` — i.e.
    // points where some producer instance's schedule value >=_lex the consumer's, exactly §7.2's
    // illegality condition.
    let violation = consumer_schedule.clone().intersect(dominated)?;
    if !violation.is_empty()? {
        return Err(CodegenError::IllegalSchedule(format!(
            "'{consumer_name}' reads '{producer_name}' but the schedule doesn't guarantee the \
             producer instance runs strictly before the consumer instance that reads it (§7.2)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::build_schedule;
    use crate::stmt::statements;
    use crate::test_util::{normalized_system, PLAIN_COPY, PREFIX_SUM, PREFIX_SUM_WITH_CONSUMER};

    /// The empty/omitted target mapping (§6) gives every statement its own independent identity
    /// schedule, padded to the shared width by appending zeros — but for `Y__init`/`Y__reduce`
    /// that collides: `Y__init`'s padded position for a given `i` (`(i, 0)`) lands on the exact
    /// same schedule-space point as `Y__reduce`'s first instance for that same `i` (`(i, 0)` at
    /// `j = 0`), leaving their relative order undefined right where `§4.2`'s own synthetic RAW
    /// dependence requires `__init` to strictly precede every `__reduce` instance. This is a real,
    /// expected illegality — not a checker bug — and documents exactly when the identity default
    /// is usable at all (only when there's no cross-statement dependency needing explicit
    /// reordering, e.g. `ordinary_copy_is_always_legal` below).
    #[test]
    fn identity_default_schedule_is_illegal_for_a_real_reduce_dependency() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let schedule = build_schedule(&ctx, &stmts, "").unwrap();
        let err = match check_legality(&stmts, &schedule) {
            Ok(()) => panic!("expected the identity default to be illegal for PrefixSum"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("Y") && err.contains("§7.2"), "{err}");
        insta::assert_snapshot!(err);
    }

    #[test]
    fn properly_ordered_explicit_schedule_is_legal() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let text = "{ Y__init[i] -> [i, 0, 0]; \
                     Y__reduce[i,j] -> [i, 1, j]; }";
        let schedule = build_schedule(&ctx, &stmts, text).unwrap();
        check_legality(&stmts, &schedule).unwrap();
    }

    #[test]
    fn consumer_scheduled_before_its_producer_is_illegal() {
        let system = normalized_system(PREFIX_SUM_WITH_CONSUMER);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        // Z (phase 0) now runs *before* Y__reduce (phase 2) — the read is no longer guaranteed
        // to see the fully-accumulated value.
        let text = "{ Z[i] -> [i, 0, 0]; \
                     Y__init[i] -> [i, 1, 0]; \
                     Y__reduce[i,j] -> [i, 2, j]; }";
        let schedule = build_schedule(&ctx, &stmts, text).unwrap();
        let err = match check_legality(&stmts, &schedule) {
            Ok(()) => panic!("expected reading before the producer completes to be illegal"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("Z") && err.contains("§7.2"), "{err}");
        insta::assert_snapshot!(err);
    }

    #[test]
    fn reduce_scheduled_before_its_own_init_is_illegal() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        // Y__reduce (phase 0) now runs *before* Y__init (phase 1) — violates §4.2's own
        // synthetic RAW dependence on the shared array cell.
        let text = "{ Y__reduce[i,j] -> [i, 0, j]; \
                     Y__init[i] -> [i, 1, 0]; }";
        let schedule = build_schedule(&ctx, &stmts, text).unwrap();
        let err = match check_legality(&stmts, &schedule) {
            Ok(()) => panic!("expected __reduce before its own __init to be illegal"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("Y__reduce") && err.contains("Y__init"),
            "{err}"
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn ordinary_copy_with_no_reduce_is_always_legal_even_at_identity() {
        // No reduce, no reordering hazard — a plain `Y[i] = X[i]` copy is legal even at the
        // trivial identity default, unlike PrefixSum's reduce case above.
        let system = normalized_system(PLAIN_COPY);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let schedule = build_schedule(&ctx, &stmts, "").unwrap();
        check_legality(&stmts, &schedule).unwrap();
    }
}
