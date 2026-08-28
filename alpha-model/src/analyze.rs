//! `pub fn analyze` — the single "run all of alpha-model" entry point `alphac`/other drivers use,
//! rather than each wiring phases 1–6 together itself (previously duplicated per fixture test —
//! see `completeness_fixtures.rs`'s own `check_all`, which this mirrors exactly for one system,
//! plus phase 5's `check_system_uniqueness`, which no per-system fixture test needed on its own
//! since `uniqueness_fixtures.rs` exercises it directly).
//!
//! A whole-`AlphaRoot` `SemanticModel` aggregating every system's analysis was sketched as a
//! possible future shape; this crate doesn't build one of those yet (no caller has needed
//! cross-system name resolution beyond
//! [`crate::uniqueness::check_program_uniqueness`]'s duplicate-detection) — [`analyze_system`] is
//! the per-system core every real caller actually needs, and [`analyze_root`] folds every system
//! in a parsed file through it plus the one whole-program check.

use crate::completeness::{
    check_case_branches, check_reduce_bounded, check_standard_equation_completeness,
    check_system_bodies, check_undefined_variables, check_use_equation_recursion,
};
use crate::uniqueness::{check_program_uniqueness, check_system_uniqueness};
use crate::{Diagnostic, Resolver};
use alpha_syntax::ast::{self, Equation};

/// Runs every phase against one already-`Resolver::new`'d system: phase 1 (via `resolver` itself,
/// threaded lazily through the calls below), phases 3–4 (`Resolver::analyze_system`), phase 5
/// (`check_system_uniqueness`), and phase 6 (every `completeness` check). Doesn't include
/// [`check_program_uniqueness`] — that's a whole-file/whole-program check, not a per-system one;
/// see [`analyze_root`].
pub fn analyze_system(resolver: &mut Resolver, system: &ast::System) -> Vec<Diagnostic> {
    let (domains, contexts, mut diagnostics) = resolver.analyze_system(system);

    diagnostics.extend(crate::multiplicity::check_system(
        resolver, system, &domains, &contexts,
    ));

    diagnostics.extend(check_system_uniqueness(system));
    diagnostics.extend(check_system_bodies(resolver, system));
    diagnostics.extend(check_case_branches(system, &contexts));
    diagnostics.extend(check_reduce_bounded(system, &contexts));
    diagnostics.extend(check_undefined_variables(system));

    for body in system.bodies() {
        for eq in body.equations() {
            match &eq {
                Equation::Standard(s) => {
                    diagnostics.extend(check_standard_equation_completeness(
                        resolver, s, &body, &domains,
                    ));
                }
                Equation::Use(u) => {
                    diagnostics.extend(check_use_equation_recursion(resolver, u, system));
                }
            }
        }
    }

    diagnostics
}

fn all_systems(root: &ast::Root) -> Vec<ast::System> {
    let mut out: Vec<ast::System> = root.systems().collect();
    fn walk_pkg(pkg: &ast::AlphaPackage, out: &mut Vec<ast::System>) {
        out.extend(pkg.systems());
        for sub in pkg.packages() {
            walk_pkg(&sub, out);
        }
    }
    for pkg in root.packages() {
        walk_pkg(&pkg, &mut out);
    }
    out
}

/// Runs [`analyze_system`] over every system in `root` (walking nested `AlphaPackage`s, same as
/// every fixture test's own `all_systems` helper), plus the one genuinely whole-program check,
/// [`check_program_uniqueness`] (duplicate systems/external functions by fully-qualified name —
/// meaningless run per-system). Returns one diagnostic list per system, paired with that system's
/// (possibly empty) name, in source order; `check_program_uniqueness`'s own diagnostics are
/// appended to the first system's list (there's no whole-file "slot" to put file-level diagnostics
/// in otherwise) unless there are no systems at all, in which case they'd simply be lost — callers
/// with a system-free root have nothing to analyze in the first place.
pub fn analyze_root(ctx: &isl::Context, root: &ast::Root) -> Vec<(String, Vec<Diagnostic>)> {
    let systems = all_systems(root);
    let mut program_diagnostics = check_program_uniqueness(std::slice::from_ref(root));

    let mut out = Vec::with_capacity(systems.len());
    for system in &systems {
        let mut resolver = Resolver::new(ctx.clone(), system);
        let mut diagnostics = analyze_system(&mut resolver, system);
        diagnostics.append(&mut program_diagnostics);
        let name = system
            .name()
            .map(|t| t.text().to_string())
            .unwrap_or_default();
        out.push((name, diagnostics));
    }
    out
}
