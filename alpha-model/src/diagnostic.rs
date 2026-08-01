//! The closed diagnostic catalog — Alpha's real "type errors". A closed enum, matching the fixed
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

    /// A `DependenceExpression`'s function output arity doesn't match its operand's expression
    /// domain dimension (§6, phase 3: `outDependenceExpression`'s explicit arity check — the one
    /// case the source system diagnoses before ever calling into isl, rather than letting isl's
    /// own space-mismatch error surface as [`Self::IslError`]).
    IncompatibleContextAndExpressionDomain { start: u32, end: u32 },

    /// An `AutoRestrictExpression` appears somewhere other than a direct child of a
    /// `CaseExpression` — the only place the source system allows it (§6, phase 4).
    AutoRestrictNotInCase { start: u32, end: u32 },

    /// More than one `AutoRestrictExpression` appears as a direct child of the same
    /// `CaseExpression` — at most one `else`-like branch is allowed per case.
    MultipleAutoRestrict { start: u32, end: u32 },

    /// An `AutoRestrictExpression`'s inferred domain (parent context minus the other branches'
    /// domains) came out empty — the `else` branch is unreachable. A warning in the source
    /// system (computation continues); this port treats it as a hard error for now, consistent
    /// with every other diagnostic here failing the containing equation's analysis rather than
    /// silently continuing with a partial result.
    EmptyAutoRestrict { start: u32, end: u32 },

    /// More than one `SystemBody` in a system lacks a `when` guard — at most one implicit
    /// `else` body is allowed (§6, phase 4: `completeSystemBody`'s syntactic check).
    MultipleUnrestrictedSystemBody { start: u32, end: u32 },

    /// A `RestrictExpression`'s explicit-tuple domain (`{[x,y]:...}`) has a different number of
    /// dimensions than the ambient index-name context it would replace (`inRestrictExpression`'s
    /// "only when the dimensions match the context, new indices can replace the context" rule).
    RestrictDomainDimensionMismatch { start: u32, end: u32 },

    /// A `SelectExpression`'s relation has a domain-side dimension count that doesn't match the
    /// ambient index-name context (the same rule as
    /// [`Self::RestrictDomainDimensionMismatch`], for `inSelectExpression`).
    SelectRelationDimensionMismatch { start: u32, end: u32 },

    /// Two systems (possibly in different files) share the same fully-qualified name (§6, phase
    /// 5: `AlphaNameUniquenessChecker.check`'s `systemNameMap`).
    DuplicateSystem { name: String, start: u32, end: u32 },

    /// Two `external` function declarations share the same fully-qualified name.
    DuplicateExternalFunction { name: String, start: u32, end: u32 },

    /// A variable (or `FuzzyVariable`, which shares this diagnostic in the source system too)
    /// and/or a `define`d object share the same name within one system — they're one namespace.
    DuplicateVariable { name: String, start: u32, end: u32 },

    /// Two `define`d objects (or a `define`d object and a variable — see
    /// [`Self::DuplicateVariable`]) share the same name within one system.
    DuplicatePolyhedralObject { name: String, start: u32, end: u32 },

    /// A `StandardEquation` defines a variable that another equation in the same `SystemBody`
    /// also defines. `UseEquation`s writing to the same variable are fine on their own (their
    /// domains only need to be disjoint, checked elsewhere) — this only fires when at least one
    /// of the conflicting definitions is a `StandardEquation`.
    DuplicateStandardEquation { name: String, start: u32, end: u32 },

    /// A `UseEquation` writes to a variable that a `StandardEquation` (or another `UseEquation`,
    /// alongside at least one `StandardEquation`) in the same `SystemBody` also defines — see
    /// [`Self::DuplicateStandardEquation`].
    DuplicateUseEquation { name: String, start: u32, end: u32 },

    /// Two `constant` declarations visible to the same system share the same name.
    DuplicateAlphaConstant { name: String, start: u32, end: u32 },

    /// A `SystemBody`'s own parameter domain (its `when` guard, or the inferred `else` domain)
    /// came out empty — a warning in the source system (`emptySystemBody`), kept as an error here
    /// for consistency with every other diagnostic in this port (§6, phase 6).
    EmptySystemBody { start: u32, end: u32 },

    /// Two or more `SystemBody`s in a system have overlapping parameter domains — every parameter
    /// value must select exactly one body (§6, phase 6: `checkSystemBodyConsistency`).
    OverlappingSystemBodies {
        detail: String,
        start: u32,
        end: u32,
    },

    /// A system's `SystemBody`s don't jointly cover its whole parameter domain — `detail` is the
    /// missing region.
    IncompleteSystem {
        name: String,
        detail: String,
        start: u32,
        end: u32,
    },

    /// A `StandardEquation`'s expression domain doesn't cover its variable's declared domain
    /// (intersected with the enclosing `SystemBody`'s parameter domain) — `domain_detail` is the
    /// undefined region (gisted against the variable's own context), `param_detail` the same
    /// region reduced to just the offending parameter values.
    IncompleteEquation {
        name: String,
        domain_detail: String,
        param_detail: String,
        start: u32,
        end: u32,
    },

    /// Two branches of a `CaseExpression` have overlapping context domains — every point must be
    /// covered by exactly one branch (`detail` is the overlapping region).
    OverlappingCaseBranch {
        detail: String,
        start: u32,
        end: u32,
    },

    /// A `ReduceExpression`'s body ranges over an index whose bounds isl can't establish as
    /// finite in both directions — the reduction wouldn't terminate.
    UnboundedReductionBody { start: u32, end: u32 },

    /// A `UseEquation` calls its own enclosing system with call parameters that are the identity
    /// function on the caller's own parameters — unconditional infinite recursion.
    InfinitelyRecursiveUseEquation { start: u32, end: u32 },

    /// Two or more `UseEquation`s in the same `SystemBody` write overlapping regions of the same
    /// output variable — their instantiation domains must be disjoint.
    OverlappingUseEquations {
        name: String,
        detail: String,
        start: u32,
        end: u32,
    },

    /// The `UseEquation`s targeting a variable in a `SystemBody` don't jointly cover its whole
    /// domain — `detail` is the missing region.
    IncompleteUseEquation {
        name: String,
        detail: String,
        start: u32,
        end: u32,
    },

    /// An output variable (or a local variable that's referenced somewhere) has no defining
    /// equation — neither a `StandardEquation` nor a `UseEquation` output — in a `SystemBody`.
    UndefinedVariable { name: String, start: u32, end: u32 },
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
            Diagnostic::IncompatibleContextAndExpressionDomain { .. } => {
                write!(
                    f,
                    "the context domain and expression domain are incompatible"
                )
            }
            Diagnostic::AutoRestrictNotInCase { .. } => write!(
                f,
                "AutoRestrict can only be a direct child of a CaseExpression"
            ),
            Diagnostic::MultipleAutoRestrict { .. } => write!(
                f,
                "more than one AutoRestrict is not allowed in a CaseExpression"
            ),
            Diagnostic::EmptyAutoRestrict { .. } => {
                write!(f, "the inferred AutoRestrict domain is empty")
            }
            Diagnostic::MultipleUnrestrictedSystemBody { .. } => write!(
                f,
                "at most one SystemBody can be free (without a when clause)"
            ),
            Diagnostic::RestrictDomainDimensionMismatch { .. } => write!(
                f,
                "dimensionality of the restrict domain does not match its context"
            ),
            Diagnostic::SelectRelationDimensionMismatch { .. } => write!(
                f,
                "dimensionality of the select relation does not match its context"
            ),
            Diagnostic::DuplicateSystem { name, .. } => {
                write!(f, "duplicate system '{name}'")
            }
            Diagnostic::DuplicateExternalFunction { name, .. } => {
                write!(f, "duplicate external function '{name}'")
            }
            Diagnostic::DuplicateVariable { name, .. } => {
                write!(f, "duplicate name '{name}'")
            }
            Diagnostic::DuplicatePolyhedralObject { name, .. } => {
                write!(f, "duplicate name '{name}'")
            }
            Diagnostic::DuplicateStandardEquation { name, .. } => write!(
                f,
                "this equation defines '{name}', which is already defined by another equation"
            ),
            Diagnostic::DuplicateUseEquation { name, .. } => write!(
                f,
                "this equation defines '{name}', which is already defined by another equation"
            ),
            Diagnostic::DuplicateAlphaConstant { name, .. } => {
                write!(f, "duplicate constant '{name}'")
            }
            Diagnostic::EmptySystemBody { .. } => {
                write!(f, "the inferred SystemBody domain is empty")
            }
            Diagnostic::OverlappingSystemBodies { detail, .. } => write!(
                f,
                "the SystemBodies define overlapping domains of the output: {detail}"
            ),
            Diagnostic::IncompleteSystem { name, detail, .. } => write!(
                f,
                "SystemBodies for {name} do not cover the entire parameter domain; missing: {detail}"
            ),
            Diagnostic::IncompleteEquation {
                name,
                domain_detail,
                param_detail,
                ..
            } => write!(
                f,
                "equation for {name} is not defined with parameters {param_detail} for {domain_detail}"
            ),
            Diagnostic::OverlappingCaseBranch { detail, .. } => write!(
                f,
                "context domains of case branches overlap: {detail}"
            ),
            Diagnostic::UnboundedReductionBody { .. } => {
                write!(f, "the expression has an unbounded reduction body")
            }
            Diagnostic::InfinitelyRecursiveUseEquation { .. } => write!(
                f,
                "self-recursion with an identity call parameter is prohibited (infinite recursion)"
            ),
            Diagnostic::OverlappingUseEquations { name, detail, .. } => write!(
                f,
                "the UseEquations defining {name} overlap: {detail}"
            ),
            Diagnostic::IncompleteUseEquation { name, detail, .. } => write!(
                f,
                "the UseEquations for {name} do not define the variable for {detail}"
            ),
            Diagnostic::UndefinedVariable { name, .. } => {
                write!(f, "'{name}' is used but not defined in this SystemBody")
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
            | Diagnostic::CyclicDefinition { start, end, .. }
            | Diagnostic::IncompatibleContextAndExpressionDomain { start, end, .. }
            | Diagnostic::AutoRestrictNotInCase { start, end, .. }
            | Diagnostic::MultipleAutoRestrict { start, end, .. }
            | Diagnostic::EmptyAutoRestrict { start, end, .. }
            | Diagnostic::MultipleUnrestrictedSystemBody { start, end, .. }
            | Diagnostic::RestrictDomainDimensionMismatch { start, end, .. }
            | Diagnostic::SelectRelationDimensionMismatch { start, end, .. }
            | Diagnostic::DuplicateSystem { start, end, .. }
            | Diagnostic::DuplicateExternalFunction { start, end, .. }
            | Diagnostic::DuplicateVariable { start, end, .. }
            | Diagnostic::DuplicatePolyhedralObject { start, end, .. }
            | Diagnostic::DuplicateStandardEquation { start, end, .. }
            | Diagnostic::DuplicateUseEquation { start, end, .. }
            | Diagnostic::DuplicateAlphaConstant { start, end, .. }
            | Diagnostic::EmptySystemBody { start, end, .. }
            | Diagnostic::OverlappingSystemBodies { start, end, .. }
            | Diagnostic::IncompleteSystem { start, end, .. }
            | Diagnostic::IncompleteEquation { start, end, .. }
            | Diagnostic::OverlappingCaseBranch { start, end, .. }
            | Diagnostic::UnboundedReductionBody { start, end, .. }
            | Diagnostic::InfinitelyRecursiveUseEquation { start, end, .. }
            | Diagnostic::OverlappingUseEquations { start, end, .. }
            | Diagnostic::IncompleteUseEquation { start, end, .. }
            | Diagnostic::UndefinedVariable { start, end, .. } => (*start, *end),
        }
    }
}
