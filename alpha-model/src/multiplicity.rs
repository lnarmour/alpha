use crate::{Diagnostic, Domains, Resolver};
use alpha_syntax::ast::{self, AstNode, Equation, Expr};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Multiplicity {
    Linear,
    #[default]
    Unrestricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VariableId(u32);

impl VariableId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index.try_into().expect("too many variables in one system"))
    }
}

fn range_of(node: &alpha_syntax::syntax_kind::SyntaxNode) -> (u32, u32) {
    let range = node.text_range();
    (range.start().into(), range.end().into())
}

fn contains_linear_reference(resolver: &Resolver<'_>, expr: &Expr) -> bool {
    match expr {
        Expr::Variable(variable) => variable
            .name()
            .and_then(|name| resolver.variable_multiplicity(name.text()))
            == Some(Multiplicity::Linear),
        Expr::Dependence(dependence) => dependence
            .applied_expr()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Restrict(restrict) => restrict
            .expr()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::AutoRestrict(restrict) => restrict
            .expr()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Select(select) => select
            .expr()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Unary(unary) => unary
            .operand()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Paren(paren) => paren
            .inner()
            .is_some_and(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Reduce(reduce) => reduce
            .body()
            .is_some_and(|body| contains_linear_reference(resolver, &body)),
        Expr::Convolution(convolution) => [
            convolution.kernel_expr(),
            convolution.data_expr(),
        ]
        .into_iter()
        .flatten()
        .any(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Case(case) => case
            .branches()
            .any(|branch| contains_linear_reference(resolver, &branch)),
        Expr::If(if_expr) => [
            if_expr.cond(),
            if_expr.then_branch(),
            if_expr.else_branch(),
        ]
        .into_iter()
        .flatten()
        .any(|operand| contains_linear_reference(resolver, &operand)),
        Expr::MultiArg(call) => call
            .args()
            .any(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Binary(binary) => [binary.lhs(), binary.rhs()]
            .into_iter()
            .flatten()
            .any(|operand| contains_linear_reference(resolver, &operand)),
        Expr::Index(_) | Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => false,
    }
}

fn unsupported_if_linear(
    resolver: &Resolver<'_>,
    expr: &Expr,
    construct: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if contains_linear_reference(resolver, expr) {
        let (start, end) = range_of(expr.syntax());
        diagnostics.push(Diagnostic::LinearityUnsupportedHere {
            construct: construct.to_string(),
            start,
            end,
        });
    }
}

fn expression_multiplicity(
    resolver: &Resolver<'_>,
    expr: &Expr,
    diagnostics: &mut Vec<Diagnostic>,
) -> Multiplicity {
    match expr {
        Expr::Variable(variable) => variable
            .name()
            .and_then(|name| resolver.variable_multiplicity(name.text()))
            .unwrap_or_default(),
        Expr::Dependence(dependence) => dependence
            .applied_expr()
            .map(|operand| expression_multiplicity(resolver, &operand, diagnostics))
            .unwrap_or_default(),
        Expr::Restrict(restrict) => restrict
            .expr()
            .map(|operand| expression_multiplicity(resolver, &operand, diagnostics))
            .unwrap_or_default(),
        Expr::AutoRestrict(restrict) => restrict
            .expr()
            .map(|operand| expression_multiplicity(resolver, &operand, diagnostics))
            .unwrap_or_default(),
        Expr::Paren(paren) => paren
            .inner()
            .map(|operand| expression_multiplicity(resolver, &operand, diagnostics))
            .unwrap_or_default(),
        Expr::Case(case) => case
            .branches()
            .map(|branch| expression_multiplicity(resolver, &branch, diagnostics))
            .find(|multiplicity| *multiplicity == Multiplicity::Linear)
            .unwrap_or_default(),
        Expr::If(if_expr) => [if_expr.then_branch(), if_expr.else_branch()]
            .into_iter()
            .flatten()
            .map(|branch| expression_multiplicity(resolver, &branch, diagnostics))
            .find(|multiplicity| *multiplicity == Multiplicity::Linear)
            .unwrap_or_default(),
        Expr::Unary(unary) => {
            let operand = unary
                .operand()
                .map(|operand| expression_multiplicity(resolver, &operand, diagnostics))
                .unwrap_or_default();
            if operand == Multiplicity::Linear {
                let (start, end) = range_of(unary.syntax());
                diagnostics.push(Diagnostic::LinearArgumentToUnrestrictedPort {
                    operator: unary
                        .operator()
                        .map(|operator| operator.text().to_string())
                        .unwrap_or_default(),
                    start,
                    end,
                });
            }
            Multiplicity::Unrestricted
        }
        Expr::Binary(binary) => {
            let operator = binary
                .operator()
                .map(|operator| operator.text().to_string())
                .unwrap_or_default();
            for operand in [binary.lhs(), binary.rhs()].into_iter().flatten() {
                if expression_multiplicity(resolver, &operand, diagnostics)
                    == Multiplicity::Linear
                {
                    let (start, end) = range_of(operand.syntax());
                    diagnostics.push(Diagnostic::LinearArgumentToUnrestrictedPort {
                        operator: operator.clone(),
                        start,
                        end,
                    });
                }
            }
            Multiplicity::Unrestricted
        }
        Expr::MultiArg(call) => {
            let operator = call
                .named_operator()
                .map(|operator| operator.text().to_string())
                .or_else(|| {
                    call.external_function().map(|name| {
                        name.segments()
                            .map(|segment| segment.text().to_string())
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                })
                .unwrap_or_default();
            for operand in call.args() {
                if expression_multiplicity(resolver, &operand, diagnostics)
                    == Multiplicity::Linear
                {
                    let (start, end) = range_of(operand.syntax());
                    diagnostics.push(Diagnostic::LinearArgumentToUnrestrictedPort {
                        operator: operator.clone(),
                        start,
                        end,
                    });
                }
            }
            Multiplicity::Unrestricted
        }
        Expr::Reduce(_) => {
            unsupported_if_linear(resolver, expr, "reduce", diagnostics);
            Multiplicity::Unrestricted
        }
        Expr::Select(_) => {
            unsupported_if_linear(resolver, expr, "select", diagnostics);
            Multiplicity::Unrestricted
        }
        Expr::Convolution(_) => {
            unsupported_if_linear(resolver, expr, "convolution", diagnostics);
            Multiplicity::Unrestricted
        }
        _ => Multiplicity::Unrestricted,
    }
}

pub fn check_system(
    resolver: &Resolver<'_>,
    system: &ast::System,
    _domains: &Domains,
    _contexts: &Domains,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for body in system.bodies() {
        for equation in body.equations() {
            let Equation::Standard(equation) = equation else {
                continue;
            };
            let Some(target) = equation.variable_name() else {
                continue;
            };
            let Some(expr) = equation.expr() else {
                continue;
            };
            if expression_multiplicity(resolver, &expr, &mut diagnostics) == Multiplicity::Linear
                && resolver.variable_multiplicity(target.text()) == Some(Multiplicity::Unrestricted)
            {
                let (start, end) = range_of(equation.syntax());
                diagnostics.push(Diagnostic::LinearValueWidened {
                    target: target.text().to_string(),
                    start,
                    end,
                });
            }
        }
    }
    diagnostics
}