//! `ScheduledC`: the scheduled code generator (§8 of `docs/scheduled-codegen-design.md`). Takes
//! an explicit, user-supplied target mapping (`crate::schedule`, §6), checks it against the
//! program's real dependences (`crate::legality`, §7), and emits a single, flat, imperative loop
//! nest that visits every statement instance (`crate::stmt`, §4) in exactly the order the schedule
//! specifies — no recursion, no memoization flags, one direct array write per instance.
//!
//! §8.1: one whole-program `AstBuild::generate` call over the union of every statement's
//! `(domain, schedule)` pair, not one per output/reduce like `WriteC`. §8.2: the resulting
//! `AstNode` tree is walked directly — `User` nodes are the only interesting case, dispatching by
//! tuple name to the matching statement's own body codegen. §8.3: expression codegen is
//! `crate::expr`'s shared `gen_value` and friends, reused essentially unchanged; only
//! `ExprGen::render_read` (every role reads via its access macro, no `Role`/`eval_`
//! distinction) and reduce-body codegen (no `gen_reduce`, no flag arrays, no `eval_<Var>`
//! functions) differ from `WriteC`.

use crate::error::{CodegenError, Result};
use crate::expr::{self, ExprGen, Role, VarInfo};
use crate::layout;
use crate::scheduled_ir::{self, CompareOp, IndexExpr, Predicate, ScheduledNode};
use crate::simplec::{CType, Expr as CExpr, Function, Stmt};
use crate::stmt::{Statement, StatementKind};
use alpha_transform::ir;
use isl::{Context, DimType, Format, Set};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

struct Gen {
    ctx: Context,
    variables: HashMap<String, VarInfo>,
    param_names: Vec<String>,
    externs: BTreeSet<String>,
}

impl ExprGen for Gen {
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

    /// §8.3: every role reads via its own access macro — the `Role`/`eval_`-call distinction
    /// `WriteC` has for reads disappears entirely here.
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
        // Unreachable for a well-formed program: every top-level Reduce is split into a
        // <name>__init/<name>__reduce statement pair before codegen runs (§4.2), and a
        // ReduceStep's own `body` is the reduce's *inner* expression, not a Reduce node itself.
        // The only way `gen_value` reaches here is a `Reduce` nested directly inside another
        // `Reduce`'s body — the one case `normalize_reduction` doesn't hoist, and stays opaque to
        // both legality checking and codegen (§7.3, §11), same carried-over limitation as
        // `WriteC`'s own `is_arg_reduce` gap.
        Err(CodegenError::Unsupported(
            "a Reduce nested directly inside another Reduce's body is not implemented — the one \
             case normalize_reduction doesn't hoist (§7.3, §11)"
                .to_string(),
        ))
    }
}

/// Generates a whole system's C source using an explicit, user-supplied target mapping —
/// `schedule_text` in the §6 format, empty for "every statement defaults to identity" (§6).
///
/// **Precondition** (documented here, not enforced by the signature — see §5.1/§13 of the design):
/// `system` must already be the output of both normalizing passes, in the required order —
/// `normalize_reduction::apply` then `normalize::apply` (§1, §3).
/// Validates `schedule_text` against `system` — parses it (§6), checks totality/injectivity/
/// shared-width agreement (§6), and checks it doesn't reorder a real dependence (§7) — without
/// running any codegen. What `alphalang`'s `NormalizedSystem.schedule(text)` (§10.1) actually needs:
/// producing a `ScheduledSystem` is cheap validation, not full generation, and `generate()` is a
/// separate, later call (possibly against a `ScheduledSystem` that's never `generate()`'d at all).
pub fn validate_scheduled_system(system: &ir::System, schedule_text: &str) -> Result<()> {
    check_no_use_equations(system)?;
    scheduled_ir::build(system, schedule_text)?;
    Ok(())
}

fn check_no_use_equations(system: &ir::System) -> Result<()> {
    for body in &system.bodies {
        for eq in &body.equations {
            if matches!(eq, ir::Equation::Use(_)) {
                return Err(CodegenError::Unsupported(
                    "UseEquation has no codegen backend — matches WriteC, which doesn't support \
                     subsystem calls either"
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn first_body_ctx(system: &ir::System) -> Result<Context> {
    let Some(first_body) = system.bodies.first() else {
        return Err(CodegenError::Unsupported(
            "system has no bodies to generate code for".to_string(),
        ));
    };
    Ok(first_body.domain.ctx())
}

pub fn generate_scheduled_system(system: &ir::System, schedule_text: &str) -> Result<String> {
    check_no_use_equations(system)?;
    let program = scheduled_ir::build(system, schedule_text)?;
    let ctx = first_body_ctx(system)?;
    let first_body = system
        .bodies
        .first()
        .expect("checked non-empty by first_body_ctx");
    let param_names = expr::param_names_of(&first_body.domain);

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

    let mut g = Gen {
        ctx: ctx.clone(),
        variables,
        param_names,
        externs: BTreeSet::new(),
    };

    let mut loop_iterators = std::collections::BTreeSet::new();
    let walked = walk_node(
        &mut g,
        &program.root,
        &program.statements,
        &mut loop_iterators,
    )?;

    // simplec::Stmt::For assumes its own iterator is already declared (matching how WriteC's own
    // driver pre-declares `i0..maxdims`) — isl chose these names itself (no custom naming set up
    // front, §8.2's own doc), so they're collected from the walk rather than known in advance.
    let mut body_stmts: Vec<Stmt> = loop_iterators
        .into_iter()
        .map(|name| Stmt::Decl {
            ty: CType::Long,
            name,
            init: None,
        })
        .collect();
    body_stmts.extend(walked);

    render_program(system, &g, body_stmts)
}

fn render_index(expression: &IndexExpr) -> String {
    render_index_at(expression, 0)
}

fn render_index_at(expression: &IndexExpr, parent_precedence: u8) -> String {
    match expression {
        IndexExpr::Constant(value) => value.to_string(),
        IndexExpr::Variable(name) => name.clone(),
        IndexExpr::Add(lhs, rhs) => render_binary(lhs, "+", rhs, 1, parent_precedence, false),
        IndexExpr::Sub(lhs, rhs) => render_binary(lhs, "-", rhs, 1, parent_precedence, true),
        IndexExpr::Mul(lhs, rhs) => render_binary(lhs, "*", rhs, 2, parent_precedence, false),
        IndexExpr::Div(lhs, rhs) => render_binary(lhs, "/", rhs, 2, parent_precedence, true),
        IndexExpr::FloorDiv(lhs, rhs) => {
            format!("floord({}, {})", render_index(lhs), render_index(rhs))
        }
        IndexExpr::CeilDiv(lhs, rhs) => {
            format!("ceild({}, {})", render_index(lhs), render_index(rhs))
        }
        IndexExpr::Mod(lhs, rhs) => format!("{} % {}", render_index(lhs), render_index(rhs)),
        IndexExpr::Min(arguments) => render_variadic("min", arguments),
        IndexExpr::Max(arguments) => render_variadic("max", arguments),
    }
}

fn render_binary(
    lhs: &IndexExpr,
    operator: &str,
    rhs: &IndexExpr,
    precedence: u8,
    parent_precedence: u8,
    protect_rhs: bool,
) -> String {
    let expression = format!(
        "{} {operator} {}",
        render_index_at(lhs, precedence),
        render_index_at(rhs, precedence + u8::from(protect_rhs))
    );
    if precedence < parent_precedence {
        format!("({expression})")
    } else {
        expression
    }
}

fn render_variadic(name: &str, arguments: &[IndexExpr]) -> String {
    let mut arguments = arguments.iter();
    let Some(first) = arguments.next() else {
        return "0".to_string();
    };
    arguments.fold(render_index(first), |left, right| {
        format!("{name}({left}, {})", render_index(right))
    })
}

fn render_predicate(predicate: &Predicate) -> String {
    match predicate {
        Predicate::Compare { op, lhs, rhs } => format!(
            "{} {} {}",
            render_index(lhs),
            match op {
                CompareOp::Eq => "==",
                CompareOp::Le => "<=",
                CompareOp::Lt => "<",
                CompareOp::Ge => ">=",
                CompareOp::Gt => ">",
            },
            render_index(rhs)
        ),
        Predicate::And(arguments) => arguments
            .iter()
            .map(|argument| format!("({})", render_predicate(argument)))
            .collect::<Vec<_>>()
            .join(" && "),
        Predicate::Or(arguments) => arguments
            .iter()
            .map(|argument| format!("({})", render_predicate(argument)))
            .collect::<Vec<_>>()
            .join(" || "),
        Predicate::Not(argument) => format!("!({})", render_predicate(argument)),
        Predicate::Constant(value) => (*value as u8).to_string(),
    }
}

fn walk_node(
    g: &mut Gen,
    node: &ScheduledNode,
    statements: &[Statement],
    loop_iterators: &mut std::collections::BTreeSet<String>,
) -> Result<Vec<Stmt>> {
    match node {
        ScheduledNode::Loop {
            iterator,
            init,
            condition,
            step,
            body,
        } => {
            let iterator = iterator.0.clone();
            loop_iterators.insert(iterator.clone());
            let body = walk_node(g, body, statements, loop_iterators)?;
            Ok(vec![Stmt::For {
                iterator,
                init: render_index(init),
                cond: render_predicate(condition),
                inc: render_index(step),
                body,
            }])
        }
        ScheduledNode::If {
            condition,
            then_body,
            else_body,
        } => {
            let then_branch = walk_node(g, then_body, statements, loop_iterators)?;
            let else_branch = match else_body {
                Some(else_body) => walk_node(g, else_body, statements, loop_iterators)?,
                None => Vec::new(),
            };
            Ok(vec![Stmt::If {
                cond: CExpr::Raw(render_predicate(condition)),
                then_branch,
                else_branch,
            }])
        }
        ScheduledNode::Sequence(nodes) => {
            let mut out = Vec::new();
            for child in nodes {
                out.extend(walk_node(g, child, statements, loop_iterators)?);
            }
            Ok(out)
        }
        ScheduledNode::Invoke { statement, indices } => {
            let statement = statements.get(statement.0).ok_or_else(|| {
                CodegenError::Unsupported(format!(
                    "internal error: unknown scheduled statement ID {}",
                    statement.0
                ))
            })?;
            let indices = indices.iter().map(render_index).collect::<Vec<_>>();
            Ok(vec![Stmt::Block(gen_statement_body(
                g, statement, &indices,
            )?)])
        }
    }
}

/// Binds each of `names` to the corresponding already-computed C index expression from the
/// enclosing `User` node — `idx_exprs[i]` is *not* necessarily a bare identifier; under a
/// non-identity schedule it can be an arbitrary affine combination of the loop iterators isl
/// chose (e.g. after a skew or permutation), so every name gets its own `long` local rather than
/// being used inline.
fn binding_prologue(names: &[String], idx_exprs: &[String]) -> Vec<Stmt> {
    names
        .iter()
        .zip(idx_exprs)
        .map(|(n, e)| Stmt::Decl {
            ty: CType::Long,
            name: n.clone(),
            init: Some(CExpr::Raw(e.clone())),
        })
        .collect()
}

fn gen_statement_body(g: &mut Gen, stmt: &Statement, idx_exprs: &[String]) -> Result<Vec<Stmt>> {
    match &stmt.kind {
        StatementKind::Ordinary { equations } => {
            let ndims = stmt.domain.dim(DimType::OutOrSet);
            let hint = &equations[0].1.index_names;
            let names = expr::pick_names(hint, ndims);
            let mut out = binding_prologue(&names, idx_exprs);

            let value_expr = if let [(_, eq)] = equations.as_slice() {
                expr::gen_value(g, &names, &eq.expr)?
            } else {
                let Some((last, rest)) = equations.split_last() else {
                    unreachable!("equations is non-empty (checked by crate::stmt)")
                };
                // §4.1's guard-selection ternary chain, one per piecewise (multi-`SystemBody`)
                // equation: `body_domain` here is the *SystemBody's own* guard domain (`when
                // {D}`), always parameter-only in this port (no way to write a per-index `when`
                // guard — confirmed empirically while building this fixture, see
                // `docs/codegen-test-design.md` §5.6), so the ambient context for evaluating it
                // must be 0-dim too (`&[]`), matching `writec.rs`'s own `gen_eval_function` — not
                // `&names`, which mismatches `body_domain`'s dimensionality and silently renders a
                // degenerate always-false condition instead of erroring (a real bug this test
                // design surfaced; see that fixture's own doc comment for the numeric symptom).
                let build = expr::ambient_build(&g.ctx, &g.param_names, &[])?;
                let mut result = expr::gen_value(g, &names, &last.1.expr)?;
                for (body_domain, eq) in rest.iter().rev() {
                    let cond_expr = build.expr_from_set((*body_domain).clone())?;
                    let cond = CExpr::Raw(cond_expr.to_string_fmt(Format::C));
                    let val = expr::gen_value(g, &names, &eq.expr)?;
                    result = CExpr::Ternary(Box::new(cond), Box::new(val), Box::new(result));
                }
                result
            };
            let target = CExpr::Raw(format!("{}({})", stmt.name, names.join(",")));
            out.push(Stmt::Assign {
                target,
                value: value_expr,
            });
            Ok(out)
        }
        StatementKind::ReduceInit {
            reduce_local,
            operator,
            body_context,
        } => {
            let ndims = stmt.domain.dim(DimType::OutOrSet);
            let names = expr::pick_names(body_context, ndims);
            let mut out = binding_prologue(&names, idx_exprs);
            let init_val = neutral_element(operator)?;
            let target = CExpr::Raw(format!("{reduce_local}({})", names.join(",")));
            out.push(Stmt::Assign {
                target,
                value: init_val,
            });
            Ok(out)
        }
        StatementKind::ReduceStep {
            reduce_local,
            operator,
            body,
            body_context,
            ..
        } => {
            let ndims = stmt.domain.dim(DimType::OutOrSet);
            let names = expr::pick_names(body_context, ndims);
            let mut out = binding_prologue(&names, idx_exprs);
            let value = expr::gen_value(g, &names, body)?;
            // `reduce_local`'s own *storage* is only `a`-dimensional (the reduce's ambient
            // domain, matching its paired `__init`'s own domain, §4.2) — every one of this
            // statement's `(a+b)`-dim instances for the same ambient `i...` accumulates into that
            // *same* cell, so the read-modify-write must index by the ambient prefix of `names`
            // only, not the full `(a+b)`-dim tuple `gen_value` needed to evaluate `body` itself.
            let ambient = g
                .variable(reduce_local)
                .map(|info| info.ndims)
                .unwrap_or(ndims);
            let cell = format!("{reduce_local}({})", names[..ambient as usize].join(","));
            let combined = combine(g, operator, &cell, value)?;
            out.push(Stmt::Assign {
                target: CExpr::Raw(cell),
                value: combined,
            });
            Ok(out)
        }
        StatementKind::OperationCall(call) => Err(CodegenError::Unsupported(format!(
            "registered operation '{}' has no Scheduled C backend",
            call.operation.name()
        ))),
    }
}

/// The neutral element for a reduce's own operator — same table `writec::gen_reduce` uses.
fn neutral_element(operator: &ir::Operator) -> Result<CExpr> {
    match operator {
        ir::Operator::External(_) => Ok(CExpr::Raw("0.0f".to_string())),
        ir::Operator::Named(op) => {
            let init = match op.as_str() {
                "+" | "sum" | "or" => "0.0f",
                "*" | "prod" | "and" => "1.0f",
                "min" => "INFINITY",
                "max" => "-INFINITY",
                other => {
                    return Err(CodegenError::Unsupported(format!(
                        "unknown reduce operator '{other}'"
                    )))
                }
            };
            Ok(CExpr::Raw(init.to_string()))
        }
    }
}

/// `cell`'s new value after folding in `value` via `operator` — same combine table
/// `writec::gen_reduce` uses, except the accumulator is the array cell itself (read-modify-write),
/// not a loop-scoped local: there's no enclosing per-reduce loop to hold one in the flat,
/// single-pass model (§8.3).
fn combine(g: &mut Gen, operator: &ir::Operator, cell: &str, value: CExpr) -> Result<CExpr> {
    match operator {
        ir::Operator::External(name) => {
            g.add_extern(name);
            Ok(CExpr::Raw(format!("{name}({cell}, {value})")))
        }
        ir::Operator::Named(op) => match op.as_str() {
            "+" | "sum" => Ok(CExpr::Raw(format!("({cell}) + ({value})"))),
            "*" | "prod" => Ok(CExpr::Raw(format!("({cell}) * ({value})"))),
            "min" | "max" => Ok(CExpr::Call(
                op.clone(),
                vec![CExpr::Raw(cell.to_string()), value],
            )),
            "and" => Ok(CExpr::Raw(format!("(({cell}) != 0) && (({value}) != 0)"))),
            "or" => Ok(CExpr::Raw(format!("(({cell}) != 0) || (({value}) != 0)"))),
            other => Err(CodegenError::Unsupported(format!(
                "unknown reduce operator '{other}'"
            ))),
        },
    }
}

fn flat_alloc_stmts(name: &str, ndims: u32, domain: &Set, elem_ctype: &str) -> Result<Vec<Stmt>> {
    let bounds = layout::FlatBounds::compute(domain).map_err(|e| match e {
        CodegenError::Isl(isl_e) => {
            crate::expr::isl_bound_err(&format!("sizing storage for '{name}'"), isl_e)
        }
        other => other,
    })?;
    let mut stmts: Vec<Stmt> = layout::flat_size_init_stmts(name, &bounds)
        .into_iter()
        .map(Stmt::Raw)
        .collect();
    let total = layout::flat_total_size_expr(name, ndims);
    stmts.push(Stmt::Assign {
        target: CExpr::Raw(name.to_string()),
        value: CExpr::Raw(format!(
            "({elem_ctype}*)(malloc((sizeof({elem_ctype})) * ({total})))"
        )),
    });
    stmts.push(Stmt::Raw(format!("mallocCheck({name},\"{name}\");")));
    Ok(stmts)
}

fn render_program(system: &ir::System, g: &Gen, body_stmts: Vec<Stmt>) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "// This code was auto-generated by alphac (alpha-rs).");
    let _ = writeln!(
        out,
        "// ScheduledC backend — a user-supplied target mapping controls loop order."
    );
    let _ = writeln!(out);
    for inc in [
        "float.h",
        "limits.h",
        "math.h",
        "stdbool.h",
        "stdio.h",
        "stdlib.h",
        "string.h",
    ] {
        let _ = writeln!(out, "#include <{inc}>");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "// Function Macros");
    let _ = writeln!(
        out,
        "#define ceild(n,d) ((int)ceil(((double)(n))/((double)(d))))"
    );
    let _ = writeln!(
        out,
        "#define floord(n,d) ((int)floor(((double)(n))/((double)(d))))"
    );
    let _ = writeln!(out, "#define div(a,b) (ceild((a),(b)))");
    let _ = writeln!(out, "#define max(a,b) (((a)>(b))?(a):(b))");
    let _ = writeln!(out, "#define min(a,b) (((a)<(b))?(a):(b))");
    let _ = writeln!(
        out,
        "#define mallocCheck(v,s) if ((v) == NULL) {{ printf(\"Failed to allocate memory for \
         variable: %s\\n\", (s)); exit(-1); }}"
    );
    let _ = writeln!(out);

    if !g.externs.is_empty() {
        let _ = writeln!(out, "// External Function Declarations");
        for name in &g.externs {
            let _ = writeln!(out, "extern float {name}();");
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "// Global Variables");
    for p in &g.param_names {
        let _ = writeln!(out, "static long {p};");
    }
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let ty = layout::interface_ctype(CType::Float, v.domain.dim(DimType::OutOrSet));
        let _ = writeln!(out, "static {ty} {};", v.name);
    }
    for v in &system.locals {
        let ndims = v.domain.dim(DimType::OutOrSet);
        let _ = writeln!(out, "static float* {};", v.name);
        for line in layout::flat_size_decls(&v.name, ndims) {
            let _ = writeln!(out, "{line}");
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "// Memory Macros");
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let _ = writeln!(
            out,
            "{}",
            layout::interface_access_macro(&v.name, v.domain.dim(DimType::OutOrSet))
        );
    }
    for v in &system.locals {
        let _ = writeln!(
            out,
            "{}",
            layout::flat_access_macro(&v.name, v.domain.dim(DimType::OutOrSet))
        );
    }
    let _ = writeln!(out);

    let driver = build_driver(system, g, body_stmts)?;
    let _ = writeln!(out, "// Function Declarations");
    let _ = writeln!(out, "{};", driver.signature());
    let _ = writeln!(out);
    let _ = writeln!(out, "{driver}");

    let _ = writeln!(out);
    let _ = writeln!(out, "// Undefine the Memory and Function Macros");
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let _ = writeln!(out, "#undef {}", v.name);
    }
    for v in &system.locals {
        let _ = writeln!(out, "#undef {}", v.name);
    }
    let _ = writeln!(out, "#undef ceild");
    let _ = writeln!(out, "#undef floord");
    let _ = writeln!(out, "#undef div");
    let _ = writeln!(out, "#undef max");
    let _ = writeln!(out, "#undef min");
    let _ = writeln!(out, "#undef mallocCheck");

    Ok(out)
}

/// Builds the system's single public entry point — parameter/argument copying, local storage
/// allocation, the *entire* AST-walked body (§8.1: one whole-program build, so there's no
/// separate per-output loop section here at all, unlike `WriteC`'s driver), then freeing. No flag
/// allocation/initialization (§8.3: the schedule's own legality guarantees single-pass causal
/// order, so `WriteC`'s `NOT_EVALUATED`/`IN_PROGRESS`/`EVALUATED` mechanism has no reason to exist
/// here).
fn build_driver(system: &ir::System, g: &Gen, body_stmts: Vec<Stmt>) -> Result<Function> {
    let mut body = Vec::new();

    for p in &g.param_names {
        body.push(Stmt::Assign {
            target: CExpr::Raw(p.clone()),
            value: CExpr::Raw(format!("_local_{p}")),
        });
    }
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        body.push(Stmt::Assign {
            target: CExpr::Raw(v.name.clone()),
            value: CExpr::Raw(format!("_local_{}", v.name)),
        });
    }

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw(
        "// Allocate memory for local storage.".to_string(),
    ));
    for v in &system.locals {
        let ndims = v.domain.dim(DimType::OutOrSet);
        body.extend(flat_alloc_stmts(&v.name, ndims, &v.domain, "float")?);
    }

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw(
        "// Evaluate every statement, in schedule order.".to_string(),
    ));
    body.extend(body_stmts);

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Free all allocated memory.".to_string()));
    for v in &system.locals {
        body.push(Stmt::Raw(format!("free({});", v.name)));
    }

    let mut params_list = Vec::new();
    for p in &g.param_names {
        params_list.push((CType::Long, format!("_local_{p}")));
    }
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let ty = layout::interface_ctype(CType::Float, v.domain.dim(DimType::OutOrSet));
        params_list.push((ty, format!("_local_{}", v.name)));
    }

    Ok(Function {
        return_type: CType::Void,
        name: system.name.clone(),
        params: params_list,
        body,
        is_static: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stmt;
    use crate::test_util::{
        normalized_system, TestGen, PIECEWISE_EQUATION, PLAIN_COPY, PREFIX_SUM, REDUCE_BINARY_BODY,
        REDUCE_EXTERNAL, REDUCE_MAX, REDUCE_MIN, REDUCE_MUL,
    };

    /// Renders a `Vec<Stmt>` as text — Tier 2 of `docs/codegen-test-design.md` §5.4: one
    /// statement's own body codegen (`gen_statement_body`), no schedule/AST walk, so no loops
    /// ever appear here (see that module's own doc for why loops are a Tier-3-only concern).
    /// `simplec::Stmt` has no standalone `Display`, only `simplec::Function` does — cheapest reuse
    /// is wrapping the statements in a throwaway `Function` rather than adding a new public method
    /// to `simplec.rs` just for this.
    fn render_stmts(stmts: Vec<Stmt>) -> String {
        let f = Function {
            return_type: CType::Void,
            name: "test".to_string(),
            params: vec![],
            body: stmts,
            is_static: false,
        };
        f.to_string()
    }

    #[test]
    fn typed_index_rendering_preserves_arithmetic_grouping() {
        let a = || IndexExpr::Variable("a".to_string());
        let b = || IndexExpr::Variable("b".to_string());
        let c = || IndexExpr::Variable("c".to_string());
        let grouped_product = IndexExpr::Mul(
            Box::new(IndexExpr::Sub(Box::new(a()), Box::new(b()))),
            Box::new(c()),
        );
        let right_nested_subtraction = IndexExpr::Sub(
            Box::new(a()),
            Box::new(IndexExpr::Sub(Box::new(b()), Box::new(c()))),
        );
        assert_eq!(render_index(&grouped_product), "(a - b) * c");
        assert_eq!(render_index(&right_nested_subtraction), "a - (b - c)");
        assert_eq!(
            render_index(&IndexExpr::Div(Box::new(a()), Box::new(b()))),
            "a / b"
        );
        assert_eq!(
            render_index(&IndexExpr::FloorDiv(Box::new(a()), Box::new(b()))),
            "floord(a, b)"
        );
        assert_eq!(
            render_index(&IndexExpr::CeilDiv(Box::new(a()), Box::new(b()))),
            "ceild(a, b)"
        );
    }

    /// Builds a `Gen` + the `Statement` for `<name>__init`/`<name>__reduce`'s pair (`crate::stmt`,
    /// §4.2) from a normalized reduce fixture, then renders both halves' `gen_statement_body`
    /// output — a synthetic index-expression list (`"i0"`, `"i1"`, ...) stands in for whatever a
    /// real `AstBuild` walk would have bound, decoupling this tier from needing one at all.
    fn render_reduce_pair(src: &str) -> String {
        let system = normalized_system(src);
        let statements = stmt::statements(&system).unwrap();
        let mut g = Gen {
            ctx: statements[0].domain.ctx(),
            variables: TestGen::for_system(&system).variables,
            param_names: expr::param_names_of(&statements[0].domain),
            externs: BTreeSet::new(),
        };
        let mut out = String::new();
        for s in &statements {
            let ndims = s.domain.dim(DimType::OutOrSet);
            let idx: Vec<String> = (0..ndims).map(|i| format!("i{i}")).collect();
            let rendered = gen_statement_body(&mut g, s, &idx).unwrap();
            let _ = writeln!(out, "// {}", s.name);
            out.push_str(&render_stmts(rendered));
        }
        out
    }

    #[test]
    fn reduce_plus_baseline() {
        insta::assert_snapshot!(render_reduce_pair(PREFIX_SUM));
    }

    #[test]
    fn reduce_mul() {
        insta::assert_snapshot!(render_reduce_pair(REDUCE_MUL));
    }

    #[test]
    fn reduce_min() {
        insta::assert_snapshot!(render_reduce_pair(REDUCE_MIN));
    }

    #[test]
    fn reduce_max() {
        insta::assert_snapshot!(render_reduce_pair(REDUCE_MAX));
    }

    #[test]
    fn reduce_external_combiner() {
        insta::assert_snapshot!(render_reduce_pair(REDUCE_EXTERNAL));
    }

    #[test]
    fn reduce_binary_body() {
        insta::assert_snapshot!(render_reduce_pair(REDUCE_BINARY_BODY));
    }

    /// Confirms `gen_value`'s shared path behaves identically whether reached from an `Ordinary`
    /// statement (§5.1) or a `ReduceStep` (§4.2) — the `X[j] + X[j]` sub-expression text inside
    /// `REDUCE_BINARY_BODY`'s `__reduce` half should match `BINARY_ADD`'s own Tier-1 snapshot
    /// (`expr::tests::binary_op`) up to the `combine`-table wrapper around it.
    #[test]
    fn reduce_binary_body_shares_gen_value_with_ordinary_binary_op() {
        let system = normalized_system(REDUCE_BINARY_BODY);
        let statements = stmt::statements(&system).unwrap();
        let StatementKind::ReduceStep {
            body, body_context, ..
        } = &statements
            .iter()
            .find(|s| s.name == "Y__reduce")
            .expect("REDUCE_BINARY_BODY should produce a Y__reduce statement")
            .kind
        else {
            panic!("Y__reduce should be a ReduceStep");
        };
        // The reduce's own full `(a+b)`-dim ambient space — `i` (context) and `j` (the reduce's
        // own new dimension) both in scope, exactly as `ReduceStep`'s own codegen binds them
        // (§4.2) — *not* just `[j]`, which would leave `X[j]`'s dependence function under-arity
        // relative to the names list (a mistake this test itself first caught).
        let names = expr::pick_names(body_context, 2);
        let mut gen = TestGen::for_system(&system);
        let rendered = expr::gen_value(&mut gen, &names, body).unwrap().to_string();
        // Matches `expr::tests::binary_op`'s own Tier-1 snapshot for `BINARY_ADD`'s `X[i] + X[i]`
        // up to the bound index name — same `ir::ExprKind::Binary` node, same `gen_value` path,
        // reached from a `ReduceStep` this time instead of an `Ordinary` statement.
        assert_eq!(rendered, "((X(j)) + (X(j)))");
    }

    /// §5.6: a piecewise (multi-`SystemBody`) equation's own body codegen — the ternary-chain
    /// selection by guard domain (§4.1), same mechanism `gen_case` uses for a `case` expression's
    /// own branches, just driven by `crate::stmt`'s equation grouping instead.
    #[test]
    fn piecewise_equation_renders_as_a_guard_selected_ternary_chain() {
        let system = normalized_system(PIECEWISE_EQUATION);
        let statements = stmt::statements(&system).unwrap();
        let s = statements
            .iter()
            .find(|s| s.name == "Y")
            .expect("PIECEWISE_EQUATION should produce a Y statement");
        let mut g = Gen {
            ctx: s.domain.ctx(),
            variables: TestGen::for_system(&system).variables,
            param_names: expr::param_names_of(&s.domain),
            externs: BTreeSet::new(),
        };
        let idx = vec!["i0".to_string()];
        let rendered = gen_statement_body(&mut g, s, &idx).unwrap();
        insta::assert_snapshot!(render_stmts(rendered));
    }

    #[test]
    fn empty_schedule_text_is_rejected_as_illegal_for_prefix_sum() {
        // Confirms end-to-end (not just at the crate::legality unit-test level) that the
        // identity default is illegal for a real reduce dependency (§6, §7) — generation must
        // fail, not silently emit miscompiled code.
        let system = normalized_system(PREFIX_SUM);
        match generate_scheduled_system(&system, "") {
            Err(e @ CodegenError::IllegalSchedule(_)) => {
                // Tier 5 (docs/codegen-test-design.md §5.8): full diagnostic text.
                insta::assert_snapshot!(e.to_string());
            }
            other => panic!("expected an IllegalSchedule error, got {other:?}"),
        }
    }

    #[test]
    fn plain_copy_generates_with_empty_schedule() {
        let system = normalized_system(PLAIN_COPY);
        let code = generate_scheduled_system(&system, "").unwrap();
        assert!(code.contains("Copy"));
        assert!(code.contains("for ("));
    }

    #[test]
    fn prefix_sum_generates_with_explicit_schedule() {
        let system = normalized_system(PREFIX_SUM);
        let text = "{ Y__init[i] -> [i, 0, 0]; \
                     Y__reduce[i,j] -> [i, 1, j]; }";
        let code = generate_scheduled_system(&system, text).unwrap();
        assert!(code.contains("Y"));
        assert!(code.contains("PrefixSum"));
    }
}
