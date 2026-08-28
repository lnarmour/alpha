//! Typed accessors over the lossless CST (`syntax_kind::SyntaxNode`), in the spirit of
//! rust-analyzer's `syntax::ast` — every type here is a thin, `Copy`-cheap wrapper around a
//! `SyntaxNode`/`SyntaxToken` that just knows how to find its own meaningful children. No
//! semantic information (name resolution, resolved ISL domains, ...) lives here — that's
//! `alpha-model`'s job, once it exists; this layer only knows about *syntax*.
//!
//! Naming strips Eclipse/Java/JNI artifacts and keeps Alpha's/the polyhedral model's own
//! vocabulary verbatim: the redundant `Alpha` prefix is dropped (`AlphaRoot` → `Root`,
//! `AlphaSystem` → `System`, `AlphaExpression` → `Expr`), and the `JNI*` prefix is dropped from
//! the calculator-layer types (`JNIDomain` → `Domain`,
//! etc.) — see `syntax_kind.rs` for where each CST node kind maps back to a source-grammar rule.

use crate::syntax_kind::{SyntaxKind as K, SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

fn child<N: AstNode>(node: &SyntaxNode) -> Option<N> {
    node.children().find_map(N::cast)
}

fn children<N: AstNode>(node: &SyntaxNode) -> impl Iterator<Item = N> {
    node.children().filter_map(N::cast)
}

fn token(node: &SyntaxNode, kind: K) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|t| t.kind() == kind)
}

fn tokens(node: &SyntaxNode, kind: K) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.children_with_tokens()
        .filter_map(|it| it.into_token())
        .filter(move |t| t.kind() == kind)
}

/// The `ID` naming a construct — by convention, direct-child `IDENT` tokens that aren't
/// `QualifiedName`s of their own (systems, variables, packages-by-segment, etc. all just have a
/// single bare `IDENT` child for their name).
fn name(node: &SyntaxNode) -> Option<SyntaxToken> {
    token(node, K::IDENT)
}

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl AstNode for $name {
            fn cast(node: SyntaxNode) -> Option<Self> {
                if node.kind() == K::$kind {
                    Some(Self(node))
                } else {
                    None
                }
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

// --- top-level structure ---

ast_node!(Root, ROOT);
ast_node!(Import, IMPORT);
ast_node!(AlphaConstant, ALPHA_CONSTANT);
ast_node!(ExternalFunction, EXTERNAL_FUNCTION);
ast_node!(AlphaPackage, ALPHA_PACKAGE);
ast_node!(QualifiedName, QUALIFIED_NAME);

impl Root {
    pub fn imports(&self) -> impl Iterator<Item = Import> {
        children(&self.0)
    }
    pub fn systems(&self) -> impl Iterator<Item = System> {
        children(&self.0)
    }
    pub fn constants(&self) -> impl Iterator<Item = AlphaConstant> {
        children(&self.0)
    }
    pub fn external_functions(&self) -> impl Iterator<Item = ExternalFunction> {
        children(&self.0)
    }
    pub fn packages(&self) -> impl Iterator<Item = AlphaPackage> {
        children(&self.0)
    }
}

impl Import {
    pub fn qualified_name(&self) -> Option<QualifiedName> {
        child(&self.0)
    }
    /// True if the import ends in the `.*` wildcard suffix.
    pub fn is_wildcard(&self) -> bool {
        token(&self.0, K::DOT_STAR).is_some()
    }
}

impl QualifiedName {
    pub fn segments(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        tokens(&self.0, K::IDENT)
    }
}

impl AlphaConstant {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn value(&self) -> Option<SyntaxToken> {
        token(&self.0, K::INT_NUMBER)
    }
}

impl ExternalFunction {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn cardinality(&self) -> Option<SyntaxToken> {
        token(&self.0, K::INT_NUMBER)
    }
    pub fn input_multiplicities(&self) -> impl Iterator<Item = Multiplicity> + '_ {
        self.0
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .take_while(|token| token.kind() != K::ARROW)
            .filter_map(|token| Multiplicity::from_kind(token.kind()))
    }
    pub fn output_multiplicities(&self) -> impl Iterator<Item = Multiplicity> + '_ {
        self.0
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .skip_while(|token| token.kind() != K::ARROW)
            .skip(1)
            .filter_map(|token| Multiplicity::from_kind(token.kind()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Multiplicity {
    Linear,
    Unrestricted,
}

impl Multiplicity {
    fn from_kind(kind: K) -> Option<Self> {
        match kind {
            K::KW_LINEAR => Some(Self::Linear),
            K::KW_UNRESTRICTED => Some(Self::Unrestricted),
            _ => None,
        }
    }
}

impl AlphaPackage {
    pub fn qualified_name(&self) -> Option<QualifiedName> {
        child(&self.0)
    }
    pub fn systems(&self) -> impl Iterator<Item = System> {
        children(&self.0)
    }
    pub fn constants(&self) -> impl Iterator<Item = AlphaConstant> {
        children(&self.0)
    }
    pub fn external_functions(&self) -> impl Iterator<Item = ExternalFunction> {
        children(&self.0)
    }
    pub fn packages(&self) -> impl Iterator<Item = AlphaPackage> {
        children(&self.0)
    }
}

// --- systems ---

ast_node!(System, SYSTEM);
ast_node!(DefineSection, DEFINE_SECTION);
ast_node!(PolyhedralObject, POLYHEDRAL_OBJECT);
ast_node!(Inputs, INPUTS);
ast_node!(Outputs, OUTPUTS);
ast_node!(Locals, LOCALS);
ast_node!(Variable, VARIABLE);
ast_node!(FuzzyVariable, FUZZY_VARIABLE);
ast_node!(SystemBody, SYSTEM_BODY);

impl System {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn param_domain(&self) -> Option<ParamDomain> {
        child(&self.0)
    }
    pub fn define_section(&self) -> Option<DefineSection> {
        child(&self.0)
    }
    pub fn inputs(&self) -> Option<Inputs> {
        child(&self.0)
    }
    pub fn outputs(&self) -> Option<Outputs> {
        child(&self.0)
    }
    pub fn locals(&self) -> Option<Locals> {
        child(&self.0)
    }
    /// The `over <calc-expr> while (<expr>)` clause's calculator-expression half, if present.
    /// Structurally indistinguishable from `define_section`'s objects by kind alone, so callers
    /// needing both should use `child`-style lookups directly rather than relying on ordering.
    pub fn while_domain(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    pub fn test_expr(&self) -> Option<Expr> {
        child(&self.0)
    }
    pub fn bodies(&self) -> impl Iterator<Item = SystemBody> {
        children(&self.0)
    }
}

impl DefineSection {
    pub fn objects(&self) -> impl Iterator<Item = PolyhedralObject> {
        children(&self.0)
    }
}

impl PolyhedralObject {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn expr(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
}

impl Inputs {
    pub fn variables(&self) -> impl Iterator<Item = Variable> {
        children(&self.0)
    }
    pub fn fuzzy_variables(&self) -> impl Iterator<Item = FuzzyVariable> {
        children(&self.0)
    }
}
impl Outputs {
    pub fn variables(&self) -> impl Iterator<Item = Variable> {
        children(&self.0)
    }
    pub fn fuzzy_variables(&self) -> impl Iterator<Item = FuzzyVariable> {
        children(&self.0)
    }
}
impl Locals {
    pub fn variables(&self) -> impl Iterator<Item = Variable> {
        children(&self.0)
    }
    pub fn fuzzy_variables(&self) -> impl Iterator<Item = FuzzyVariable> {
        children(&self.0)
    }
}

impl Variable {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn is_linear(&self) -> bool {
        token(&self.0, K::KW_LINEAR).is_some()
    }
    /// `None` for a bare name in a comma-separated list (e.g. the `A`, `B` in `inputs A, B :
    /// [N]`) — semantic analysis inherits the domain from the next sibling that has one, exactly
    /// as the source system's `JNIDomainCalculator.resolveVariableDeclaration` does.
    pub fn domain(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
}

impl FuzzyVariable {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn domain(&self) -> Option<CalcExpr> {
        children(&self.0).next()
    }
    pub fn range(&self) -> Option<CalcExpr> {
        children(&self.0).nth(1)
    }
}

impl SystemBody {
    /// The `when {...}` guard's domain, if this body has one. `None` for both `else` bodies and
    /// completely unguarded bodies — distinguish those with [`Self::is_else`].
    pub fn when_domain(&self) -> Option<ArrayDomain> {
        child(&self.0)
    }
    pub fn is_else(&self) -> bool {
        token(&self.0, K::KW_ELSE).is_some()
    }
    pub fn equations(&self) -> impl Iterator<Item = Equation> {
        self.0.children().filter_map(Equation::cast)
    }
}

// --- equations ---

ast_node!(StandardEquation, STANDARD_EQUATION);
ast_node!(UseEquation, USE_EQUATION);

/// `Equation: StandardEquation | UseEquation`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Equation {
    Standard(StandardEquation),
    Use(UseEquation),
}

impl AstNode for Equation {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            K::STANDARD_EQUATION => StandardEquation::cast(node).map(Equation::Standard),
            K::USE_EQUATION => UseEquation::cast(node).map(Equation::Use),
            _ => None,
        }
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Equation::Standard(it) => it.syntax(),
            Equation::Use(it) => it.syntax(),
        }
    }
}

impl StandardEquation {
    pub fn variable_name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    /// The optional `[i,j]` array-notation index names on the left-hand side.
    pub fn index_names(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        tokens(&self.0, K::IDENT).skip(1)
    }
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl UseEquation {
    /// The `over <calc-expr>` instantiation domain, if present.
    pub fn instantiation_domain(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    /// The `with [i,j]` subsystem dimension names, if present. Distinguishing "no `with`
    /// clause" from "`with` with an empty `[]`" isn't needed structurally — both yield no names.
    pub fn subsystem_dims(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        tokens(&self.0, K::IDENT)
    }
    pub fn output_exprs(&self) -> impl Iterator<Item = Expr> {
        // Output exprs are every `Expr` child before `callee()`'s `QUALIFIED_NAME`; since a
        // `QualifiedName` never itself contains `Expr`-kinded children, splitting on it isn't
        // needed structurally: input exprs come after the callee `QualifiedName` and
        // `ArrayFunction`, output exprs before. Cheapest correct split: take until we hit the
        // callee's `QualifiedName` node among direct children.
        self.0
            .children()
            .take_while(|n| n.kind() != K::QUALIFIED_NAME)
            .filter_map(Expr::cast)
    }
    pub fn callee(&self) -> Option<QualifiedName> {
        child(&self.0)
    }
    pub fn call_params(&self) -> Option<ArrayFunction> {
        child(&self.0)
    }
    pub fn input_exprs(&self) -> impl Iterator<Item = Expr> {
        self.0
            .children()
            .skip_while(|n| n.kind() != K::QUALIFIED_NAME)
            .filter_map(Expr::cast)
    }
}

// --- expressions ---

ast_node!(IfExpr, IF_EXPR);
ast_node!(RestrictExpr, RESTRICT_EXPR);
ast_node!(AutoRestrictExpr, AUTO_RESTRICT_EXPR);
ast_node!(CaseExpr, CASE_EXPR);
ast_node!(VariableExpr, VARIABLE_EXPR);
ast_node!(DependenceExpr, DEPENDENCE_EXPR);
ast_node!(IndexExpr, INDEX_EXPR);
ast_node!(ReduceExpr, REDUCE_EXPR);
ast_node!(ConvolutionExpr, CONVOLUTION_EXPR);
ast_node!(SelectExpr, SELECT_EXPR);
ast_node!(MultiArgExpr, MULTI_ARG_EXPR);
ast_node!(BinaryExpr, BINARY_EXPR);
ast_node!(UnaryExpr, UNARY_EXPR);
ast_node!(ParenExpr, PAREN_EXPR);
ast_node!(BoolLit, BOOL_LIT);
ast_node!(IntLit, INT_LIT);
ast_node!(RealLit, REAL_LIT);

/// `AlphaExpression`. See `syntax_kind.rs`'s module doc for which source-grammar alternatives
/// (the 8 reduce/argreduce variants, the 3 index-flavored expressions, plain vs. fuzzy
/// dependence) share a single variant here, disambiguated by the variant's own accessors instead
/// of by a dedicated node kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    If(IfExpr),
    Restrict(RestrictExpr),
    AutoRestrict(AutoRestrictExpr),
    Case(CaseExpr),
    Variable(VariableExpr),
    Dependence(DependenceExpr),
    Index(IndexExpr),
    Reduce(ReduceExpr),
    Convolution(ConvolutionExpr),
    Select(SelectExpr),
    MultiArg(MultiArgExpr),
    Binary(BinaryExpr),
    Unary(UnaryExpr),
    Paren(ParenExpr),
    Bool(BoolLit),
    Int(IntLit),
    Real(RealLit),
}

impl AstNode for Expr {
    fn cast(node: SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            K::IF_EXPR => Expr::If(IfExpr(node)),
            K::RESTRICT_EXPR => Expr::Restrict(RestrictExpr(node)),
            K::AUTO_RESTRICT_EXPR => Expr::AutoRestrict(AutoRestrictExpr(node)),
            K::CASE_EXPR => Expr::Case(CaseExpr(node)),
            K::VARIABLE_EXPR => Expr::Variable(VariableExpr(node)),
            K::DEPENDENCE_EXPR => Expr::Dependence(DependenceExpr(node)),
            K::INDEX_EXPR => Expr::Index(IndexExpr(node)),
            K::REDUCE_EXPR => Expr::Reduce(ReduceExpr(node)),
            K::CONVOLUTION_EXPR => Expr::Convolution(ConvolutionExpr(node)),
            K::SELECT_EXPR => Expr::Select(SelectExpr(node)),
            K::MULTI_ARG_EXPR => Expr::MultiArg(MultiArgExpr(node)),
            K::BINARY_EXPR => Expr::Binary(BinaryExpr(node)),
            K::UNARY_EXPR => Expr::Unary(UnaryExpr(node)),
            K::PAREN_EXPR => Expr::Paren(ParenExpr(node)),
            K::BOOL_LIT => Expr::Bool(BoolLit(node)),
            K::INT_LIT => Expr::Int(IntLit(node)),
            K::REAL_LIT => Expr::Real(RealLit(node)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Expr::If(it) => it.syntax(),
            Expr::Restrict(it) => it.syntax(),
            Expr::AutoRestrict(it) => it.syntax(),
            Expr::Case(it) => it.syntax(),
            Expr::Variable(it) => it.syntax(),
            Expr::Dependence(it) => it.syntax(),
            Expr::Index(it) => it.syntax(),
            Expr::Reduce(it) => it.syntax(),
            Expr::Convolution(it) => it.syntax(),
            Expr::Select(it) => it.syntax(),
            Expr::MultiArg(it) => it.syntax(),
            Expr::Binary(it) => it.syntax(),
            Expr::Unary(it) => it.syntax(),
            Expr::Paren(it) => it.syntax(),
            Expr::Bool(it) => it.syntax(),
            Expr::Int(it) => it.syntax(),
            Expr::Real(it) => it.syntax(),
        }
    }
}

impl IfExpr {
    pub fn cond(&self) -> Option<Expr> {
        children(&self.0).next()
    }
    pub fn then_branch(&self) -> Option<Expr> {
        children(&self.0).nth(1)
    }
    pub fn else_branch(&self) -> Option<Expr> {
        children(&self.0).nth(2)
    }
}

impl RestrictExpr {
    /// The domain source: either a raw ISL domain literal or an arbitrary nested calculator
    /// expression — see the parser's `restrict_expr` for why both are one node kind's worth of
    /// alternatives at the syntax level.
    pub fn domain_source(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl AutoRestrictExpr {
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl CaseExpr {
    /// The optional `case Name { ... }` name.
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn branches(&self) -> impl Iterator<Item = Expr> {
        children(&self.0)
    }
}

impl VariableExpr {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
}

/// What a [`DependenceExpr`] applies its function to — either an arbitrary expression (the
/// `f @ expr` form) or the constant/variable it's directly attached to via array notation
/// (`X[expr]`/`5[expr]`). Both shapes share the `DEPENDENCE_EXPR` node kind; see
/// `syntax_kind.rs`.
impl DependenceExpr {
    /// The function/relation being applied — a [`Function`], [`ArrayFunction`], or (fuzzy
    /// variant) [`FuzzyFunction`]/[`ArrayFuzzyFunction`].
    pub fn function(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    /// The operand: present for the `f @ expr` form; `None` for the array-notation form, where
    /// the operand is a [`VariableExpr`]/[`ConstantExpr`] that appears *before* the function in
    /// source order and is more naturally read via [`Self::array_notation_base`].
    pub fn applied_expr(&self) -> Option<Expr> {
        // In the `f @ expr` form, the only `Expr` child is `expr` itself (the function is a
        // `CalcExpr`-kinded child, never an `Expr`). In the array-notation form, the base
        // (`VariableExpr`/constant) *is* an `Expr` child — callers wanting that one specifically
        // should use `array_notation_base`, which is the same accessor under a clearer name for
        // that shape.
        children(&self.0).next()
    }
    /// Same underlying child as [`Self::applied_expr`], named for the array-notation reading
    /// (`X[expr]`/`5[expr]`) where this is the base being indexed, not "the expr `f` applies to".
    pub fn array_notation_base(&self) -> Option<Expr> {
        self.applied_expr()
    }
}

impl IndexExpr {
    /// The function/polynomial (or bare fuzzy array literal) `val` applies to — a [`Function`],
    /// [`ArrayFunction`], [`Polynomial`], [`ArrayPolynomial`], or fuzzy equivalent.
    pub fn source(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
}

impl ReduceExpr {
    /// `true` for `argreduce`, `false` for `reduce`.
    pub fn is_arg_reduce(&self) -> bool {
        token(&self.0, K::KW_ARG_REDUCE).is_some()
    }
    /// The named operator token (`min`/`max`/`prod`/`sum`/`and`/`or`/`xor`/`+`/`*`), if this is
    /// not an external-function reduction.
    pub fn named_operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::KW_MIN
                        | K::KW_MAX
                        | K::KW_PROD
                        | K::KW_SUM
                        | K::KW_AND
                        | K::KW_OR
                        | K::KW_XOR
                        | K::PLUS
                        | K::STAR
                )
            })
    }
    /// The external-function operator reference, if this is not a named-operator reduction.
    pub fn external_operator(&self) -> Option<QualifiedName> {
        child(&self.0)
    }
    pub fn projection(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl ConvolutionExpr {
    pub fn kernel_domain(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    pub fn kernel_expr(&self) -> Option<Expr> {
        children(&self.0).next()
    }
    pub fn data_expr(&self) -> Option<Expr> {
        children(&self.0).nth(1)
    }
}

impl SelectExpr {
    pub fn relation(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl MultiArgExpr {
    pub fn named_operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::KW_MIN
                        | K::KW_MAX
                        | K::KW_PROD
                        | K::KW_SUM
                        | K::KW_AND
                        | K::KW_OR
                        | K::KW_XOR
                        | K::PLUS
                        | K::STAR
                )
            })
    }
    pub fn external_function(&self) -> Option<QualifiedName> {
        child(&self.0)
    }
    pub fn args(&self) -> impl Iterator<Item = Expr> {
        children(&self.0)
    }
}

impl BinaryExpr {
    pub fn lhs(&self) -> Option<Expr> {
        children(&self.0).next()
    }
    pub fn rhs(&self) -> Option<Expr> {
        children(&self.0).nth(1)
    }
    pub fn operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::KW_OR
                        | K::KW_XOR
                        | K::KW_AND
                        | K::EQ
                        | K::NOT_EQ
                        | K::GT_EQ
                        | K::GT
                        | K::LT
                        | K::LT_EQ
                        | K::PLUS
                        | K::MINUS
                        | K::STAR
                        | K::SLASH
                        | K::KW_MIN
                        | K::KW_MAX
                )
            })
    }
}

impl UnaryExpr {
    pub fn operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| matches!(t.kind(), K::KW_NOT | K::MINUS))
    }
    pub fn operand(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl ParenExpr {
    pub fn inner(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl BoolLit {
    pub fn value(&self) -> Option<bool> {
        match name_ish_keyword(&self.0)?.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

fn name_ish_keyword(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|t| matches!(t.kind(), K::KW_TRUE | K::KW_FALSE))
        .map(|t| t.text().to_string())
}

impl IntLit {
    /// Raw source text (e.g. `"-5"`), including the leading `-` if present — parsing to an
    /// actual integer is a semantic-analysis concern (`alpha-model`), not a syntax one.
    pub fn text(&self) -> String {
        self.0.text().to_string()
    }
}

impl RealLit {
    pub fn text(&self) -> String {
        self.0.text().to_string()
    }
}

// --- the "calculator" (domain/relation/function/polynomial) layer ---

ast_node!(ParamDomain, PARAM_DOMAIN);
ast_node!(Domain, DOMAIN);
ast_node!(ArrayDomain, ARRAY_DOMAIN);
ast_node!(Relation, RELATION);
ast_node!(Function, FUNCTION);
ast_node!(ArrayFunction, ARRAY_FUNCTION);
ast_node!(Polynomial, POLYNOMIAL);
ast_node!(ArrayPolynomial, ARRAY_POLYNOMIAL);
ast_node!(DefinedObject, DEFINED_OBJECT);
ast_node!(VariableDomain, VARIABLE_DOMAIN);
ast_node!(RectangularDomain, RECTANGULAR_DOMAIN);
ast_node!(UnaryCalcExpr, UNARY_CALC_EXPR);
ast_node!(BinaryCalcExpr, BINARY_CALC_EXPR);
ast_node!(CalcParenExpr, CALC_PAREN_EXPR);
ast_node!(FuzzyFunction, FUZZY_FUNCTION);
ast_node!(ArrayFuzzyFunction, ARRAY_FUZZY_FUNCTION);

/// `CalculatorExpression` and its terminals — the small algebra over domains/relations/functions
/// (`define X = <calc expr>`, `RectangularDomain`, `{ myDefinedThing }`, ...). Everything here is
/// raw-text-captured where the source grammar itself doesn't parse (domain/relation/polynomial
/// bodies) — see `calculator.rs`'s module doc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CalcExpr {
    ParamDomain(ParamDomain),
    Domain(Domain),
    ArrayDomain(ArrayDomain),
    Relation(Relation),
    Function(Function),
    ArrayFunction(ArrayFunction),
    Polynomial(Polynomial),
    ArrayPolynomial(ArrayPolynomial),
    DefinedObject(DefinedObject),
    VariableDomain(VariableDomain),
    RectangularDomain(RectangularDomain),
    Unary(UnaryCalcExpr),
    Binary(BinaryCalcExpr),
    Paren(CalcParenExpr),
    FuzzyFunction(FuzzyFunction),
    ArrayFuzzyFunction(ArrayFuzzyFunction),
}

impl AstNode for CalcExpr {
    fn cast(node: SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            K::PARAM_DOMAIN => CalcExpr::ParamDomain(ParamDomain(node)),
            K::DOMAIN => CalcExpr::Domain(Domain(node)),
            K::ARRAY_DOMAIN => CalcExpr::ArrayDomain(ArrayDomain(node)),
            K::RELATION => CalcExpr::Relation(Relation(node)),
            K::FUNCTION => CalcExpr::Function(Function(node)),
            K::ARRAY_FUNCTION => CalcExpr::ArrayFunction(ArrayFunction(node)),
            K::POLYNOMIAL => CalcExpr::Polynomial(Polynomial(node)),
            K::ARRAY_POLYNOMIAL => CalcExpr::ArrayPolynomial(ArrayPolynomial(node)),
            K::DEFINED_OBJECT => CalcExpr::DefinedObject(DefinedObject(node)),
            K::VARIABLE_DOMAIN => CalcExpr::VariableDomain(VariableDomain(node)),
            K::RECTANGULAR_DOMAIN => CalcExpr::RectangularDomain(RectangularDomain(node)),
            K::UNARY_CALC_EXPR => CalcExpr::Unary(UnaryCalcExpr(node)),
            K::BINARY_CALC_EXPR => CalcExpr::Binary(BinaryCalcExpr(node)),
            K::CALC_PAREN_EXPR => CalcExpr::Paren(CalcParenExpr(node)),
            K::FUZZY_FUNCTION => CalcExpr::FuzzyFunction(FuzzyFunction(node)),
            K::ARRAY_FUZZY_FUNCTION => CalcExpr::ArrayFuzzyFunction(ArrayFuzzyFunction(node)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            CalcExpr::ParamDomain(it) => it.syntax(),
            CalcExpr::Domain(it) => it.syntax(),
            CalcExpr::ArrayDomain(it) => it.syntax(),
            CalcExpr::Relation(it) => it.syntax(),
            CalcExpr::Function(it) => it.syntax(),
            CalcExpr::ArrayFunction(it) => it.syntax(),
            CalcExpr::Polynomial(it) => it.syntax(),
            CalcExpr::ArrayPolynomial(it) => it.syntax(),
            CalcExpr::DefinedObject(it) => it.syntax(),
            CalcExpr::VariableDomain(it) => it.syntax(),
            CalcExpr::RectangularDomain(it) => it.syntax(),
            CalcExpr::Unary(it) => it.syntax(),
            CalcExpr::Binary(it) => it.syntax(),
            CalcExpr::Paren(it) => it.syntax(),
            CalcExpr::FuzzyFunction(it) => it.syntax(),
            CalcExpr::ArrayFuzzyFunction(it) => it.syntax(),
        }
    }
}

impl Domain {
    /// The raw ISL set text, e.g. `"{[i,j] : 0<=i<N}"` — handed to isl's own parser during
    /// semantic analysis (`alpha-model`), not interpreted here.
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
}
impl ArrayDomain {
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
}
impl ParamDomain {
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
    /// The `[N,M]` parameter names, if the domain has an explicit parameter-list prefix. Only
    /// the `IDENT` tokens *before* the `->` count — `PARAM_DOMAIN`'s `{...}` constraint body is
    /// raw-captured directly as further child tokens of this same node (see `calculator.rs`'s
    /// `param_domain`, which has no separate wrapper node for it), so an identifier used inside
    /// the constraint text itself (e.g. the `N` in `[N]->{:N>0}`) is *also* a direct `IDENT`
    /// child — naively collecting all of them would double-count `N` as if it were declared
    /// twice in the parameter list.
    pub fn param_names(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        self.0
            .children_with_tokens()
            .take_while(|it| it.kind() != K::ARROW)
            .filter_map(|it| it.into_token())
            .filter(|t| t.kind() == K::IDENT)
    }
}
impl Relation {
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
}
impl Polynomial {
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
}
impl ArrayPolynomial {
    pub fn isl_text(&self) -> String {
        self.0.text().to_string()
    }
}
impl ArrayFunction {
    /// Raw per-element affine-expression text, comma-split at the top nesting level. Simple
    /// (no nested brackets in practice for this position) — a proper split would need
    /// depth-tracking like the parser's own; revisit if a real program needs it.
    pub fn raw_elements(&self) -> Vec<String> {
        let inner = self.0.text().to_string();
        let inner = inner
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        if inner.trim().is_empty() {
            Vec::new()
        } else {
            inner.split(',').map(|s| s.trim().to_string()).collect()
        }
    }
}

impl DefinedObject {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
}

impl VariableDomain {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
}

impl RectangularDomain {
    /// The `as [i,j]` index names, if present. Only `IDENT` tokens *after* the `as` keyword
    /// count — the bound list itself (`[N,N]`/`[0:N-1,...]`, raw-captured, no wrapper node —
    /// see `calculator.rs`'s `rectangular_domain`) commonly contains identifiers too (`N` here),
    /// which are bound *expressions*, not index names, and must not be confused with them (same
    /// class of bug as `ParamDomain::param_names`'s doc explains).
    pub fn index_names(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        self.0
            .children_with_tokens()
            .skip_while(|it| it.kind() != K::KW_AS)
            .filter_map(|it| it.into_token())
            .filter(|t| t.kind() == K::IDENT)
    }
}

impl Function {
    pub fn index_names(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        tokens(&self.0, K::IDENT)
    }
    pub fn exprs(&self) -> impl Iterator<Item = FnExpr> {
        children(&self.0)
    }
}

impl UnaryCalcExpr {
    pub fn operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::KW_DOMAIN
                        | K::KW_RANGE
                        | K::KW_COMPLEMENT
                        | K::KW_AFFINE_HULL
                        | K::KW_POLY_HULL
                        | K::KW_REVERSE
                )
            })
    }
    pub fn operand(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
}

impl BinaryCalcExpr {
    pub fn lhs(&self) -> Option<CalcExpr> {
        children(&self.0).next()
    }
    pub fn rhs(&self) -> Option<CalcExpr> {
        children(&self.0).nth(1)
    }
    pub fn operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::KW_CROSS
                        | K::PLUS
                        | K::MINUS
                        | K::STAR
                        | K::AT
                        | K::KW_INTERSECT_RANGE
                        | K::KW_SUBTRACT_RANGE
                )
            })
    }
}

impl CalcParenExpr {
    pub fn inner(&self) -> Option<CalcExpr> {
        child(&self.0)
    }
}

// --- the tiny `AlphaFunction` expression sub-grammar ---

ast_node!(FnLiteral, FN_LITERAL);
ast_node!(FnFloor, FN_FLOOR);
ast_node!(FnBinaryExpr, FN_BINARY_EXPR);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FnExpr {
    Literal(FnLiteral),
    Floor(FnFloor),
    Binary(FnBinaryExpr),
}

impl AstNode for FnExpr {
    fn cast(node: SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            K::FN_LITERAL => FnExpr::Literal(FnLiteral(node)),
            K::FN_FLOOR => FnExpr::Floor(FnFloor(node)),
            K::FN_BINARY_EXPR => FnExpr::Binary(FnBinaryExpr(node)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            FnExpr::Literal(it) => it.syntax(),
            FnExpr::Floor(it) => it.syntax(),
            FnExpr::Binary(it) => it.syntax(),
        }
    }
}

impl FnLiteral {
    /// Raw text, e.g. `"2j"` (implicit-multiplication coefficient notation) or `"-1"` — see
    /// `calculator.rs`'s `fn_terminal_expr` for why this can be a run of several tokens.
    pub fn text(&self) -> String {
        self.0.text().to_string()
    }
}

impl FnFloor {
    pub fn operand(&self) -> Option<FnExpr> {
        child(&self.0)
    }
}

impl FnBinaryExpr {
    pub fn lhs(&self) -> Option<FnExpr> {
        children(&self.0).next()
    }
    pub fn rhs(&self) -> Option<FnExpr> {
        children(&self.0).nth(1)
    }
    pub fn operator(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| {
                matches!(
                    t.kind(),
                    K::PLUS | K::MINUS | K::STAR | K::SLASH | K::PERCENT | K::EQ
                )
            })
    }
}
