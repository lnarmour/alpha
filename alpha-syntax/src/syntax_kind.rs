//! The unified token+node kind enum rowan needs, plus the `rowan::Language` wiring.
//!
//! Node kinds are deliberately more coarse-grained than `Alpha.xtext`'s grammar rules in a few
//! places where the source grammar defines several near-identical productions only to satisfy
//! Xtext's "each alternative needs its own rule" style (e.g. `reduce`/`argreduce` crossed with
//! named-vs-external-operator crossed with fuzzy-vs-not gives 8 grammar rules that are all "a
//! reduction with a projection and a body"). Those collapse into one `REDUCE_EXPR` node here,
//! disambiguated by its children/tokens (see `ast.rs`) — fewer node kinds, no loss of
//! information, and a typed `ast::` layer that's easier to consume than 8 near-duplicate types.
use crate::token_kind::TokenKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
pub enum SyntaxKind {
    // --- tokens (mirrors TokenKind 1:1; see `From<TokenKind>` below) ---
    WHITESPACE,
    LINE_COMMENT,
    BLOCK_COMMENT,
    INT_NUMBER,
    FLOAT_NUMBER,
    IDENT,
    STRING_LIT,
    KW_AFFINE,
    KW_DEFINE,
    KW_INPUTS,
    KW_OUTPUTS,
    KW_LOCALS,
    KW_OVER,
    KW_WITH,
    KW_WHILE,
    KW_WHEN,
    KW_ELSE,
    KW_LET,
    KW_FUZZY,
    KW_CONSTANT,
    KW_EXTERNAL,
    KW_IMPORT,
    KW_PACKAGE,
    KW_IF,
    KW_THEN,
    KW_VAL,
    KW_FLOOR,
    KW_AUTO,
    KW_CASE,
    KW_REDUCE,
    KW_ARG_REDUCE,
    KW_CONV,
    KW_SELECT,
    KW_FROM,
    KW_AS,
    KW_TRUE,
    KW_FALSE,
    KW_MIN,
    KW_MAX,
    KW_PROD,
    KW_SUM,
    KW_AND,
    KW_OR,
    KW_XOR,
    KW_NOT,
    KW_DOMAIN,
    KW_RANGE,
    KW_COMPLEMENT,
    KW_AFFINE_HULL,
    KW_POLY_HULL,
    KW_REVERSE,
    KW_CROSS,
    KW_INTERSECT_RANGE,
    KW_SUBTRACT_RANGE,
    ARROW,
    LT_EQ,
    GT_EQ,
    NOT_EQ,
    DOT_STAR,
    L_BRACK2,
    R_BRACK2,
    L_PAREN,
    R_PAREN,
    L_BRACK,
    R_BRACK,
    L_BRACE,
    R_BRACE,
    COMMA,
    SEMICOLON,
    COLON,
    DOT,
    EQ,
    LT,
    GT,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    AT,
    AMP,
    PIPE,
    CARET,

    EOF,
    /// A run of unexpected input the parser couldn't attach to any real construct; carries
    /// whatever raw tokens it swallowed while recovering, so the tree stays lossless.
    ERROR,

    // --- top-level structure ---
    ROOT,
    IMPORT,
    ALPHA_CONSTANT,
    EXTERNAL_FUNCTION,
    ALPHA_PACKAGE,
    QUALIFIED_NAME,

    // --- systems ---
    SYSTEM,
    DEFINE_SECTION,
    POLYHEDRAL_OBJECT,
    INPUTS,
    OUTPUTS,
    LOCALS,
    VARIABLE,
    FUZZY_VARIABLE,
    SYSTEM_BODY,

    // --- equations ---
    STANDARD_EQUATION,
    USE_EQUATION,

    // --- expressions ---
    IF_EXPR,
    RESTRICT_EXPR,
    AUTO_RESTRICT_EXPR,
    CASE_EXPR,
    VARIABLE_EXPR,
    DEPENDENCE_EXPR,
    INDEX_EXPR,
    REDUCE_EXPR,
    CONVOLUTION_EXPR,
    SELECT_EXPR,
    MULTI_ARG_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,
    PAREN_EXPR,
    BOOL_LIT,
    INT_LIT,
    REAL_LIT,

    // --- the "calculator" (domain/relation/function/polynomial) layer ---
    // Domain/relation/polynomial literal bodies are raw-text-captured (isl parses them later) —
    // these nodes just mark the span, structurally.
    PARAM_DOMAIN,
    DOMAIN,
    ARRAY_DOMAIN,
    RELATION,
    FUNCTION,
    ARRAY_FUNCTION,
    POLYNOMIAL,
    ARRAY_POLYNOMIAL,
    DEFINED_OBJECT,
    VARIABLE_DOMAIN,
    RECTANGULAR_DOMAIN,
    UNARY_CALC_EXPR,
    BINARY_CALC_EXPR,
    CALC_PAREN_EXPR,

    // --- the tiny real sub-grammar inside `(idx -> exprs)` function literals ---
    FN_LITERAL,
    FN_FLOOR,
    FN_BINARY_EXPR,

    // --- fuzzy-variable machinery (secondary feature, ported structurally) ---
    FUZZY_FUNCTION,
    ARRAY_FUZZY_FUNCTION,
    NESTED_FUZZY_FUNCTION,
    AFFINE_FUZZY_VARIABLE_USE,
}

impl From<TokenKind> for SyntaxKind {
    fn from(t: TokenKind) -> SyntaxKind {
        use SyntaxKind as S;
        use TokenKind as T;
        match t {
            T::Whitespace => S::WHITESPACE,
            T::LineComment => S::LINE_COMMENT,
            T::BlockComment => S::BLOCK_COMMENT,
            T::IntNumber => S::INT_NUMBER,
            T::FloatNumber => S::FLOAT_NUMBER,
            T::Ident => S::IDENT,
            T::StringLit => S::STRING_LIT,
            T::KwAffine => S::KW_AFFINE,
            T::KwDefine => S::KW_DEFINE,
            T::KwInputs => S::KW_INPUTS,
            T::KwOutputs => S::KW_OUTPUTS,
            T::KwLocals => S::KW_LOCALS,
            T::KwOver => S::KW_OVER,
            T::KwWith => S::KW_WITH,
            T::KwWhile => S::KW_WHILE,
            T::KwWhen => S::KW_WHEN,
            T::KwElse => S::KW_ELSE,
            T::KwLet => S::KW_LET,
            T::KwFuzzy => S::KW_FUZZY,
            T::KwConstant => S::KW_CONSTANT,
            T::KwExternal => S::KW_EXTERNAL,
            T::KwImport => S::KW_IMPORT,
            T::KwPackage => S::KW_PACKAGE,
            T::KwIf => S::KW_IF,
            T::KwThen => S::KW_THEN,
            T::KwVal => S::KW_VAL,
            T::KwFloor => S::KW_FLOOR,
            T::KwAuto => S::KW_AUTO,
            T::KwCase => S::KW_CASE,
            T::KwReduce => S::KW_REDUCE,
            T::KwArgReduce => S::KW_ARG_REDUCE,
            T::KwConv => S::KW_CONV,
            T::KwSelect => S::KW_SELECT,
            T::KwFrom => S::KW_FROM,
            T::KwAs => S::KW_AS,
            T::KwTrue => S::KW_TRUE,
            T::KwFalse => S::KW_FALSE,
            T::KwMin => S::KW_MIN,
            T::KwMax => S::KW_MAX,
            T::KwProd => S::KW_PROD,
            T::KwSum => S::KW_SUM,
            T::KwAnd => S::KW_AND,
            T::KwOr => S::KW_OR,
            T::KwXor => S::KW_XOR,
            T::KwNot => S::KW_NOT,
            T::KwDomain => S::KW_DOMAIN,
            T::KwRange => S::KW_RANGE,
            T::KwComplement => S::KW_COMPLEMENT,
            T::KwAffineHull => S::KW_AFFINE_HULL,
            T::KwPolyHull => S::KW_POLY_HULL,
            T::KwReverse => S::KW_REVERSE,
            T::KwCross => S::KW_CROSS,
            T::KwIntersectRange => S::KW_INTERSECT_RANGE,
            T::KwSubtractRange => S::KW_SUBTRACT_RANGE,
            T::Arrow => S::ARROW,
            T::LtEq => S::LT_EQ,
            T::GtEq => S::GT_EQ,
            T::NotEq => S::NOT_EQ,
            T::DotStar => S::DOT_STAR,
            T::LBrack2 => S::L_BRACK2,
            T::RBrack2 => S::R_BRACK2,
            T::LParen => S::L_PAREN,
            T::RParen => S::R_PAREN,
            T::LBrack => S::L_BRACK,
            T::RBrack => S::R_BRACK,
            T::LBrace => S::L_BRACE,
            T::RBrace => S::R_BRACE,
            T::Comma => S::COMMA,
            T::Semicolon => S::SEMICOLON,
            T::Colon => S::COLON,
            T::Dot => S::DOT,
            T::Eq => S::EQ,
            T::Lt => S::LT,
            T::Gt => S::GT,
            T::Plus => S::PLUS,
            T::Minus => S::MINUS,
            T::Star => S::STAR,
            T::Slash => S::SLASH,
            T::Percent => S::PERCENT,
            T::At => S::AT,
            T::Amp => S::AMP,
            T::Pipe => S::PIPE,
            T::Caret => S::CARET,
        }
    }
}

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
        )
    }
}

/// Marker type tying `SyntaxKind` into rowan's generic tree types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {}

impl rowan::Language for Lang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        assert!(raw.0 <= SyntaxKind::AFFINE_FUZZY_VARIABLE_USE as u16);
        // SAFETY: `SyntaxKind` is `#[repr(u16)]` and we just checked `raw.0` is in range.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<Lang>;
pub type SyntaxToken = rowan::SyntaxToken<Lang>;
pub type SyntaxElement = rowan::SyntaxElement<Lang>;
