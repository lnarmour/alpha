//! Hand-written recursive-descent parser producing a lossless rowan CST. See
//! `docs/rust-port-design.md` §4 in the workspace root for the overall design and rationale.
//!
//! Trivia handling: whitespace/comment tokens are attached to the tree the moment they're
//! encountered, immediately before whatever real token follows them — this keeps the tree
//! 100% lossless (concatenating every leaf token reproduces the source exactly), though it
//! doesn't try to be clever about *which* node "owns" a given comment (e.g. for doc-comment
//! attachment). That refinement, if ever needed, can be layered on later without changing the
//! grammar functions below. Lexical errors (bytes the lexer couldn't turn into any token) are
//! folded into the same stream as `SyntaxKind::ERROR` leaves, for the same reason — a byte the
//! lexer rejected still has to end up *somewhere* in the tree, or the tree isn't lossless.
//!
//! Error recovery: every parsing function that can fail to make progress is guarded so the
//! parser always terminates — see [`Parser::recover_until`]/[`Parser::tick`].

mod calculator;
mod expr;
mod items;

use crate::lexer::tokenize;
use crate::syntax_kind::{Lang, SyntaxKind};
use crate::token_kind::TokenKind;
use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, Language};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub start: u32,
    pub end: u32,
}

pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    pub fn syntax_node(&self) -> crate::syntax_kind::SyntaxNode {
        crate::syntax_kind::SyntaxNode::new_root(self.green.clone())
    }
}

/// `QualifiedName: ID ('.' ID)*` — used for imports, and for `[AlphaSystem|QualifiedName]` /
/// `[ExternalFunction|QualifiedName]` cross-references (`UseEquation`'s callee, `reduce`'s
/// external operator, `MultiArgExpression`'s external function). Cross-reference *resolution*
/// (does this name actually denote a system/external-function) is a semantic-analysis concern
/// (`alpha-model`, not yet implemented) — the parser only needs to capture the dotted name.
pub(crate) fn qualified_name(p: &mut Parser) {
    p.start_node(SyntaxKind::QUALIFIED_NAME);
    p.expect(TokenKind::Ident);
    while p.at(TokenKind::Dot) && p.nth(1) == Some(TokenKind::Ident) {
        p.tick();
        p.bump();
        p.bump();
    }
    p.finish_node();
}

/// True if the upcoming tokens form a `QualifiedName` immediately followed by `(` — the shape
/// of `ExternalMultiArgExpression`'s callee, distinguishing it from a plain `VariableExpression`
/// (which is always a single, undotted identifier with no call parens).
pub(crate) fn at_qualified_name_call(p: &Parser) -> bool {
    if p.current() != Some(TokenKind::Ident) {
        return false;
    }
    let mut i = 1;
    while p.nth(i) == Some(TokenKind::Dot) && p.nth(i + 1) == Some(TokenKind::Ident) {
        i += 2;
    }
    p.nth(i) == Some(TokenKind::LParen)
}

pub fn parse(source: &str) -> Parse {
    let (tokens, lex_errors) = tokenize(source);

    // Merge the two already-position-ordered lists into one combined stream, so every byte of
    // `source` is represented by exactly one entry (real token or lex error) that the parser
    // can bump into the tree — see the module doc on why lex errors can't just be dropped.
    let mut raw = Vec::with_capacity(tokens.len() + lex_errors.len());
    let (mut ti, mut ei) = (0, 0);
    while ti < tokens.len() || ei < lex_errors.len() {
        let take_token = match (tokens.get(ti), lex_errors.get(ei)) {
            (Some(t), Some(e)) => t.start <= e.start,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => unreachable!(),
        };
        if take_token {
            let t = tokens[ti];
            raw.push(Raw {
                kind: RawKind::Tok(t.kind),
                start: t.start,
                end: t.end,
            });
            ti += 1;
        } else {
            let e = lex_errors[ei];
            raw.push(Raw {
                kind: RawKind::LexError,
                start: e.start,
                end: e.end,
            });
            ei += 1;
        }
    }

    let mut p = Parser::new(source, raw);
    for e in &lex_errors {
        p.errors.push(SyntaxError {
            message: "invalid character".to_string(),
            start: e.start,
            end: e.end,
        });
    }
    items::root(&mut p);
    p.finish()
}

#[derive(Clone, Copy)]
enum RawKind {
    Tok(TokenKind),
    LexError,
}

impl RawKind {
    /// True for whitespace/comments *and* lex errors — both get auto-flushed into the tree and
    /// skipped over during lookahead the same way, even though only real trivia is emitted with
    /// a trivia `SyntaxKind` (lex errors emit `ERROR`, see `to_syntax`).
    fn is_trivia(self) -> bool {
        match self {
            RawKind::LexError => true,
            RawKind::Tok(t) => t.is_trivia(),
        }
    }

    fn to_syntax(self) -> SyntaxKind {
        match self {
            RawKind::LexError => SyntaxKind::ERROR,
            RawKind::Tok(t) => t.into(),
        }
    }
}

#[derive(Clone, Copy)]
struct Raw {
    kind: RawKind,
    start: u32,
    end: u32,
}

pub(crate) struct Parser<'a> {
    source: &'a str,
    raw: Vec<Raw>,
    /// Index into `raw` of the next entry not yet bumped (includes trivia/lex-errors).
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
    /// Guards against a grammar function looping forever without consuming input.
    fuel: std::cell::Cell<u32>,
    /// Whether `start_node` has been called yet — see `start_node`'s doc for why the very first
    /// call (opening `ROOT`) must *not* flush trivia first, while every later call must.
    started_first_node: bool,
}

const STARTING_FUEL: u32 = 256;

impl<'a> Parser<'a> {
    fn new(source: &'a str, raw: Vec<Raw>) -> Self {
        Parser {
            source,
            raw,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            fuel: std::cell::Cell::new(STARTING_FUEL),
            started_first_node: false,
        }
    }

    fn finish(self) -> Parse {
        Parse {
            green: self.builder.finish(),
            errors: self.errors,
        }
    }

    // --- token stream, trivia-skipping lookahead ---

    fn nth_raw(&self, n: usize) -> Option<Raw> {
        self.raw[self.pos..]
            .iter()
            .filter(|t| !t.kind.is_trivia())
            .nth(n)
            .copied()
    }

    /// Lookahead `n` non-trivia tokens ahead (0 = current). `None` past EOF. Lexical-error spans
    /// are skipped over here too (same as whitespace/comments) — grammar decisions are only ever
    /// made against real token kinds.
    pub(crate) fn nth(&self, n: usize) -> Option<TokenKind> {
        match self.nth_raw(n)?.kind {
            RawKind::Tok(t) => Some(t),
            RawKind::LexError => unreachable!("nth_raw already filters out non-trivia-like raws"),
        }
    }

    pub(crate) fn current(&self) -> Option<TokenKind> {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: TokenKind) -> bool {
        self.current() == Some(kind)
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.current().is_none()
    }

    pub(crate) fn at_any(&self, kinds: &[TokenKind]) -> bool {
        self.current().is_some_and(|c| kinds.contains(&c))
    }

    /// Byte offset of the next non-trivia token, or the end of source at EOF — used for
    /// zero-width error spans.
    fn current_offset(&self) -> u32 {
        match self.nth_raw(0) {
            Some(t) => t.start,
            None => self.source.len() as u32,
        }
    }

    // --- trivia flushing + token consumption ---

    pub(crate) fn flush_trivia(&mut self) {
        while self.pos < self.raw.len() && self.raw[self.pos].kind.is_trivia() {
            let t = self.raw[self.pos];
            self.builder.token(
                Lang::kind_to_raw(t.kind.to_syntax()),
                &self.source[t.start as usize..t.end as usize],
            );
            self.pos += 1;
        }
    }

    /// Consume the current (non-trivia) token. Panics if at EOF or past the end of the token
    /// list — callers must check `at`/`at_eof` first.
    pub(crate) fn bump(&mut self) {
        self.flush_trivia();
        assert!(self.pos < self.raw.len(), "bump() called at EOF");
        let t = self.raw[self.pos];
        self.builder.token(
            Lang::kind_to_raw(t.kind.to_syntax()),
            &self.source[t.start as usize..t.end as usize],
        );
        self.pos += 1;
        self.fuel.set(STARTING_FUEL);
    }

    pub(crate) fn expect(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            self.error(format!(
                "expected {kind:?}, found {}",
                self.current()
                    .map_or("<eof>".to_string(), |k| format!("{k:?}"))
            ));
            false
        }
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        let start = self.current_offset();
        let end = self.nth_raw(0).map(|t| t.end).unwrap_or(start);
        self.errors.push(SyntaxError {
            message: message.into(),
            start,
            end,
        });
    }

    /// Wraps unexpected tokens in an `ERROR` node while advancing until one of `terminators` (or
    /// EOF) is the *next* token. Does not consume the terminator itself. Always makes progress,
    /// so it's safe to call even from a state that would otherwise loop.
    pub(crate) fn recover_until(&mut self, terminators: &[TokenKind]) {
        if self.at_eof() || self.at_any(terminators) {
            return;
        }
        let cp = self.checkpoint();
        let mut consumed = false;
        while !self.at_eof() && !self.at_any(terminators) {
            self.bump();
            consumed = true;
        }
        if consumed {
            self.start_node_at(cp, SyntaxKind::ERROR);
            self.finish_node();
        }
    }

    /// Anti-infinite-loop guard for grammar functions driven by a `while` loop over tokens that
    /// might not actually progress (e.g. malformed list separators). Call once per iteration;
    /// panics (a bug in the parser, not in user input) if fuel runs out without a `bump`.
    pub(crate) fn tick(&self) {
        let f = self.fuel.get();
        assert!(f > 0, "parser stuck without making progress");
        self.fuel.set(f - 1);
    }

    // --- tree building ---

    /// Flushes pending trivia first — *except* on the very first call (opening `ROOT`), which
    /// has no enclosing node yet. Getting this backwards either way breaks the tree:
    /// - Flushing unconditionally would, for `ROOT`'s own call, attach leading trivia (e.g. a
    ///   file starting with a comment) as a sibling *before* `ROOT` opens rather than a child
    ///   inside it, violating rowan's single-root invariant.
    /// - Never flushing (this method's previous behavior) instead attaches trivia that precedes
    ///   *any* node — not just `ROOT` — as that node's own first child once its first `bump()`
    ///   flushes it, since by then the new node is already open. E.g. `X: [N]` would end up with
    ///   the space before `[N]` (and everything up to the next real token) misattributed *inside*
    ///   the `RectangularDomain` node instead of staying outside it, corrupting that node's text.
    pub(crate) fn start_node(&mut self, kind: SyntaxKind) {
        if self.started_first_node {
            self.flush_trivia();
        }
        self.started_first_node = true;
        self.builder.start_node(Lang::kind_to_raw(kind));
    }

    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        self.builder.checkpoint()
    }

    pub(crate) fn start_node_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder
            .start_node_at(checkpoint, Lang::kind_to_raw(kind));
    }

    /// Deliberately does *not* flush pending trivia: trivia between this node's last real token
    /// and whatever comes next (another token, or EOF) isn't necessarily this node's — it could
    /// just as well belong to the parent, once the next `bump()` (wherever that ends up) flushes
    /// it into whatever's open *then*. The one exception is trivia trailing the very last real
    /// token in the whole file, which has no following `bump()` to flush it — `items::root`
    /// handles that explicitly (one final `flush_trivia()` right before `ROOT`'s own
    /// `finish_node`, since `ROOT` is the only node that legitimately "ends at EOF").
    pub(crate) fn finish_node(&mut self) {
        self.builder.finish_node();
    }
}
