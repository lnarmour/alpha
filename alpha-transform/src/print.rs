//! Three read-only renderings of an [`System`], ported from alpha-language's `alpha.model.util`:
//! - [`print_ast`] — an indented debug tree dump (every node's own kind, `expression_domain`/
//!   `context_domain`), ported from `PrintAST.xtend`.
//! - [`show`] — reconstructs Alpha-like source syntax from the model ("Show notation"), ported
//!   from `Show.xtend`.
//! - [`ashow`] — like [`show`], but renders a [`ExprKind::Dependence`] over a `Variable`/constant
//!   in array-index notation (`X[f]`) instead of `Show`'s point-free composition (`f@X`), and
//!   shows each equation's own ambient index names explicitly (`X[i,j] = ...`) — ported from
//!   `AShow.xtend`.
//!
//! **Deliberately simpler than the source system's own printers in one respect**: domains/
//! functions/polynomials/relations are printed via isl's own native string form (`Display`), not
//! `AlphaPrintingUtil`'s parameter-context-relative gisting/reformatting into concise Alpha-style
//! text — real isl syntax, not attempted-Alpha-syntax reconstruction (matches this port's existing
//! precedent of using isl's own `Display` for domains elsewhere, e.g. `alpha-codegen/src/
//! describe.rs`). `ashow`'s array notation is the one place this crate does *more* than a plain
//! `Display` call: it renames a function/domain's own input dims to the ambient index names
//! tracked from the enclosing equation/reduce (`MultiAff`/`Set::set_dim_name`, best-effort — any
//! isl error just falls back to the unrenamed form) before printing it, since isl's own printer
//! already renders named dims symbolically. `IndexPolynomial`'s `PwQPolynomial` isn't renamed this
//! way (no `set_dim_name` exposed on it in this crate's `isl` wrapper) — a minor, deliberately
//! accepted gap, since index-polynomial expressions are rare in practice.
//!
//! None of these three attempt exact upstream paren-minimality (which parent contexts allow a
//! child to skip parens) — every `Dependence`/`Binary`/`Unary` operand that could otherwise be
//! ambiguous is always parenthesized. Slightly more verbose than upstream in some spots, always
//! unambiguous, and far simpler than threading parent-operator-precedence context through the
//! recursion.

use crate::ir::{
    Equation, Expr, ExprKind, Operator, System, SystemBody, UseEquation, Variable,
};
use isl::{DimType, MultiAff, Set};

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
            push(&mut out, 2, &format!("{} : {}", v.name, v.domain));
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
                &format!("StandardEquation {} index_names={:?}", s.variable, s.index_names),
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
            let tag = if *is_arg_reduce { "ArgReduce" } else { "Reduce" };
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
                &format!("MultiArg operator={} exp={exp} ctx={ctx}", operator_text(operator)),
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

/// Best-effort: names `f`'s own input dims from `ctx` (as many positions as `ctx` covers), so
/// isl's own printer renders them symbolically instead of anonymously. Any isl error along the
/// way just falls back to `f` completely unrenamed — this is cosmetic, never load-bearing.
fn named_multiaff(f: &MultiAff, ctx: &[String]) -> MultiAff {
    let mut out = f.clone();
    let n = out.dim(DimType::In).min(ctx.len() as u32);
    for (i, name) in ctx.iter().enumerate().take(n as usize) {
        out = match out.set_dim_name(DimType::In, i as u32, name) {
            Ok(v) => v,
            Err(_) => return f.clone(),
        };
    }
    out
}

/// Same idea as [`named_multiaff`], for a `Set`'s own set-dims.
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

struct ShowPrinter {
    /// `false` for `show` (`Show.xtend`), `true` for `ashow` (`AShow.xtend`).
    array_notation: bool,
}

impl ShowPrinter {
    fn print(&self, system: &System) -> String {
        let mut out = String::new();
        out.push_str(&format!("affine {} {}\n", system.name, system.parameter_domain));
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
            out.push_str(&format!("        {} : {}\n", v.name, v.domain));
        }
    }

    /// `Show.xtend`'s own `when <domain> let` guard is omitted here when a body's domain is
    /// exactly the system's overall parameter domain — the common, no-explicit-guard case.
    fn body(&self, body: &SystemBody, system: &System) -> String {
        if body.equations.is_empty() {
            return String::new();
        }
        let guard = if body.domain.is_equal(&system.parameter_domain).unwrap_or(false) {
            String::new()
        } else {
            format!("when {} ", body.domain)
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
        if self.array_notation {
            named_set(d, ctx).to_string()
        } else {
            d.to_string()
        }
    }

    fn function_str(&self, f: &MultiAff, ctx: &[String]) -> String {
        if self.array_notation {
            named_multiaff(f, ctx).to_string()
        } else {
            f.to_string()
        }
    }

    fn expr(&self, e: &Expr, ctx: &[String]) -> String {
        match &*e.kind {
            ExprKind::Variable(name) => name.clone(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Int(s) | ExprKind::Real(s) => s.clone(),
            ExprKind::Dependence { function, operand } => self.dependence(function, operand, ctx),
            ExprKind::IndexFunction { function } => {
                format!("val{}", self.function_str(function, ctx))
            }
            // No `set_dim_name` on `PwQPolynomial` in this crate's `isl` wrapper (module doc) —
            // printed the same way in both `show` and `ashow`.
            ExprKind::IndexPolynomial { polynomial } => format!("val{polynomial}"),
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
                format!("{} : {}", self.domain_str(domain, ctx), self.expr(operand, ctx))
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
                let kw = if *is_arg_reduce { "argreduce" } else { "reduce" };
                format!(
                    "{kw}({}, {}, {})",
                    operator_text(operator),
                    self.function_str(projection, body_context),
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
            // `relation: Map` has no ambient-context-aware rendering here (unlike `AShow.xtend`,
            // which tracks the relation's own range names) — Select is rare enough in practice
            // that this crate's own `alpha_model::domain` module doc already calls out gaps
            // around it; printed identically in `show` and `ashow`.
            ExprKind::Select { relation, operand } => {
                format!("select {relation} from {}", self.expr(operand, ctx))
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
                format!("{operator} {}", self.paren_child(operand, ctx))
            }
        }
    }

    /// Always parenthesizes a `Binary`/`Unary`/`If`/`Restrict`/`AutoRestrict` child — see module
    /// doc: no attempt at upstream's exact paren-minimality, just always-unambiguous output.
    fn paren_child(&self, e: &Expr, ctx: &[String]) -> String {
        let s = self.expr(e, ctx);
        match &*e.kind {
            ExprKind::If { .. }
            | ExprKind::Restrict { .. }
            | ExprKind::AutoRestrict { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Unary { .. } => format!("({s})"),
            _ => s,
        }
    }

    /// `show`: always `f@operand` (point-free). `ashow`: `operand[f]` when `operand` is a bare
    /// `Variable`/constant (array-index notation); falls back to `show`'s form otherwise, matching
    /// `AShow.xtend`'s own `caseDependenceExpression` fallback.
    fn dependence(&self, function: &MultiAff, operand: &Expr, ctx: &[String]) -> String {
        if self.array_notation {
            if let Some(name) = variable_or_literal_text(operand) {
                return format!("{name}[{}]", self.function_str(function, ctx));
            }
        }
        format!("{}@{}", self.function_str(function, ctx), self.paren_child(operand, ctx))
    }
}

/// Reconstructs Alpha-like source syntax from `system` — ported from `Show.xtend`. A `Dependence`
/// prints in point-free composition form (`f@X`); see [`ashow`] for array-index notation instead.
pub fn show(system: &System) -> String {
    ShowPrinter { array_notation: false }.print(system)
}

/// Like [`show`], but renders a `Dependence` over a `Variable`/constant in array-index notation
/// (`X[f]`) and shows each equation's own ambient index names explicitly (`X[i,j] = ...`) — ported
/// from `AShow.xtend`.
pub fn ashow(system: &System) -> String {
    ShowPrinter { array_notation: true }.print(system)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_model::Resolver;
    use isl::Context;

    const PREFIX_SCAN: &str = "affine PrefixScan [N]->{:N>0}
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
        let system = lowered(PREFIX_SCAN);
        let text = print_ast(&system);
        assert!(text.contains("System \"PrefixScan\""));
        assert!(text.contains("Reduce operator=+"));
        assert!(text.contains("exp="));
        assert!(text.contains("ctx="));
    }

    #[test]
    fn show_reconstructs_alpha_like_source() {
        let system = lowered(PREFIX_SCAN);
        let text = show(&system);
        assert!(text.starts_with("affine PrefixScan"));
        assert!(text.contains("inputs"));
        assert!(text.contains("X :"));
        assert!(text.contains("outputs"));
        assert!(text.contains("Y :"));
        assert!(text.contains("Y = reduce(+,"));
        assert!(text.trim_end().ends_with('.'));
    }

    #[test]
    fn ashow_shows_ambient_index_names_on_the_equation() {
        let system = lowered(PREFIX_SCAN);
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
