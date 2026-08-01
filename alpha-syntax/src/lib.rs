//! Lexer, lossless CST, parser, and typed AST for the Alpha language.
//!
//! Design: a `logos` lexer, a hand-written recursive-descent/Pratt parser, a lossless `rowan`
//! CST, and a typed `ast::` accessor layer over it. All four pieces are implemented: the lexer
//! (`token_kind`, `lexer`), the resilient
//! recursive-descent/Pratt parser (`parser`) building a lossless `rowan` CST, and the typed
//! `ast::` accessor layer over that CST.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod syntax_kind;
pub mod token_kind;

pub use ast::AstNode;
pub use lexer::{tokenize, LexicalError, Token};
pub use parser::{parse, Parse, SyntaxError};
pub use syntax_kind::SyntaxKind;
pub use token_kind::TokenKind;

impl Parse {
    /// The typed root of the tree — `ast::Root::cast` can't fail on a tree this crate produced
    /// itself (`items::root` always emits a `ROOT`-kinded node as the tree's root).
    pub fn tree(&self) -> ast::Root {
        ast::AstNode::cast(self.syntax_node()).expect("parser always produces a ROOT node")
    }
}
