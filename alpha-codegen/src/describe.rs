//! Renders an `ir::System`'s current schedule-space skeleton as text — the ISL union-map form a
//! `repr()` shows in the notebook (§5.2 of `docs/scheduled-codegen-design.md`), callable at any
//! time with no precondition (§5.1). Two variants, matching the two shapes a system can be in:
//! pre-normalization (`describe_system`, one entry per output/local variable, no reduce split —
//! §4.2's split doesn't exist yet) and post-normalization (`describe_normalized_system`, the real
//! statement set via `crate::stmt`, at either the identity default or an already-validated
//! schedule).

use crate::error::Result;
use crate::schedule::{build_schedule, build_schedule_for_names};
use crate::stmt;
use alpha_transform::ir;
use isl::Format;

/// A pre-normalization system's own skeleton: one entry per output/local variable, at its
/// identity schedule, uniformly padded (§6's width rule applies here too — this is still a real
/// `UnionMap`, just always the identity default, since there's no target-mapping text to apply
/// before normalization even makes the statement names §6 keys off of exist, §5.1).
pub fn describe_system(system: &ir::System) -> Result<String> {
    let Some(first_body) = system.bodies.first() else {
        return Ok("{\n}".to_string());
    };
    let ctx = first_body.domain.ctx();
    let names_and_domains: Vec<(String, isl::Set)> = system
        .outputs
        .iter()
        .chain(system.locals.iter())
        .map(|v| (v.name.clone(), v.domain.clone()))
        .collect();
    let skeleton = build_schedule_for_names(&ctx, &names_and_domains, "")?;
    Ok(skeleton.to_string_fmt(Format::Isl))
}

/// A post-normalization system's own skeleton, at `schedule_text` (empty for the identity
/// default, §6) — the real statement set (§4.2's reduce split included), via `crate::stmt`.
pub fn describe_normalized_system(system: &ir::System, schedule_text: &str) -> Result<String> {
    let statements = stmt::statements(system)?;
    let Some(first) = statements.first() else {
        return Ok("{\n}".to_string());
    };
    let ctx = first.domain.ctx();
    let skeleton = build_schedule(&ctx, &statements, schedule_text)?;
    Ok(skeleton.to_string_fmt(Format::Isl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{normalized_system, PREFIX_SCAN};
    use alpha_model::Resolver;

    fn unnormalized_prefix_scan() -> ir::System {
        let ctx = isl::Context::new();
        let parse = alpha_syntax::parse(PREFIX_SCAN);
        assert!(parse.errors.is_empty());
        let tree = parse.tree();
        let system = tree.systems().next().unwrap();
        let mut resolver = Resolver::new(ctx, &system);
        let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (ir_system, lower_diagnostics) =
            alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
        assert!(lower_diagnostics.is_empty());
        ir_system
    }

    #[test]
    fn pre_normalization_skeleton_has_no_reduce_split() {
        let system = unnormalized_prefix_scan();
        let text = describe_system(&system).unwrap();
        // Just `Y`, no `Y_NR__init`/`Y_NR__reduce` — the split doesn't exist before normalize_reduction runs.
        assert!(text.contains("Y["), "{text}");
        assert!(
            !text.contains("__init") && !text.contains("__reduce"),
            "{text}"
        );
    }

    #[test]
    fn post_normalization_skeleton_has_the_reduce_split() {
        let system = normalized_system(PREFIX_SCAN);
        let text = describe_normalized_system(&system, "").unwrap();
        assert!(
            text.contains("Y_NR__init") && text.contains("Y_NR__reduce"),
            "{text}"
        );
    }

    #[test]
    fn post_normalization_skeleton_reflects_an_explicit_schedule() {
        let system = normalized_system(PREFIX_SCAN);
        let sched_text = "{ Y_NR__init[i] -> [i, 0, 0]; \
                     Y_NR__reduce[i,j] -> [i, 1, j]; \
                     Y[i] -> [i, 2, 0]; }";
        let text = describe_normalized_system(&system, sched_text).unwrap();
        assert!(
            text.contains("0") && text.contains("1") && text.contains("2"),
            "{text}"
        );
    }
}
