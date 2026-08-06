//! Target-mapping parsing/validation (§6 of `docs/scheduled-codegen-design.md`): turns
//! user-supplied ISL union-map text into one validated, fully-fused `UnionMap` covering every
//! statement (`crate::stmt`, §4), defaulting an unmentioned statement to its own identity
//! schedule. No codegen happens here — this only produces the schedule `ScheduledC` (§8) will
//! later feed to one `AstBuild::generate` call.

use crate::error::{CodegenError, Result};
use crate::expr::space_prefix;
use crate::stmt::Statement;
use isl::{Context, DimType, Map, MultiAff, UnionMap};
use std::collections::HashMap;

/// Parses `schedule_text` (empty ⇒ no statement mentioned, §6) and validates + fuses it against
/// `statements` into one `UnionMap`, padding every statement — mentioned or defaulted to
/// identity — to the same shared schedule-space width. A thin wrapper over
/// [`build_schedule_for_names`] — everything here only ever needs a statement's `name`/`domain`,
/// never its `kind` (§4's body-codegen concerns are irrelevant to §6's own syntax-level
/// validation), so the shared core works directly off `(name, domain)` pairs, letting
/// `crate::describe`'s `repr()`-only callers (no real `Statement`s to hand it — a `System`'s own
/// pre-normalization skeleton, §5.1, has no reduce split to speak of) reuse it too.
pub(crate) fn build_schedule(
    ctx: &Context,
    statements: &[Statement],
    schedule_text: &str,
) -> Result<UnionMap> {
    let names_and_domains: Vec<(String, isl::Set)> = statements
        .iter()
        .map(|s| (s.name.clone(), s.domain.clone()))
        .collect();
    build_schedule_for_names(ctx, &names_and_domains, schedule_text)
}

/// The actual §6 implementation — see [`build_schedule`]'s own doc for why this operates on plain
/// `(name, domain)` pairs rather than `crate::stmt::Statement` directly. Padding to a *uniform*
/// width isn't cosmetic: confirmed empirically (`isl/tests/smoke.rs`) that isl's own AST builder
/// doesn't reliably preserve cross-statement lexicographic order when per-statement widths differ,
/// even though such a union map parses and builds without any isl-level error.
pub(crate) fn build_schedule_for_names(
    ctx: &Context,
    names_and_domains: &[(String, isl::Set)],
    schedule_text: &str,
) -> Result<UnionMap> {
    let written = discover_written_fragments(ctx, schedule_text)?;

    let known_names: std::collections::HashSet<&str> =
        names_and_domains.iter().map(|(n, _)| n.as_str()).collect();
    for name in written.keys() {
        if !known_names.contains(name.as_str()) {
            let mut known: Vec<&str> = known_names.iter().copied().collect();
            known.sort_unstable();
            return Err(CodegenError::InvalidSchedule(format!(
                "target mapping mentions unknown statement '{name}' — known statements: {}",
                known.join(", ")
            )));
        }
    }

    // Every statement's own domain, tagged with its statement name (§4.3) — untagged by default
    // (`Set::set_tuple_name` is new, §9; nothing upstream of this module ever calls it).
    let named_domains: HashMap<&str, isl::Set> = names_and_domains
        .iter()
        .map(|(n, d)| Ok((n.as_str(), d.clone().set_tuple_name(n)?)))
        .collect::<Result<_>>()?;

    for (name, m) in &written {
        validate_written_fragment(name, m, &named_domains[name.as_str()])?;
    }

    // Every *mentioned* statement must agree on one exact width — silently auto-padding a
    // mismatched explicit entry up to some other explicit entry's width is deferred (§13); v1
    // rejects the mismatch outright, matching §6's rule 1. (An *unmentioned* statement defaulting
    // to identity is a different case, handled below — its own narrower width is always padded up
    // to whatever the final shared width ends up being, unconditionally.) Iterate in a
    // deterministic (sorted-by-name) order so the diagnostic naming "the other one" is stable.
    let mut written_sorted: Vec<(&str, &Map)> =
        written.iter().map(|(n, m)| (n.as_str(), m)).collect();
    written_sorted.sort_unstable_by_key(|(n, _)| *n);
    let mut explicit_width: Option<(&str, u32)> = None;
    for (name, m) in &written_sorted {
        let w = m.space().dim(DimType::OutOrSet);
        match explicit_width {
            None => explicit_width = Some((name, w)),
            Some((other_name, other_w)) if other_w != w => {
                return Err(CodegenError::InvalidSchedule(format!(
                    "target mapping statements don't share one schedule-space width — '{name}' \
                     has width {w}, but '{other_name}' has width {other_w}; every *mentioned* \
                     statement must map into exactly the same width (§6) — pad the narrower one \
                     by hand"
                )));
            }
            _ => {}
        }
    }

    // The final shared width can still exceed the explicit entries' own agreed-upon width, if
    // some *unmentioned* statement's own natural (identity) domain is wider — padding an
    // unmentioned default up is always §6-sanctioned, unconditionally, unlike reconciling
    // disagreeing explicit entries above.
    let shared_width = names_and_domains
        .iter()
        .map(|(_, d)| d.dim(isl::DimType::OutOrSet))
        .chain(explicit_width.map(|(_, w)| w))
        .max()
        .unwrap_or(0);

    let mut fused = UnionMap::empty(ctx);
    for (name, _) in names_and_domains {
        let named_domain = named_domains[name.as_str()].clone();
        let map = match written.get(name) {
            // Restrict to the statement's *real* domain regardless of what the written text
            // technically covers — §6 only requires totality (every real point has a schedule
            // value), not that the text's own domain expression be written exactly as
            // `named_domain`.
            Some(m) => m.clone().intersect_domain(named_domain)?,
            None => identity_map(named_domain)?,
        };
        let width = map.space().dim(DimType::OutOrSet);
        let padded = pad_range(ctx, map, width, shared_width)?;
        // Always reset the range's tuple identity, whether or not padding actually ran: an
        // unpadded identity map's range inherits its domain's own tuple name (domain and range
        // share one space), while a padded map's range comes out anonymous from
        // `flat_range_product` — left as-is, that inconsistency makes different statements'
        // schedule fragments mutually incompatible for `apply_range`/`intersect` later
        // (`crate::legality`, §7), even though each is individually well-formed.
        let anonymized = padded.reset_tuple_name(DimType::OutOrSet)?;
        fused = fused.union(UnionMap::from_map(anonymized))?;
    }

    Ok(fused)
}

/// Visits every fragment in `schedule_text`'s parsed union map, keyed by its own domain tuple
/// name — the only way to discover what's actually written without already knowing each
/// statement's exact schedule width up front (`UnionMap::extract_map` needs the *whole* map space,
/// domain and range both, to find a fragment — see `isl/src/union_map.rs`'s own doc).
fn discover_written_fragments(ctx: &Context, schedule_text: &str) -> Result<HashMap<String, Map>> {
    let mut written: HashMap<String, Map> = HashMap::new();
    if schedule_text.trim().is_empty() {
        return Ok(written);
    }
    let parsed = UnionMap::read_from_str(ctx, schedule_text).map_err(|e| {
        CodegenError::InvalidSchedule(format!("target mapping failed to parse: {}", e.message))
    })?;
    let mut duplicate: Option<String> = None;
    parsed
        .for_each_map(|m| {
            let Some(name) = m.space().tuple_name(DimType::In) else {
                return;
            };
            // isl's own union map already merges two entries with an *identical* domain/range
            // space into one fragment — reaching this as two separate `for_each_map` callbacks
            // for the same tuple name only happens when their spaces genuinely differ (e.g.
            // mismatched arity), which is exactly the case worth a clear diagnostic for.
            if written.insert(name.clone(), m).is_some() {
                duplicate.get_or_insert(name);
            }
        })
        .map_err(CodegenError::Isl)?;
    if let Some(name) = duplicate {
        return Err(CodegenError::InvalidSchedule(format!(
            "target mapping has two inconsistent entries for statement '{name}' (same tuple \
             name, different arity) — isl merges same-shaped entries automatically, so seeing \
             two here means their domain tuples don't actually match"
        )));
    }
    Ok(written)
}

fn validate_written_fragment(name: &str, m: &Map, named_domain: &isl::Set) -> Result<()> {
    let uncovered = named_domain.clone().subtract(m.clone().domain()?)?;
    if !uncovered.is_empty()? {
        return Err(CodegenError::InvalidSchedule(format!(
            "target mapping for statement '{name}' doesn't cover its whole domain — a \
             partially-specified schedule for a mentioned statement is an error, not a silent \
             partial-identity fallback (§6)"
        )));
    }
    if !m.is_injective()? {
        return Err(CodegenError::InvalidSchedule(format!(
            "target mapping for statement '{name}' is not injective — two instances would land \
             on the same schedule-space point, making their relative order undefined (§6)"
        )));
    }
    Ok(())
}

fn identity_map(named_domain: isl::Set) -> Result<Map> {
    let ident = MultiAff::identity_on_domain_space(named_domain.space())?
        .into_map()?
        .intersect_domain(named_domain)?;
    Ok(ident)
}

/// Right-pads `map`'s range from `current_width` to `target_width` extra constant-`0` dims, via
/// `flat_range_product` against a small dynamically-built "zero tail" map sharing `map`'s exact
/// *domain* space (tuple name included). Deliberately not `apply_range`: composing against
/// `map`'s domain only needs the domain's own tuple *id* to match exactly (`isl_space_tuple_is_equal`
/// — confirmed the hard way, an `apply_range` attempt against an anonymous-domain embedding map
/// hit isl's own tuple-equality assertion), whereas `crate::expr::ambient_build`'s sibling
/// leniency (dim count/param names only, no tuple identity needed) is specific to
/// `AstBuild::expr_from_set`, not this operation.
fn pad_range(ctx: &Context, map: Map, current_width: u32, target_width: u32) -> Result<Map> {
    if current_width == target_width {
        return Ok(map);
    }
    let param_names = param_names_of(&map);
    let domain_space = map.space();
    let in_dims = domain_space.dim(DimType::In);
    let tuple_name = domain_space.tuple_name(DimType::In);
    let names: Vec<String> = (0..in_dims).map(|i| format!("x{i}")).collect();
    let zeros: Vec<&str> = vec!["0"; (target_width - current_width) as usize];
    let tuple_prefix = tuple_name.as_deref().unwrap_or("");
    let text = format!(
        "{}{{ {}[{}] -> [{}] }}",
        space_prefix(&param_names),
        tuple_prefix,
        names.join(","),
        zeros.join(",")
    );
    let zero_tail = Map::read_from_str(ctx, &text)?;
    Ok(map.flat_range_product(zero_tail)?)
}

fn param_names_of(map: &Map) -> Vec<String> {
    let space = map.space();
    let n = space.dim(DimType::Param);
    (0..n)
        .map(|d| {
            space
                .dim_name(DimType::Param, d)
                .unwrap_or_else(|| format!("P{d}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stmt::statements;
    use crate::test_util::{normalized_system, PREFIX_SUM};

    #[test]
    fn empty_text_defaults_every_statement_to_identity_padded_to_shared_width() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let fused = build_schedule(&ctx, &stmts, "").unwrap();

        // Y__init is naturally 1-d; Y__reduce is naturally 2-d — every fragment in the fused
        // result must come out 2-d (the shared width), not left heterogeneous.
        let mut widths = std::collections::BTreeMap::new();
        fused
            .for_each_map(|m| {
                let space = m.space();
                widths.insert(
                    space.tuple_name(DimType::In).unwrap(),
                    space.dim(DimType::OutOrSet),
                );
            })
            .unwrap();
        assert_eq!(widths.len(), 2);
        assert!(widths.values().all(|&w| w == 2), "{widths:?}");
    }

    #[test]
    fn explicit_uniformly_padded_schedule_builds_the_correct_nesting() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let text = "{ Y__init[i] -> [i, 0, 0]; \
                     Y__reduce[i,j] -> [i, 1, j]; }";
        let fused = build_schedule(&ctx, &stmts, text).unwrap();

        let context = isl::Set::read_from_str(&ctx, "[N] -> { : N > 0 }").unwrap();
        let build = isl::AstBuild::from_context(context).unwrap();
        let printed = build.generate(fused).unwrap().to_string_fmt(isl::Format::C);
        let init_pos = printed.find("Y__init(").unwrap();
        let reduce_pos = printed.find("Y__reduce(").unwrap();
        assert!(
            init_pos < reduce_pos,
            "expected init, then reduce, fully nested: {printed}"
        );
    }

    /// `UnionMap` (the `Ok` side of `build_schedule`'s `Result`) doesn't implement `Debug`, so
    /// `.unwrap_err()` can't be used directly — extract just the error's own `Display` text.
    fn expect_err(result: Result<UnionMap>) -> String {
        match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn unknown_statement_name_is_rejected() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        let msg = expect_err(build_schedule(&ctx, &stmts, "{ Bogus[i] -> [i]; }"));
        assert!(msg.contains("Bogus"), "{msg}");
        // Tier 5 (docs/codegen-test-design.md §5.8): the full diagnostic text, not just a
        // substring — the `contains` check above stays as a quick, snapshot-independent statement
        // of intent.
        insta::assert_snapshot!(msg);
    }

    #[test]
    fn partially_specified_mentioned_statement_is_rejected() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        // Covers only part of Y__init's real domain (0 <= i < N) — missing the i == 0 point.
        let text = "{ Y__init[i] -> [i, 0, 0] : 1 <= i; }";
        let msg = expect_err(build_schedule(&ctx, &stmts, text));
        assert!(msg.contains("Y__init") && msg.contains("cover"), "{msg}");
        insta::assert_snapshot!(msg);
    }

    #[test]
    fn non_injective_mentioned_statement_is_rejected() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        // Every instance maps to the same point — not injective.
        let text = "{ Y__init[i] -> [0, 0, 0]; }";
        let msg = expect_err(build_schedule(&ctx, &stmts, text));
        assert!(
            msg.contains("Y__init") && msg.contains("injective"),
            "{msg}"
        );
        insta::assert_snapshot!(msg);
    }

    #[test]
    fn mismatched_explicit_widths_are_rejected() {
        let system = normalized_system(PREFIX_SUM);
        let stmts = statements(&system).unwrap();
        let ctx = stmts[0].domain.ctx();
        // Y__init at width 2, Y__reduce at width 3 — explicit entries disagreeing must be
        // rejected, not silently auto-padded (§13: auto-padding mismatched explicit entries is
        // deferred).
        let text = "{ Y__init[i] -> [i, 2]; Y__reduce[i,j] -> [i, 0, j]; }";
        let msg = expect_err(build_schedule(&ctx, &stmts, text));
        assert!(msg.contains("width"), "{msg}");
        insta::assert_snapshot!(msg);
    }
}
