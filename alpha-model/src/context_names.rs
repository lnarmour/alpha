//! Small text-based helpers for reading the bound names a construct introduces into scope for
//! its sub-expressions (`RestrictExpression`'s domain tuple, `SelectExpression`'s relation
//! range, `ReduceExpression`'s array-notation projection, `UseEquation`'s `over`/`with` clauses)
//! — shared between [`crate::function`]'s context-dependent resolution and [`crate::domain`]'s
//! phase 3/4 inference.
//!
//! Deliberately simple raw-text parsing rather than full calculator-expression evaluation —
//! sufficient for reading a construct's own leading/range-side tuple names. The exact same
//! approach (and the exact same scoping rules) was worked out and validated against the whole
//! 82-file fixture corpus in `tests/function_fixtures.rs`, which has its own copy of this logic
//! for its own narrower purpose (gathering names for a test harness, not real domain inference);
//! this module is the library's copy, used for the real thing.

use alpha_syntax::ast;

/// A single tuple position's own bound name, stripped of any embedded `= value` binding — isl's
/// own `Display` for a `Set`/`Map` with a dimension pinned to a fixed value writes that dimension
/// as `name = value` (`{ [i = 0, j = 0] }`), not just `name`, so a naive comma-split alone (as
/// this function replaced) would extract `"i = 0"` as if it were the name itself. Confirmed as a
/// real, previously-latent bug via `alpha-transform/src/print.rs`'s `show`/`ashow` round-trip
/// tests: reprinting a domain isl resolved from a hand-written `{:i=0 && j=0}` (bare-colon,
/// ambient-context form — no equality-bound tuple names to trip this) comes back out as the
/// explicit-tuple form instead, since that's how isl's `Display` always renders a fixed-value
/// dimension — exposing this for the first time.
fn tuple_position_name(s: &str) -> String {
    s.split('=').next().unwrap_or("").trim().to_string()
}

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
        inner.split(',').map(tuple_position_name).collect()
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
        inner.split(',').map(tuple_position_name).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_syntax::ast::AstNode;
    use alpha_syntax::syntax_kind::SyntaxKind;

    fn first_domain(src: &str) -> ast::Domain {
        let parse = alpha_syntax::parse(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        parse
            .tree()
            .syntax()
            .descendants()
            .find_map(|n| {
                (n.kind() == SyntaxKind::DOMAIN)
                    .then(|| ast::Domain::cast(n))
                    .flatten()
            })
            .expect("one DOMAIN node in fixture")
    }

    #[test]
    fn plain_names_extract_unchanged() {
        let d = first_domain(
            "affine T [N] -> {:N>0}
    inputs A: [N]
    outputs B: [N]
    let B = {[i,j]: i<j} : A;
.",
        );
        assert_eq!(domain_tuple_names(&d), vec!["i", "j"]);
    }

    /// The bug this module doc/`ensure_domain_colon` in `alpha-transform/src/print.rs` was
    /// written to work around: isl's own `Display` for a domain with a dim pinned to a fixed
    /// value writes it as `name = value` (`{[i = 0, j = 0]}`), and a naive comma-split alone used
    /// to extract `"i = 0"` as if it were the name — confirmed as a real, previously-latent bug
    /// via `alpha-transform`'s `show`/`ashow` round-trip tests reprinting a resolved domain back
    /// out in this exact shape.
    #[test]
    fn equality_bound_tuple_positions_extract_the_bare_name() {
        let d = first_domain(
            "affine T [N] -> {:N>0}
    inputs A: [N,N]
    outputs B: [N,N]
    let B = {[i = 0, j = 0] : 1 = 1} : A;
.",
        );
        assert_eq!(domain_tuple_names(&d), vec!["i", "j"]);
    }
}
