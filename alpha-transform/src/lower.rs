//! Lowers an analyzed `alpha_syntax::ast::System` (plus the `Domains` maps
//! `alpha_model::domain::Resolver::analyze_system` already computed) into this crate's owned
//! [`crate::ir`] tree. See that module's doc for why lowering exists at all (Normalize needs a
//! mutable tree; rowan's CST isn't one).
//!
//! Deliberately *transcribes*, never *re-derives*: every isl object a node needs (a
//! `DependenceExpression`'s function, a `ReduceExpression`'s projection, ...) is obtained by
//! calling the exact same `alpha_model::domain::Resolver` methods phases 3–4 already used to
//! compute domains (now `pub` for this reason — see that module), threading the ambient
//! index-name context identically. This is a deliberate choice to never risk the two passes
//! disagreeing about what a node's function/context is — a real risk given how many
//! fixture-discovered subtleties phase 3/4 needed (see `domain.rs`'s module doc).
//!
//! **`IndexExpression` shape note**: the syntax layer collapses `val(f)` (function-valued) and
//! `val{...}` (polynomial-valued) into one `INDEX_EXPR` node kind (see `alpha-syntax`'s
//! `syntax_kind.rs`), but the source system's `Normalize` only has a rewrite rule for the
//! function-valued shape (`f1 @ val(f2) -> val(f2 o f1)` dispatches on `IndexExpression`
//! specifically — `PolynomialIndexExpression` is a different EMF class with no such overload, so
//! it silently falls through to source's no-op default). This module preserves that distinction
//! via [`crate::ir::ExprKind::IndexFunction`] vs. [`crate::ir::ExprKind::IndexPolynomial`].
//!
//! **Equations that don't lower**: an equation whose expression domain didn't fully resolve
//! (`ConvolutionExpression`'s own domain — a documented gap in `alpha_model::domain` — or a fuzzy
//! feature, both of which already produce a phase-3 `Diagnostic`) is skipped rather than lowered
//! partially; [`lower_system`] collects one [`Diagnostic`] per skipped equation and still lowers
//! everything else in the system.

use crate::ir;
use alpha_model::domain::Domains;
use alpha_model::value::Value;
use alpha_model::{Diagnostic, Resolver};
use alpha_syntax::ast::{self, AstNode, CalcExpr, Equation, Expr};
use alpha_syntax::syntax_kind::SyntaxNode;
use isl::Set;

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn missing_domain(node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: "internal error: node has no precomputed expression domain (lowering must run \
                   after a successful `Resolver::analyze_system`)"
            .to_string(),
        start,
        end,
    }
}

fn lookup(domains: &Domains, node: &SyntaxNode) -> Result<Set, Diagnostic> {
    domains
        .get(node)
        .cloned()
        .ok_or_else(|| missing_domain(node))
}

/// Lowers a whole system: its inputs/outputs/locals, and every `SystemBody`'s equations.
/// Equations that fail to lower (see the module doc) are skipped, each contributing one
/// diagnostic to the returned list; everything else in the system still lowers.
pub fn lower_system(
    resolver: &mut Resolver,
    system: &ast::System,
) -> Result<(ir::System, Vec<Diagnostic>), Diagnostic> {
    let mut diagnostics = Vec::new();

    // `Inputs`/`Outputs`/`Locals` are distinct syntax-node types with identical shape (see
    // `alpha-syntax`'s `ast.rs`), so each section is lowered inline rather than through one
    // shared helper (a generic helper would need a trait purely to unify three otherwise-identical
    // loops — not worth it for three call sites).
    let inputs = {
        let mut out = Vec::new();
        if let Some(s) = system.inputs() {
            for v in s.variables() {
                let Some(name) = v.name() else { continue };
                let domain = resolver.variable_domain(name.text())?;
                out.push(ir::Variable {
                    name: name.text().to_string(),
                    domain,
                    multiplicity: resolver
                        .variable_multiplicity(name.text())
                        .expect("lowered variable was registered by the resolver"),
                });
            }
        }
        out
    };
    let outputs = {
        let mut out = Vec::new();
        if let Some(s) = system.outputs() {
            for v in s.variables() {
                let Some(name) = v.name() else { continue };
                let domain = resolver.variable_domain(name.text())?;
                out.push(ir::Variable {
                    name: name.text().to_string(),
                    domain,
                    multiplicity: resolver
                        .variable_multiplicity(name.text())
                        .expect("lowered variable was registered by the resolver"),
                });
            }
        }
        out
    };
    let locals = {
        let mut out = Vec::new();
        if let Some(s) = system.locals() {
            for v in s.variables() {
                let Some(name) = v.name() else { continue };
                let domain = resolver.variable_domain(name.text())?;
                out.push(ir::Variable {
                    name: name.text().to_string(),
                    domain,
                    multiplicity: resolver
                        .variable_multiplicity(name.text())
                        .expect("lowered variable was registered by the resolver"),
                });
            }
        }
        out
    };

    let mut bodies = Vec::new();
    for body in system.bodies() {
        let domain = resolver.system_body_domain(&body)?;
        let mut equations = Vec::new();
        for eq in body.equations() {
            match lower_equation(resolver, &eq, &body) {
                Ok(lowered) => equations.push(lowered),
                Err(d) => diagnostics.push(d),
            }
        }
        bodies.push(ir::SystemBody { domain, equations });
    }

    let name = system
        .name()
        .map(|t| t.text().to_string())
        .unwrap_or_default();
    let parameter_domain = resolver.param_domain()?;
    Ok((
        ir::System {
            name,
            parameter_domain,
            inputs,
            outputs,
            locals,
            bodies,
        },
        diagnostics,
    ))
}

fn lower_equation(
    resolver: &mut Resolver,
    eq: &Equation,
    body: &ast::SystemBody,
) -> Result<ir::Equation, Diagnostic> {
    match eq {
        Equation::Standard(s) => {
            let context: Vec<String> = s.index_names().map(|t| t.text().to_string()).collect();
            let mut domains = Domains::new();
            let mut contexts = Domains::new();
            let ast_expr = s.expr().ok_or_else(|| missing_domain(s.syntax()))?;
            resolver.expression_domain(&ast_expr, &context, &mut domains)?;
            let Some(var_name) = s.variable_name() else {
                return Err(missing_domain(s.syntax()));
            };
            resolver.equation_context_domains(s, body, &domains, &mut contexts)?;
            let expr = lower_expr(resolver, &ast_expr, &context, &domains, &contexts)?;
            Ok(ir::Equation::Standard(ir::StandardEquation {
                variable: var_name.text().to_string(),
                index_names: context,
                expr,
            }))
        }
        Equation::Use(u) => {
            let context = alpha_model::domain::use_equation_context(u);
            let mut domains = Domains::new();
            let mut output_exprs = Vec::new();
            let mut input_exprs = Vec::new();
            for e in u.output_exprs() {
                resolver.expression_domain(&e, &context, &mut domains)?;
                output_exprs.push(lower_expr(
                    resolver,
                    &e,
                    &context,
                    &domains,
                    &Domains::new(),
                )?);
            }
            for e in u.input_exprs() {
                resolver.expression_domain(&e, &context, &mut domains)?;
                input_exprs.push(lower_expr(
                    resolver,
                    &e,
                    &context,
                    &domains,
                    &Domains::new(),
                )?);
            }
            let callee = u
                .callee()
                .map(|qn| {
                    qn.segments()
                        .map(|t| t.text().to_string())
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .unwrap_or_default();
            Ok(ir::Equation::Use(ir::UseEquation {
                callee,
                output_exprs,
                input_exprs,
            }))
        }
    }
}

fn lower_expr(
    resolver: &mut Resolver,
    expr: &Expr,
    context: &[String],
    domains: &Domains,
    contexts: &Domains,
) -> Result<ir::Expr, Diagnostic> {
    // `Paren` is purely syntactic (kept only for lossless round-tripping); it has no semantic
    // node in the source system's grammar either, so lowering unwraps it transparently.
    if let Expr::Paren(p) = expr {
        let inner = p.inner().ok_or_else(|| missing_domain(p.syntax()))?;
        return lower_expr(resolver, &inner, context, domains, contexts);
    }

    let expression_domain = lookup(domains, expr.syntax())?;
    let context_domain = contexts.get(expr.syntax()).cloned();

    let kind = match expr {
        Expr::Paren(_) => unreachable!("handled above"),
        Expr::Bool(b) => ir::ExprKind::Bool(b.value().unwrap_or(false)),
        Expr::Int(i) => ir::ExprKind::Int(i.text()),
        Expr::Real(r) => ir::ExprKind::Real(r.text()),
        Expr::Variable(v) => {
            ir::ExprKind::Variable(v.name().map(|t| t.text().to_string()).unwrap_or_default())
        }
        Expr::Binary(b) => {
            let lhs_ast = b.lhs().ok_or_else(|| missing_domain(b.syntax()))?;
            let rhs_ast = b.rhs().ok_or_else(|| missing_domain(b.syntax()))?;
            let lhs = lower_expr(resolver, &lhs_ast, context, domains, contexts)?;
            let rhs = lower_expr(resolver, &rhs_ast, context, domains, contexts)?;
            let operator = b
                .operator()
                .map(|t| t.text().to_string())
                .unwrap_or_default();
            ir::ExprKind::Binary { operator, lhs, rhs }
        }
        Expr::Unary(u) => {
            let operand_ast = u.operand().ok_or_else(|| missing_domain(u.syntax()))?;
            let operand = lower_expr(resolver, &operand_ast, context, domains, contexts)?;
            let operator = u
                .operator()
                .map(|t| t.text().to_string())
                .unwrap_or_default();
            ir::ExprKind::Unary { operator, operand }
        }
        Expr::MultiArg(m) => {
            let operator = if let Some(t) = m.named_operator() {
                ir::Operator::Named(t.text().to_string())
            } else if let Some(qn) = m.external_function() {
                ir::Operator::External(
                    qn.segments()
                        .map(|t| t.text().to_string())
                        .collect::<Vec<_>>()
                        .join("."),
                )
            } else {
                ir::Operator::Named(String::new())
            };
            let mut args = Vec::new();
            for a in m.args() {
                args.push(lower_expr(resolver, &a, context, domains, contexts)?);
            }
            ir::ExprKind::MultiArg { operator, args }
        }
        Expr::Case(c) => {
            let name = c.name().map(|t| t.text().to_string());
            let mut branches = Vec::new();
            for b in c.branches() {
                branches.push(lower_expr(resolver, &b, context, domains, contexts)?);
            }
            ir::ExprKind::Case { name, branches }
        }
        Expr::AutoRestrict(a) => {
            let operand_ast = a.expr().ok_or_else(|| missing_domain(a.syntax()))?;
            let operand = lower_expr(resolver, &operand_ast, context, domains, contexts)?;
            ir::ExprKind::AutoRestrict { operand }
        }
        Expr::If(i) => {
            let cond_ast = i.cond().ok_or_else(|| missing_domain(i.syntax()))?;
            let then_ast = i.then_branch().ok_or_else(|| missing_domain(i.syntax()))?;
            let else_ast = i.else_branch().ok_or_else(|| missing_domain(i.syntax()))?;
            let cond = lower_expr(resolver, &cond_ast, context, domains, contexts)?;
            let then_branch = lower_expr(resolver, &then_ast, context, domains, contexts)?;
            let else_branch = lower_expr(resolver, &else_ast, context, domains, contexts)?;
            ir::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            }
        }
        Expr::Restrict(r) => {
            let (domain, extended) = resolver.restrict_domain(r, context)?;
            let operand_ast = r.expr().ok_or_else(|| missing_domain(r.syntax()))?;
            let operand = lower_expr(resolver, &operand_ast, &extended, domains, contexts)?;
            ir::ExprKind::Restrict { domain, operand }
        }
        Expr::Dependence(d) => {
            let operand_ast = d.applied_expr().ok_or_else(|| missing_domain(d.syntax()))?;
            let operand = lower_expr(resolver, &operand_ast, context, domains, contexts)?;
            let f = d.function().ok_or_else(|| missing_domain(d.syntax()))?;
            let function = resolver.eval_function(&f, context)?;
            ir::ExprKind::Dependence { function, operand }
        }
        Expr::Index(ie) => {
            let src = ie.source().ok_or_else(|| missing_domain(ie.syntax()))?;
            match &src {
                CalcExpr::Function(_) | CalcExpr::ArrayFunction(_) => {
                    let function = resolver.eval_function(&src, context)?;
                    ir::ExprKind::IndexFunction { function }
                }
                CalcExpr::Polynomial(_) => match resolver.eval_calc_expr(&src)? {
                    Value::Polynomial(p) => ir::ExprKind::IndexPolynomial { polynomial: p },
                    _ => return Err(missing_domain(ie.syntax())),
                },
                CalcExpr::ArrayPolynomial(ap) => {
                    let polynomial = resolver.eval_polynomial_in_context(ap, context)?;
                    ir::ExprKind::IndexPolynomial { polynomial }
                }
                _ => return Err(missing_domain(ie.syntax())),
            }
        }
        Expr::Reduce(r) => {
            let (projection, body_context) = resolver.reduce_projection(r, context)?;
            let body_ast = r.body().ok_or_else(|| missing_domain(r.syntax()))?;
            let body = lower_expr(resolver, &body_ast, &body_context, domains, contexts)?;
            let operator = if let Some(t) = r.named_operator() {
                ir::Operator::Named(t.text().to_string())
            } else if let Some(qn) = r.external_operator() {
                ir::Operator::External(
                    qn.segments()
                        .map(|t| t.text().to_string())
                        .collect::<Vec<_>>()
                        .join("."),
                )
            } else {
                ir::Operator::Named(String::new())
            };
            ir::ExprKind::Reduce {
                is_arg_reduce: r.is_arg_reduce(),
                operator,
                projection,
                body_context,
                body,
            }
        }
        Expr::Select(s) => {
            let (relation, extended) = resolver.select_relation(s, context)?;
            let operand_ast = s.expr().ok_or_else(|| missing_domain(s.syntax()))?;
            let operand = lower_expr(resolver, &operand_ast, &extended, domains, contexts)?;
            ir::ExprKind::Select { relation, operand }
        }
        Expr::Convolution(c) => {
            let extended = resolver.convolution_kernel_names(c, context)?;
            let kernel_ast = c.kernel_expr().ok_or_else(|| missing_domain(c.syntax()))?;
            let data_ast = c.data_expr().ok_or_else(|| missing_domain(c.syntax()))?;
            let kernel_expr = lower_expr(resolver, &kernel_ast, &extended, domains, contexts)?;
            let data_expr = lower_expr(resolver, &data_ast, &extended, domains, contexts)?;
            let kernel_domain = match c.kernel_domain() {
                Some(calc) => match resolver.eval_calc_expr(&calc)? {
                    Value::Set(s) => s,
                    _ => return Err(missing_domain(c.syntax())),
                },
                None => return Err(missing_domain(c.syntax())),
            };
            ir::ExprKind::Convolution {
                kernel_domain,
                kernel_expr,
                data_expr,
            }
        }
    };

    Ok(ir::Expr::new(kind, expression_domain, context_domain))
}
