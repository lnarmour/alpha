use crate::{Diagnostic, Domains, Resolver};
use alpha_syntax::ast::{self, AstNode, Equation, Expr};
use isl::{Map, MultiAff, Set};
use std::collections::{HashMap, HashSet};

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

struct LinearVariable {
    id: VariableId,
    name: String,
    domain: Set,
    is_output: bool,
    start: u32,
    end: u32,
}

struct ResourceUse {
    relation: Map,
    start: u32,
    end: u32,
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

fn mark_linear_references(
    resolver: &Resolver<'_>,
    expr: &Expr,
    blocked: &mut HashSet<VariableId>,
) {
    match expr {
        Expr::Variable(variable) => {
            if let Some(id) = variable.name().and_then(|name| resolver.variable_id(name.text())) {
                if resolver.variable_multiplicity(variable.name().unwrap().text())
                    == Some(Multiplicity::Linear)
                {
                    blocked.insert(id);
                }
            }
        }
        Expr::Dependence(dependence) => {
            if let Some(operand) = dependence.applied_expr() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Restrict(restrict) => {
            if let Some(operand) = restrict.expr() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::AutoRestrict(restrict) => {
            if let Some(operand) = restrict.expr() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Select(select) => {
            if let Some(operand) = select.expr() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Unary(unary) => {
            if let Some(operand) = unary.operand() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Paren(paren) => {
            if let Some(operand) = paren.inner() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Reduce(reduce) => {
            if let Some(body) = reduce.body() {
                mark_linear_references(resolver, &body, blocked);
            }
        }
        Expr::Convolution(convolution) => {
            for operand in [convolution.kernel_expr(), convolution.data_expr()]
                .into_iter()
                .flatten()
            {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Case(case) => {
            for branch in case.branches() {
                mark_linear_references(resolver, &branch, blocked);
            }
        }
        Expr::If(if_expr) => {
            for operand in [
                if_expr.cond(),
                if_expr.then_branch(),
                if_expr.else_branch(),
            ]
            .into_iter()
            .flatten()
            {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::MultiArg(call) => {
            for operand in call.args() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Binary(binary) => {
            for operand in [binary.lhs(), binary.rhs()].into_iter().flatten() {
                mark_linear_references(resolver, &operand, blocked);
            }
        }
        Expr::Index(_) | Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => {}
    }
}

fn identity_relation(domain: &Set) -> Result<Map, isl::IslError> {
    MultiAff::identity_on_domain_space(domain.space())?
        .into_map()?
        .intersect_domain(domain.clone())
}

fn collect_uses(
    resolver: &Resolver<'_>,
    expr: &Expr,
    relation: Map,
    context_names: &[String],
    contexts: &Domains,
    uses: &mut HashMap<VariableId, Vec<ResourceUse>>,
    blocked: &mut HashSet<VariableId>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Variable(variable) => {
            let Some(name) = variable.name() else {
                return Ok(());
            };
            if resolver.variable_multiplicity(name.text()) == Some(Multiplicity::Linear) {
                let id = resolver
                    .variable_id(name.text())
                    .expect("resolved linear variable has an ID");
                let (start, end) = range_of(variable.syntax());
                uses.entry(id).or_default().push(ResourceUse {
                    relation,
                    start,
                    end,
                });
            }
        }
        Expr::Dependence(dependence) => {
            let Some(operand) = dependence.applied_expr() else {
                return Ok(());
            };
            let Some(function) = dependence.function() else {
                return Ok(());
            };
            let mapped = resolver
                .eval_function(&function, context_names)
                .and_then(|function| {
                    function.into_map().map_err(|error| Diagnostic::IslError {
                        message: error.message,
                        start: range_of(dependence.syntax()).0,
                        end: range_of(dependence.syntax()).1,
                    })
                })
                .and_then(|function| {
                    relation
                        .apply_range(function)
                        .map_err(|error| Diagnostic::IslError {
                            message: error.message,
                            start: range_of(dependence.syntax()).0,
                            end: range_of(dependence.syntax()).1,
                        })
                })?;
            collect_uses(
                resolver,
                &operand,
                mapped,
                context_names,
                contexts,
                uses,
                blocked,
            )?;
        }
        Expr::Restrict(restrict) => {
            if let Some(operand) = restrict.expr() {
                let narrowed = match contexts.get(operand.syntax()) {
                    Some(context) => relation.intersect_range(context.clone()).map_err(|error| {
                        let (start, end) = range_of(restrict.syntax());
                        Diagnostic::IslError {
                            message: error.message,
                            start,
                            end,
                        }
                    })?,
                    None => relation,
                };
                collect_uses(
                    resolver,
                    &operand,
                    narrowed,
                    context_names,
                    contexts,
                    uses,
                    blocked,
                )?;
            }
        }
        Expr::AutoRestrict(restrict) => {
            if let Some(operand) = restrict.expr() {
                let narrowed = match contexts.get(restrict.syntax()) {
                    Some(context) => relation.intersect_range(context.clone()).map_err(|error| {
                        let (start, end) = range_of(restrict.syntax());
                        Diagnostic::IslError {
                            message: error.message,
                            start,
                            end,
                        }
                    })?,
                    None => relation,
                };
                collect_uses(
                    resolver,
                    &operand,
                    narrowed,
                    context_names,
                    contexts,
                    uses,
                    blocked,
                )?;
            }
        }
        Expr::Paren(paren) => {
            if let Some(operand) = paren.inner() {
                collect_uses(
                    resolver,
                    &operand,
                    relation,
                    context_names,
                    contexts,
                    uses,
                    blocked,
                )?;
            }
        }
        Expr::Case(case) => {
            for branch in case.branches() {
                let narrowed = match contexts.get(branch.syntax()) {
                    Some(context) => relation
                        .clone()
                        .intersect_range(context.clone())
                        .map_err(|error| {
                            let (start, end) = range_of(branch.syntax());
                            Diagnostic::IslError {
                                message: error.message,
                                start,
                                end,
                            }
                        })?,
                    None => relation.clone(),
                };
                collect_uses(
                    resolver,
                    &branch,
                    narrowed,
                    context_names,
                    contexts,
                    uses,
                    blocked,
                )?;
            }
        }
        Expr::If(_) => {
            if contains_linear_reference(resolver, expr) {
                mark_linear_references(resolver, expr, blocked);
            }
        }
        Expr::Unary(_) | Expr::Binary(_) | Expr::MultiArg(_) => {
            mark_linear_references(resolver, expr, blocked);
        }
        Expr::Reduce(_) | Expr::Select(_) | Expr::Convolution(_) => {
            mark_linear_references(resolver, expr, blocked);
        }
        Expr::Index(_) | Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => {}
    }
    Ok(())
}

fn check_resources(
    variables: &[LinearVariable],
    uses: &HashMap<VariableId, Vec<ResourceUse>>,
    blocked: &HashSet<VariableId>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for variable in variables {
        if blocked.contains(&variable.id) {
            continue;
        }
        let variable_uses = uses.get(&variable.id).map(Vec::as_slice).unwrap_or(&[]);
        for resource_use in variable_uses {
            if !resource_use.relation.is_injective().unwrap_or(false) {
                diagnostics.push(Diagnostic::LinearUseNotInjective {
                    variable: variable.name.clone(),
                    detail: resource_use.relation.to_string(),
                    start: resource_use.start,
                    end: resource_use.end,
                });
            }
        }
        for left in 0..variable_uses.len() {
            for right in left + 1..variable_uses.len() {
                let overlap = variable_uses[left]
                    .relation
                    .clone()
                    .range()
                    .and_then(|left_range| {
                        variable_uses[right]
                            .relation
                            .clone()
                            .range()
                            .and_then(|right_range| left_range.intersect(right_range))
                    });
                if let Ok(overlap) = overlap {
                    if !overlap.is_empty().unwrap_or(true) {
                        diagnostics.push(Diagnostic::LinearUsesOverlap {
                            variable: variable.name.clone(),
                            detail: overlap.to_string(),
                            start: variable_uses[right].start,
                            end: variable_uses[right].end,
                        });
                    }
                }
            }
        }
        let consumed = variable_uses
            .iter()
            .filter_map(|resource_use| resource_use.relation.clone().range().ok())
            .reduce(|left, right| left.union(right).unwrap_or_else(|_| variable.domain.clone()));
        let missing = match consumed {
            Some(consumed) => variable.domain.clone().subtract(consumed),
            None => Ok(variable.domain.clone()),
        };
        if let Ok(missing) = missing {
            if !missing.is_empty().unwrap_or(true) {
                diagnostics.push(Diagnostic::LinearValueUnconsumed {
                    variable: variable.name.clone(),
                    detail: missing.to_string(),
                    start: variable.start,
                    end: variable.end,
                });
            }
        }
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
    resolver: &mut Resolver<'_>,
    system: &ast::System,
    _domains: &Domains,
    contexts: &Domains,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut linear_variables = Vec::new();
    for (variables, is_output) in [
        (
            system
                .inputs()
                .map(|section| section.variables().collect::<Vec<_>>())
                .unwrap_or_default(),
            false,
        ),
        (
            system
                .outputs()
                .map(|section| section.variables().collect::<Vec<_>>())
                .unwrap_or_default(),
            true,
        ),
        (
            system
                .locals()
                .map(|section| section.variables().collect::<Vec<_>>())
                .unwrap_or_default(),
            false,
        ),
    ] {
        for variable in variables {
            let Some(name) = variable.name() else {
                continue;
            };
            if resolver.variable_multiplicity(name.text()) != Some(Multiplicity::Linear) {
                continue;
            }
            if let (Some(id), Ok(domain)) = (
                resolver.variable_id(name.text()),
                resolver.variable_domain(name.text()),
            ) {
                let (start, end) = range_of(variable.syntax());
                linear_variables.push(LinearVariable {
                    id,
                    name: name.text().to_string(),
                    domain,
                    is_output,
                    start,
                    end,
                });
            }
        }
    }

    let mut uses: HashMap<VariableId, Vec<ResourceUse>> = HashMap::new();
    let mut blocked = HashSet::new();
    for variable in &linear_variables {
        if variable.is_output {
            if let Ok(relation) = identity_relation(&variable.domain) {
                uses.entry(variable.id).or_default().push(ResourceUse {
                    relation,
                    start: variable.start,
                    end: variable.end,
                });
            }
        }
    }
    for body in system.bodies() {
        for equation in body.equations() {
            let Equation::Standard(equation) = equation else {
                if let Equation::Use(use_equation) = equation {
                    for expr in use_equation
                        .input_exprs()
                        .chain(use_equation.output_exprs())
                    {
                        if contains_linear_reference(resolver, &expr) {
                            let (start, end) = range_of(use_equation.syntax());
                            diagnostics.push(Diagnostic::LinearityUnsupportedHere {
                                construct: "use-equation".to_string(),
                                start,
                                end,
                            });
                            mark_linear_references(resolver, &expr, &mut blocked);
                        }
                    }
                }
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
            if let Some(context) = contexts.get(expr.syntax()) {
                match identity_relation(context) {
                    Ok(relation) => {
                        let context_names: Vec<String> = equation
                            .index_names()
                            .map(|name| name.text().to_string())
                            .collect();
                        if let Err(diagnostic) = collect_uses(
                            resolver,
                            &expr,
                            relation,
                            &context_names,
                            contexts,
                            &mut uses,
                            &mut blocked,
                        ) {
                            diagnostics.push(diagnostic);
                        }
                    }
                    Err(error) => {
                        let (start, end) = range_of(expr.syntax());
                        diagnostics.push(Diagnostic::IslError {
                            message: error.message,
                            start,
                            end,
                        });
                    }
                }
            }
        }
    }
    check_resources(&linear_variables, &uses, &blocked, &mut diagnostics);
    diagnostics
}