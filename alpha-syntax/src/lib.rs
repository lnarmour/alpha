//! Lexer, lossless CST, parser, and typed AST for the Alpha language.
//!
//! See `docs/rust-port-design.md` §4 in the workspace root for the design (logos lexer +
//! hand-written recursive-descent/Pratt parser + rowan lossless CST + typed `ast::` layer).
//! Currently implemented: the lexer (`token_kind`, `lexer`). The rowan CST, parser, and typed
//! `ast::` layer land in subsequent milestones.

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
