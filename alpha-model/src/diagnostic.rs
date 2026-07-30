//! The closed diagnostic catalog — Alpha's real "type errors". See
//! `docs/rust-port-design.md` §6/§10 in the workspace root: closed enum, matching the fixed
//! catalog the source project's `AlphaIssueFactory` already established. Variants are added as
//! the corresponding check is implemented, not speculatively ahead of one.
use alpha_syntax::SyntaxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// A syntax error from the parser, carried through unchanged so callers have one combined
    /// diagnostic stream instead of two.
    Syntax(SyntaxError),

    /// An isl operation failed (malformed domain/relation/function text, or a genuine isl-level
    /// error) — mirrors the source system's `UnexpectedISLErrorIssue`/`callISLwithErrorHandling`
    /// pattern (§6): every isl call site turns a native error into one of these instead of
    /// propagating a panic.
    IslError {
        message: String,
        start: u32,
        end: u32,
    },

    /// A calculator-expression operator was applied to an operand kind it isn't defined for
    /// (e.g. `domain` of a `Function`) — the calculator layer's "type error" (§6:
    /// `CalculatorExpressionEvaluator`'s dispatch-over-dynamic-type table).
    InvalidCalculatorOperand {
        operator: String,
        operand_kind: String,
        start: u32,
        end: u32,
    },

    /// A binary calculator operator was applied to two operand kinds it isn't defined for.
    InvalidCalculatorOperandPair {
        operator: String,
        left_kind: String,
        right_kind: String,
        start: u32,
        end: u32,
    },

    /// A calculator operator this port doesn't implement yet (see `value.rs`'s module doc for
    /// which ones, and why) — distinct from [`Self::InvalidCalculatorOperand`], which means the
    /// operator is defined by the language but not for *this* operand kind.
    UnsupportedCalculatorOp {
        operator: String,
        start: u32,
        end: u32,
    },

    /// A system, variable, or `define`d object references a name (via `DefinedObject`) that
    /// doesn't resolve to anything in scope.
    UndefinedReference { name: String, start: u32, end: u32 },

    /// A `define`d object's value depends on itself, directly or transitively (mirrors the
    /// source system's `CyclicDefinitionException`).
    CyclicDefinition { name: String, start: u32, end: u32 },
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Diagnostic::Syntax(e) => write!(f, "{}", e.message),
            Diagnostic::IslError { message, .. } => write!(f, "{message}"),
            Diagnostic::InvalidCalculatorOperand {
                operator,
                operand_kind,
                ..
            } => write!(f, "'{operator}' is not defined for a {operand_kind}"),
            Diagnostic::InvalidCalculatorOperandPair {
                operator,
                left_kind,
                right_kind,
                ..
            } => write!(
                f,
                "'{operator}' is not defined between a {left_kind} and a {right_kind}"
            ),
            Diagnostic::UnsupportedCalculatorOp { operator, .. } => {
                write!(f, "'{operator}' is not yet supported by this compiler")
            }
            Diagnostic::UndefinedReference { name, .. } => {
                write!(f, "'{name}' is not defined")
            }
            Diagnostic::CyclicDefinition { name, .. } => {
                write!(f, "'{name}' is defined in terms of itself")
            }
        }
    }
}

impl Diagnostic {
    pub fn range(&self) -> (u32, u32) {
        match self {
            Diagnostic::Syntax(e) => (e.start, e.end),
            Diagnostic::IslError { start, end, .. }
            | Diagnostic::InvalidCalculatorOperand { start, end, .. }
            | Diagnostic::InvalidCalculatorOperandPair { start, end, .. }
            | Diagnostic::UnsupportedCalculatorOp { start, end, .. }
            | Diagnostic::UndefinedReference { start, end, .. }
            | Diagnostic::CyclicDefinition { start, end, .. } => (*start, *end),
        }
    }
}
