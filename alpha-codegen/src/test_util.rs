//! Shared test scaffolding for `crate`'s own unit/snapshot tests — parses, analyzes, lowers, and
//! fully normalizes small inline Alpha source strings, plus a minimal `ExprGen` test double for
//! Tier-1 expression-fragment snapshot tests (`docs/codegen-test-design.md` §5.1/§5.4) that don't
//! need a whole schedule/AST build to exercise `crate::expr::gen_value`.

use crate::expr::{ExprGen, Role, VarInfo};
use crate::simplec::Expr as CExpr;
use crate::{CodegenError, Result};
use alpha_transform::ir;
use isl::{Context, DimType};
use std::collections::{BTreeSet, HashMap};

pub(crate) fn normalized_system(src: &str) -> ir::System {
    let ctx = isl::Context::new();
    let parse = alpha_syntax::parse(src);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system in fixture");
    let mut resolver = alpha_model::Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (mut ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
    alpha_transform::normalize_reduction::apply(&mut ir_system);
    alpha_transform::normalize::apply(ir_system, true)
}

/// Finds the first `(domain, equation)` pair for `var` in `system` — a small scan mirroring
/// `crate::stmt`'s own `equations_by_var` grouping, but returning just one match: enough for the
/// single-equation (non-piecewise) Tier-1/Tier-2 fixtures below, which by construction have
/// exactly one equation per variable except `PIECEWISE_EQUATION` (§5.6), which looks its second
/// equation up by index instead — see `nth_equation`.
pub(crate) fn first_equation<'a>(
    system: &'a ir::System,
    var: &str,
) -> (&'a isl::Set, &'a ir::StandardEquation) {
    nth_equation(system, var, 0)
}

pub(crate) fn nth_equation<'a>(
    system: &'a ir::System,
    var: &str,
    n: usize,
) -> (&'a isl::Set, &'a ir::StandardEquation) {
    let mut matches = system.bodies.iter().flat_map(|body| {
        body.equations.iter().filter_map(move |eq| match eq {
            ir::Equation::Standard(s) if s.variable == var => Some((&body.domain, s)),
            _ => None,
        })
    });
    matches
        .nth(n)
        .unwrap_or_else(|| panic!("no equation #{n} for variable '{var}' in system"))
}

/// A minimal `ExprGen` test double (`docs/codegen-test-design.md` §3) — just enough to resolve
/// variable roles/dimensionality and render a read the same way `ScheduledC`'s own `Gen` does
/// (every role via its access macro, §8.3), without needing a whole schedule/AST build. Built from
/// a normalized `ir::System`'s own `inputs`/`outputs`/`locals`, exactly like
/// `scheduledc::generate_scheduled_system`'s own setup.
pub(crate) struct TestGen {
    pub ctx: Context,
    pub variables: HashMap<String, VarInfo>,
    pub param_names: Vec<String>,
    pub externs: BTreeSet<String>,
}

impl TestGen {
    pub fn for_system(system: &ir::System) -> TestGen {
        let first_body = system
            .bodies
            .first()
            .expect("fixture system has at least one body");
        let ctx = first_body.domain.ctx();
        let param_names = crate::expr::param_names_of(&first_body.domain);

        let mut variables = HashMap::new();
        for v in &system.inputs {
            variables.insert(
                v.name.clone(),
                VarInfo {
                    role: Role::Input,
                    ndims: v.domain.dim(DimType::OutOrSet),
                },
            );
        }
        for v in &system.outputs {
            variables.insert(
                v.name.clone(),
                VarInfo {
                    role: Role::Output,
                    ndims: v.domain.dim(DimType::OutOrSet),
                },
            );
        }
        for v in &system.locals {
            variables.insert(
                v.name.clone(),
                VarInfo {
                    role: Role::Local,
                    ndims: v.domain.dim(DimType::OutOrSet),
                },
            );
        }

        TestGen {
            ctx,
            variables,
            param_names,
            externs: BTreeSet::new(),
        }
    }
}

impl ExprGen for TestGen {
    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn param_names(&self) -> &[String] {
        &self.param_names
    }

    fn variable(&self, name: &str) -> Option<&VarInfo> {
        self.variables.get(name)
    }

    fn add_extern(&mut self, name: &str) {
        self.externs.insert(name.to_string());
    }

    fn render_read(&self, name: &str, _role: Role, idx: &[String]) -> String {
        format!("{name}({})", idx.join(","))
    }

    fn gen_reduce_value(
        &mut self,
        _names: &[String],
        _operator: &ir::Operator,
        _body_context_hint: &[String],
        _body: &ir::Expr,
    ) -> Result<CExpr> {
        // Same invariant as `scheduledc::Gen`'s own implementation: every top-level `Reduce` is
        // split into its own `<name>__init`/`<name>__reduce` statement pair before codegen runs
        // (§4.2), so `gen_value` only reaches here for a `Reduce` nested directly inside another
        // `Reduce`'s body — the one case `normalize_reduction` doesn't hoist (§7.3, §11).
        Err(CodegenError::Unsupported(
            "a Reduce nested directly inside another Reduce's body is not implemented — the one \
             case normalize_reduction doesn't hoist (§7.3, §11)"
                .to_string(),
        ))
    }
}

pub(crate) const PREFIX_SUM: &str = "affine PrefixSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.";

pub(crate) const PLAIN_COPY: &str = "affine Copy [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = X[i];
.";

/// Like `PREFIX_SUM`, but with a real downstream consumer (`Z`) reading the reduce's result —
/// `Y`'s own equation is still directly `reduce(...)`, so it's left as the `Y__init`/`Y__reduce`
/// pair untouched by `normalize_reduction` (see that module's doc), and `Z` becomes a genuine
/// third, ordinary statement whose own legality depends on running after `Y__reduce` completes.
pub(crate) const PREFIX_SUM_WITH_CONSUMER: &str = "affine PrefixSumConsumer [N]->{:N>0}
    inputs  X: [N]
    outputs Z: [N]
    locals  Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        Z[i] = Y[i] + 1[];
.";

// ---------------------------------------------------------------------------------------------
// §5.1: single 1D output, RHS node-kind ladder (docs/codegen-test-design.md).
// ---------------------------------------------------------------------------------------------

pub(crate) const INT_LITERAL: &str = "affine IntLiteral [N]->{:N>0}
    outputs Y: [N]
    let Y[i] = 3[];
.";

pub(crate) const REAL_LITERAL: &str = "affine RealLiteral [N]->{:N>0}
    outputs Y: [N]
    let Y[i] = 3.0[];
.";

pub(crate) const BINARY_ADD: &str = "affine BinaryAdd [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = X[i] + X[i];
.";

pub(crate) const UNARY_NEG: &str = "affine UnaryNeg [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = -(X[i]);
.";

pub(crate) const INDEX_FUNCTION_EXPR: &str = "affine IndexFunctionExpr [N]->{:N>0}
    outputs Y: [N]
    let Y[i] = val (i->N-i);
.";

pub(crate) const IF_THEN_ELSE: &str = "affine IfThenElse [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = if (X[i] > 0[]) then X[i] else -(X[i]);
.";

pub(crate) const CASE_TWO_BRANCH: &str = "affine CaseTwoBranch [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = case {
        {:i<N/2}: X[i];
        auto: -(X[i]);
    };
.";

pub(crate) const CASE_THREE_BRANCH: &str = "affine CaseThreeBranch [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = case {
        {:i=0}: X[i] + X[i];
        {:i>0 && i<N/2}: X[i];
        auto: -(X[i]);
    };
.";

pub(crate) const MULTI_ARG_SUM: &str = "affine MultiArgSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = sum(X[i], 1[], 2[]);
.";

pub(crate) const MULTI_ARG_MIN: &str = "affine MultiArgMin [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = min(X[i], X[i]);
.";

pub(crate) const EXTERNAL_CALL: &str = "external sqrt(1)

affine ExternalCall [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = sqrt(X[i]);
.";

// ---------------------------------------------------------------------------------------------
// §5.4: reduction-body node-kind ladder.
// ---------------------------------------------------------------------------------------------

pub(crate) const REDUCE_MUL: &str = "affine ReduceMul [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(*, [j], {:j<=i}: X[j]);
.";

pub(crate) const REDUCE_MIN: &str = "affine ReduceMin [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(min, [j], {:j<=i}: X[j]);
.";

pub(crate) const REDUCE_MAX: &str = "affine ReduceMax [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(max, [j], {:j<=i}: X[j]);
.";

pub(crate) const REDUCE_EXTERNAL: &str = "external delta(2)

affine ReduceExternal [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(delta, [j], {:j<=i}: X[j]);
.";

/// The flagship example from `docs/codegen-test-design.md` §5.4 row 6: a `case` expression living
/// inside a reduce's own body — exercises `gen_case` reached via `ReduceStep` codegen rather than
/// an `Ordinary` statement.
///
/// **Deliberately uses two fully-explicit branches, no `auto`.** An `auto`-default version of this
/// same fixture (`{:j=i}: X[j]+X[j]; auto: X[j];`) was tried first and found to mis-generate: the
/// `Y__reduce` statement's own computed domain came out as the *whole* `0<=j<N` range instead of
/// the reduce's own `{:j<=i}` restrict, silently summing over the entire array on every iteration
/// (confirmed by a numeric compile-and-run check — verified correct with explicit branches, wrong
/// with `auto`, same restrict/case shape otherwise). This looks like a real, pre-existing bug in
/// how the outer restrict composes with a `case`'s `auto`-branch domain-fill when the `case` is a
/// reduce's own body — not something to route around silently here; it's a discovered issue worth
/// its own investigation outside this test-design work, and this fixture avoids it only so this
/// particular row can test what it's actually meant to (case-shaped ternary logic inside a
/// reduce's loops), not accidentally pin a wrong answer as a passing snapshot.
#[allow(dead_code)] // only consumed by tests/scheduledc_snapshots.rs's own duplicated copy (pub(crate) blocks cross-crate reuse) — kept here as the canonical, documented reference copy
pub(crate) const REDUCE_CASE_BODY: &str = "affine ReduceCaseBody [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: case {
        {:j=i}: X[j] + X[j];
        {:j<i}: X[j];
    });
.";

pub(crate) const REDUCE_BINARY_BODY: &str = "affine ReduceBinaryBody [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j] + X[j]);
.";

// ---------------------------------------------------------------------------------------------
// §5.5: two 1D outputs, dataflow x schedule combos.
// ---------------------------------------------------------------------------------------------

#[allow(dead_code)] // only consumed by tests/scheduledc_snapshots.rs's own duplicated copy (pub(crate) blocks cross-crate reuse) — kept here as the canonical, documented reference copy
pub(crate) const TWO_INDEPENDENT: &str = "affine TwoIndependent [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        W: [N]
    let
        Y[i] = X[i];
        W[i] = -(X[i]);
.";

#[allow(dead_code)] // only consumed by tests/scheduledc_snapshots.rs's own duplicated copy (pub(crate) blocks cross-crate reuse) — kept here as the canonical, documented reference copy
pub(crate) const PRODUCER_CONSUMER_NO_REDUCE: &str = "affine ProducerConsumer [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        W: [N]
    let
        Y[i] = X[i];
        W[i] = Y[i] + 1[];
.";

pub(crate) const TWO_REDUCES_FAN_OUT: &str = "affine TwoReducesFanOut [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        MaxY: [N]
    let
        Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        MaxY[i] = reduce(max, [j], {:j<=i}: X[j]);
.";

/// Prefix-sum-of-prefix-sum: `Z`'s own reduce body reads `Y`, itself a reduce's result — the
/// richest legality case in the matrix (§5.5 row 5). `Y`'s *entire* accumulation for a given `i`
/// must complete before any `Z__reduce` instance reads `Y[j]` for `j<=i`.
#[allow(dead_code)] // only consumed by tests/scheduledc_snapshots.rs's own duplicated copy (pub(crate) blocks cross-crate reuse) — kept here as the canonical, documented reference copy
pub(crate) const NESTED_REDUCE_DEPENDENCY: &str = "affine NestedReduceDependency [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        Z: [N]
    let
        Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        Z[i] = reduce(+, [j], {:j<=i}: Y[j]);
.";

// ---------------------------------------------------------------------------------------------
// §5.6: piecewise (multi-`SystemBody`) equations for one output (design decision 2, §2).
// ---------------------------------------------------------------------------------------------

pub(crate) const PIECEWISE_EQUATION: &str = "affine PiecewiseEquation [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
    when {:N>10} let
        Y[i] = X[i];
    let
        Y[i] = X[i] + X[i];
.";
