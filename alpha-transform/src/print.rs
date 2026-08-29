//! Three read-only renderings of an [`System`], ported from alpha-language's `alpha.model.util`:
//! - [`print_ast`] — an indented debug tree dump (every node's own kind, `expression_domain`/
//!   `context_domain`), ported from `PrintAST.xtend`.
//! - [`show`] — reconstructs Alpha-like source syntax from the model ("Show notation"), ported
//!   from `Show.xtend`.
//! - [`ashow`] — like [`show`], but renders a [`ExprKind::Dependence`] over a `Variable`/constant
//!   in array-index notation (`X[j]`) instead of `Show`'s point-free composition (`(...->j)@X`),
//!   and shows each equation's own ambient index names explicitly (`X[i,j] = ...`) — ported from
//!   `AShow.xtend`.
//!
//! **`show`/`ashow` output is meant to be pasted into a new `.alpha` file and reparsed** — not
//! merely readable. That rules out isl's own native `Display` for anything at a `MultiAff`
//! position: `alpha-syntax`'s grammar accepts *only* its own `(idx,idx,...->expr,...)`
//! function-literal syntax at a `Dependence`'s function / `Reduce`'s projection / `IndexFunction`'s
//! function (`alpha-syntax/src/parser/expr.rs`'s `paren_or_dependence_expr`/
//! `projection_function`/`index_expr`), never isl's `{ [i,j] -> [...] }` map syntax — so every
//! `MultiAff` here goes through [`function_str`]/[`multi_aff_output_exprs`], which reconstructs
//! real Alpha function-literal text term by term from `Aff`'s own coefficient/constant/denominator
//! accessors (new `isl` wrapper surface, see that crate's `val.rs` and `aff.rs`'s new methods),
//! including a `floor(...)`-derived function via `Aff::get_div`. Domains/relations/polynomials are
//! a different story: `alpha-syntax`'s domain/relation/polynomial literals are raw-captured and
//! handed *directly* to isl's own string parser at resolution time
//! (`alpha-syntax/src/parser/calculator.rs`'s module doc) — so isl's native `Set`/`Map`/
//! `PwQPolynomial` `Display` output is *already* valid, reparseable Alpha source there, no
//! reconstruction needed. `ashow`'s [`named_set`] only renames a `Set`'s dims for nicer output in
//! that case, never for correctness.
//!
//! None of these three attempt exact upstream paren-minimality (which parent contexts allow a
//! child to skip parens) — every `Dependence`/`Binary`/`Unary` operand that could otherwise be
//! ambiguous is always parenthesized. Slightly more verbose than upstream in some spots, always
//! unambiguous, and far simpler than threading parent-operator-precedence context through the
//! recursion.

use crate::ir::{Equation, Expr, ExprKind, Operator, System, SystemBody, UseEquation, Variable};
use isl::{Aff, DimType, Map, MultiAff, Set, Val};

// ---------------------------------------------------------------------------------------------
// print_ast: PrintAST.xtend port — an indented debug tree dump.
// ---------------------------------------------------------------------------------------------

fn push(out: &mut String, indent: usize, text: &str) {
    for line in text.lines() {
        out.push_str(&"    ".repeat(indent));
        out.push_str(line);
        out.push('\n');
    }
}

fn context_str(e: &Expr) -> String {
    e.context_domain
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "None".to_string())
}

/// An indented tree dump of `system` — every node's own kind, `expression_domain`, and (for
/// expressions) `context_domain`, mirroring `PrintAST.xtend`'s debugging dump. Mainly useful for
/// inspecting exactly what a pass did to the tree, not for reading a program back as source.
pub fn print_ast(system: &System) -> String {
    let mut out = String::new();
    push(&mut out, 0, &format!("System {:?}", system.name));
    push(
        &mut out,
        1,
        &format!("parameter_domain: {}", system.parameter_domain),
    );
    for (label, vars) in [
        ("inputs", &system.inputs),
        ("outputs", &system.outputs),
        ("locals", &system.locals),
    ] {
        if vars.is_empty() {
            continue;
        }
        push(&mut out, 1, label);
        for v in vars {
            push(
                &mut out,
                2,
                &format!(
                    "{} : {} multiplicity={:?} element_type={:?}",
                    v.name, v.domain, v.multiplicity, v.element_type
                ),
            );
        }
    }
    for (bi, body) in system.bodies.iter().enumerate() {
        push(
            &mut out,
            1,
            &format!("SystemBody[{bi}] domain={}", body.domain),
        );
        for eq in &body.equations {
            dump_equation(eq, &mut out, 2);
        }
    }
    out
}

fn dump_equation(eq: &Equation, out: &mut String, indent: usize) {
    match eq {
        Equation::Standard(s) => {
            push(
                out,
                indent,
                &format!(
                    "StandardEquation {} index_names={:?}",
                    s.variable, s.index_names
                ),
            );
            dump_expr(&s.expr, out, indent + 1);
        }
        Equation::Use(u) => {
            push(out, indent, &format!("UseEquation callee={}", u.callee));
            push(out, indent + 1, "outputs");
            for e in &u.output_exprs {
                dump_expr(e, out, indent + 2);
            }
            push(out, indent + 1, "inputs");
            for e in &u.input_exprs {
                dump_expr(e, out, indent + 2);
            }
        }
    }
}

fn dump_expr(e: &Expr, out: &mut String, indent: usize) {
    let exp = &e.expression_domain;
    let ctx = context_str(e);
    match &*e.kind {
        ExprKind::Variable(name) => push(
            out,
            indent,
            &format!("Variable {name:?} exp={exp} ctx={ctx}"),
        ),
        ExprKind::Bool(b) => push(out, indent, &format!("Bool {b} exp={exp} ctx={ctx}")),
        ExprKind::Int(s) => push(out, indent, &format!("Int {s:?} exp={exp} ctx={ctx}")),
        ExprKind::Real(s) => push(out, indent, &format!("Real {s:?} exp={exp} ctx={ctx}")),
        ExprKind::Dependence { function, operand } => {
            push(
                out,
                indent,
                &format!("Dependence function={function} exp={exp} ctx={ctx}"),
            );
            dump_expr(operand, out, indent + 1);
        }
        ExprKind::IndexFunction { function } => push(
            out,
            indent,
            &format!("IndexFunction function={function} exp={exp} ctx={ctx}"),
        ),
        ExprKind::IndexPolynomial { polynomial } => push(
            out,
            indent,
            &format!("IndexPolynomial polynomial={polynomial} exp={exp} ctx={ctx}"),
        ),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            push(out, indent, &format!("If exp={exp} ctx={ctx}"));
            push(out, indent + 1, "cond:");
            dump_expr(cond, out, indent + 2);
            push(out, indent + 1, "then:");
            dump_expr(then_branch, out, indent + 2);
            push(out, indent + 1, "else:");
            dump_expr(else_branch, out, indent + 2);
        }
        ExprKind::Restrict { domain, operand } => {
            push(
                out,
                indent,
                &format!("Restrict domain={domain} exp={exp} ctx={ctx}"),
            );
            dump_expr(operand, out, indent + 1);
        }
        ExprKind::AutoRestrict { operand } => {
            push(out, indent, &format!("AutoRestrict exp={exp} ctx={ctx}"));
            dump_expr(operand, out, indent + 1);
        }
        ExprKind::Case { name, branches } => {
            push(
                out,
                indent,
                &format!("Case name={name:?} exp={exp} ctx={ctx}"),
            );
            for (i, b) in branches.iter().enumerate() {
                push(out, indent + 1, &format!("branch[{i}]:"));
                dump_expr(b, out, indent + 2);
            }
        }
        ExprKind::Reduce {
            is_arg_reduce,
            operator,
            projection,
            body_context,
            body,
        } => {
            let tag = if *is_arg_reduce {
                "ArgReduce"
            } else {
                "Reduce"
            };
            push(
                out,
                indent,
                &format!(
                    "{tag} operator={} projection={projection} body_context={body_context:?} \
                     exp={exp} ctx={ctx}",
                    operator_text(operator)
                ),
            );
            dump_expr(body, out, indent + 1);
        }
        ExprKind::Convolution {
            kernel_domain,
            kernel_expr,
            data_expr,
        } => {
            push(
                out,
                indent,
                &format!("Convolution kernel_domain={kernel_domain} exp={exp} ctx={ctx}"),
            );
            push(out, indent + 1, "kernel:");
            dump_expr(kernel_expr, out, indent + 2);
            push(out, indent + 1, "data:");
            dump_expr(data_expr, out, indent + 2);
        }
        ExprKind::Select { relation, operand } => {
            push(
                out,
                indent,
                &format!("Select relation={relation} exp={exp} ctx={ctx}"),
            );
            dump_expr(operand, out, indent + 1);
        }
        ExprKind::MultiArg { operator, args } => {
            push(
                out,
                indent,
                &format!(
                    "MultiArg operator={} exp={exp} ctx={ctx}",
                    operator_text(operator)
                ),
            );
            for (i, a) in args.iter().enumerate() {
                push(out, indent + 1, &format!("arg[{i}]:"));
                dump_expr(a, out, indent + 2);
            }
        }
        ExprKind::Binary { operator, lhs, rhs } => {
            push(
                out,
                indent,
                &format!("Binary operator={operator:?} exp={exp} ctx={ctx}"),
            );
            push(out, indent + 1, "lhs:");
            dump_expr(lhs, out, indent + 2);
            push(out, indent + 1, "rhs:");
            dump_expr(rhs, out, indent + 2);
        }
        ExprKind::Unary { operator, operand } => {
            push(
                out,
                indent,
                &format!("Unary operator={operator:?} exp={exp} ctx={ctx}"),
            );
            dump_expr(operand, out, indent + 1);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// show / ashow: Show.xtend / AShow.xtend ports — Alpha-like source-syntax reconstruction.
// ---------------------------------------------------------------------------------------------

fn operator_text(op: &Operator) -> &str {
    match op {
        Operator::Named(s) | Operator::External(s) => s,
    }
}

fn variable_or_literal_text(e: &Expr) -> Option<String> {
    match &*e.kind {
        ExprKind::Variable(name) => Some(name.clone()),
        ExprKind::Bool(b) => Some(b.to_string()),
        ExprKind::Int(s) | ExprKind::Real(s) => Some(s.clone()),
        _ => None,
    }
}

/// This whole `Aff`'s numerator, given `whole_denom` (`Aff::denominator()`) — isl normalizes an
/// `Aff` to one shared denominator across every term, but a per-term `coefficient`/`constant`
/// `Val` can itself carry a smaller denominator (seen on a `Div`'s own defining `Aff` — an isl
/// invariant, not documented on the C API, confirmed empirically), so this always derives the
/// integer numerator relative to the *whole* `Aff`'s denominator rather than assuming `den_si() ==
/// 1` on every term.
fn term_numerator(coeff: &Val, whole_denom: i64) -> i64 {
    let den = coeff.den_si().max(1);
    coeff.num_si() * (whole_denom / den)
}

/// Appends one term (as `(is_positive, text)`, sign kept separate from `text` — see
/// [`join_terms`]) unless `coeff` is zero. `force_explicit_coeff`: `true` for a synthesized
/// `floor(...)` sub-term — a bare `-floor(...)` isn't valid Alpha function-literal syntax (the
/// grammar's leading-`-` literal rule only continues past a `Minus` token into `Ident`/`IntNumber`
/// runs, never into `floor`; `-1*floor(...)` sidesteps this by keeping the numeric literal, not
/// `floor`, adjacent to the `-`) — so a floor term always gets an explicit coefficient, even `1`.
fn push_term(terms: &mut Vec<(bool, String)>, coeff: i64, name: &str, force_explicit_coeff: bool) {
    if coeff == 0 {
        return;
    }
    let positive = coeff > 0;
    let mag = coeff.unsigned_abs();
    let text = if mag == 1 && !force_explicit_coeff {
        name.to_string()
    } else {
        format!("{mag}*{name}")
    };
    terms.push((positive, text));
}

/// Joins `terms` (each `(is_positive, text)`, `text` carrying no sign of its own) into Alpha's
/// additive-expression syntax — only the very first term's sign is a bare prefixed `-` (valid:
/// `fn_terminal_expr`'s literal rule *does* accept a leading `-` directly on a plain
/// identifier/number), every later term's sign is the infix `+`/`-` operator instead (so it never
/// depends on what kind of term follows).
fn join_terms(terms: &[(bool, String)]) -> String {
    let mut out = String::new();
    for (i, (positive, text)) in terms.iter().enumerate() {
        if i == 0 {
            if !*positive {
                out.push('-');
            }
        } else {
            out.push_str(if *positive { " + " } else { " - " });
        }
        out.push_str(text);
    }
    out
}

/// Formats `aff` as Alpha function-literal *body* syntax (the comma-separated expression, not the
/// `(names->...)` wrapper — [`multi_aff_output_exprs`] adds that where needed) — a linear
/// combination of `param_names`/`in_names` (positional: `aff`'s own `Param`/`In` coefficients,
/// `in_names[i]` naming coefficient `i`) plus a constant, divided by `aff`'s own denominator when
/// not `1`. A `Div` dim (a `floor`/`mod`-derived term) recurses into its own defining expression
/// via [`Aff::get_div`] and renders as a `floor(...)` sub-term. Returns `None` only if some
/// isl accessor call fails outright (not expected for an `Aff` already round-tripped through isl
/// once during resolution) — callers fall back to isl's own map syntax in that case; see module
/// doc.
fn aff_text(aff: &Aff, in_names: &[String], param_names: &[String]) -> Option<String> {
    let denom = aff.denominator().ok()?.num_si().max(1);
    let mut terms: Vec<(bool, String)> = Vec::new();
    for (i, name) in param_names.iter().enumerate() {
        let c = aff.coefficient(DimType::Param, i as u32).ok()?;
        push_term(&mut terms, term_numerator(&c, denom), name, false);
    }
    for (i, name) in in_names.iter().enumerate() {
        let c = aff.coefficient(DimType::In, i as u32).ok()?;
        push_term(&mut terms, term_numerator(&c, denom), name, false);
    }
    for i in 0..aff.dim(DimType::Div) {
        let c = aff.coefficient(DimType::Div, i).ok()?;
        let numer = term_numerator(&c, denom);
        if numer == 0 {
            // isl keeps every `Div` dim's *slot* in the local space even on an `Aff` that
            // doesn't actually use it — including, confirmed empirically, on a `Div`'s own
            // defining `Aff` (`get_div(i)` on `floor((i+j)/2)`'s sole div dim reports back
            // `dim(Div) == 1` again, coefficient `0` on itself). Recursing into a zero-coefficient
            // div here would walk straight into that and loop forever; skipping before ever
            // calling `get_div` is what actually bottoms the recursion out.
            continue;
        }
        let div_aff = aff.get_div(i).ok()?;
        let inner = aff_text(&div_aff, in_names, param_names)?;
        push_term(&mut terms, numer, &format!("floor({inner})"), true);
    }
    let constant_numer = term_numerator(&aff.constant().ok()?, denom);
    if constant_numer != 0 {
        terms.push((
            constant_numer > 0,
            constant_numer.unsigned_abs().to_string(),
        ));
    }
    let numerator = if terms.is_empty() {
        "0".to_string()
    } else {
        join_terms(&terms)
    };
    Some(if denom == 1 {
        numerator
    } else {
        format!("({numerator})/{denom}")
    })
}

fn multi_aff_param_names(f: &MultiAff) -> Vec<String> {
    let space = f.space();
    (0..f.dim(DimType::Param))
        .map(|i| {
            space
                .dim_name(DimType::Param, i)
                .unwrap_or_else(|| format!("p{i}"))
        })
        .collect()
}

/// Explicitly names every range-side (output) dim of `relation` that isn't already named —
/// needed *before* printing at all: isl's own `Display` can show a plausible-looking synthesized
/// name for an unnamed dim (confirmed empirically: `{[x]->[x]}`'s range prints as `[x]`) that
/// `Space::dim_name` still reports back as unset — a display-only convenience, not a queryable
/// fact. Without this, the relation's own printed text and [`select_range_names`]'s extraction
/// (independently reading the *same* "no name" state) synthesize *different* default names,
/// producing a `Select` whose `operand` references a name its own relation's text never declares
/// (confirmed against the real fixture corpus: `array1.alpha`'s `array1c`). Renaming an
/// already-named dim to its own name is a harmless no-op, so this always runs, not just when a
/// name is missing.
fn ensure_relation_range_named(relation: &Map) -> Map {
    let space = relation.space();
    let mut out = relation.clone();
    for i in 0..relation.dim(DimType::OutOrSet) {
        let name = space
            .dim_name(DimType::OutOrSet, i)
            .unwrap_or_else(|| format!("x{i}"));
        out = match out.set_dim_name(DimType::OutOrSet, i, &name) {
            Ok(v) => v,
            Err(_) => return relation.clone(),
        };
    }
    out
}

/// `relation`'s own range-side (output) dim names — the local context a `Select`'s own `operand`
/// needs (see the `Select` arm of [`ShowPrinter::expr`]). Callers pass a `relation` already run
/// through [`ensure_relation_range_named`] so this never actually hits its own fallback; kept
/// anyway as a harmless defensive default.
fn select_range_names(relation: &Map) -> Vec<String> {
    let space = relation.space();
    (0..relation.dim(DimType::OutOrSet))
        .map(|i| {
            space
                .dim_name(DimType::OutOrSet, i)
                .unwrap_or_else(|| format!("x{i}"))
        })
        .collect()
}

/// The actual input-dim names to use for `f`'s function-literal reconstruction: `ctx`'s own names
/// for as much of `f`'s real input arity as `ctx` covers, then `f`'s own already-attached dim
/// names (falling back to a synthesized default) for anything beyond. Needed because `ctx` (an
/// equation's own `index_names`, a reduce's `body_context`) can be *shorter* than a function's
/// real arity — an equation with no declared index binder at all (`X = (i->)@-1.9;`, a real
/// fixture) still has `index_names == []`, even though the dependence function inside it declares
/// its own local `i`.
///
/// The second return value is what [`ShowPrinter::dependence`]'s array-notation branch needs:
/// `alpha_model::function::eval_function`'s `ArrayFunction` case has no way to declare a *fresh*
/// local name, only to borrow the *ambient* one, so array notation (`X[expr,...]`) is only
/// actually valid when `ctx` alone already covered the whole arity — `false` here means falling
/// back to the explicit function-literal form is required, not just prettier.
fn resolve_in_names(f: &MultiAff, ctx: &[String]) -> (Vec<String>, bool) {
    let arity = f.dim(DimType::In) as usize;
    if ctx.len() >= arity {
        return (ctx[..arity].to_vec(), true);
    }
    let space = f.space();
    let mut names: Vec<String> = ctx.to_vec();
    while names.len() < arity {
        let pos = names.len() as u32;
        names.push(
            space
                .dim_name(DimType::In, pos)
                .unwrap_or_else(|| format!("i{pos}")),
        );
    }
    (names, false)
}

/// `f`'s own per-output-dimension expression texts, each in terms of `in_names` (positional: this
/// crate always calls this with the exact ambient index-name list `f`'s own input space
/// corresponds to — an equation's `index_names`, a reduce's `body_context`, ... — never `f`'s own
/// isl-internal dim names, which may be stale/anonymous after a rewrite pass composed functions
/// together). `None` if [`aff_text`] fails for any output.
fn multi_aff_output_exprs(f: &MultiAff, in_names: &[String]) -> Option<Vec<String>> {
    let params = multi_aff_param_names(f);
    let mut out = Vec::with_capacity(f.n_out() as usize);
    for k in 0..f.n_out() {
        out.push(aff_text(&f.get_aff(k).ok()?, in_names, &params)?);
    }
    Some(out)
}

/// Formats `f` as Alpha's own `(idx,idx,...->expr,expr,...)` function-literal syntax — the *only*
/// form `alpha-syntax`'s grammar accepts at a `Dependence`'s function / `Reduce`'s projection /
/// `IndexFunction`'s function position (`alpha-syntax/src/parser/expr.rs`'s
/// `paren_or_dependence_expr`/`projection_function`/`index_expr` all dispatch to
/// `calculator::function`, never to raw isl map syntax) — isl's own `{ [i,j] -> [...] }` map
/// syntax parses at *no* Alpha expression position, so printing it there would produce text that
/// merely *looks* like it could round-trip. Falls back to isl's own map-string form (meaningful
/// for `print_ast`'s debug dump, not Alpha-parseable) only if [`multi_aff_output_exprs`] fails.
fn function_str(f: &MultiAff, ctx: &[String]) -> String {
    let (in_names, _) = resolve_in_names(f, ctx);
    match multi_aff_output_exprs(f, &in_names) {
        Some(exprs) => format!("({}->{})", in_names.join(","), exprs.join(",")),
        None => f.to_string(),
    }
}

/// Best-effort: names `d`'s own set-dims from `ctx` (as many positions as `ctx` covers), so isl's
/// own printer renders them symbolically instead of anonymously. Unlike [`function_str`]'s
/// text-substitution approach, `Restrict`'s domain/`Convolution`'s kernel domain/a `Variable`
/// declaration's domain print through isl's own native `Set` syntax directly (via
/// [`strip_params_prefix`]) — already valid, parseable Alpha source that way (a
/// `RestrictExpression`'s domain literal is raw-captured and hands the exact text to isl's own set
/// parser, `alpha-syntax/src/parser/calculator.rs`'s module doc) — so this exists purely for
/// nicer names in `ashow`, never for correctness. Any isl error along the way just falls back to
/// `d` completely unrenamed.
fn named_set(d: &Set, ctx: &[String]) -> Set {
    let mut out = d.clone();
    let n = out.dim(DimType::OutOrSet).min(ctx.len() as u32);
    for (i, name) in ctx.iter().enumerate().take(n as usize) {
        out = match out.set_dim_name(DimType::OutOrSet, i as u32, name) {
            Ok(v) => v,
            Err(_) => return d.clone(),
        };
    }
    out
}

/// Strips isl's own leading `[params] -> ` prefix from a `Set`/`Map`/`PwQPolynomial`'s `Display`
/// text — present whenever the object's space has parameters, absent otherwise (confirmed
/// empirically: a param-free `Set`'s `Display` never has one to strip, so this is a no-op there).
/// Every position `show`/`ashow` embeds one of these at (`Restrict`'s domain, `Convolution`'s
/// kernel domain, a `Variable` declaration's domain, `Select`'s relation, `IndexPolynomial`'s
/// polynomial) expects the literal text to start directly at `{` — `alpha-syntax/src/parser/
/// calculator.rs`'s `domain`/`relation`/`polynomial`/`array_polynomial` all raw-capture starting
/// from an `LBrace` token, so a leading `[N] ->` would be rejected outright (confirmed by an
/// earlier version of this module actually failing to round-trip its own `show` output on exactly
/// this). The one place isl's *un-stripped* form is correct is the system's own header line
/// (`param_domain`'s grammar has an explicit, optional `['idx',...] ->` prefix) — [`ShowPrinter::
/// print`] uses `system.parameter_domain`'s `Display` directly there, not this.
fn strip_params_prefix(text: &str) -> &str {
    text.find('{').map(|i| &text[i..]).unwrap_or(text)
}

/// Bracket-depth-aware top-level split, mirroring `alpha-syntax/src/parser/calculator.rs`'s own
/// `contains_top_level_before_close` but operating on a plain `&str` post-hoc rather than a token
/// stream mid-parse.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            c2 if c2 == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn contains_top_level_colon(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            ':' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// A domain literal like `{ [i = 0]; [i = 1] }` — a union of pure point-equality tuples, no
/// separate constraint clause needed — is genuinely valid raw ISL domain syntax, but
/// `alpha-syntax`'s own `RestrictExpression` parser can't tell it apart from an arbitrary nested
/// `CalculatorExpression` (`{myDefinedThing}`, `{A + B}`) without one: its disambiguation
/// (`alpha-syntax/src/parser/calculator.rs`'s `looks_like_raw_domain`) is a deliberately shallow,
/// token-lookahead heuristic — a top-level `:` is literally the only signal it has, and this shape
/// doesn't carry one. Confirmed against the real fixture corpus
/// (`splitUnionIntoCase1.alpha`'s own case-branch guards): this exact shape sends the parser into
/// `alpha-syntax`'s own "parser stuck without making progress" panic. Rather than touch that
/// heuristic (real, but a bigger and riskier change than this fix's scope — it would need genuine
/// understanding of ISL tuple syntax to disambiguate correctly in general, which the module's own
/// doc explains was deliberately never built), every domain this module ever embeds gets an
/// explicit, trivially-true `: 1 = 1` appended to any top-level-colon-free, non-empty piece, so
/// the heuristic always has one to find.
fn ensure_domain_colon(text: &str) -> String {
    let (Some(open), Some(close)) = (text.find('{'), text.rfind('}')) else {
        return text.to_string();
    };
    let inner = &text[open + 1..close];
    let pieces: Vec<String> = split_top_level(inner, ';')
        .into_iter()
        .map(|piece| {
            let trimmed = piece.trim();
            if trimmed.is_empty() || contains_top_level_colon(trimmed) {
                trimmed.to_string()
            } else {
                format!("{trimmed} : 1 = 1")
            }
        })
        .collect();
    format!("{}{{ {} }}", &text[..open], pieces.join("; "))
}

struct ShowPrinter {
    /// `false` for `show` (`Show.xtend`), `true` for `ashow` (`AShow.xtend`).
    array_notation: bool,
}

impl ShowPrinter {
    fn print(&self, system: &System) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "affine {} {}\n",
            system.name, system.parameter_domain
        ));
        self.var_section(&mut out, "inputs", &system.inputs);
        self.var_section(&mut out, "outputs", &system.outputs);
        self.var_section(&mut out, "locals", &system.locals);
        for body in &system.bodies {
            out.push_str(&self.body(body, system));
        }
        out.push_str(".\n");
        out
    }

    fn var_section(&self, out: &mut String, label: &str, vars: &[Variable]) {
        if vars.is_empty() {
            return;
        }
        out.push_str(&format!("    {label}\n"));
        for v in vars {
            let multiplicity = match v.multiplicity {
                alpha_model::Multiplicity::Linear => "linear ",
                alpha_model::Multiplicity::Unrestricted => "",
            };
            let element_type = match v.element_type {
                alpha_model::ElementType::Unspecified => "",
                alpha_model::ElementType::Bool => " of bool",
                alpha_model::ElementType::Int => " of int",
                alpha_model::ElementType::Real => " of real",
                alpha_model::ElementType::Qubit => " of qubit",
            };
            out.push_str(&format!(
                "        {multiplicity}{} : {}{element_type}\n",
                v.name,
                ensure_domain_colon(strip_params_prefix(&v.domain.to_string()))
            ));
        }
    }

    /// `Show.xtend`'s own `when <domain> let` guard is omitted here when a body's domain is
    /// exactly the system's overall parameter domain — the common, no-explicit-guard case.
    fn body(&self, body: &SystemBody, system: &System) -> String {
        if body.equations.is_empty() {
            return String::new();
        }
        let guard = if body
            .domain
            .is_equal(&system.parameter_domain)
            .unwrap_or(false)
        {
            String::new()
        } else {
            format!(
                "when {} ",
                ensure_domain_colon(strip_params_prefix(&body.domain.to_string()))
            )
        };
        let eqs: Vec<String> = body.equations.iter().map(|eq| self.equation(eq)).collect();
        format!("    {guard}let\n        {}\n", eqs.join("\n\n        "))
    }

    fn equation(&self, eq: &Equation) -> String {
        match eq {
            Equation::Standard(s) => {
                let lhs = if self.array_notation && !s.index_names.is_empty() {
                    format!("{}[{}]", s.variable, s.index_names.join(","))
                } else {
                    s.variable.clone()
                };
                format!("{lhs} = {};", self.expr(&s.expr, &s.index_names))
            }
            Equation::Use(u) => self.use_equation(u),
        }
    }

    fn use_equation(&self, u: &UseEquation) -> String {
        let outs: Vec<String> = u.output_exprs.iter().map(|e| self.expr(e, &[])).collect();
        let ins: Vec<String> = u.input_exprs.iter().map(|e| self.expr(e, &[])).collect();
        format!("({}) = {}({});", outs.join(", "), u.callee, ins.join(", "))
    }

    fn domain_str(&self, d: &Set, ctx: &[String]) -> String {
        let d = if self.array_notation {
            named_set(d, ctx)
        } else {
            d.clone()
        };
        ensure_domain_colon(strip_params_prefix(&d.to_string()))
    }

    fn expr(&self, e: &Expr, ctx: &[String]) -> String {
        match &*e.kind {
            ExprKind::Variable(name) => name.clone(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Int(s) | ExprKind::Real(s) => s.clone(),
            ExprKind::Dependence { function, operand } => self.dependence(function, operand, ctx),
            ExprKind::IndexFunction { function } => format!("val{}", function_str(function, ctx)),
            // No `set_dim_name` on `PwQPolynomial` in this crate's `isl` wrapper (module doc) —
            // printed the same way in both `show` and `ashow`; still needs
            // `strip_params_prefix` for the same reason every other embedded isl literal does.
            ExprKind::IndexPolynomial { polynomial } => {
                format!("val{}", strip_params_prefix(&polynomial.to_string()))
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => format!(
                "if {} then {} else {}",
                self.expr(cond, ctx),
                self.expr(then_branch, ctx),
                self.expr(else_branch, ctx)
            ),
            ExprKind::Restrict { domain, operand } => {
                format!(
                    "{} : {}",
                    self.domain_str(domain, ctx),
                    self.expr(operand, ctx)
                )
            }
            ExprKind::AutoRestrict { operand } => format!("auto : {}", self.expr(operand, ctx)),
            ExprKind::Case { name, branches } => {
                let label = name.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
                let body: Vec<String> = branches.iter().map(|b| self.expr(b, ctx)).collect();
                format!("case{label} {{\n{};\n}}", body.join(";\n"))
            }
            ExprKind::Reduce {
                is_arg_reduce,
                operator,
                projection,
                body_context,
                body,
            } => {
                let kw = if *is_arg_reduce {
                    "argreduce"
                } else {
                    "reduce"
                };
                format!(
                    "{kw}({}, {}, {})",
                    operator_text(operator),
                    function_str(projection, body_context),
                    self.expr(body, body_context)
                )
            }
            ExprKind::Convolution {
                kernel_domain,
                kernel_expr,
                data_expr,
            } => format!(
                "conv({}, {}, {})",
                self.domain_str(kernel_domain, ctx),
                self.expr(kernel_expr, ctx),
                self.expr(data_expr, ctx)
            ),
            // `operand`'s own local context is `relation`'s *range*-side names, not the ambient
            // `ctx` this `Select` node itself sits in — confirmed against the real fixture corpus
            // (`array1.alpha`'s `array1c`, `array2.alpha`'s `domain2d`): printing `operand` under
            // the wrong context produced text that reparsed to a different (or invalid) relation
            // entirely (`alpha_model::domain::Resolver::select_relation`'s own doc: "once the
            // relation's domain-side dimension count matches the ambient context's, its
            // *range*-side tuple names *replace* the context for the sub-expression").
            ExprKind::Select { relation, operand } => {
                let relation = ensure_relation_range_named(relation);
                format!(
                    "select {} from {}",
                    strip_params_prefix(&relation.to_string()),
                    self.expr(operand, &select_range_names(&relation))
                )
            }
            ExprKind::MultiArg { operator, args } => {
                let args: Vec<String> = args.iter().map(|a| self.expr(a, ctx)).collect();
                format!("{}({})", operator_text(operator), args.join(", "))
            }
            ExprKind::Binary { operator, lhs, rhs } => format!(
                "({} {operator} {})",
                self.paren_child(lhs, ctx),
                self.paren_child(rhs, ctx)
            ),
            ExprKind::Unary { operator, operand } => {
                format!("{operator} {}", self.unary_operand(operand, ctx))
            }
        }
    }

    /// A `Unary`'s own operand needs handling no other position does: `alpha-syntax`'s
    /// `unary_terminal_expr` (`alpha-syntax/src/parser/expr.rs`) has no dependence-vs-paren-expr
    /// disambiguation the way a general expression position does — even wrapped in parens, a
    /// point-free `f@X` is not a valid `UnaryExpression` operand *at all* (confirmed against the
    /// real fixture corpus: `dependence.alpha`'s own `unaryExpression` system, `- (i->i-1)@A`
    /// failing to reparse). Only the array-notation form (`-(A[i-1])`, matching that grammar's own
    /// error-message hint) is accepted there. Every `Dependence` this module ever prints has a
    /// `Variable`/constant as its own direct operand once `Normalize` has run (this crate's own
    /// normal-form invariant — see `alpha-transform/tests/normalize_fixtures.rs`), so this is
    /// always available on a normalized system; a still-nested `Dependence` (only possible on a
    /// not-yet-normalized `System`) falls back to [`Self::paren_child`]'s ordinary — here,
    /// structurally invalid — form.
    fn unary_operand(&self, e: &Expr, ctx: &[String]) -> String {
        if let ExprKind::Dependence { function, operand } = &*e.kind {
            if let Some(name) = variable_or_literal_text(operand) {
                let (in_names, fully_covered) = resolve_in_names(function, ctx);
                if fully_covered {
                    if let Some(exprs) = multi_aff_output_exprs(function, &in_names) {
                        return format!("({name}[{}])", exprs.join(","));
                    }
                }
            }
        }
        self.paren_child(e, ctx)
    }

    /// Always parenthesizes a `Binary`/`Unary`/`If`/`Restrict`/`AutoRestrict` child — see module
    /// doc: no attempt at upstream's exact paren-minimality, just always-unambiguous output.
    fn paren_child(&self, e: &Expr, ctx: &[String]) -> String {
        let s = self.expr(e, ctx);
        match &*e.kind {
            // `Binary` is deliberately *not* here — its own `expr()` arm already wraps its whole
            // `(lhs op rhs)` output in parens unconditionally; wrapping it again here would just
            // add a redundant layer per nesting level, compounding into absurdly deep
            // parenthesization on any real expression tree of some depth (confirmed against the
            // real fixture corpus: `rnaMEA.alpha`'s `Pbp` equation nests eight `Binary`s and
            // produced eight *redundant* extra parens on top of that before this fix — enough to
            // trip a real parser bug, `alpha-syntax`'s own "parser stuck without making progress"
            // panic, on top of just being needlessly ugly).
            ExprKind::If { .. }
            | ExprKind::Restrict { .. }
            | ExprKind::AutoRestrict { .. }
            | ExprKind::Unary { .. } => format!("({s})"),
            _ => s,
        }
    }

    /// `show`: always `f@operand` (point-free). `ashow`: `operand[expr,...]` when `operand` is a
    /// bare `Variable`/constant — `alpha-syntax`'s own `JNIFunctionInArrayNotation` (`X[i+1,j]`,
    /// *not* a full `(names->exprs)` function literal in brackets — `alpha-syntax/src/parser/
    /// expr.rs`'s `variable_expr_maybe_dependence`/`constant_expr_maybe_dependence`) — falls back
    /// to `show`'s form otherwise, matching `AShow.xtend`'s own `caseDependenceExpression`
    /// fallback.
    fn dependence(&self, function: &MultiAff, operand: &Expr, ctx: &[String]) -> String {
        if self.array_notation {
            if let Some(name) = variable_or_literal_text(operand) {
                let (in_names, fully_covered) = resolve_in_names(function, ctx);
                if fully_covered {
                    if let Some(exprs) = multi_aff_output_exprs(function, &in_names) {
                        return format!("{name}[{}]", exprs.join(","));
                    }
                }
            }
        }
        format!(
            "{}@{}",
            function_str(function, ctx),
            self.paren_child(operand, ctx)
        )
    }
}

/// Reconstructs Alpha-like source syntax from `system` — ported from `Show.xtend`. A `Dependence`
/// prints in point-free composition form (`f@X`); see [`ashow`] for array-index notation instead.
pub fn show(system: &System) -> String {
    ShowPrinter {
        array_notation: false,
    }
    .print(system)
}

/// Like [`show`], but renders a `Dependence` over a `Variable`/constant in array-index notation
/// (`X[f]`) and shows each equation's own ambient index names explicitly (`X[i,j] = ...`) — ported
/// from `AShow.xtend`.
pub fn ashow(system: &System) -> String {
    ShowPrinter {
        array_notation: true,
    }
    .print(system)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_model::Resolver;
    use isl::Context;

    const PREFIX_SUM: &str = "affine PrefixSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.";

    fn lowered(src: &str) -> System {
        let ctx = Context::new();
        let parse = alpha_syntax::parse(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let tree = parse.tree();
        let system = tree.systems().next().expect("one system in fixture");
        let mut resolver = Resolver::new(ctx, &system);
        let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (ir_system, lower_diagnostics) =
            crate::lower::lower_system(&mut resolver, &system).unwrap();
        assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
        ir_system
    }

    #[test]
    fn print_ast_shows_every_node_and_its_domains() {
        let system = lowered(PREFIX_SUM);
        let text = print_ast(&system);
        assert!(text.contains("System \"PrefixSum\""));
        assert!(text.contains("Reduce operator=+"));
        assert!(text.contains("exp="));
        assert!(text.contains("ctx="));
    }

    #[test]
    fn show_reconstructs_alpha_like_source() {
        let system = lowered(PREFIX_SUM);
        let text = show(&system);
        assert!(text.starts_with("affine PrefixSum"));
        assert!(text.contains("inputs"));
        assert!(text.contains("X :"));
        assert!(text.contains("outputs"));
        assert!(text.contains("Y :"));
        assert!(text.contains("Y = reduce(+,"));
        assert!(text.trim_end().ends_with('.'));
    }

    #[test]
    fn show_renders_a_dependence_function_as_alpha_function_literal_not_isl_map_syntax() {
        // The whole point of this fix: `(i,j->j)@X`, not isl's own `[N] -> { [i, j] -> [(j)] }@X`.
        let system = lowered(PREFIX_SUM);
        let text = show(&system);
        assert!(text.contains("(i,j->j)@X"), "{text}");
        assert!(
            !text.contains("-> [(j)]"),
            "isl map syntax leaked into show() output: {text}"
        );
    }

    /// The actual guarantee this fix is about: paste `show`/`ashow`'s output into a fresh file and
    /// it parses + resolves + lowers again, unchanged in essence. Covers a coefficient, a system
    /// parameter in the function body, a negative coefficient, and a `floor(...)`-derived
    /// dependence — the cases [`aff_text`] special-cases.
    #[test]
    fn show_and_ashow_output_round_trips_through_the_whole_pipeline() {
        const CASES: &[&str] = &[
            PREFIX_SUM,
            "affine Shift [N] -> {:N>10}
    inputs A: [N,N]
    outputs B: {[i,j]: 0<=i and 2*i+3<N and 0<=j<N}
    let B[i,j] = A[2*i+3,N-j-1];
.",
            "affine Neg [N] -> {:N>10}
    inputs A: [N]
    outputs B: [N]
    let B[i] = A[N-1-i];
.",
            "affine Floory [N] -> {:N>10}
    inputs A: [N]
    outputs B: {[i,j]: 0<=i<N and 0<=j<N}
    let B[i,j] = A[floor((i+j)/2)];
.",
            "affine FloorLead [N] -> {:N>10}
    inputs A: [N]
    outputs B: {[i,j]: 0<=i<1 and 0<=j<1}
    let B[i,j] = A[0 - floor((i+j)/2)];
.",
        ];
        for src in CASES {
            let system = lowered(src);
            let show_text = show(&system);
            lowered(&show_text);
            let ashow_text = ashow(&system);
            lowered(&ashow_text);
        }
    }

    #[test]
    fn ashow_shows_ambient_index_names_on_the_equation() {
        let system = lowered(PREFIX_SUM);
        let text = ashow(&system);
        // `Y`'s own declared index binder is `i` (`Y[i] = ...`) — `show` never prints it, `ashow`
        // always does.
        assert!(text.contains("Y[i] = reduce(+,"));
        assert!(!show(&system).contains("Y[i] ="));
    }

    #[test]
    fn ashow_renders_a_dependence_over_a_variable_in_array_notation() {
        // `A[i+1,j]`-style: a `Dependence` directly over a `Variable` operand.
        const SRC: &str = "affine Shift [N] -> {:N>10}
    inputs A: [N,N]
    outputs B: {[i,j]: 0<=i<N-1 and 0<=j<N}
    let B[i,j] = A[i+1,j];
.";
        let system = lowered(SRC);
        let text = ashow(&system);
        // Array notation: `A[...]`, not `show`'s point-free `...@A`.
        assert!(text.contains("A["), "{text}");
        assert!(!text.contains("@A"), "{text}");
        let shown = show(&system);
        assert!(shown.contains("@A") || shown.contains("@(A"), "{shown}");
    }
}
