//! `WriteC`: the demand-driven C code generator (`docs/rust-port-design.md` §7). Every equation
//! compiles to a memoized `eval_<Var>(indices...)` function guarded by a `NOT_EVALUATED
//! ('N')/IN_PROGRESS ('I')/EVALUATED ('F')` flag array, exactly like the source system; loops
//! (the driver's "evaluate all outputs" nest, and each `Reduce`'s own summation loop) are
//! generated via `isl_ast_build` over an identity schedule rather than hand-rolled, and every
//! affine/boolean condition is rendered through isl's own C-format printer (see
//! `crate::layout`/`isl::ast`'s module docs) rather than a hand-written affine-to-C converter.
//!
//! **Scope, relative to the source system's own `WriteC`** — see `docs/rust-port-design.md` §7
//! and this crate's `error::CodegenError::Unsupported` sites for the specifics:
//! - `UseEquation`: no codegen backend, matching the source system exactly (not a port
//!   regression).
//! - `Convolution`: unreachable in practice — `alpha_model::domain`'s own documented gap means
//!   `alpha_transform::lower` already excludes any equation containing one.
//! - `Select` (relation-based reindexing), `IndexPolynomial` (`val{...}`), `argreduce`: real
//!   features, genuinely not implemented this session (the whole 82-fixture corpus has zero
//!   `argreduce` uses and only a handful of `select`/`val{...}` uses — see the design doc
//!   discussion this module's development was based on).
//! - Storage for anything this generator itself allocates (locals, and every flag array) uses a
//!   **bounding-box** linearization (`crate::layout`), not the source system's exact
//!   Barvinok/Ehrhart-derived one — see that module's doc for why this is the sanctioned
//!   isl-only fallback, not a correctness gap.

use crate::error::{CodegenError, Result};
use crate::layout;
use crate::simplec::{CType, Expr as CExpr, Function, Stmt};
use alpha_transform::ir;
use isl::{AstBuild, AstNodeKind, Context, DimType, Format, MultiAff, Set, UnionMap};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Input,
    Output,
    Local,
}

struct VarInfo {
    role: Role,
    ndims: u32,
}

struct Gen {
    ctx: Context,
    variables: HashMap<String, VarInfo>,
    param_names: Vec<String>,
    reduce_counter: u32,
    reduce_fns: Vec<Function>,
    externs: BTreeSet<String>,
}

/// Generates a whole system's C source as one self-contained file (preamble, globals, memory
/// macros, every `eval_<Var>`/`reduce<N>` helper, and the system's own public entry point).
pub fn generate_system(system: &ir::System) -> Result<String> {
    for body in &system.bodies {
        for eq in &body.equations {
            if matches!(eq, ir::Equation::Use(_)) {
                return Err(CodegenError::Unsupported(
                    "UseEquation has no codegen backend — matches the source system's own \
                     WriteC, which doesn't support subsystem calls either (docs/rust-port-design.md §7)"
                        .to_string(),
                ));
            }
        }
    }
    let Some(first_body) = system.bodies.first() else {
        return Err(CodegenError::Unsupported(
            "system has no bodies to generate code for".to_string(),
        ));
    };

    let ctx = first_body.domain.ctx();
    let param_names = param_names_of(&first_body.domain);

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

    let mut equations_by_var: HashMap<&str, Vec<(&Set, &ir::StandardEquation)>> = HashMap::new();
    for body in &system.bodies {
        for eq in &body.equations {
            if let ir::Equation::Standard(s) = eq {
                equations_by_var
                    .entry(s.variable.as_str())
                    .or_default()
                    .push((&body.domain, s));
            }
        }
    }

    let mut g = Gen {
        ctx,
        variables,
        param_names,
        reduce_counter: 0,
        reduce_fns: Vec::new(),
        externs: BTreeSet::new(),
    };

    let ordered_vars: Vec<&ir::Variable> = system
        .outputs
        .iter()
        .chain(system.locals.iter())
        .collect();
    let mut eval_fns = Vec::new();
    for v in &ordered_vars {
        let Some(eqs) = equations_by_var.get(v.name.as_str()) else {
            continue;
        };
        eval_fns.push(gen_eval_function(&mut g, v, eqs)?);
    }

    let driver = gen_driver(&mut g, system)?;

    Ok(render_program(system, &g, &eval_fns, &driver))
}

fn param_names_of(domain: &Set) -> Vec<String> {
    let space = domain.space();
    let n = space.dim(DimType::Param);
    (0..n)
        .map(|d| space.dim_name(DimType::Param, d).unwrap_or_else(|| format!("P{d}")))
        .collect()
}

fn space_prefix(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("[{}] -> ", params.join(","))
    }
}

/// Builds an `AstBuild` whose "current" dims are exactly `names` (plus every system param) — used
/// to render a `Set` sharing that ambient space as a boolean C condition via
/// `AstBuild::expr_from_set`. The passed set's *own* dim names don't need to match `names` (isl
/// matches positionally here — verified empirically against a real isl build before writing this
/// module); only the dim count and param names need to agree.
fn ambient_build(g: &Gen, names: &[String]) -> Result<AstBuild> {
    let text = if names.is_empty() {
        format!("{}{{: }}", space_prefix(&g.param_names))
    } else {
        format!("{}{{[{}]: }}", space_prefix(&g.param_names), names.join(","))
    };
    let universe = Set::read_from_str(&g.ctx, &text)?;
    Ok(AstBuild::from_context(universe)?)
}

fn is_valid_c_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    !matches!(
        s,
        "int" | "float" | "double" | "return" | "if" | "else" | "for" | "while" | "long"
            | "char" | "void" | "static" | "struct" | "const" | "break" | "continue"
    )
}

/// Prefers `hint` (real source names, threaded through from `alpha_transform::ir` purely as a
/// naming aid — see that crate's doc comments on `StandardEquation::index_names`/
/// `ExprKind::Reduce::body_context`) when it has exactly `n` valid C identifiers; falls back to
/// synthesized `<prefix><0..n>` names otherwise. Either choice is semantically equivalent — the
/// actual C variable a dimension gets named is cosmetic, since every isl object this generator
/// prints from is explicitly renamed to match via `MultiAff::set_dim_name` before printing.
fn pick_names(hint: &[String], n: u32) -> Vec<String> {
    pick_names_prefixed(hint, n, "i")
}

fn pick_names_prefixed(hint: &[String], n: u32, prefix: &str) -> Vec<String> {
    if hint.len() as u32 == n && hint.iter().all(|s| is_valid_c_ident(s)) {
        hint.to_vec()
    } else {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }
}

/// Classifies an isl failure from a bound-dependent operation (`Set::dim_max`/`dim_min`,
/// `AstBuild::generate`) as a documented scope boundary rather than a bug, when it's isl's own
/// "unbounded optimum" — a real, if rare, tree shape this port doesn't handle: e.g. a `case`'s
/// `auto` branch's true domain is computed purely as "parent context minus siblings"
/// (`Normalize`'s own rule discards a wrapping restrict for `auto` specifically — see that
/// module's doc), and if the reduce/equation's own ambient context is *itself* unbounded (nothing
/// else constrains it, e.g. a 0-dimensional output whose whole body is one `reduce` with an
/// `auto` case branch), the `auto` branch's resulting domain can be unbounded even though the
/// source program's intent was clearly finite. A real limitation of this session's from-context
/// bound derivation, not something to silently mis-generate.
fn isl_bound_err(context: &str, e: isl::IslError) -> CodegenError {
    if e.message.contains("unbounded") {
        CodegenError::Unsupported(format!(
            "{context}: isl couldn't establish a bound (\"{}\") — likely a `case` branch (often \
             `auto`) whose domain isn't independently bounded once combined with only the \
             enclosing equation's own ambient context; a documented limitation, not a crash",
            e.message
        ))
    } else {
        CodegenError::Isl(e)
    }
}

fn rename_inputs(f: MultiAff, names: &[String]) -> Result<MultiAff> {
    let mut f = f;
    for (d, name) in names.iter().enumerate() {
        f = f.set_dim_name(DimType::In, d as u32, name)?;
    }
    Ok(f)
}

fn binary_c_op(op: &str) -> Option<&'static str> {
    Some(match op {
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "=" => "==",
        "!=" => "!=",
        ">=" => ">=",
        ">" => ">",
        "<" => "<",
        "<=" => "<=",
        "and" => "&&",
        "or" => "||",
        _ => return None,
    })
}

fn gen_value(g: &mut Gen, names: &[String], e: &ir::Expr) -> Result<CExpr> {
    match &*e.kind {
        ir::ExprKind::Variable(name) => Err(CodegenError::Unsupported(format!(
            "internal error: bare Variable('{name}') reached codegen outside a Dependence — \
             Normalize should have wrapped every Variable in an identity Dependence"
        ))),
        ir::ExprKind::Bool(b) => Ok(CExpr::Raw(if *b { "1.0f".to_string() } else { "0.0f".to_string() })),
        ir::ExprKind::Int(s) => Ok(CExpr::Raw(format!("((float)({s}))"))),
        ir::ExprKind::Real(s) => Ok(CExpr::Raw(format!("((float)({s}))"))),
        ir::ExprKind::Dependence { function, operand } => gen_dependence(g, names, function, operand),
        ir::ExprKind::IndexFunction { function } => gen_index_function(names, function),
        ir::ExprKind::IndexPolynomial { .. } => Err(CodegenError::Unsupported(
            "val{...} (polynomial-valued index expression) codegen not implemented this session"
                .to_string(),
        )),
        ir::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let c = gen_value(g, names, cond)?;
            let t = gen_value(g, names, then_branch)?;
            let e2 = gen_value(g, names, else_branch)?;
            Ok(CExpr::Ternary(Box::new(c), Box::new(t), Box::new(e2)))
        }
        ir::ExprKind::Restrict { operand, .. } => gen_value(g, names, operand),
        ir::ExprKind::AutoRestrict { operand } => gen_value(g, names, operand),
        ir::ExprKind::Case { branches, .. } => gen_case(g, names, branches),
        ir::ExprKind::Reduce {
            is_arg_reduce,
            operator,
            body_context,
            body,
            ..
        } => {
            if *is_arg_reduce {
                return Err(CodegenError::Unsupported(
                    "argreduce codegen not implemented (unseen across the whole fixture corpus)"
                        .to_string(),
                ));
            }
            gen_reduce(g, names, operator, body_context, body)
        }
        ir::ExprKind::Convolution { .. } => Err(CodegenError::Unsupported(
            "Convolution codegen not implemented — should be unreachable, since \
             alpha_model::domain's own documented gap means lowering already excludes any \
             equation containing one"
                .to_string(),
        )),
        ir::ExprKind::Select { .. } => Err(CodegenError::Unsupported(
            "select {relation} from E codegen not implemented this session".to_string(),
        )),
        ir::ExprKind::MultiArg { operator, args } => gen_multi_arg(g, names, operator, args),
        ir::ExprKind::Binary { operator, lhs, rhs } => gen_binary(g, names, operator, lhs, rhs),
        ir::ExprKind::Unary { operator, operand } => gen_unary(g, names, operator, operand),
    }
}

fn gen_dependence(
    g: &mut Gen,
    names: &[String],
    function: &MultiAff,
    operand: &ir::Expr,
) -> Result<CExpr> {
    match &*operand.kind {
        ir::ExprKind::Bool(_) | ir::ExprKind::Int(_) | ir::ExprKind::Real(_) => {
            // A dependence function applied to a constant doesn't change its value — only the
            // (already-accounted-for-elsewhere) domain it's replicated across.
            gen_value(g, names, operand)
        }
        ir::ExprKind::Variable(name) => {
            let info = g.variables.get(name.as_str()).ok_or_else(|| {
                CodegenError::Unsupported(format!("reference to undeclared variable '{name}'"))
            })?;
            let n_in = function.dim(DimType::In);
            if n_in != names.len() as u32 {
                return Err(CodegenError::Unsupported(format!(
                    "dependence function arity ({n_in}) doesn't match the ambient index count ({})",
                    names.len()
                )));
            }
            if function.dim(DimType::OutOrSet) != info.ndims {
                return Err(CodegenError::Unsupported(format!(
                    "dependence function's output arity doesn't match '{name}''s own dimensionality"
                )));
            }
            let renamed = rename_inputs(function.clone(), names)?;
            let mut idx = Vec::with_capacity(info.ndims as usize);
            for d in 0..info.ndims {
                idx.push(renamed.get_aff(d)?.to_string_fmt(Format::C));
            }
            let call = match info.role {
                Role::Input => format!("{name}({})", idx.join(",")),
                Role::Output | Role::Local => format!("eval_{name}({})", idx.join(",")),
            };
            Ok(CExpr::Raw(call))
        }
        other => Err(CodegenError::Unsupported(format!(
            "internal error: Dependence child is {} (Normalize guarantees Variable or a constant)",
            other_kind_name(other)
        ))),
    }
}

fn other_kind_name(k: &ir::ExprKind) -> &'static str {
    match k {
        ir::ExprKind::Variable(_) => "Variable",
        ir::ExprKind::Bool(_) => "Bool",
        ir::ExprKind::Int(_) => "Int",
        ir::ExprKind::Real(_) => "Real",
        ir::ExprKind::Dependence { .. } => "Dependence",
        ir::ExprKind::IndexFunction { .. } => "IndexFunction",
        ir::ExprKind::IndexPolynomial { .. } => "IndexPolynomial",
        ir::ExprKind::If { .. } => "If",
        ir::ExprKind::Restrict { .. } => "Restrict",
        ir::ExprKind::AutoRestrict { .. } => "AutoRestrict",
        ir::ExprKind::Case { .. } => "Case",
        ir::ExprKind::Reduce { .. } => "Reduce",
        ir::ExprKind::Convolution { .. } => "Convolution",
        ir::ExprKind::Select { .. } => "Select",
        ir::ExprKind::MultiArg { .. } => "MultiArg",
        ir::ExprKind::Binary { .. } => "Binary",
        ir::ExprKind::Unary { .. } => "Unary",
    }
}

fn gen_index_function(names: &[String], function: &MultiAff) -> Result<CExpr> {
    if function.dim(DimType::OutOrSet) != 1 {
        return Err(CodegenError::Unsupported(
            "val(f) codegen only implemented for a single-output function".to_string(),
        ));
    }
    if function.dim(DimType::In) != names.len() as u32 {
        return Err(CodegenError::Unsupported(
            "val(f) function's arity doesn't match the ambient index count".to_string(),
        ));
    }
    let renamed = rename_inputs(function.clone(), names)?;
    Ok(CExpr::Raw(format!(
        "((float)({}))",
        renamed.get_aff(0)?.to_string_fmt(Format::C)
    )))
}

fn gen_case(g: &mut Gen, names: &[String], branches: &[ir::Expr]) -> Result<CExpr> {
    let Some((last, rest)) = branches.split_last() else {
        return Err(CodegenError::Unsupported("case with zero branches".to_string()));
    };
    let mut result = gen_value(g, names, last)?;
    if rest.is_empty() {
        return Ok(result);
    }
    let build = ambient_build(g, names)?;
    for b in rest.iter().rev() {
        let guard = b.context_domain.clone().ok_or_else(|| {
            CodegenError::Unsupported(
                "case branch missing a context domain — codegen must run after Normalize's \
                 context refresh"
                    .to_string(),
            )
        })?;
        let cond_expr = build.expr_from_set(guard)?;
        let cond = CExpr::Raw(cond_expr.to_string_fmt(Format::C));
        let val = gen_value(g, names, b)?;
        result = CExpr::Ternary(Box::new(cond), Box::new(val), Box::new(result));
    }
    Ok(result)
}

fn gen_binary(
    g: &mut Gen,
    names: &[String],
    operator: &str,
    lhs: &ir::Expr,
    rhs: &ir::Expr,
) -> Result<CExpr> {
    let l = gen_value(g, names, lhs)?;
    let r = gen_value(g, names, rhs)?;
    match operator {
        "min" | "max" => Ok(CExpr::Call(operator.to_string(), vec![l, r])),
        "xor" => Ok(CExpr::Raw(format!("((({l}) != 0) != (({r}) != 0))"))),
        _ => {
            let c_op = binary_c_op(operator).ok_or_else(|| {
                CodegenError::Unsupported(format!("unknown binary operator '{operator}'"))
            })?;
            Ok(CExpr::Raw(format!("(({l}) {c_op} ({r}))")))
        }
    }
}

fn gen_unary(g: &mut Gen, names: &[String], operator: &str, operand: &ir::Expr) -> Result<CExpr> {
    let v = gen_value(g, names, operand)?;
    match operator {
        "-" => Ok(CExpr::Raw(format!("(-({v}))"))),
        "not" => Ok(CExpr::Raw(format!("(!({v}))"))),
        _ => Err(CodegenError::Unsupported(format!("unknown unary operator '{operator}'"))),
    }
}

fn gen_multi_arg(
    g: &mut Gen,
    names: &[String],
    operator: &ir::Operator,
    args: &[ir::Expr],
) -> Result<CExpr> {
    let vals: Vec<CExpr> = args
        .iter()
        .map(|a| gen_value(g, names, a))
        .collect::<Result<_>>()?;
    match operator {
        ir::Operator::External(name) => {
            g.externs.insert(name.clone());
            Ok(CExpr::Call(name.clone(), vals))
        }
        ir::Operator::Named(op) => {
            let mut iter = vals.into_iter();
            let first = iter.next().ok_or_else(|| {
                CodegenError::Unsupported("multi-arg with zero operands".to_string())
            })?;
            // `sum`/`prod` are `MultiArgExpression`'s own N-ary spellings of `+`/`*` (see
            // `alpha-syntax`'s `MultiArgExpr::named_operator`) — not reduce-operator tokens here.
            match op.as_str() {
                "min" | "max" => Ok(iter.fold(first, |acc, v| CExpr::Call(op.clone(), vec![acc, v]))),
                "xor" => Ok(iter.fold(first, |acc, v| {
                    CExpr::Raw(format!("((({acc}) != 0) != (({v}) != 0))"))
                })),
                "sum" => Ok(iter.fold(first, |acc, v| CExpr::Raw(format!("(({acc}) + ({v}))")))),
                "prod" => Ok(iter.fold(first, |acc, v| CExpr::Raw(format!("(({acc}) * ({v}))")))),
                _ => {
                    let c_op = binary_c_op(op).ok_or_else(|| {
                        CodegenError::Unsupported(format!("unknown multi-arg operator '{op}'"))
                    })?;
                    Ok(iter.fold(first, |acc, v| {
                        CExpr::Raw(format!("(({acc}) {c_op} ({v}))"))
                    }))
                }
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn gen_reduce(
    g: &mut Gen,
    names: &[String],
    operator: &ir::Operator,
    body_context_hint: &[String],
    body: &ir::Expr,
) -> Result<CExpr> {
    let a = names.len() as u32;
    // `expression_domain` alone is only ever a bottom-up derivation — it has no reason to be
    // bounded in the *ambient* dims (e.g. `PrefixScan`'s `reduce(+, [j], {:j<=i}: X[j])` bounds
    // `j` only relative to `i`, never bounding `i` itself: nothing about `X[j]`'s own domain or
    // the explicit `j<=i` restrict says anything about `i`'s own range). `context_domain` is
    // exactly the complementary, top-down-established piece that *does* carry the ambient
    // equation's own bounds (`refresh_context`'s `Reduce` case: `own_context.preimage_multi_aff`)
    // — intersecting both is what actually gives a domain isl's per-dimension `dim_max`/`dim_min`
    // can bound (calling `dim_max`/`dim_min` on a set that's unbounded in that dimension is an
    // isl error, "unbounded optimum", not a wrong-but-silent answer, so this was caught by an
    // actual test run against `PrefixScan`, not by inspection).
    let context_domain = body.context_domain.clone().ok_or_else(|| {
        CodegenError::Unsupported(
            "reduce body missing a context domain — codegen must run after Normalize's context \
             refresh"
                .to_string(),
        )
    })?;
    let full_domain = body.expression_domain.clone().intersect(context_domain)?;
    let total = full_domain.dim(DimType::OutOrSet);
    if total < a {
        return Err(CodegenError::Unsupported(
            "reduce body's domain has fewer dims than the ambient index tuple".to_string(),
        ));
    }
    let b = total - a;
    if b == 0 {
        return Err(CodegenError::Unsupported("reduce with no new index dims".to_string()));
    }
    let primed: Vec<String> = names.iter().map(|n| format!("{n}p")).collect();
    let new_hint: Vec<String> = if body_context_hint.len() as u32 == total {
        body_context_hint[a as usize..].to_vec()
    } else {
        Vec::new()
    };
    let new_names = pick_names_prefixed(&new_hint, b, "k");

    let n_param = g.param_names.len() as u32;
    let mut domain = full_domain;
    if a > 0 {
        domain = domain.move_dims(DimType::Param, n_param, DimType::OutOrSet, 0, a)?;
        for (i, name) in primed.iter().enumerate() {
            domain = domain.set_dim_name(DimType::Param, n_param + i as u32, name)?;
        }
    }
    let param_only = domain.clone().params()?;
    let iter_refs: Vec<&str> = new_names.iter().map(String::as_str).collect();
    let build = AstBuild::from_context(param_only)?.set_iterators(&iter_refs)?;
    let ident = MultiAff::identity_on_domain_space(domain.space())?
        .into_map()?
        .intersect_domain(domain.clone())?;
    let ast = build
        .generate(UnionMap::from_map(ident))
        .map_err(|e| isl_bound_err("generating this reduce's own summation loop", e))?;

    let mut for_headers = Vec::new();
    let mut node = ast;
    while let AstNodeKind::For { .. } = node.kind() {
        for_headers.push((
            node.for_iterator()?.to_string_fmt(Format::C),
            node.for_init()?.to_string_fmt(Format::C),
            node.for_cond()?.to_string_fmt(Format::C),
            node.for_inc()?.to_string_fmt(Format::C),
        ));
        node = node.for_body()?;
    }
    // A new index dim isl's own `ast_build` recognizes as single-valued (e.g.
    // `scalarReduction1a`'s `reduce(+, [j], {:i=j} : Y[j])`, where `j` has exactly one value for
    // any given `i`) never gets its own `for` node at all — isl substitutes the fixed value
    // directly instead of generating a degenerate one-iteration loop. Detect exactly which
    // `new_names` didn't get a `for` (by name, since `set_iterators` named every one of them) and
    // give those a plain `long j = <value>;` initializer (via `Set::dim_max`, safe here precisely
    // because they're single-valued — `dim_min` would give the same answer) instead of a loop.
    let for_names: std::collections::HashSet<&str> =
        for_headers.iter().map(|(n, ..)| n.as_str()).collect();
    let mut degenerate: HashMap<String, String> = HashMap::new();
    for (pos, name) in new_names.iter().enumerate() {
        if for_names.contains(name.as_str()) {
            continue;
        }
        let value = domain
            .dim_max(pos as u32)
            .map_err(|e| isl_bound_err(&format!("resolving reduce index '{name}'"), e))?
            .to_string_fmt(Format::C);
        degenerate.insert(name.clone(), value);
    }

    let reduce_local_names: Vec<String> = primed
        .iter()
        .cloned()
        .chain(new_names.iter().cloned())
        .collect();
    let value = gen_value(g, &reduce_local_names, body)?;

    let (init_val, combine_extern): (String, Option<String>) = match operator {
        ir::Operator::External(name) => {
            g.externs.insert(name.clone());
            ("0.0f".to_string(), Some(name.clone()))
        }
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
            (init.to_string(), None)
        }
    };
    let accumulate = match operator {
        ir::Operator::External(_) => format!(
            "{}(reduceVar, {value})",
            combine_extern.as_deref().unwrap_or_default()
        ),
        ir::Operator::Named(op) => match op.as_str() {
            "+" | "sum" => format!("(reduceVar) + ({value})"),
            "*" | "prod" => format!("(reduceVar) * ({value})"),
            "min" | "max" => format!("{op}(reduceVar, {value})"),
            "and" => format!("((reduceVar) != 0) && (({value}) != 0)"),
            "or" => format!("((reduceVar) != 0) || (({value}) != 0)"),
            _ => unreachable!("validated above"),
        },
    };

    let fn_name = format!("reduce{}", g.reduce_counter);
    g.reduce_counter += 1;

    let mut params: Vec<(CType, String)> =
        g.param_names.iter().map(|p| (CType::Long, p.clone())).collect();
    for p in &primed {
        params.push((CType::Long, p.clone()));
    }

    let mut body_stmts = vec![
        Stmt::Decl {
            ty: CType::Float,
            name: "reduceVar".to_string(),
            init: None,
        },
        Stmt::Raw(String::new()),
    ];
    for n in &new_names {
        body_stmts.push(Stmt::Decl {
            ty: CType::Long,
            name: n.clone(),
            init: degenerate.get(n).map(|v| CExpr::Raw(v.clone())),
        });
    }
    body_stmts.push(Stmt::Raw(String::new()));
    body_stmts.push(Stmt::Assign {
        target: CExpr::Raw("reduceVar".to_string()),
        value: CExpr::Raw(init_val),
    });
    let mut nested = vec![Stmt::Assign {
        target: CExpr::Raw("reduceVar".to_string()),
        value: CExpr::Raw(accumulate),
    }];
    for (iter_name, init, cond, inc) in for_headers.into_iter().rev() {
        nested = vec![Stmt::For {
            iterator: iter_name,
            init,
            cond,
            inc,
            body: nested,
        }];
    }
    body_stmts.extend(nested);
    body_stmts.push(Stmt::Return(Some(CExpr::Raw("reduceVar".to_string()))));

    g.reduce_fns.push(Function {
        return_type: CType::Float,
        name: fn_name.clone(),
        params,
        body: body_stmts,
        is_static: true,
    });

    let mut call_args: Vec<CExpr> = g
        .param_names
        .iter()
        .map(|p| CExpr::Raw(p.clone()))
        .collect();
    call_args.extend(names.iter().map(|n| CExpr::Raw(n.clone())));
    Ok(CExpr::Call(fn_name, call_args))
}

fn gen_eval_function(
    g: &mut Gen,
    v: &ir::Variable,
    eqs: &[(&Set, &ir::StandardEquation)],
) -> Result<Function> {
    let ndims = v.domain.dim(DimType::OutOrSet);
    let hint = &eqs[0].1.index_names;
    let names = pick_names(hint, ndims);
    let params: Vec<(CType, String)> =
        names.iter().map(|n| (CType::Long, n.clone())).collect();

    let value_expr = if eqs.len() == 1 {
        gen_value(g, &names, &eqs[0].1.expr)?
    } else {
        let Some((last, rest)) = eqs.split_last() else {
            unreachable!("eqs is non-empty (checked by caller)")
        };
        let build = ambient_build(g, &[])?;
        let mut result = gen_value(g, &names, &last.1.expr)?;
        for (body_domain, s) in rest.iter().rev() {
            let cond_expr = build.expr_from_set((*body_domain).clone())?;
            let cond = CExpr::Raw(cond_expr.to_string_fmt(Format::C));
            let val = gen_value(g, &names, &s.expr)?;
            result = CExpr::Ternary(Box::new(cond), Box::new(val), Box::new(result));
        }
        result
    };

    let flag_access = format!("_flag_{}({})", v.name, names.join(","));
    let data_access = format!("{}({})", v.name, names.join(","));
    let fmt_args: String = names.iter().map(|n| format!(",{n}")).collect();
    let fmt_spec = names.iter().map(|_| "%ld").collect::<Vec<_>>().join(",");

    let body = vec![
        Stmt::Raw(String::new()),
        Stmt::Raw("// Check the flags.".to_string()),
        Stmt::If {
            cond: CExpr::Raw(format!("({flag_access}) == ('N')")),
            then_branch: vec![
                Stmt::Assign {
                    target: CExpr::Raw(flag_access.clone()),
                    value: CExpr::Raw("'I'".to_string()),
                },
                Stmt::Assign {
                    target: CExpr::Raw(data_access.clone()),
                    value: value_expr,
                },
                Stmt::Assign {
                    target: CExpr::Raw(flag_access.clone()),
                    value: CExpr::Raw("'F'".to_string()),
                },
            ],
            else_branch: vec![Stmt::If {
                cond: CExpr::Raw(format!("({flag_access}) == ('I')")),
                then_branch: vec![
                    Stmt::Raw(format!(
                        "printf(\"There is a self dependence on {} at ({fmt_spec})\\n\"{fmt_args});",
                        v.name
                    )),
                    Stmt::Raw("exit(-1);".to_string()),
                ],
                else_branch: vec![],
            }],
        },
        Stmt::Raw(String::new()),
        Stmt::Return(Some(CExpr::Raw(data_access))),
    ];

    Ok(Function {
        return_type: CType::Float,
        name: format!("eval_{}", v.name),
        params,
        body,
        is_static: true,
    })
}

fn union_of_body_domains(system: &ir::System) -> Result<Set> {
    let mut iter = system.bodies.iter();
    let Some(first) = iter.next() else {
        return Err(CodegenError::Unsupported("system has no bodies".to_string()));
    };
    let mut acc = first.domain.clone();
    for b in iter {
        acc = acc.union(b.domain.clone())?;
    }
    Ok(acc)
}

fn flat_alloc_stmts(name: &str, ndims: u32, domain: &Set, elem_ctype: &str) -> Result<Vec<Stmt>> {
    let bounds = layout::FlatBounds::compute(domain).map_err(|e| match e {
        CodegenError::Isl(isl_e) => isl_bound_err(&format!("sizing storage for '{name}'"), isl_e),
        other => other,
    })?;
    let mut stmts: Vec<Stmt> = layout::flat_size_init_stmts(name, &bounds)
        .into_iter()
        .map(Stmt::Raw)
        .collect();
    let total = layout::flat_total_size_expr(name, ndims);
    stmts.push(Stmt::Assign {
        target: CExpr::Raw(name.to_string()),
        value: CExpr::Raw(format!("({elem_ctype}*)(malloc((sizeof({elem_ctype})) * ({total})))")),
    });
    stmts.push(Stmt::Raw(format!("mallocCheck({name},\"{name}\");")));
    Ok(stmts)
}

fn gen_eval_loop(v: &ir::Variable) -> Result<Vec<Stmt>> {
    let ndims = v.domain.dim(DimType::OutOrSet);
    let names: Vec<String> = (0..ndims).map(|d| format!("i{d}")).collect();
    let call = Stmt::Expr(CExpr::Call(
        format!("eval_{}", v.name),
        names.iter().map(|n| CExpr::Raw(n.clone())).collect(),
    ));
    if ndims == 0 {
        return Ok(vec![call]);
    }
    let ctx_set = v.domain.clone().params()?;
    let iter_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let build = AstBuild::from_context(ctx_set)?.set_iterators(&iter_refs)?;
    let ident = MultiAff::identity_on_domain_space(v.domain.space())?
        .into_map()?
        .intersect_domain(v.domain.clone())?;
    let ast = build
        .generate(UnionMap::from_map(ident))
        .map_err(|e| isl_bound_err(&format!("generating the driver's loop over '{}'", v.name), e))?;

    let mut headers = Vec::new();
    let mut node = ast;
    while let AstNodeKind::For { .. } = node.kind() {
        headers.push((
            node.for_iterator()?.to_string_fmt(Format::C),
            node.for_init()?.to_string_fmt(Format::C),
            node.for_cond()?.to_string_fmt(Format::C),
            node.for_inc()?.to_string_fmt(Format::C),
        ));
        node = node.for_body()?;
    }
    if headers.len() as u32 != ndims {
        return Err(CodegenError::Unsupported(format!(
            "driver loop for '{}' produced {} nested loops, expected {}",
            v.name,
            headers.len(),
            ndims
        )));
    }

    let mut nested = vec![call];
    for (iter_name, init, cond, inc) in headers.into_iter().rev() {
        nested = vec![Stmt::For {
            iterator: iter_name,
            init,
            cond,
            inc,
            body: nested,
        }];
    }
    Ok(nested)
}

fn gen_driver(g: &mut Gen, system: &ir::System) -> Result<Function> {
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

    let params_only_domain = union_of_body_domains(system)?;
    let build0 = ambient_build(g, &[])?;
    let cond = build0.expr_from_set(params_only_domain)?;
    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Check parameter validity.".to_string()));
    body.push(Stmt::If {
        cond: CExpr::Raw(format!("!({})", cond.to_string_fmt(Format::C))),
        then_branch: vec![
            Stmt::Raw("printf(\"The value of the parameters are invalid.\\n\");".to_string()),
            Stmt::Raw("exit(-1);".to_string()),
        ],
        else_branch: vec![],
    });

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Allocate memory for local storage.".to_string()));
    for v in &system.locals {
        let ndims = v.domain.dim(DimType::OutOrSet);
        body.extend(flat_alloc_stmts(&v.name, ndims, &v.domain, "float")?);
    }

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Allocate and initialize flag variables.".to_string()));
    for v in system.outputs.iter().chain(system.locals.iter()) {
        let flag = format!("_flag_{}", v.name);
        let ndims = v.domain.dim(DimType::OutOrSet);
        let mut stmts = flat_alloc_stmts(&flag, ndims, &v.domain, "char")?;
        let total = layout::flat_total_size_expr(&flag, ndims);
        stmts.push(Stmt::Raw(format!("memset({flag},'N',{total});")));
        body.extend(stmts);
    }

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Evaluate all the outputs.".to_string()));
    for v in &system.outputs {
        body.extend(gen_eval_loop(v)?);
    }

    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw("// Free all allocated memory.".to_string()));
    for v in &system.locals {
        body.push(Stmt::Raw(format!("free({});", v.name)));
    }
    for v in system.outputs.iter().chain(system.locals.iter()) {
        body.push(Stmt::Raw(format!("free(_flag_{});", v.name)));
    }

    let mut params_list = Vec::new();
    for p in &g.param_names {
        params_list.push((CType::Long, format!("_local_{p}")));
    }
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let ty = layout::interface_ctype(CType::Float, v.domain.dim(DimType::OutOrSet));
        params_list.push((ty, format!("_local_{}", v.name)));
    }

    let maxdims = system
        .outputs
        .iter()
        .map(|v| v.domain.dim(DimType::OutOrSet))
        .max()
        .unwrap_or(0);
    let mut full_body: Vec<Stmt> = (0..maxdims)
        .map(|d| Stmt::Decl {
            ty: CType::Long,
            name: format!("i{d}"),
            init: None,
        })
        .collect();
    full_body.push(Stmt::Raw(String::new()));
    full_body.push(Stmt::Raw("// Copy arguments to the global variables.".to_string()));
    full_body.extend(body);

    Ok(Function {
        return_type: CType::Void,
        name: system.name.clone(),
        params: params_list,
        body: full_body,
        is_static: false,
    })
}

fn render_program(system: &ir::System, g: &Gen, eval_fns: &[Function], driver: &Function) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "// This code was auto-generated by alphac (alpha-rs).");
    let _ = writeln!(out);
    for inc in ["float.h", "limits.h", "math.h", "stdbool.h", "stdio.h", "stdlib.h", "string.h"] {
        let _ = writeln!(out, "#include <{inc}>");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "// Function Macros");
    let _ = writeln!(out, "#define ceild(n,d) ((int)ceil(((double)(n))/((double)(d))))");
    let _ = writeln!(out, "#define floord(n,d) ((int)floor(((double)(n))/((double)(d))))");
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
    for v in system.outputs.iter().chain(system.locals.iter()) {
        let flag = format!("_flag_{}", v.name);
        let ndims = v.domain.dim(DimType::OutOrSet);
        let _ = writeln!(out, "static char* {flag};");
        for line in layout::flat_size_decls(&flag, ndims) {
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
    for v in system.outputs.iter().chain(system.locals.iter()) {
        let flag = format!("_flag_{}", v.name);
        let _ = writeln!(
            out,
            "{}",
            layout::flat_access_macro(&flag, v.domain.dim(DimType::OutOrSet))
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "// Function Declarations");
    for f in &g.reduce_fns {
        let _ = writeln!(out, "{};", f.signature());
    }
    for f in eval_fns {
        let _ = writeln!(out, "{};", f.signature());
    }
    let _ = writeln!(out, "{};", driver.signature());
    let _ = writeln!(out);

    for f in &g.reduce_fns {
        let _ = writeln!(out, "{f}");
        let _ = writeln!(out);
    }
    for f in eval_fns {
        let _ = writeln!(out, "{f}");
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "{driver}");

    let _ = writeln!(out);
    let _ = writeln!(out, "// Undefine the Memory and Function Macros");
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let _ = writeln!(out, "#undef {}", v.name);
    }
    for v in &system.locals {
        let _ = writeln!(out, "#undef {}", v.name);
    }
    for v in system.outputs.iter().chain(system.locals.iter()) {
        let _ = writeln!(out, "#undef _flag_{}", v.name);
    }
    let _ = writeln!(out, "#undef ceild");
    let _ = writeln!(out, "#undef floord");
    let _ = writeln!(out, "#undef div");
    let _ = writeln!(out, "#undef max");
    let _ = writeln!(out, "#undef min");
    let _ = writeln!(out, "#undef mallocCheck");

    out
}
