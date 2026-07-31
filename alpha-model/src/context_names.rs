//! Small text-based helpers for reading the bound names a construct introduces into scope for
//! its sub-expressions (`RestrictExpression`'s domain tuple, `SelectExpression`'s relation
//! range, `ReduceExpression`'s array-notation projection, `UseEquation`'s `over`/`with` clauses)
//! — shared between [`crate::function`]'s context-dependent resolution and [`crate::domain`]'s
//! phase 3/4 inference. See `docs/rust-port-design.md` §6 in the workspace root.
//!
//! Deliberately simple raw-text parsing rather than full calculator-expression evaluation —
//! sufficient for reading a construct's own leading/range-side tuple names. The exact same
//! approach (and the exact same scoping rules) was worked out and validated against the whole
//! 82-file fixture corpus in `tests/function_fixtures.rs`, which has its own copy of this logic
//! for its own narrower purpose (gathering names for a test harness, not real domain inference);
//! this module is the library's copy, used for the real thing.

use alpha_syntax::ast;

/// A `Domain`'s (`{[i,j]:...}`) own leading tuple names, read directly off its raw text.
pub(crate) fn domain_tuple_names(d: &ast::Domain) -> Vec<String> {
    let text = d.isl_text();
    let inner = text
        .trim_start_matches('{')
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if inner.is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    }
}

/// A `Relation`'s (`{[i,j]->[x]:...}`) range-side (output) tuple names.
pub(crate) fn relation_range_names(r: &ast::Relation) -> Vec<String> {
    let text = r.isl_text();
    let Some(after_arrow) = text.split("->").nth(1) else {
        return Vec::new();
    };
    let inner = after_arrow
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if inner.is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    }
}

/// Reads an `ArrayFunction`'s raw comma-separated elements, keeping only the ones that are bare
/// identifiers (vs. a general expression) — used for a reduce's array-notation projection, whose
/// elements are always newly-declared bound names, never arbitrary expressions.
pub(crate) fn bare_identifier_elements(af: &ast::ArrayFunction) -> Vec<String> {
    af.raw_elements()
        .into_iter()
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect()
}

/// Extends `base` with `new`, skipping any name already present — a construct that re-introduces
/// an already-in-scope name (e.g. a `RestrictExpression`'s own domain tuple happening to reuse
/// the outer equation's own index name) isn't shadowing, it's the *same* name; without this a
/// naive extend would produce a tuple with a duplicate name, which isl rejects.
pub(crate) fn extend_unique(base: &[String], new: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = base.to_vec();
    for n in new {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}
