use crate::error::{CodegenError, Result};
use crate::{legality, schedule, stmt};
use alpha_transform::ir;
use isl::{
    AstBuild, AstExpr, AstExprKind, AstNode, AstNodeKind, AstOpType, Context, Set, UnionMap,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatementId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVar(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexExpr {
    Constant(i64),
    Variable(String),
    Add(Box<IndexExpr>, Box<IndexExpr>),
    Sub(Box<IndexExpr>, Box<IndexExpr>),
    Mul(Box<IndexExpr>, Box<IndexExpr>),
    Div(Box<IndexExpr>, Box<IndexExpr>),
    FloorDiv(Box<IndexExpr>, Box<IndexExpr>),
    CeilDiv(Box<IndexExpr>, Box<IndexExpr>),
    Mod(Box<IndexExpr>, Box<IndexExpr>),
    Min(Vec<IndexExpr>),
    Max(Vec<IndexExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Le,
    Lt,
    Ge,
    Gt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Compare {
        op: CompareOp,
        lhs: IndexExpr,
        rhs: IndexExpr,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
    Constant(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledNode {
    Loop {
        iterator: IndexVar,
        init: IndexExpr,
        condition: Predicate,
        step: IndexExpr,
        body: Box<ScheduledNode>,
    },
    If {
        condition: Predicate,
        then_body: Box<ScheduledNode>,
        else_body: Option<Box<ScheduledNode>>,
    },
    Sequence(Vec<ScheduledNode>),
    Invoke {
        statement: StatementId,
        indices: Vec<IndexExpr>,
    },
}

pub struct ScheduledProgram<'a> {
    pub system: &'a ir::System,
    pub root: ScheduledNode,
    pub schedule: UnionMap,
    pub statements: Vec<stmt::Statement<'a>>,
}

pub fn build<'a>(system: &'a ir::System, schedule_text: &str) -> Result<ScheduledProgram<'a>> {
    let ctx = first_body_ctx(system)?;
    let statements = stmt::statements(system)?;
    let schedule = schedule::build_schedule(&ctx, &statements, schedule_text)?;
    legality::check_legality(&statements, &schedule)?;

    let build = AstBuild::from_context(params_only_domain(&ctx, &statements)?)?;
    let ast = build.generate(schedule.clone())?;
    let statement_ids: HashMap<_, _> = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement.name.as_str(), StatementId(index)))
        .collect();
    let root = convert_node(&ast, &statement_ids)?;
    Ok(ScheduledProgram {
        system,
        root,
        schedule,
        statements,
    })
}

fn first_body_ctx(system: &ir::System) -> Result<Context> {
    system
        .bodies
        .first()
        .map(|body| body.domain.ctx())
        .ok_or_else(|| {
            CodegenError::Unsupported("system has no bodies to generate code for".to_string())
        })
}

fn params_only_domain(ctx: &Context, statements: &[stmt::Statement<'_>]) -> Result<Set> {
    let mut statements = statements.iter();
    let Some(first) = statements.next() else {
        return Ok(Set::read_from_str(ctx, "{ : }")?);
    };
    let mut domain = first.domain.clone().params()?;
    for statement in statements {
        domain = domain.union(statement.domain.clone().params()?)?;
    }
    Ok(domain)
}

fn convert_node(
    node: &AstNode,
    statement_ids: &HashMap<&str, StatementId>,
) -> Result<ScheduledNode> {
    match node.kind() {
        AstNodeKind::For { .. } => Ok(ScheduledNode::Loop {
            iterator: IndexVar(node.for_iterator()?.id_name()?),
            init: convert_index(&node.for_init()?)?,
            condition: convert_predicate(&node.for_cond()?)?,
            step: convert_index(&node.for_inc()?)?,
            body: Box::new(convert_node(&node.for_body()?, statement_ids)?),
        }),
        AstNodeKind::If => Ok(ScheduledNode::If {
            condition: convert_predicate(&node.if_cond()?)?,
            then_body: Box::new(convert_node(&node.if_then()?, statement_ids)?),
            else_body: node
                .if_else()
                .map(|node| convert_node(&node, statement_ids).map(Box::new))
                .transpose()?,
        }),
        AstNodeKind::Block => Ok(ScheduledNode::Sequence(
            node.block_children()?
                .iter()
                .map(|child| convert_node(child, statement_ids))
                .collect::<Result<Vec<_>>>()?,
        )),
        AstNodeKind::User => {
            let arguments = node.user_expr()?.op_args()?;
            let Some((name, indices)) = arguments.split_first() else {
                return Err(CodegenError::Unsupported(
                    "internal error: scheduled invocation has no statement name".to_string(),
                ));
            };
            let name = name.id_name()?;
            let statement = statement_ids.get(name.as_str()).copied().ok_or_else(|| {
                CodegenError::Unsupported(format!(
                    "internal error: scheduled invocation names unknown statement '{name}'"
                ))
            })?;
            Ok(ScheduledNode::Invoke {
                statement,
                indices: indices
                    .iter()
                    .map(convert_index)
                    .collect::<Result<Vec<_>>>()?,
            })
        }
        AstNodeKind::Other => Err(CodegenError::Unsupported(
            "unexpected ISL AST node kind".to_string(),
        )),
    }
}

fn binary(arguments: &[AstExpr]) -> Result<(IndexExpr, IndexExpr)> {
    if let [lhs, rhs] = arguments {
        Ok((convert_index(lhs)?, convert_index(rhs)?))
    } else {
        Err(CodegenError::Unsupported(format!(
            "expected binary ISL AST operator, found {} arguments",
            arguments.len()
        )))
    }
}

fn convert_index(expression: &AstExpr) -> Result<IndexExpr> {
    match expression.kind() {
        AstExprKind::Int => Ok(IndexExpr::Constant(expression.int_value()?)),
        AstExprKind::Id => Ok(IndexExpr::Variable(expression.id_name()?)),
        AstExprKind::Op => {
            let arguments = expression.op_args()?;
            let operation = expression.op_type();
            let result = match operation {
                AstOpType::isl_ast_expr_op_minus => {
                    let [argument] = arguments.as_slice() else {
                        return Err(CodegenError::Unsupported(
                            "unary minus requires one argument".to_string(),
                        ));
                    };
                    IndexExpr::Sub(
                        Box::new(IndexExpr::Constant(0)),
                        Box::new(convert_index(argument)?),
                    )
                }
                AstOpType::isl_ast_expr_op_add => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::Add(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_sub => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::Sub(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_mul => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::Mul(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_div | AstOpType::isl_ast_expr_op_pdiv_q => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::Div(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_fdiv_q => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::FloorDiv(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_pdiv_r | AstOpType::isl_ast_expr_op_zdiv_r => {
                    let (lhs, rhs) = binary(&arguments)?;
                    IndexExpr::Mod(Box::new(lhs), Box::new(rhs))
                }
                AstOpType::isl_ast_expr_op_min => IndexExpr::Min(
                    arguments
                        .iter()
                        .map(convert_index)
                        .collect::<Result<Vec<_>>>()?,
                ),
                AstOpType::isl_ast_expr_op_max => IndexExpr::Max(
                    arguments
                        .iter()
                        .map(convert_index)
                        .collect::<Result<Vec<_>>>()?,
                ),
                _ => {
                    return Err(CodegenError::Unsupported(format!(
                        "ISL AST operator {operation} is not an index expression"
                    )))
                }
            };
            Ok(result)
        }
    }
}

fn convert_predicate(expression: &AstExpr) -> Result<Predicate> {
    if expression.kind() == AstExprKind::Int {
        return Ok(Predicate::Constant(expression.int_value()? != 0));
    }
    if expression.kind() != AstExprKind::Op {
        return Err(CodegenError::Unsupported(
            "ISL predicate must be a comparison or Boolean expression".to_string(),
        ));
    }
    let arguments = expression.op_args()?;
    match expression.op_type() {
        AstOpType::isl_ast_expr_op_eq
        | AstOpType::isl_ast_expr_op_le
        | AstOpType::isl_ast_expr_op_lt
        | AstOpType::isl_ast_expr_op_ge
        | AstOpType::isl_ast_expr_op_gt => {
            let (lhs, rhs) = binary(&arguments)?;
            let op = match expression.op_type() {
                AstOpType::isl_ast_expr_op_eq => CompareOp::Eq,
                AstOpType::isl_ast_expr_op_le => CompareOp::Le,
                AstOpType::isl_ast_expr_op_lt => CompareOp::Lt,
                AstOpType::isl_ast_expr_op_ge => CompareOp::Ge,
                AstOpType::isl_ast_expr_op_gt => CompareOp::Gt,
                _ => unreachable!(),
            };
            Ok(Predicate::Compare { op, lhs, rhs })
        }
        AstOpType::isl_ast_expr_op_and | AstOpType::isl_ast_expr_op_and_then => Ok(Predicate::And(
            arguments
                .iter()
                .map(convert_predicate)
                .collect::<Result<Vec<_>>>()?,
        )),
        AstOpType::isl_ast_expr_op_or | AstOpType::isl_ast_expr_op_or_else => Ok(Predicate::Or(
            arguments
                .iter()
                .map(convert_predicate)
                .collect::<Result<Vec<_>>>()?,
        )),
        operation => Err(CodegenError::Unsupported(format!(
            "ISL AST operator {operation} is not a predicate"
        ))),
    }
}
