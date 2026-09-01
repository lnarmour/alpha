//! The lexer's token kinds, derived from `Alpha.xtext`'s terminal rules and implicit keyword
//! literals.
//!
//! Deliberately lexer-only: `logos::Logos` requires every variant to be reachable by a
//! `#[token]`/`#[regex]` pattern, so node kinds (added once the parser/CST land) live in a
//! separate, larger `SyntaxKind` enum that this one maps into — see `syntax_kind.rs`.
//!
//! One deliberate departure from the source grammar: `Alpha.xtext`'s `SINT`/`FLOAT` terminals
//! bake an optional leading `-` directly into the number literal (`'-'? INT`), which is exactly
//! the kind of context-free-lexer-vs-context-sensitive-meaning ambiguity that trips up "is `N-5`
//! one subtraction or a variable next to a negative literal?" (the grammar's own comments flag
//! this same ambiguity around unary-minus). This lexer always lexes bare unsigned digits and a
//! separate `Minus` token; the parser treats a leading `Minus` immediately before a number
//! literal, specifically where a constant expression is expected, as a negative literal — the
//! standard resolution used by most hand-written parsers, and simpler than replicating the
//! ANTLR terminal's embedded sign.
use logos::Logos;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(error = LexError)]
pub enum TokenKind {
    // --- trivia (kept as real tokens: this is a lossless lexer for a lossless CST) ---
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
    #[regex(r"//[^\n]*")]
    LineComment,
    #[regex(r"/\*([^*]|\*[^/])*\*/")]
    BlockComment,

    // --- literals ---
    #[regex(r"[0-9]+")]
    IntNumber,
    #[regex(r"[0-9]+\.[0-9]+")]
    FloatNumber,
    /// Covers both plain `ID` (with optional leading `^` escape) and the quoted
    /// `'special chars...'` "prime identifier" form.
    #[regex(r"\^?[a-zA-Z_][a-zA-Z_0-9]*")]
    #[regex(r"'[^'\n]+'")]
    Ident,
    #[regex(r#""([^"\\]|\\.)*""#)]
    StringLit,

    // --- keywords (reserved words; see the module doc for why these can't collide with Ident) ---
    #[token("affine")]
    KwAffine,
    #[token("define")]
    KwDefine,
    #[token("inputs")]
    KwInputs,
    #[token("outputs")]
    KwOutputs,
    #[token("locals")]
    KwLocals,
    #[token("linear")]
    KwLinear,
    #[token("unrestricted")]
    KwUnrestricted,
    #[token("of")]
    KwOf,
    #[token("bool")]
    KwBool,
    #[token("int")]
    KwInt,
    #[token("real")]
    KwReal,
    #[token("qubit")]
    KwQubit,
    #[token("over")]
    KwOver,
    #[token("with")]
    KwWith,
    #[token("while")]
    KwWhile,
    #[token("when")]
    KwWhen,
    #[token("else")]
    KwElse,
    #[token("let")]
    KwLet,
    #[token("fuzzy")]
    KwFuzzy,
    #[token("constant")]
    KwConstant,
    #[token("external")]
    KwExternal,
    #[token("import")]
    KwImport,
    #[token("package")]
    KwPackage,
    #[token("if")]
    KwIf,
    #[token("then")]
    KwThen,
    #[token("val")]
    KwVal,
    #[token("floor")]
    KwFloor,
    #[token("auto")]
    KwAuto,
    #[token("case")]
    KwCase,
    #[token("reduce")]
    KwReduce,
    #[token("argreduce")]
    KwArgReduce,
    #[token("conv")]
    KwConv,
    #[token("select")]
    KwSelect,
    #[token("from")]
    KwFrom,
    #[token("as")]
    KwAs,
    #[token("true")]
    KwTrue,
    #[token("false")]
    KwFalse,
    #[token("min")]
    KwMin,
    #[token("max")]
    KwMax,
    #[token("prod")]
    KwProd,
    #[token("sum")]
    KwSum,
    #[token("and")]
    KwAnd,
    #[token("or")]
    KwOr,
    #[token("xor")]
    KwXor,
    #[token("not")]
    KwNot,
    #[token("domain")]
    KwDomain,
    #[token("range")]
    KwRange,
    #[token("complement")]
    KwComplement,
    #[token("affine-hull")]
    KwAffineHull,
    #[token("poly-hull")]
    KwPolyHull,
    #[token("reverse")]
    KwReverse,
    #[token("cross")]
    KwCross,
    #[token("intersectRange")]
    KwIntersectRange,
    #[token("subtractRange")]
    KwSubtractRange,

    // --- multi-char punctuation (checked before single-char thanks to longest-match) ---
    #[token("->")]
    Arrow,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("!=")]
    NotEq,
    #[token(".*")]
    DotStar,
    #[token("[[")]
    LBrack2,
    #[token("]]")]
    RBrack2,

    // --- single-char punctuation ---
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBrack,
    #[token("]")]
    RBrack,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(";")]
    Semicolon,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("@")]
    At,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    /// Exponentiation, used only inside `AISLPolynomialExpression` bodies (e.g. `N^2+N`) — not
    /// to be confused with the identifier-escape `^` baked directly into the `Ident` regex
    /// above, which only applies when immediately followed by an identifier-start character.
    #[token("^")]
    Caret,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LexError;

impl TokenKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
        )
    }

    /// The subset of keyword-shaped tokens that are ALSO valid `AREDUCTION_OP`/binary-op words
    /// in the grammar (`min`, `max`, `prod`, `sum`, `and`, `or`, `xor`) — kept as a named helper
    /// since several grammar rules (`AREDUCTION_OP`, `AOrOP`, `AAndOP`, `AMINMAX_OP`) reuse the
    /// same reserved words in different operator positions; the parser distinguishes them by
    /// context, not by a different token kind.
    pub fn is_reduction_or_binary_op_word(self) -> bool {
        matches!(
            self,
            TokenKind::KwMin
                | TokenKind::KwMax
                | TokenKind::KwProd
                | TokenKind::KwSum
                | TokenKind::KwAnd
                | TokenKind::KwOr
                | TokenKind::KwXor
        )
    }
}
