//! Thin wrapper turning `logos`'s streaming lexer into a materialized token list carrying
//! source spans (byte ranges), which the parser needs for both error reporting and for slicing
//! out the raw text of domain/relation/function literals to hand to isl later (see
//! `docs/rust-port-design.md` §4/§5).

use crate::token_kind::TokenKind;
use logos::Logos;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: u32,
    pub end: u32,
}

impl Token {
    pub fn range(self) -> std::ops::Range<u32> {
        self.start..self.end
    }
}

/// An invalid character sequence the lexer couldn't turn into any token, at the given byte
/// offset. The parser turns these into diagnostics rather than aborting (see §4: resilient
/// parsing is the whole point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexicalError {
    pub start: u32,
    pub end: u32,
}

pub fn tokenize(source: &str) -> (Vec<Token>, Vec<LexicalError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut lexer = TokenKind::lexer(source);
    while let Some(result) = lexer.next() {
        let span = lexer.span();
        let start = span.start as u32;
        let end = span.end as u32;
        match result {
            Ok(kind) => tokens.push(Token { kind, start, end }),
            Err(_) => errors.push(LexicalError { start, end }),
        }
    }
    (tokens, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_kind::TokenKind::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (tokens, errors) = tokenize(src);
        assert!(errors.is_empty(), "unexpected lexical errors: {errors:?}");
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn keywords_win_over_plain_identifiers() {
        assert_eq!(kinds("affine"), vec![KwAffine]);
        assert_eq!(kinds("if"), vec![KwIf]);
        assert_eq!(kinds("affine_x"), vec![Ident]); // not a keyword: longer ident
    }

    #[test]
    fn hyphenated_keywords_beat_ident_minus_ident() {
        assert_eq!(kinds("affine-hull"), vec![KwAffineHull]);
        assert_eq!(kinds("poly-hull"), vec![KwPolyHull]);
        // but a real subtraction between two idents still lexes as three tokens
        assert_eq!(kinds("foo-bar"), vec![Ident, Minus, Ident]);
    }

    #[test]
    fn caret_escaped_and_quoted_prime_identifiers() {
        assert_eq!(kinds("^affine"), vec![Ident]);
        assert_eq!(kinds("'a-b+c'"), vec![Ident]);
    }

    #[test]
    fn numbers_are_unsigned_minus_is_separate() {
        assert_eq!(kinds("42"), vec![IntNumber]);
        assert_eq!(kinds("3.14"), vec![FloatNumber]);
        assert_eq!(kinds("-5"), vec![Minus, IntNumber]);
        assert_eq!(kinds("N-5"), vec![Ident, Minus, IntNumber]);
    }

    #[test]
    fn multichar_punctuation_beats_single_char_split() {
        assert_eq!(kinds("->"), vec![Arrow]);
        assert_eq!(kinds("<="), vec![LtEq]);
        assert_eq!(kinds(">="), vec![GtEq]);
        assert_eq!(kinds("!="), vec![NotEq]);
        assert_eq!(kinds("[["), vec![LBrack2]);
        assert_eq!(kinds("]]"), vec![RBrack2]);
        assert_eq!(kinds(".*"), vec![DotStar]);
    }

    #[test]
    fn comments_and_whitespace_are_kept_as_trivia() {
        let (tokens, errors) = tokenize("affine // trailing comment\n/* block */let");
        assert!(errors.is_empty());
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                KwAffine,
                Whitespace,
                LineComment,
                Whitespace,
                BlockComment,
                KwLet
            ]
        );
        assert!(tokens.iter().all(|t| t.end > t.start));
    }

    #[test]
    fn install_test_fixture_tokenizes_cleanly() {
        // From the wiki's Installation-Instructions.md "Install Test Alpha File".
        let src = "affine InstallTest [N] -> {: N>0}\n    inputs  X: [N]\n    outputs Y: [N]\n    let Y[i] = X[N-i-1];\n.\n";
        let (_tokens, errors) = tokenize(src);
        assert!(errors.is_empty(), "unexpected lexical errors: {errors:?}");
    }
}
