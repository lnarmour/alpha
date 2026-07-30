//! The "calculator" layer: domain/relation/function/polynomial literals, and the small
//! `CalculatorExpression` algebra over them (`define X = <calc expr>`). See
//! `docs/rust-port-design.md` §1/§4/§6 in the workspace root.
//!
//! Domain/relation/polynomial literal *bodies* are raw-text-captured, not semantically parsed —
//! `Alpha.xtext` itself doesn't parse them either (its `AISLString`/`AISLExpression`/etc. rules
//! are permissive token-soup terminals); the real parsing happens later when that text is handed
//! to isl's own string parser (see `alpha-model`, not yet implemented). The one exception is the
//! `(idx -> exprs)` function-literal form (`JNIFunction`/`AlphaFunction`), which gets a real,
//! tiny recursive-descent parse — the source grammar's own comments explain this was added
//! specifically to disambiguate parenthesization, not because the expressions need semantic
//! understanding at parse time.
//!
//! Fuzzy-variable machinery (`FuzzyFunction` and friends) is a secondary, uncommon feature (see
//! the survey) and is captured wholesale as one raw-text span rather than broken into the full
//! `NestedFuzzyFunction`/`AffineFuzzyVariableUse` sub-structure `Alpha.xtext` defines — round-trips
//! losslessly, just with a flatter tree than the source grammar's, matching the "take liberties on
//! anything low-value" steer. Revisit if a real program needs the finer structure.

use super::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_kind::TokenKind as T;

/// Bumps tokens from the current opener through its matching closer (inclusive), tracking
/// nesting depth across *any* of `{ [ ( [[` / `} ] ) ]]` so interior brackets of any kind don't
/// confuse the scan — sufficient because domain/relation/polynomial literals are always a
/// single outer group (unlike `PARAM_DOMAIN`, which has two sequential groups and is handled
/// separately in `param_domain`). Does not start/finish a node — callers wrap as needed.
fn bump_balanced_group(p: &mut Parser) {
    debug_assert!(p.at_any(&[T::LBrace, T::LBrack, T::LParen, T::LBrack2]));
    let mut depth: i32 = 0;
    loop {
        p.tick();
        match p.current() {
            None => {
                p.error("unterminated literal");
                return;
            }
            Some(k) => {
                let is_open = matches!(k, T::LBrace | T::LBrack | T::LParen | T::LBrack2);
                let is_close = matches!(k, T::RBrace | T::RBrack | T::RParen | T::RBrack2);
                p.bump();
                if is_open {
                    depth += 1;
                } else if is_close {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
            }
        }
    }
}

fn capture_balanced(p: &mut Parser, wrapper: SyntaxKind, open: T) {
    p.start_node(wrapper);
    if p.at(open) {
        bump_balanced_group(p);
    } else {
        p.error(format!("expected {open:?}"));
    }
    p.finish_node();
}

/// `JNIDomain` (`AISLSet`): `{ [idx] : constraints (; [idx] : constraints)* } | {}`.
pub(super) fn domain(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::DOMAIN, T::LBrace);
}

/// `JNIDomainInArrayNotation` / `JNIParamDomainInArrayNotation`: `{ : constraints }` — the
/// `when`/`else` system-body guard, and one alternative of `RestrictExpression`.
pub(super) fn array_domain(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::ARRAY_DOMAIN, T::LBrace);
}

/// `JNIParamDomain` (`AParamDomain`): `(['idx',...] '->')? '{' ':' constraints '}'` — two
/// sequential groups, so handled explicitly rather than via `bump_balanced_group`'s
/// single-group assumption (a plain balanced-scan would stop at the first `]`, before ever
/// reaching the `{...}` part).
pub(super) fn param_domain(p: &mut Parser) {
    p.start_node(SyntaxKind::PARAM_DOMAIN);
    if p.at(T::LBrack) {
        p.bump();
        while p.at(T::Ident) {
            p.tick();
            p.bump();
            if p.at(T::Comma) {
                p.bump();
            } else {
                break;
            }
        }
        p.expect(T::RBrack);
        p.expect(T::Arrow);
    }
    if p.at(T::LBrace) {
        bump_balanced_group(p);
    } else {
        p.error("expected '{' starting a parameter domain");
    }
    p.finish_node();
}

/// `JNIRelation` (`AISLRelation`): `{ [idx] -> [idx] : constraints (; ...)* }`.
pub(super) fn relation(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::RELATION, T::LBrace);
}

/// `JNIPolynomial` (`AISLPWQPolynomial`): `{ [idx] -> poly (: constraints)? (; ...)* }`.
pub(super) fn polynomial(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::POLYNOMIAL, T::LBrace);
}

/// `JNIPolynomialInArrayNotation`: `{ poly (: constraints)? (; ...)* }`.
pub(super) fn array_polynomial(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::ARRAY_POLYNOMIAL, T::LBrace);
}

/// `JNIFunctionInArrayNotation`: `[ expr (, expr)* ]` — each element is raw `AISLExpression`
/// soup (an affine expression per output dimension), so the whole bracketed list is one
/// raw-captured group, same as the domain/relation forms above.
pub(super) fn array_function(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::ARRAY_FUNCTION, T::LBrack);
}

/// `FuzzyFunction`: `( <wrapped basic relation text> (; indirection)* )` — captured wholesale;
/// see the module doc for why the indirection sub-structure isn't broken out.
pub(super) fn fuzzy_function(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::FUZZY_FUNCTION, T::LParen);
}

/// `FuzzyFunctionInArrayNotation`: `[[ ... ]]`, same treatment.
pub(super) fn array_fuzzy_function(p: &mut Parser) {
    capture_balanced(p, SyntaxKind::ARRAY_FUZZY_FUNCTION, T::LBrack2);
}

/// `JNIFunction` (`AlphaFunction`): `( idx,idx,... -> expr, expr, ... )` — the one calculator
/// form that gets a real (tiny) recursive-descent parse rather than raw capture.
pub(super) fn function(p: &mut Parser) {
    p.start_node(SyntaxKind::FUNCTION);
    p.expect(T::LParen);
    // AIndexList: possibly-empty comma-separated identifiers.
    while p.at(T::Ident) {
        p.tick();
        p.bump();
        if p.at(T::Comma) {
            p.bump();
        } else {
            break;
        }
    }
    p.expect(T::Arrow);
    if !p.at(T::RParen) {
        loop {
            p.tick();
            fn_additive_expr(p);
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

// --- the tiny `AlphaFunction` expression sub-grammar ---
// Precedence chain exactly as `Alpha.xtext` defines it: additive -> multiplicative -> relational
// -> terminal (yes, relational binds *tighter* than multiplicative here — unusual, but that's
// what the source grammar specifies, and this sub-language is only ever used for simple affine
// index arithmetic in practice).

fn fn_additive_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    fn_multiplicative_expr(p);
    while p.at_any(&[T::Plus, T::Minus]) {
        p.tick();
        p.bump();
        fn_multiplicative_expr(p);
        p.start_node_at(cp, SyntaxKind::FN_BINARY_EXPR);
        p.finish_node();
    }
}

fn fn_multiplicative_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    fn_relational_expr(p);
    while p.at_any(&[T::Star, T::Slash, T::Percent]) {
        p.tick();
        p.bump();
        fn_relational_expr(p);
        p.start_node_at(cp, SyntaxKind::FN_BINARY_EXPR);
        p.finish_node();
    }
}

fn fn_relational_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    fn_terminal_expr(p);
    while p.at(T::Eq) {
        p.tick();
        p.bump();
        fn_terminal_expr(p);
        p.start_node_at(cp, SyntaxKind::FN_BINARY_EXPR);
        p.finish_node();
    }
}

fn fn_terminal_expr(p: &mut Parser) {
    match p.current() {
        Some(T::KwFloor) => {
            p.start_node(SyntaxKind::FN_FLOOR);
            p.bump();
            p.expect(T::LParen);
            fn_additive_expr(p);
            p.expect(T::RParen);
            p.finish_node();
        }
        Some(T::LParen) => {
            p.bump();
            fn_additive_expr(p);
            p.expect(T::RParen);
        }
        Some(T::Minus) | Some(T::Ident) | Some(T::IntNumber) => {
            // `AISLExpressionLiteral: '-'? (IndexName|INT|WS)+` in the source grammar — a
            // literal can be a *run* of adjacent number/identifier tokens with no operator
            // between them, e.g. `2j` (implicit-multiplication coefficient notation: "2 times
            // j"), not just a single token.
            p.start_node(SyntaxKind::FN_LITERAL);
            if p.at(T::Minus) {
                p.bump();
            }
            if !p.at_any(&[T::Ident, T::IntNumber]) {
                p.error("expected a name or number in function literal");
            } else {
                while p.at_any(&[T::Ident, T::IntNumber]) {
                    p.tick();
                    p.bump();
                }
            }
            p.finish_node();
        }
        _ => {
            p.error("expected a function-literal expression");
        }
    }
}

/// `CalculatorExpression`: the domain/relation/function algebra
/// (`domain X`, `X + Y`, `X @ Y`, `{ Y }`, `[N,N] as [i,j]`, ...).
pub(super) fn calculator_expr(p: &mut Parser) {
    let cp = p.checkpoint();
    calculator_unary_or_terminal(p);
    while is_binary_calc_op(p) {
        p.tick();
        p.bump();
        calculator_unary_or_terminal(p);
        p.start_node_at(cp, SyntaxKind::BINARY_CALC_EXPR);
        p.finish_node();
    }
}

fn is_binary_calc_op(p: &Parser) -> bool {
    p.at_any(&[
        T::KwCross,
        T::Plus,
        T::Minus,
        T::Star,
        T::At,
        T::KwIntersectRange,
        T::KwSubtractRange,
    ])
}

fn is_unary_calc_op(p: &Parser) -> bool {
    p.at_any(&[
        T::KwDomain,
        T::KwRange,
        T::KwComplement,
        T::KwAffineHull,
        T::KwPolyHull,
        T::KwReverse,
    ])
}

fn calculator_unary_or_terminal(p: &mut Parser) {
    if is_unary_calc_op(p) {
        p.start_node(SyntaxKind::UNARY_CALC_EXPR);
        p.bump();
        calculator_terminal(p);
        p.finish_node();
    } else {
        calculator_terminal(p);
    }
}

fn calculator_terminal(p: &mut Parser) {
    match p.current() {
        Some(T::LBrace) => {
            // Ambiguous prefix between `JNIDomain` (`{[idx]:...}`/`{}`), `JNIRelation`
            // (`{[idx]->[idx]:...}`), and `VariableDomain` (`{Name}`) — all start with `{` and
            // their raw interiors are token soup, so the only reliable discriminator without a
            // real ISL parser at hand is a bounded lookahead scan for a top-level `->` (relation)
            // before assuming "domain". `VariableDomain` is distinguished separately: it's
            // exactly `{ IDENT }`, nothing else, checked first since it's unambiguous.
            if is_variable_domain(p) {
                p.start_node(SyntaxKind::VARIABLE_DOMAIN);
                p.bump(); // {
                p.bump(); // ident
                p.expect(T::RBrace);
                p.finish_node();
            } else if contains_top_level_arrow(p) {
                relation(p);
            } else {
                domain(p);
            }
        }
        Some(T::LParen) => {
            // Either a parenthesized calculator expression, or a `JNIFunction` literal
            // (`(idx -> exprs)`) — disambiguated by scanning for a top-level `->` before the
            // matching `)`, same bounded-lookahead approach as above.
            if contains_top_level_arrow(p) {
                function(p);
            } else {
                p.start_node(SyntaxKind::CALC_PAREN_EXPR);
                p.bump();
                calculator_expr(p);
                p.expect(T::RParen);
                p.finish_node();
            }
        }
        Some(T::LBrack) => rectangular_domain(p),
        Some(T::Ident) => {
            p.start_node(SyntaxKind::DEFINED_OBJECT);
            p.bump();
            p.finish_node();
        }
        _ => p.error("expected a domain, relation, function, or defined-object reference"),
    }
}

/// `{ Name }` with nothing else inside — `VariableDomain`.
fn is_variable_domain(p: &Parser) -> bool {
    p.at(T::LBrace) && p.nth(1) == Some(T::Ident) && p.nth(2) == Some(T::RBrace)
}

/// Scans ahead (without consuming) for a top-level `->` before the matching close delimiter,
/// treating nested open/close pairs as raising/lowering depth so an inner relation's own `->`
/// doesn't get mistaken for the outer one. Used only to disambiguate `{domain}` vs `{relation}`
/// and `(calc-expr)` vs `(function-literal)` — both cases where the source grammar itself is
/// only unambiguous once you understand ISL's syntax, which the parser deliberately doesn't.
pub(super) fn contains_top_level_arrow(p: &Parser) -> bool {
    contains_top_level_before_close(p, T::Arrow)
}

/// Same idea, but scanning for a top-level `:` — used by `RestrictExpression` (see `expr.rs`) to
/// decide whether a `{...}` is raw ISL domain syntax (`JNIDomain`/`JNIDomainInArrayNotation`,
/// both of which have a `:` separating the index tuple/nothing from the constraint text) or an
/// arbitrary nested `CalculatorExpression` (`{myDefinedThing}`, `{[N,N] as [i,j]}`, `{A + B}`,
/// ...), which the source grammar's second `RestrictExpression` alternative also allows.
pub(super) fn looks_like_raw_domain(p: &Parser) -> bool {
    contains_top_level_before_close(p, T::Colon)
}

fn contains_top_level_before_close(p: &Parser, target: T) -> bool {
    let mut depth: i32 = 0;
    let mut i = 0;
    loop {
        let Some(k) = p.nth(i) else { return false };
        let is_open = matches!(k, T::LBrace | T::LBrack | T::LParen | T::LBrack2);
        let is_close = matches!(k, T::RBrace | T::RBrack | T::RParen | T::RBrack2);
        if is_open {
            depth += 1;
        } else if is_close {
            depth -= 1;
            if depth == 0 {
                return false;
            }
        } else if k == target && depth == 1 {
            return true;
        }
        i += 1;
        if i > 10_000 {
            return false; // pathological/unterminated input; let the real parse report it
        }
    }
}

/// `RectangularDomain`: `[N,N] as [i,j]` or `[0:N-1,...] as [...]`, `as [...]` optional. Only
/// reachable from `calculator_terminal` (`CalculatorExpressionTerminal` in the source grammar);
/// `JNIFunctionInArrayNotation`'s own `[expr,...]` shorthand is parsed separately, by `expr.rs`'s
/// call sites (`DependenceExpression`, `UseEquation`'s call-params), never through here.
fn rectangular_domain(p: &mut Parser) {
    p.start_node(SyntaxKind::RECTANGULAR_DOMAIN);
    bump_balanced_group(p); // the `[...]` bound list, raw-captured (AISLExpression soup)
    if p.at(T::KwAs) {
        p.bump();
        p.expect(T::LBrack);
        while p.at(T::Ident) {
            p.tick();
            p.bump();
            if p.at(T::Comma) {
                p.bump();
            } else {
                break;
            }
        }
        p.expect(T::RBrack);
    }
    p.finish_node();
}
