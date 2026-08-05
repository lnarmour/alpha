//! Backend-agnostic expression codegen, shared between `writec::Gen` and `scheduledc`'s own
//! generator. Extracted from `writec.rs` with no behavior change for `WriteC` — see that module's
//! original doc comments (preserved below on each function) for the isl-specific rationale.
//!
//! The one place backends genuinely differ in how they read a variable
//! (`WriteC`: `Role::Input` reads via the interface access macro, `Role::Output`/`Role::Local` via
//! an `eval_<name>(...)` call; `ScheduledC`: every role reads via its access macro, uniformly) is
//! factored out as [`ExprGen::render_read`] — everything else here is a pure function of
//! `names: &[String]`, not of *how* those names got bound, so it's reused unchanged.

use crate::error::{CodegenError, Result};
use crate::simplec::Expr as CExpr;
use alpha_transform::ir;
use isl::{AstBuild, Context, DimType, Format, MultiAff, Set};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    Input,
    Output,
    Local,
}

pub(crate) struct VarInfo {
    pub role: Role,
    pub ndims: u32,
}

/// The minimal capability set `gen_value` and everything it calls need from a backend's own
/// generator state. Each backend's own struct (`writec::Gen`, `scheduledc`'s own context)
/// implements this directly rather than wrapping/duplicating state.
pub(crate) trait ExprGen {
    fn ctx(&self) -> &Context;
    fn param_names(&self) -> &[String];
    fn variable(&self, name: &str) -> Option<&VarInfo>;
    /// Record that `name` (an `Operator::External`/external combiner function) is called
    /// somewhere in the generated program, so the caller can emit an `extern` declaration for it.
    fn add_extern(&mut self, name: &str);
    /// Render a read of `name` (already arity/dimension-checked, with `idx` the already-computed
    /// C index expressions) as a C expression string.
    fn render_read(&self, name: &str, role: Role, idx: &[String]) -> String;
    /// Handle a non-arg `Reduce` node reached during expression codegen. `gen_reduce` itself is
    /// *not* shared between backends (§8.3 of the scheduled-codegen design) — `WriteC` generates
    /// its own private summation loop here; `ScheduledC` should never actually reach this, since
    /// normalization splits every reduce into its own `<name>__init`/`<name>__reduce` statement
    /// pair (§4.2) before codegen runs, so a `ScheduledC` context's own implementation of this
    /// method can simply report that invariant violation as an internal error.
    fn gen_reduce_value(
        &mut self,
        names: &[String],
        operator: &ir::Operator,
        body_context_hint: &[String],
        body: &ir::Expr,
    ) -> Result<CExpr>;
}

pub(crate) fn param_names_of(domain: &Set) -> Vec<String> {
    let space = domain.space();
    let n = space.dim(DimType::Param);
    (0..n)
        .map(|d| {
            space
                .dim_name(DimType::Param, d)
                .unwrap_or_else(|| format!("P{d}"))
        })
        .collect()
}

pub(crate) fn space_prefix(params: &[String]) -> String {
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
pub(crate) fn ambient_build(
    ctx: &Context,
    param_names: &[String],
    names: &[String],
) -> Result<AstBuild> {
    let text = if names.is_empty() {
        format!("{}{{: }}", space_prefix(param_names))
    } else {
        format!("{}{{[{}]: }}", space_prefix(param_names), names.join(","))
    };
    let universe = Set::read_from_str(ctx, &text)?;
    Ok(AstBuild::from_context(universe)?)
}

pub(crate) fn is_valid_c_ident(s: &str) -> bool {
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
        "int"
            | "float"
            | "double"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "long"
            | "char"
            | "void"
            | "static"
            | "struct"
            | "const"
            | "break"
            | "continue"
    )
}

/// Prefers `hint` (real source names, threaded through from `alpha_transform::ir` purely as a
/// naming aid — see that crate's doc comments on `StandardEquation::index_names`/
/// `ExprKind::Reduce::body_context`) when it has exactly `n` valid C identifiers; falls back to
/// synthesized `<prefix><0..n>` names otherwise. Either choice is semantically equivalent — the
/// actual C variable a dimension gets named is cosmetic, since every isl object this generator
/// prints from is explicitly renamed to match via `MultiAff::set_dim_name` before printing.
pub(crate) fn pick_names(hint: &[String], n: u32) -> Vec<String> {
    pick_names_prefixed(hint, n, "i")
}

pub(crate) fn pick_names_prefixed(hint: &[String], n: u32, prefix: &str) -> Vec<String> {
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
pub(crate) fn isl_bound_err(context: &str, e: isl::IslError) -> CodegenError {
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

pub(crate) fn rename_inputs(f: MultiAff, names: &[String]) -> Result<MultiAff> {
    let mut f = f;
    for (d, name) in names.iter().enumerate() {
        f = f.set_dim_name(DimType::In, d as u32, name)?;
    }
    Ok(f)
}

pub(crate) fn binary_c_op(op: &str) -> Option<&'static str> {
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

pub(crate) fn other_kind_name(k: &ir::ExprKind) -> &'static str {
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

pub(crate) fn gen_value<G: ExprGen>(g: &mut G, names: &[String], e: &ir::Expr) -> Result<CExpr> {
    match &*e.kind {
        ir::ExprKind::Variable(name) => Err(CodegenError::Unsupported(format!(
            "internal error: bare Variable('{name}') reached codegen outside a Dependence — \
             Normalize should have wrapped every Variable in an identity Dependence"
        ))),
        ir::ExprKind::Bool(b) => Ok(CExpr::Raw(if *b {
            "1.0f".to_string()
        } else {
            "0.0f".to_string()
        })),
        ir::ExprKind::Int(s) => Ok(CExpr::Raw(format!("((float)({s}))"))),
        ir::ExprKind::Real(s) => Ok(CExpr::Raw(format!("((float)({s}))"))),
        ir::ExprKind::Dependence { function, operand } => {
            gen_dependence(g, names, function, operand)
        }
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
            g.gen_reduce_value(names, operator, body_context, body)
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

pub(crate) fn gen_dependence<G: ExprGen>(
    g: &mut G,
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
            let info = g.variable(name.as_str()).ok_or_else(|| {
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
            let role = info.role;
            let call = g.render_read(name, role, &idx);
            Ok(CExpr::Raw(call))
        }
        other => Err(CodegenError::Unsupported(format!(
            "internal error: Dependence child is {} (Normalize guarantees Variable or a constant)",
            other_kind_name(other)
        ))),
    }
}

pub(crate) fn gen_index_function(names: &[String], function: &MultiAff) -> Result<CExpr> {
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

pub(crate) fn gen_case<G: ExprGen>(
    g: &mut G,
    names: &[String],
    branches: &[ir::Expr],
) -> Result<CExpr> {
    let Some((last, rest)) = branches.split_last() else {
        return Err(CodegenError::Unsupported(
            "case with zero branches".to_string(),
        ));
    };
    let mut result = gen_value(g, names, last)?;
    if rest.is_empty() {
        return Ok(result);
    }
    let build = ambient_build(g.ctx(), g.param_names(), names)?;
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

pub(crate) fn gen_binary<G: ExprGen>(
    g: &mut G,
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

pub(crate) fn gen_unary<G: ExprGen>(
    g: &mut G,
    names: &[String],
    operator: &str,
    operand: &ir::Expr,
) -> Result<CExpr> {
    let v = gen_value(g, names, operand)?;
    match operator {
        "-" => Ok(CExpr::Raw(format!("(-({v}))"))),
        "not" => Ok(CExpr::Raw(format!("(!({v}))"))),
        _ => Err(CodegenError::Unsupported(format!(
            "unknown unary operator '{operator}'"
        ))),
    }
}

pub(crate) fn gen_multi_arg<G: ExprGen>(
    g: &mut G,
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
            g.add_extern(name);
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
                "min" | "max" => {
                    Ok(iter.fold(first, |acc, v| CExpr::Call(op.clone(), vec![acc, v])))
                }
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
