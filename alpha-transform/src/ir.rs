//! An owned, mutable "resolved AST" for `Normalize`/`NormalizeReduction` to rewrite — deliberately
//! distinct from `alpha_syntax::ast`'s lossless, immutable rowan tree.
//!
//! The source system's `Normalize` is a term-rewriting pass that
//! mutates a live EMF object graph in place: it replaces nodes (`EcoreUtil.replace`), reassigns
//! containment references, and incrementally recomputes attached domains as it goes. Rowan's CST
//! is a persistent, structurally-shared tree built specifically for the opposite property
//! (lossless, cheap incremental *parsing* edits) — it's the right tool for the syntax layer, but
//! not something a rewrite pass should mutate in place. So this module defines a plain, owned
//! Rust tree instead: every node is a normal (`Box`-recursive) value, expression/context domains
//! are plain fields (not looked up from a side-table keyed by syntax node), and a rewrite rule is
//! just an ordinary function `Expr -> Expr`.
//!
//! [`crate::lower`] builds one of these from an `alpha_syntax::ast::Root`/`System` plus the
//! `Domains` maps `alpha_model::domain::Resolver::analyze_system` already computed — no isl
//! re-computation happens here that phases 3–4 didn't already do; lowering only *transcribes* the
//! syntax tree plus its already-resolved domains/functions into this shape.
//!
//! Deliberately minimal: this crate does not attempt to track source spans/provenance back to the
//! original syntax tree (Normalize's whole point is that exact source shape no longer matters
//! once semantic analysis is done — the same "semantic equivalence, not bit-for-bit" codegen-
//! fidelity call `alpha-codegen`'s README documents applies just as much here). Diagnostics from
//! `alpha-model` (phases 1–6) already carry real spans from the syntax tree; nothing past that
//! point needs them.

use isl::{Map, MultiAff, PwQPolynomial, Set};

/// A `Function`/`ArrayFunction`-or-external-operator reference, shared by
/// `ReduceExpression`/`ArgReduceExpression` and `MultiArgExpression` — both can name either a
/// built-in operator token (`+`, `min`, ...) or an external function by (qualified) name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    /// A built-in operator's raw token text (`"+"`, `"min"`, `"and"`, ...).
    Named(String),
    /// An `external`-declared function's (possibly qualified) name.
    External(String),
}

/// One node of the resolved expression tree. Every node carries its expression domain; the
/// context domain is `None` exactly where `alpha_model::domain` doesn't compute one (inside a
/// `UseEquation`, or transitively beneath an unresolved `ConvolutionExpression` — see that
/// module's doc for both gaps).
#[derive(Clone)]
pub struct Expr {
    pub kind: Box<ExprKind>,
    pub expression_domain: Set,
    pub context_domain: Option<Set>,
}

impl Expr {
    pub fn new(kind: ExprKind, expression_domain: Set, context_domain: Option<Set>) -> Expr {
        Expr {
            kind: Box::new(kind),
            expression_domain,
            context_domain,
        }
    }

    /// Short tag naming this node's variant — cheap to compare, used by `Normalize`'s dispatch-by-
    /// child-shape rules (mirroring the source system's Xtend `dispatch def` overloads) and by
    /// tests asserting normal-form shape invariants without needing full `Debug`/`PartialEq` on
    /// isl objects.
    pub fn kind_tag(&self) -> &'static str {
        match &*self.kind {
            ExprKind::Variable(_) => "Variable",
            ExprKind::Bool(_) => "Bool",
            ExprKind::Int(_) => "Int",
            ExprKind::Real(_) => "Real",
            ExprKind::Dependence { .. } => "Dependence",
            ExprKind::IndexFunction { .. } => "IndexFunction",
            ExprKind::IndexPolynomial { .. } => "IndexPolynomial",
            ExprKind::If { .. } => "If",
            ExprKind::Restrict { .. } => "Restrict",
            ExprKind::AutoRestrict { .. } => "AutoRestrict",
            ExprKind::Case { .. } => "Case",
            ExprKind::Reduce { .. } => "Reduce",
            ExprKind::Convolution { .. } => "Convolution",
            ExprKind::Select { .. } => "Select",
            ExprKind::MultiArg { .. } => "MultiArg",
            ExprKind::Binary { .. } => "Binary",
            ExprKind::Unary { .. } => "Unary",
        }
    }
}

impl std::fmt::Debug for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Expr")
            .field("kind", &self.kind)
            .field("expression_domain", &self.expression_domain.to_string())
            .field(
                "context_domain",
                &self.context_domain.as_ref().map(|s| s.to_string()),
            )
            .finish()
    }
}

#[derive(Clone)]
pub enum ExprKind {
    Variable(String),
    Bool(bool),
    /// Raw token text — Normalize never evaluates literals, only rearranges them.
    Int(String),
    Real(String),

    /// `f @ E` / array notation (`X[E]`) — `function` maps the ambient index space to `operand`'s
    /// own. Normalize's dependence-pushdown rules are the bulk of what this crate ports.
    Dependence {
        function: MultiAff,
        operand: Expr,
    },

    /// `val(f)` — a function-valued index expression. Kept distinct from
    /// [`Self::IndexPolynomial`] because only this shape participates in the dependence-pullback
    /// rule (`f1 @ val(f2) -> val(f2 o f1)`) — the source system's `PolynomialIndexExpression` is
    /// a separate node kind with no such rule (see `lower.rs`'s doc for why the two `val(...)`
    /// shapes need different treatment here despite sharing one syntax-layer node kind).
    IndexFunction {
        function: MultiAff,
    },
    /// `val{...}` — a polynomial-valued index expression; opaque to Normalize (no rewrite rule
    /// touches it in the source system either).
    IndexPolynomial {
        polynomial: PwQPolynomial,
    },

    If {
        cond: Expr,
        then_branch: Expr,
        else_branch: Expr,
    },

    /// `D : E`. `domain` is already fully resolved (with the ambient index-name context already
    /// baked in — see `alpha_model::domain::Resolver::restrict_domain`), so rewrite rules can
    /// manipulate it with plain isl set algebra.
    Restrict {
        domain: Set,
        operand: Expr,
    },

    /// `auto : E`, always (after phase 4) a direct child of a [`Self::Case`] branch.
    AutoRestrict {
        operand: Expr,
    },

    /// `case { E1; E2; ... }`. `name` is `Some` for a named case (`case Name { ... }`) —
    /// `Normalize`'s shallow mode preserves named cases as a readability aid instead of flattening
    /// them (see this crate's module doc / the source system's own two-level "shallow vs. deep"
    /// normal form).
    Case {
        name: Option<String>,
        branches: Vec<Expr>,
    },

    /// `reduce`/`argreduce`. `projection` maps `body`'s own (larger) index space down to this
    /// node's — see `alpha_model::domain::Resolver::reduce_projection`. `body_context` is that
    /// same call's second return value: the *full* ambient-plus-new index-name list for `body`'s
    /// own index space, in order (ambient names first, this reduce's own new names appended) —
    /// carried through only for `alpha-codegen`'s benefit (`Normalize` itself never inspects
    /// names, only isl objects; see this module's doc for why the syntax layer's real source
    /// names otherwise don't survive into this IR). A rewrite rule that reconstructs a `Reduce`
    /// node must copy this field through unchanged; it never changes under rewriting.
    Reduce {
        is_arg_reduce: bool,
        operator: Operator,
        projection: MultiAff,
        body_context: Vec<String>,
        body: Expr,
    },

    /// `conv(kernel, weight, data)`. Note: `alpha_model::domain` doesn't compute this node's own
    /// expression/context domain yet (needs isl vertex-enumeration — see that module's doc), so
    /// lowering an equation containing one fails today (see `lower.rs`) — this variant exists for
    /// forward-compatibility once that gap closes, not because it's reachable yet.
    Convolution {
        kernel_domain: Set,
        kernel_expr: Expr,
        data_expr: Expr,
    },

    /// `select {relation} from E`. `relation` already has the ambient context baked into its
    /// domain side — see `Resolver::select_relation`.
    Select {
        relation: Map,
        operand: Expr,
    },

    /// A predefined-operator or external-function call over 2+ operands.
    MultiArg {
        operator: Operator,
        args: Vec<Expr>,
    },

    /// A predefined binary operator (`+`, `-`, `*`, `/`, relational/logical operators, `min`/
    /// `max`) — kept as raw token text; Normalize only ever copies/compares it, never interprets
    /// it (codegen is what maps it to a real C operator).
    Binary {
        operator: String,
        lhs: Expr,
        rhs: Expr,
    },

    /// A predefined unary operator (`-`, `not`).
    Unary {
        operator: String,
        operand: Expr,
    },
}

impl std::fmt::Debug for ExprKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprKind::Variable(name) => f.debug_tuple("Variable").field(name).finish(),
            ExprKind::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            ExprKind::Int(s) => f.debug_tuple("Int").field(s).finish(),
            ExprKind::Real(s) => f.debug_tuple("Real").field(s).finish(),
            ExprKind::Dependence { function, operand } => f
                .debug_struct("Dependence")
                .field("function", &function.to_string())
                .field("operand", operand)
                .finish(),
            ExprKind::IndexFunction { function } => f
                .debug_struct("IndexFunction")
                .field("function", &function.to_string())
                .finish(),
            ExprKind::IndexPolynomial { polynomial } => f
                .debug_struct("IndexPolynomial")
                .field("polynomial", &polynomial.to_string())
                .finish(),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => f
                .debug_struct("If")
                .field("cond", cond)
                .field("then_branch", then_branch)
                .field("else_branch", else_branch)
                .finish(),
            ExprKind::Restrict { domain, operand } => f
                .debug_struct("Restrict")
                .field("domain", &domain.to_string())
                .field("operand", operand)
                .finish(),
            ExprKind::AutoRestrict { operand } => f
                .debug_struct("AutoRestrict")
                .field("operand", operand)
                .finish(),
            ExprKind::Case { name, branches } => f
                .debug_struct("Case")
                .field("name", name)
                .field("branches", branches)
                .finish(),
            ExprKind::Reduce {
                is_arg_reduce,
                operator,
                projection,
                body_context,
                body,
            } => f
                .debug_struct("Reduce")
                .field("is_arg_reduce", is_arg_reduce)
                .field("operator", operator)
                .field("projection", &projection.to_string())
                .field("body_context", body_context)
                .field("body", body)
                .finish(),
            ExprKind::Convolution {
                kernel_domain,
                kernel_expr,
                data_expr,
            } => f
                .debug_struct("Convolution")
                .field("kernel_domain", &kernel_domain.to_string())
                .field("kernel_expr", kernel_expr)
                .field("data_expr", data_expr)
                .finish(),
            ExprKind::Select { relation, operand } => f
                .debug_struct("Select")
                .field("relation", &relation.to_string())
                .field("operand", operand)
                .finish(),
            ExprKind::MultiArg { operator, args } => f
                .debug_struct("MultiArg")
                .field("operator", operator)
                .field("args", args)
                .finish(),
            ExprKind::Binary { operator, lhs, rhs } => f
                .debug_struct("Binary")
                .field("operator", operator)
                .field("lhs", lhs)
                .field("rhs", rhs)
                .finish(),
            ExprKind::Unary { operator, operand } => f
                .debug_struct("Unary")
                .field("operator", operator)
                .field("operand", operand)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub struct Variable {
    pub name: String,
    pub domain: Set,
    pub multiplicity: alpha_model::Multiplicity,
    pub element_type: alpha_model::ElementType,
}

#[derive(Clone)]
pub enum Equation {
    Standard(StandardEquation),
    /// `UseEquation`s are carried through unchanged apart from normalizing their input/output
    /// expressions (matching the source system's own doc comment: "the same [normal-form rules]
    /// applies to each input expression in an UseEquation") — `Normalize` never restructures the
    /// equation itself, and `NormalizeReduction` explicitly skips `UseEquation`s outright (its own
    /// doc comment: reductions aren't expected in `UseEquation` inputs).
    Use(UseEquation),
}

#[derive(Clone)]
pub struct StandardEquation {
    pub variable: String,
    /// The equation's own declared ambient index-binder names (e.g. `i`/`j` in `U[i,j] = ...`),
    /// empty if the equation has no explicit index binder (e.g. `let Y = X;`) — carried through
    /// only for `alpha-codegen`'s benefit, see [`ExprKind::Reduce::body_context`]'s doc for why.
    pub index_names: Vec<String>,
    pub expr: Expr,
}

#[derive(Clone)]
pub struct UseEquation {
    pub callee: String,
    pub output_exprs: Vec<Expr>,
    pub input_exprs: Vec<Expr>,
}

#[derive(Clone)]
pub struct SystemBody {
    pub domain: Set,
    pub equations: Vec<Equation>,
}

/// A whole system, lowered and ready for `Normalize`/`NormalizeReduction`. `NormalizeReduction`
/// needs `locals` to be mutable (it adds a fresh local + equation per extracted reduction).
///
/// `Clone` is a cheap, deep-in-value-but-shallow-in-isl-refcount copy (every isl `Set`/`Map`/
/// `MultiAff` field clones via its own `_copy` — a refcount bump, not a real memory-duplicating
/// deep copy, per every `isl` wrapper type's own `Clone` impl) — this is exactly what
/// `alphalang`'s "transformations return a new copy, never mutate the input" contract (§5.2 of
/// `docs/scheduled-codegen-design.md`) needs: `alpha.normalize(sys)` clones `sys`'s underlying
/// `System` before running `normalize_reduction::apply`/`normalize::apply` on the clone, leaving
/// the original untouched.
#[derive(Clone)]
pub struct System {
    pub name: String,
    /// The system's own declared parameter domain (`[N]->{:N>0}`, or a bare `{:...}`) — see
    /// [`alpha_model::Resolver::param_domain`]. Carried through only for
    /// [`crate::print`]'s benefit (`Normalize`/`NormalizeReduction` never inspect it); every
    /// variable/body domain already has its own resolved constraints baked in independently, so
    /// nothing else in this crate depends on this field.
    pub parameter_domain: Set,
    pub inputs: Vec<Variable>,
    pub outputs: Vec<Variable>,
    pub locals: Vec<Variable>,
    pub bodies: Vec<SystemBody>,
}
