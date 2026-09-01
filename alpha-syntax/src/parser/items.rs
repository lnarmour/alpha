//! Top-level structure: `AlphaRoot`, imports, constants, external functions, packages, systems,
//! variables, and equations.

use super::calculator;
use super::expr::{alpha_expr, recover_stmt};
use super::{qualified_name, Parser};
use crate::syntax_kind::SyntaxKind;
use crate::token_kind::TokenKind as T;

const ITEM_RECOVERY: &[T] = &[
    T::KwConstant,
    T::KwExternal,
    T::KwImport,
    T::KwPackage,
    T::KwAffine,
    T::RBrace,
];

pub(super) fn root(p: &mut Parser) {
    p.start_node(SyntaxKind::ROOT);
    while p.at(T::KwImport) {
        p.tick();
        import(p);
    }
    while !p.at_eof() {
        p.tick();
        if !top_level_element(p) {
            p.error("expected a constant, external function, package, or system declaration");
            p.recover_until(ITEM_RECOVERY);
            // `recover_until` won't consume a lone unexpected token that IS itself one of the
            // terminators (e.g. a stray `}`); force progress so `root`'s loop can't spin.
            if !p.at_eof() && p.at_any(ITEM_RECOVERY) && !p.at(T::RBrace) {
                // one of our own start keywords: loop will retry it as a fresh item, fine.
            } else if p.at(T::RBrace) {
                p.error("unexpected '}'");
                p.bump();
            }
        }
    }
    // `ROOT` is the one node that legitimately "ends at EOF" — flush explicitly so trivia
    // trailing the last real token (e.g. a final blank line) lands as its last child instead of
    // being lost to (or misattributed by) `finish_node`, which deliberately doesn't flush on its
    // own; see `Parser::finish_node`'s doc.
    p.flush_trivia();
    p.finish_node();
}

fn import(p: &mut Parser) {
    p.start_node(SyntaxKind::IMPORT);
    p.bump(); // import
    qualified_name(p);
    if p.at(T::DotStar) {
        p.bump();
    }
    p.finish_node();
}

/// One of `AlphaConstant | ExternalFunction | AlphaPackage | AlphaSystem`. Returns `false`
/// (consuming nothing) if the current token doesn't start any of them, so callers can report
/// "expected ..." and recover.
fn top_level_element(p: &mut Parser) -> bool {
    match p.current() {
        Some(T::KwConstant) => {
            alpha_constant(p);
            true
        }
        Some(T::KwExternal) => {
            external_function(p);
            true
        }
        Some(T::KwPackage) => {
            alpha_package(p);
            true
        }
        Some(T::KwAffine) => {
            alpha_system(p);
            true
        }
        _ => false,
    }
}

fn alpha_constant(p: &mut Parser) {
    p.start_node(SyntaxKind::ALPHA_CONSTANT);
    p.bump(); // constant
    p.expect(T::Ident);
    p.expect(T::Eq);
    p.expect(T::IntNumber);
    p.finish_node();
}

fn external_function(p: &mut Parser) {
    p.start_node(SyntaxKind::EXTERNAL_FUNCTION);
    p.bump(); // external
    p.expect(T::Ident);
    p.expect(T::LParen);
    if p.at(T::IntNumber) {
        p.bump();
    } else if !p.at(T::RParen) {
        multiplicity(p);
        while p.at(T::Comma) {
            p.bump();
            multiplicity(p);
        }
    }
    p.expect(T::RParen);
    if p.at(T::Arrow) {
        p.bump();
        if p.at(T::LParen) {
            p.bump();
            if !p.at(T::RParen) {
                multiplicity(p);
                while p.at(T::Comma) {
                    p.bump();
                    multiplicity(p);
                }
            }
            p.expect(T::RParen);
        } else {
            multiplicity(p);
        }
    }
    p.finish_node();
}

fn multiplicity(p: &mut Parser) {
    if p.at_any(&[T::KwLinear, T::KwUnrestricted]) {
        p.bump();
    } else {
        p.error("expected 'linear' or 'unrestricted'");
        p.tick();
    }
}

fn alpha_package(p: &mut Parser) {
    p.start_node(SyntaxKind::ALPHA_PACKAGE);
    p.bump(); // package
    qualified_name(p);
    p.expect(T::LBrace);
    while !p.at(T::RBrace) && !p.at_eof() {
        p.tick();
        if !top_level_element(p) {
            p.error("expected a constant, external function, package, or system declaration");
            p.recover_until(&[
                T::RBrace,
                T::KwConstant,
                T::KwExternal,
                T::KwPackage,
                T::KwAffine,
            ]);
            if p.at_any(&[T::KwConstant, T::KwExternal, T::KwPackage, T::KwAffine]) {
                continue;
            }
            break;
        }
    }
    p.expect(T::RBrace);
    p.finish_node();
}

// --- systems ---

const SYSTEM_BODY_START: &[T] = &[T::KwWhen, T::KwElse, T::KwLet];

fn alpha_system(p: &mut Parser) {
    p.start_node(SyntaxKind::SYSTEM);
    p.bump(); // affine
    p.expect(T::Ident); // SystemName
    calculator::param_domain(p);

    if p.at(T::KwDefine) {
        define_section(p);
    }
    if p.at(T::KwInputs) {
        variable_section(p, SyntaxKind::INPUTS);
    }
    if p.at(T::KwOutputs) {
        variable_section(p, SyntaxKind::OUTPUTS);
    }
    if p.at(T::KwLocals) {
        variable_section(p, SyntaxKind::LOCALS);
    }
    if p.at(T::KwOver) {
        p.bump();
        calculator::calculator_expr(p);
        p.expect(T::KwWhile);
        p.expect(T::LParen);
        alpha_expr(p);
        p.expect(T::RParen);
    }
    while p.at_any(SYSTEM_BODY_START) {
        p.tick();
        system_body(p);
    }
    p.expect(T::Dot);
    p.finish_node();
}

fn define_section(p: &mut Parser) {
    p.start_node(SyntaxKind::DEFINE_SECTION);
    p.bump(); // define
    while p.at(T::Ident) {
        p.tick();
        polyhedral_object(p);
    }
    p.finish_node();
}

fn polyhedral_object(p: &mut Parser) {
    p.start_node(SyntaxKind::POLYHEDRAL_OBJECT);
    p.bump(); // name
    p.expect(T::Eq);
    calculator::calculator_expr(p);
    p.finish_node();
}

fn variable_section(p: &mut Parser, wrapper: SyntaxKind) {
    p.start_node(wrapper);
    p.bump(); // inputs | outputs | locals
    while p.at_any(&[T::Ident, T::KwFuzzy, T::KwLinear]) {
        p.tick();
        variable_clause(p);
    }
    p.finish_node();
}

/// One `Variable | VariableNameOnly,...,Variable | FuzzyVariable | FuzzyVariableNameOnly,...,
/// FuzzyVariable` group: a comma-separated run of bare names ending in one fully-specified
/// entry that supplies the domain (and range, if fuzzy) the bare names inherit — see this
/// module's design note above `clause_is_fuzzy` for why fuzziness is decided by lookahead.
fn variable_clause(p: &mut Parser) {
    let is_fuzzy = clause_is_fuzzy(p);
    let mut has_linear_prefix = p.at(T::KwLinear);
    let kind = if is_fuzzy {
        SyntaxKind::FUZZY_VARIABLE
    } else {
        SyntaxKind::VARIABLE
    };
    loop {
        p.tick();
        p.start_node(kind);
        if has_linear_prefix {
            p.bump();
            has_linear_prefix = false;
        }
        if is_fuzzy && p.at(T::KwFuzzy) {
            p.bump();
        }
        p.expect(T::Ident);
        if p.at(T::Colon) {
            p.bump();
            calculator::calculator_expr(p);
            if is_fuzzy {
                p.expect(T::Arrow);
                calculator::calculator_expr(p);
            }
            if p.at(T::KwOf) {
                p.start_node(SyntaxKind::ELEMENT_TYPE);
                p.bump();
                if p.at_any(&[T::KwBool, T::KwInt, T::KwReal, T::KwQubit]) {
                    p.bump();
                } else {
                    p.error("expected 'bool', 'int', 'real', or 'qubit' after 'of'");
                }
                p.finish_node();
            }
            p.finish_node();
            if p.at(T::Semicolon) {
                p.bump();
            }
            return;
        }
        p.finish_node(); // bare name; domain inherited from the terminating entry (alpha-model)
        if p.at(T::Comma) {
            p.bump();
            continue;
        }
        p.error("expected ':' (domain) or ',' (more names) after variable name");
        return;
    }
}

/// `Alpha.xtext`'s `VariableNameOnly`/`FuzzyVariableNameOnly` are syntactically identical
/// (`name=ID`, no marker) — only the *terminating* entry of a comma-separated group carries
/// `fuzzy`/`:`/`->`, so the source grammar itself needs lookahead all the way to that entry to
/// know whether the whole group is fuzzy. This scans past the `Ident ','` run to find it.
fn clause_is_fuzzy(p: &Parser) -> bool {
    let mut i = usize::from(p.at(T::KwLinear));
    while p.nth(i) == Some(T::Ident) && p.nth(i + 1) == Some(T::Comma) {
        i += 2;
    }
    p.nth(i) == Some(T::KwFuzzy)
}

fn system_body(p: &mut Parser) {
    p.start_node(SyntaxKind::SYSTEM_BODY);
    if p.at(T::KwWhen) {
        p.bump();
        calculator::array_domain(p);
    } else if p.at(T::KwElse) {
        p.bump();
    }
    p.expect(T::KwLet);
    while p.at_any(&[T::Ident, T::LParen, T::KwOver, T::KwWith]) {
        p.tick();
        equation(p);
    }
    p.finish_node();
}

// --- equations ---

fn equation(p: &mut Parser) {
    // `UseEquation`'s optional `('over' ...)? ('with' ...)? ':'` prefix means it can start with
    // `over`/`with` too, not just its mandatory `(outputExprs)` — e.g. `with [i,j] : (C[i,j]) =
    // matmult[N/2](...)` in the recursive-subsystems fixtures has no `over` clause at all.
    if p.at_any(&[T::LParen, T::KwOver, T::KwWith]) {
        use_equation(p);
    } else {
        standard_equation(p);
    }
}

fn standard_equation(p: &mut Parser) {
    p.start_node(SyntaxKind::STANDARD_EQUATION);
    p.expect(T::Ident); // variable reference
    if p.at(T::LBrack) {
        p.bump();
        index_name_list(p);
        p.expect(T::RBrack);
    }
    p.expect(T::Eq);
    alpha_expr(p);
    p.expect(T::Semicolon);
    p.finish_node();
    recover_stmt_if_stuck(p);
}

/// `UseEquation`'s optional `('over' calc-expr)? ('with' ('[' idx,... ']')?)? ':'` prefix, then
/// `'(' outputExprs ')' '=' system callParams '(' inputExprs ')' ';'`.
fn use_equation(p: &mut Parser) {
    p.start_node(SyntaxKind::USE_EQUATION);
    if p.at_any(&[T::KwOver, T::KwWith]) {
        if p.at(T::KwOver) {
            p.bump();
            calculator::calculator_expr(p);
        }
        if p.at(T::KwWith) {
            p.bump();
            if p.at(T::LBrack) {
                p.bump();
                index_name_list(p);
                p.expect(T::RBrack);
            }
        }
        p.expect(T::Colon);
    }
    p.expect(T::LParen);
    expr_list_until(p, T::RParen);
    p.expect(T::RParen);
    p.expect(T::Eq);
    qualified_name(p); // system reference
    if p.at(T::LBrack) {
        calculator::array_function(p); // callParamsExpr: JNIFunctionInArrayNotation
    }
    p.expect(T::LParen);
    expr_list_until(p, T::RParen);
    p.expect(T::RParen);
    p.expect(T::Semicolon);
    p.finish_node();
    recover_stmt_if_stuck(p);
}

fn index_name_list(p: &mut Parser) {
    while p.at(T::Ident) {
        p.tick();
        p.bump();
        if p.at(T::Comma) {
            p.bump();
        } else {
            break;
        }
    }
}

fn expr_list_until(p: &mut Parser, end: T) {
    if p.at(end) {
        return;
    }
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

/// If an equation's own `expect`s left us stuck mid-statement (not at the start of the next
/// equation, system body keyword, or system terminator), recover to a safe boundary rather than
/// let a malformed equation cascade into every equation after it.
fn recover_stmt_if_stuck(p: &mut Parser) {
    let at_boundary = p.at_eof()
        || p.at_any(&[
            T::Ident,
            T::LParen,
            T::KwOver,
            T::KwWith,
            T::KwWhen,
            T::KwElse,
            T::KwLet,
            T::Dot,
        ]);
    if !at_boundary {
        recover_stmt(p);
    }
}
