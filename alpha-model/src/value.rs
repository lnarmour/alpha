//! The calculator layer's dynamic type system: every `CalculatorExpression` evaluates to a
//! [`Value`] tagged `Set`/`Map`/`Function`/`Polynomial` (`POLY_OBJECT_TYPE` in the source Java),
//! and unary/binary calculator operators are only defined for certain operand-kind combinations
//! — checked here at evaluation time, not statically.
//!
//! Deliberately partial: the well-understood, clearly-specified core operators (union/subtract/
//! intersect, domain/range, the hulls, intersect/subtract-range) are implemented against real
//! isl calls. `cross` (`flatProduct`) is only implemented for the `Map`×`Map` case the source
//! system's own naming hint (`cross(flatRangeProduct)`) documents unambiguously; `Set`×`Set`
//! cross product has no equally unambiguous isl equivalent in the bound header set, so it
//! reports [`Diagnostic::UnsupportedCalculatorOp`] rather than guessing. Expand as real programs
//! need it, rather than speculatively now.

use crate::diagnostic::Diagnostic;
use alpha_syntax::ast::{AstNode, BinaryCalcExpr, UnaryCalcExpr};
use alpha_syntax::syntax_kind::SyntaxKind;
use isl::{Map, MultiAff, PwQPolynomial, Set};

#[derive(Clone)]
pub enum Value {
    Set(Set),
    Map(Map),
    Function(MultiAff),
    Polynomial(PwQPolynomial),
}

impl Value {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Set(_) => "set",
            Value::Map(_) => "relation",
            Value::Function(_) => "function",
            Value::Polynomial(_) => "polynomial",
        }
    }
}

fn range_of(node: &alpha_syntax::syntax_kind::SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn isl_err(e: isl::IslError, node: &alpha_syntax::syntax_kind::SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: e.message,
        start,
        end,
    }
}

pub fn eval_unary(op: &UnaryCalcExpr, operand: Value) -> Result<Value, Diagnostic> {
    let operator_text = op
        .operator()
        .map(|t| t.text().to_string())
        .unwrap_or_default();
    let node = op.syntax();
    let kind = op
        .operator()
        .map(|t| t.kind())
        .unwrap_or(SyntaxKind::KW_DOMAIN);
    let invalid = |operand: &Value| Diagnostic::InvalidCalculatorOperand {
        operator: operator_text.clone(),
        operand_kind: operand.kind_name().to_string(),
        start: range_of(node).0,
        end: range_of(node).1,
    };
    match (kind, operand) {
        (SyntaxKind::KW_DOMAIN, Value::Map(m)) => {
            Ok(Value::Set(m.domain().map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::KW_RANGE, Value::Map(m)) => {
            Ok(Value::Set(m.range().map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::KW_COMPLEMENT, Value::Set(s)) => {
            Ok(Value::Set(s.complement().map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::KW_AFFINE_HULL, Value::Set(s)) => Ok(Value::Set(
            s.affine_hull().map_err(|e| isl_err(e, node))?.into_set(),
        )),
        (SyntaxKind::KW_POLY_HULL, Value::Set(s)) => Ok(Value::Set(
            s.polyhedral_hull()
                .map_err(|e| isl_err(e, node))?
                .into_set(),
        )),
        (SyntaxKind::KW_REVERSE, Value::Map(m)) => {
            Ok(Value::Map(m.reverse().map_err(|e| isl_err(e, node))?))
        }
        (_, operand) => Err(invalid(&operand)),
    }
}

pub fn eval_binary(op: &BinaryCalcExpr, lhs: Value, rhs: Value) -> Result<Value, Diagnostic> {
    let operator_text = op
        .operator()
        .map(|t| t.text().to_string())
        .unwrap_or_default();
    let node = op.syntax();
    let kind = op.operator().map(|t| t.kind()).unwrap_or(SyntaxKind::PLUS);
    let invalid_pair = |l: &Value, r: &Value| Diagnostic::InvalidCalculatorOperandPair {
        operator: operator_text.clone(),
        left_kind: l.kind_name().to_string(),
        right_kind: r.kind_name().to_string(),
        start: range_of(node).0,
        end: range_of(node).1,
    };
    match (kind, lhs, rhs) {
        (SyntaxKind::PLUS, Value::Set(l), Value::Set(r)) => {
            Ok(Value::Set(l.union(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::PLUS, Value::Map(l), Value::Map(r)) => {
            Ok(Value::Map(l.union(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::MINUS, Value::Set(l), Value::Set(r)) => {
            Ok(Value::Set(l.subtract(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::MINUS, Value::Map(l), Value::Map(r)) => {
            Ok(Value::Map(l.subtract(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::STAR, Value::Set(l), Value::Set(r)) => {
            Ok(Value::Set(l.intersect(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::STAR, Value::Map(l), Value::Map(r)) => {
            Ok(Value::Map(l.intersect(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::KW_INTERSECT_RANGE, Value::Map(l), Value::Set(r)) => Ok(Value::Map(
            l.intersect_range(r).map_err(|e| isl_err(e, node))?,
        )),
        (SyntaxKind::KW_SUBTRACT_RANGE, Value::Map(l), Value::Set(r)) => Ok(Value::Map(
            l.subtract_range(r).map_err(|e| isl_err(e, node))?,
        )),
        // `@` (pullback/composition): `f @ g` in Alpha's calculator algebra composes two
        // relations. isl's `apply_range` computes exactly this graph composition.
        (SyntaxKind::AT, Value::Map(l), Value::Map(r)) => {
            Ok(Value::Map(l.apply_range(r).map_err(|e| isl_err(e, node))?))
        }
        (SyntaxKind::KW_CROSS, Value::Map(l), Value::Map(r)) => Ok(Value::Map(
            l.flat_range_product(r).map_err(|e| isl_err(e, node))?,
        )),
        (_, l, r) => {
            if matches!(kind, SyntaxKind::KW_CROSS) {
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: operator_text.clone(),
                    start: range_of(node).0,
                    end: range_of(node).1,
                })
            } else {
                Err(invalid_pair(&l, &r))
            }
        }
    }
}
