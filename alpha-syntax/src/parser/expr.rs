//! `AlphaExpression` and friends: the full expression grammar. See `syntax_kind.rs`'s module doc
//! for which near-duplicate grammar rules (the 8 reduce/argreduce variants, the 3 index-flavored
//! expressions, plain vs. fuzzy dependence) collapse into one CST node kind here.
//!
//! Precedence chain, loosest to tightest (mirrors `Alpha.xtext` exactly): `if`/`restrict`/`auto`
//! (all at the very top, mutually exclusive by first token) → or → and → relational → additive →
//! multiplicative → min/max → unary-or-terminal.
//!
//! One deliberate departure from the source grammar, already flagged in `token_kind.rs`: since
//! this lexer always lexes numbers unsigned and `-` as its own token, a `Minus` immediately
//! followed by a number *at a position where a fresh terminal expression is expected* is treated
//! as a negative literal (folded into the `INT_LIT`/`REAL_LIT` node) rather than a `UnaryExpr`.
//! This exactly reproduces the source grammar's actual behavior (where `UnaryExpression`'s operand
//! grammar, `AlphaUnaryTerminalExpression`, explicitly excludes `ConstantExpression` — so `-5` was
//! never reachable as `UnaryExpression(-, 5)` there either, only ever as one signed literal
//! token) — it's implemented at the parser level here instead of the lexer level, that's all.

use super::calculator;
use super::{at_qualified_name_call, qualified_name, Parser};
use crate::syntax_kind::SyntaxKind;
use crate::token_kind::TokenKind as T;

const STMT_RECOVERY: &[T] = &[T::Semicolon, T::Dot, T::KwLet, T::RBrace];

/// Parses an `AlphaExpression`, recovering to a statement boundary on failure so one malformed
/// expression doesn't cascade into the rest of the file.
pub(super) fn alpha_expr(p: &mut Parser) {
    if p.at_eof() {
        p.error("expected an expression");
        return;
    }
    top_level_expr(p);
}

fn top_level_expr(p: &mut Parser) {
    match p.current() {
        Some(T::KwIf) => if_expr(p),
        Some(T::KwAuto) => auto_restrict_expr(p),
        Some(T::LBrace) => restrict_expr(p),
        _ => or_expr(p),
    }
}

fn if_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::IF_EXPR);
    p.bump(); // if
    alpha_expr(p);
    if p.expect(T::KwThen) {
        alpha_expr(p);
    }
    if p.expect(T::KwElse) {
        alpha_expr(p);
    }
    p.finish_node();
}

fn auto_restrict_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::AUTO_RESTRICT_EXPR);
    p.bump(); // auto
    p.expect(T::Colon);
    alpha_expr(p);
    p.finish_node();
}

/// `RestrictExpression`: both source-grammar alternatives start with a single `{...}` group
/// (see the long design note in an earlier revision of this file / the module doc) — the
/// interior is either raw ISL domain syntax (has a top-level `:`) or an arbitrary nested
/// `CalculatorExpression` (doesn't).
fn restrict_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::RESTRICT_EXPR);
    if calculator::looks_like_raw_domain(p) {
        calculator::domain(p);
    } else {
        p.expect(T::LBrace);
        calculator::calculator_expr(p);
        p.expect(T::RBrace);
    }
    p.expect(T::Colon);
    alpha_expr(p);
    p.finish_node();
}

fn or_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    and_expr(p);
    while p.at_any(&[T::KwOr, T::KwXor]) {
        p.tick();
        p.bump();
        and_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

fn and_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    relational_expr(p);
    while p.at(T::KwAnd) {
        p.tick();
        p.bump();
        relational_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

const RELATIONAL_OPS: &[T] = &[T::Eq, T::NotEq, T::GtEq, T::Gt, T::Lt, T::LtEq];

fn relational_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    additive_expr(p);
    while p.at_any(RELATIONAL_OPS) {
        p.tick();
        p.bump();
        additive_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

fn additive_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    multiplicative_expr(p);
    while p.at_any(&[T::Plus, T::Minus]) {
        p.tick();
        p.bump();
        multiplicative_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

fn multiplicative_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    minmax_expr(p);
    // Note: no '%' here — `AAdditiveOP`'s sibling `AMultiplicativeOP` at this level is only
    // '*'/'/'  in the source grammar (unlike the tiny `AlphaFunction` sub-grammar's
    // `AISLMultiplicativeOperator`, which does include '%' — see `calculator.rs`).
    while p.at_any(&[T::Star, T::Slash]) {
        p.tick();
        p.bump();
        minmax_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

fn minmax_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    unary_or_terminal_expr(p);
    while p.at_any(&[T::KwMin, T::KwMax]) {
        p.tick();
        p.bump();
        unary_or_terminal_expr(p);
        p.start_node_at(cp, SyntaxKind::BINARY_EXPR);
        p.finish_node();
    }
}

fn unary_or_terminal_expr(p: &mut Parser) {
    match p.current() {
        Some(T::KwNot) => {
            p.start_node(SyntaxKind::UNARY_EXPR);
            p.bump();
            unary_terminal_expr(p);
            p.finish_node();
        }
        Some(T::Minus) if p.nth(1) != Some(T::IntNumber) && p.nth(1) != Some(T::FloatNumber) => {
            p.start_node(SyntaxKind::UNARY_EXPR);
            p.bump();
            unary_terminal_expr(p);
            p.finish_node();
        }
        _ => terminal_expr(p),
    }
}

/// The full `AlphaTerminalExpression` alternative set — everything a fresh expression position
/// can start with.
fn terminal_expr(p: &mut Parser) {
    match p.current() {
        Some(T::Minus) | Some(T::IntNumber) | Some(T::FloatNumber) | Some(T::KwTrue)
        | Some(T::KwFalse) => constant_expr_maybe_dependence(p),
        Some(T::KwCase) => case_expr(p),
        Some(T::KwReduce) | Some(T::KwArgReduce) => reduce_expr(p),
        Some(T::KwConv) => convolution_expr(p),
        Some(T::KwSelect) => select_expr(p),
        Some(T::KwVal) => index_expr(p),
        Some(T::LBrack2) => index_expr_bare_fuzzy(p),
        Some(T::LParen) => paren_or_dependence_expr(p),
        Some(t) if is_multi_arg_op(t) && p.nth(1) == Some(T::LParen) => multi_arg_expr(p),
        Some(T::Ident) if at_qualified_name_call(p) => multi_arg_expr(p),
        Some(T::Ident) => variable_expr_maybe_dependence(p),
        _ => p.error("expected an expression"),
    }
}

/// The restricted terminal set `UnaryExpression`'s operand (`AlphaUnaryTerminalExpression`)
/// allows: everything `terminal_expr` allows *except* bare `ConstantExpression` and
/// `DependenceExpression`/`FuzzyDependenceExpression` (those need explicit parens as an operand
/// of `not`/unary `-`, per the source grammar).
fn unary_terminal_expr(p: &mut Parser) {
    match p.current() {
        Some(T::KwCase) => case_expr(p),
        Some(T::KwReduce) | Some(T::KwArgReduce) => reduce_expr(p),
        Some(T::KwConv) => convolution_expr(p),
        Some(T::KwSelect) => select_expr(p),
        Some(T::KwVal) => index_expr(p),
        Some(T::LBrack2) => index_expr_bare_fuzzy(p),
        Some(T::LParen) => paren_expr(p),
        Some(t) if is_multi_arg_op(t) && p.nth(1) == Some(T::LParen) => multi_arg_expr(p),
        Some(T::Ident) if at_qualified_name_call(p) => multi_arg_expr(p),
        Some(T::Ident) => variable_expr_bare(p),
        _ => p.error(
            "expected an expression (constants and dependences need parentheses here, e.g. `-(X[i])`)",
        ),
    }
}

fn is_multi_arg_op(t: T) -> bool {
    matches!(
        t,
        T::KwMin
            | T::KwMax
            | T::KwProd
            | T::KwSum
            | T::KwAnd
            | T::KwOr
            | T::KwXor
            | T::Plus
            | T::Star
    )
}

/// `(` is ambiguous between a parenthesized `AlphaExpression` and a `JNIFunction`/`FuzzyFunction`
/// destined for `f @ expr` (`DependenceExpression`/`FuzzyDependenceExpression`) — the source
/// grammar has no `->` anywhere in `AlphaExpression` itself, so a top-level `->` before the
/// matching `)` unambiguously signals the latter.
fn paren_or_dependence_expr(p: &mut Parser) {
    if calculator::contains_top_level_arrow(p) {
        p.start_node(SyntaxKind::DEPENDENCE_EXPR);
        function_or_fuzzy_function(p);
        p.expect(T::At);
        // The grammar's `expr=AlphaTerminalExpression` here is the *full* terminal set (unlike
        // `UnaryExpression`'s operand) — e.g. `(i->)@-1` (a negative-literal operand) and
        // `(i,x->x+i,x)@(i,x->x)@W` (a chained dependence as the operand) are both real fixtures.
        terminal_expr(p);
        p.finish_node();
    } else {
        paren_expr(p);
    }
}

fn paren_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::PAREN_EXPR);
    p.bump(); // (
    alpha_expr(p);
    p.expect(T::RParen);
    p.finish_node();
}

/// `JNIFunction` (`(idx -> exprs)`) vs `FuzzyFunction` (`([[idx]->[exprs]]->[exprs])`) — both
/// start with `(`; distinguished by whether a `[` immediately follows.
fn function_or_fuzzy_function(p: &mut Parser) {
    if p.nth(1) == Some(T::LBrack) {
        calculator::fuzzy_function(p);
    } else {
        calculator::function(p);
    }
}

/// `expr=ConstantExpression functionExpr=JNIFunctionInArrayNotation` — the grammar allows only
/// the plain array-notation dependence after a constant, not the fuzzy `[[...]]` form (that one's
/// restricted to `VariableExpression` — see `variable_expr_maybe_dependence`).
fn constant_expr_maybe_dependence(p: &mut Parser) {
    let cp = p.checkpoint();
    constant_expr(p);
    if p.at(T::LBrack) {
        calculator::array_function(p);
        p.start_node_at(cp, SyntaxKind::DEPENDENCE_EXPR);
        p.finish_node();
    }
}

fn constant_expr(p: &mut Parser) {
    match p.current() {
        Some(T::KwTrue) | Some(T::KwFalse) => {
            p.start_node(SyntaxKind::BOOL_LIT);
            p.bump();
            p.finish_node();
        }
        Some(T::Minus) => {
            let is_float = p.nth(1) == Some(T::FloatNumber);
            p.start_node(if is_float {
                SyntaxKind::REAL_LIT
            } else {
                SyntaxKind::INT_LIT
            });
            p.bump(); // -
            if !p.at_any(&[T::IntNumber, T::FloatNumber]) {
                p.error("expected a number after '-'");
            } else {
                p.bump();
            }
            p.finish_node();
        }
        Some(T::IntNumber) => {
            p.start_node(SyntaxKind::INT_LIT);
            p.bump();
            p.finish_node();
        }
        Some(T::FloatNumber) => {
            p.start_node(SyntaxKind::REAL_LIT);
            p.bump();
            p.finish_node();
        }
        _ => p.error("expected true, false, or a number"),
    }
}

fn case_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::CASE_EXPR);
    p.bump(); // case
    if p.at(T::Ident) {
        p.bump();
    }
    p.expect(T::LBrace);
    while !p.at(T::RBrace) && !p.at_eof() {
        p.tick();
        alpha_expr(p);
        p.expect(T::Semicolon);
    }
    p.expect(T::RBrace);
    p.finish_node();
}

/// `reduce`/`argreduce`, named or external operator, plain or array-notation or fuzzy
/// projection — all fold into one `REDUCE_EXPR`; see `syntax_kind.rs`'s module doc.
fn reduce_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::REDUCE_EXPR);
    p.bump(); // reduce | argreduce
    p.expect(T::LParen);
    if is_named_reduction_op(p.current()) {
        p.bump();
    } else {
        qualified_name(p);
    }
    p.expect(T::Comma);
    projection_function(p);
    p.expect(T::Comma);
    alpha_expr(p);
    p.expect(T::RParen);
    p.finish_node();
}

fn is_named_reduction_op(t: Option<T>) -> bool {
    matches!(
        t,
        Some(T::KwMin)
            | Some(T::KwMax)
            | Some(T::KwProd)
            | Some(T::KwSum)
            | Some(T::KwAnd)
            | Some(T::KwOr)
            | Some(T::KwXor)
            | Some(T::Plus)
            | Some(T::Star)
    )
}

fn projection_function(p: &mut Parser) {
    match p.current() {
        Some(T::LParen) => function_or_fuzzy_function(p),
        Some(T::LBrack) => calculator::array_function(p),
        _ => p.error("expected a projection function"),
    }
}

fn convolution_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::CONVOLUTION_EXPR);
    p.bump(); // conv
    p.expect(T::LParen);
    calculator::calculator_expr(p);
    p.expect(T::Comma);
    alpha_expr(p);
    p.expect(T::Comma);
    alpha_expr(p);
    p.expect(T::RParen);
    p.finish_node();
}

fn select_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::SELECT_EXPR);
    p.bump(); // select
    calculator::calculator_expr(p);
    p.expect(T::KwFrom);
    terminal_expr(p); // full `AlphaTerminalExpression`, same note as `paren_or_dependence_expr`
    p.finish_node();
}

/// `val <function|array-function|polynomial|array-polynomial>`, or a bare `[[...]]` fuzzy
/// literal — all fold into `INDEX_EXPR`; see `syntax_kind.rs`'s module doc.
fn index_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::INDEX_EXPR);
    p.bump(); // val
    match p.current() {
        Some(T::LParen) => function_or_fuzzy_function(p),
        Some(T::LBrack) => calculator::array_function(p),
        Some(T::LBrace) => {
            if calculator::contains_top_level_arrow(p) {
                calculator::polynomial(p);
            } else {
                calculator::array_polynomial(p);
            }
        }
        _ => p.error("expected a function or polynomial after 'val'"),
    }
    p.finish_node();
}

fn index_expr_bare_fuzzy(p: &mut Parser) {
    p.start_node(SyntaxKind::INDEX_EXPR);
    calculator::array_fuzzy_function(p);
    p.finish_node();
}

fn multi_arg_expr(p: &mut Parser) {
    p.start_node(SyntaxKind::MULTI_ARG_EXPR);
    if is_named_reduction_op(p.current()) {
        p.bump();
    } else {
        qualified_name(p);
    }
    p.expect(T::LParen);
    if !p.at(T::RParen) {
        loop {
            p.tick();
            alpha_expr(p);
            if p.at(T::Comma) {
                p.bump();
            } else {
                break;
            }
        }
    }
    p.expect(T::RParen);
    p.finish_node();
}

fn variable_expr_bare(p: &mut Parser) {
    p.start_node(SyntaxKind::VARIABLE_EXPR);
    p.bump();
    p.finish_node();
}

fn variable_expr_maybe_dependence(p: &mut Parser) {
    let cp = p.checkpoint();
    variable_expr_bare(p);
    if p.at(T::LBrack) {
        calculator::array_function(p);
        p.start_node_at(cp, SyntaxKind::DEPENDENCE_EXPR);
        p.finish_node();
    } else if p.at(T::LBrack2) {
        calculator::array_fuzzy_function(p);
        p.start_node_at(cp, SyntaxKind::DEPENDENCE_EXPR);
        p.finish_node();
    }
}

pub(super) fn recover_stmt(p: &mut Parser) {
    p.recover_until(STMT_RECOVERY);
}
