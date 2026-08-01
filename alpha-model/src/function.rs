//! Phase 2 (partial): resolving `Function`/`ArrayFunction` calculator literals — Alpha's
//! `(idx -> exprs)` and `[exprs]` access-function notations — into real isl `MultiAff` objects.
//!
//! This is the piece phase 1 deliberately left unsupported (`eval_calc_expr`'s `Function`/
//! `ArrayFunction` branches report `UnsupportedCalculatorOp`, since they need per-equation
//! ambient index-name context phase 1's interface-only scope doesn't have — see `resolve.rs`'s
//! module doc). `Function` is self-contained (it declares its own input tuple, `(i,j -> ...)`)
//! and doesn't need that context at all; `ArrayFunction` (`X[i+1,j-1]`) does — the ambient index
//! names (from the enclosing equation's LHS, a `with` clause, or a reduction's projection — see
//! the source Java's `contextHistory` stack) become its implicit input tuple.
//!
//! Deliberately *not yet* threading the full context-stack the source system's
//! `JNIDomainCalculator` maintains (pushed/popped at `RestrictExpression`/`SelectExpression`/
//! `AbstractReduceExpression`/`ConvolutionExpression`/`UseEquation`) — callers pass whatever
//! index names are in scope at the call site explicitly, one level at a time, rather than this
//! module managing a stack itself. Each caller in `alpha-model` is responsible for extending the
//! context correctly at the points the source system does (e.g. a reduction's projection input
//! names get added to its body's scope) as those callers are built.

use crate::diagnostic::Diagnostic;
use crate::resolve::Resolver;
use alpha_syntax::ast::{self, AstNode, CalcExpr, FnExpr};
use alpha_syntax::syntax_kind::SyntaxNode;
use isl::MultiAff;

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn isl_err(e: isl::IslError, node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: e.message,
        start,
        end,
    }
}

impl Resolver<'_> {
    /// Resolves a `Function` (`(i,j -> i+1,j-1)`) or `ArrayFunction` (`[i+1,j-1]`) into a
    /// `MultiAff`. `index_names` is the ambient context `ArrayFunction` implicitly takes as its
    /// input tuple — ignored for `Function`, which declares its own.
    pub fn eval_function(
        &self,
        calc: &CalcExpr,
        index_names: &[String],
    ) -> Result<MultiAff, Diagnostic> {
        match calc {
            CalcExpr::Function(f) => {
                let inputs: Vec<String> = f.index_names().map(|t| t.text().to_string()).collect();
                let mut exprs = Vec::new();
                for e in f.exprs() {
                    exprs.push(render_fn_expr(&e)?);
                }
                let text = format!("{{ [{}] -> [{}] }}", inputs.join(","), exprs.join(","));
                MultiAff::read_from_str(&self.ctx, &self.with_param_prefix(&text))
                    .map_err(|e| isl_err(e, f.syntax()))
            }
            CalcExpr::ArrayFunction(af) => {
                let elements = self.array_function_elements(af);
                let text = format!(
                    "{{ [{}] -> [{}] }}",
                    index_names.join(","),
                    elements.join(",")
                );
                MultiAff::read_from_str(&self.ctx, &self.with_param_prefix(&text))
                    .map_err(|e| isl_err(e, af.syntax()))
            }
            other => {
                let (start, end) = range_of(other.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "function position".to_string(),
                    operand_kind: "non-function calculator expression".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// Resolves an `ArrayPolynomial` (`{ poly (: constraints)? (; poly (: constraints)?)* }`)
    /// into a `PwQPolynomial`, given the ambient index-name context as its implicit input tuple —
    /// the polynomial-literal analog of `eval_function`'s `ArrayFunction` case (§6, phase 3's
    /// `IndexExpression`/`PolynomialIndexExpression` handling in [`crate::domain`]).
    ///
    /// Each `;`-separated piece needs its *own* `[ctx] ->` prefix synthesized (`polynomial2.alpha`
    /// in the real fixture corpus — `val { N^2+1/2*i : N>1; i : N=0}` — has two pieces sharing one
    /// pair of braces; naively wrapping the whole inner text in a single `[ctx] -> (...)` produces
    /// invalid isl syntax on the second piece onward).
    pub fn eval_polynomial_in_context(
        &self,
        ap: &ast::ArrayPolynomial,
        index_names: &[String],
    ) -> Result<isl::PwQPolynomial, Diagnostic> {
        let text = self.text_of(ap.syntax());
        let inner = text
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or("")
            .trim();
        let prefix = format!("[{}] ->", index_names.join(","));
        let pieces: Vec<String> = inner
            .split(';')
            .map(|piece| format!("{prefix} {}", piece.trim()))
            .collect();
        let full = format!("{{ {} }}", pieces.join("; "));
        isl::PwQPolynomial::read_from_str(&self.ctx, &self.with_param_prefix(&full))
            .map_err(|e| isl_err(e, ap.syntax()))
    }

    /// `ArrayFunction`'s raw per-element affine-expression text, comma-split at the top nesting
    /// level (bracket-depth-tracked, unlike `ast::ArrayFunction::raw_elements`'s plain split —
    /// needed because an element itself can contain a nested `(...)`, e.g. `floor(i/2)`), with
    /// constant substitution applied (see `Resolver::text_of`).
    fn array_function_elements(&self, af: &ast::ArrayFunction) -> Vec<String> {
        let text = self.text_of(af.syntax());
        let inner = text
            .trim()
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or("")
            .trim();
        if inner.is_empty() {
            return Vec::new();
        }
        let mut elements = Vec::new();
        let mut depth = 0i32;
        let mut current = String::new();
        for c in inner.chars() {
            match c {
                '(' | '[' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    elements.push(std::mem::take(&mut current).trim().to_string());
                }
                _ => current.push(c),
            }
        }
        if !current.trim().is_empty() {
            elements.push(current.trim().to_string());
        }
        elements
    }
}

/// Renders the tiny `AlphaFunction` expression sub-grammar (see `alpha-syntax`'s
/// `parser::calculator` module) back into isl's own affine-expression syntax — trivial for
/// `FnLiteral` (already isl-compatible token soup, e.g. `2j`), a direct `floor(...)` call for
/// `FnFloor` (isl supports `floor` natively in its C-like expression syntax), and a parenthesized
/// infix expression for `FnBinaryExpr`.
fn render_fn_expr(e: &FnExpr) -> Result<String, Diagnostic> {
    match e {
        FnExpr::Literal(l) => Ok(l.text()),
        FnExpr::Floor(f) => {
            let inner = f
                .operand()
                .ok_or_else(|| missing_operand(f.syntax()))
                .and_then(|o| render_fn_expr(&o))?;
            Ok(format!("floor({inner})"))
        }
        FnExpr::Binary(b) => {
            let lhs = b
                .lhs()
                .ok_or_else(|| missing_operand(b.syntax()))
                .and_then(|o| render_fn_expr(&o))?;
            let rhs = b
                .rhs()
                .ok_or_else(|| missing_operand(b.syntax()))
                .and_then(|o| render_fn_expr(&o))?;
            let op = b
                .operator()
                .map(|t| t.text().to_string())
                .unwrap_or_default();
            Ok(format!("({lhs} {op} {rhs})"))
        }
    }
}

fn missing_operand(node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::UndefinedReference {
        name: "<missing operand>".to_string(),
        start,
        end,
    }
}
