//! `NormalizeReduction` conformance against the real `normalizeReductionDeep.alpha` fixture
//! (`tests/alpha-language-fixtures/.../normalize-reduction-deep/`), which exists specifically to
//! cover the "is this reduce already the equation's own topmost node" distinction: `apply` must
//! leave a `Reduce` that's already `eq.expr` itself untouched, but still hoist one that's merely
//! *reachable* from `eq.expr` (through a `Dependence`, or nested inside another `Reduce`'s body).

use alpha_model::Resolver;
use alpha_syntax::ast;
use alpha_transform::ir::{Equation, ExprKind};
use alpha_transform::{lower, normalize_reduction};
use isl::Context;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../tests/alpha-language-fixtures/alpha.model.tests/resources/src-valid/\
         transformation-reduction-tests/normalize-reduction-deep/normalizeReductionDeep.alpha",
    )
}

fn lower_named_system(name: &str) -> alpha_transform::ir::System {
    let path = fixture_path();
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
    let parse = alpha_syntax::parse(&src);
    assert!(parse.errors.is_empty(), "parse errors: {:?}", parse.errors);
    let tree = parse.tree();
    let system: ast::System = tree
        .systems()
        .find(|s| s.name().is_some_and(|n| n.text() == name))
        .unwrap_or_else(|| panic!("no system named '{name}' in {path:?}"));

    let ctx = Context::new();
    let mut resolver = Resolver::new(ctx, &system);
    let (ir_system, diags) =
        lower::lower_system(&mut resolver, &system).expect("lowering '{name}' should succeed");
    assert!(
        diags.is_empty(),
        "unexpected lowering diagnostics: {diags:?}"
    );
    ir_system
}

fn standard_eq<'a>(
    system: &'a alpha_transform::ir::System,
    var: &str,
) -> &'a alpha_transform::ir::StandardEquation {
    system
        .bodies
        .iter()
        .flat_map(|b| &b.equations)
        .find_map(|eq| match eq {
            Equation::Standard(s) if s.variable == var => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no equation for '{var}'"))
}

/// A `Reduce` that is already an equation's own expression is already normal form — nothing to
/// extract, no new local/equation created.
#[test]
fn reduce_already_at_equation_root_is_left_untouched() {
    let mut system = lower_named_system("topLevelReduction");
    let locals_before = system.locals.len();

    let extracted = normalize_reduction::apply(&mut system);

    assert_eq!(extracted, 0, "a root-level reduce must not be extracted");
    assert_eq!(system.locals.len(), locals_before, "no new local created");
    let eq = standard_eq(&system, "X");
    assert!(
        matches!(&*eq.expr.kind, ExprKind::Reduce { .. }),
        "X's equation should still be a bare Reduce, got {:?}",
        eq.expr.kind
    );
}

/// A `Reduce` reachable only through a `Dependence` (not `eq.expr` itself) is still nested and
/// must be hoisted into its own equation.
#[test]
fn reduce_under_a_dependence_is_still_hoisted() {
    let mut system = lower_named_system("reductionInsideDependence");
    let locals_before = system.locals.len();

    let extracted = normalize_reduction::apply(&mut system);

    assert_eq!(
        extracted, 1,
        "the dependence-wrapped reduce must be extracted"
    );
    assert_eq!(
        system.locals.len(),
        locals_before + 1,
        "one new local created"
    );
    let eq = standard_eq(&system, "X");
    assert!(
        !matches!(&*eq.expr.kind, ExprKind::Reduce { .. }),
        "X's equation should no longer directly be a Reduce"
    );
    let new_local = system.locals.last().expect("a new local was just pushed");
    let new_eq = standard_eq(&system, &new_local.name);
    assert!(
        matches!(&*new_eq.expr.kind, ExprKind::Reduce { .. }),
        "the newly extracted equation should be exactly the reduce"
    );
}

/// The outer reduce of `reduce(max, ..., reduce(+, ..., ...))` is already the equation's root and
/// is left in place; the module's own documented gap (a `Reduce` nested directly inside another
/// `Reduce`'s body isn't hoisted, matching `alpha-codegen/src/scheduledc.rs`'s carried-over
/// limitation) means the inner one is left alone too, in the same single pass.
#[test]
fn outer_of_a_directly_nested_reduce_pair_is_left_in_place() {
    let mut system = lower_named_system("nestedReduction_01");
    let locals_before = system.locals.len();

    let extracted = normalize_reduction::apply(&mut system);

    assert_eq!(
        extracted, 0,
        "no equation-boundary-reachable reduce to hoist"
    );
    assert_eq!(system.locals.len(), locals_before);
    let eq = standard_eq(&system, "X");
    let ExprKind::Reduce { body, .. } = &*eq.expr.kind else {
        panic!(
            "X's equation should still be a bare Reduce, got {:?}",
            eq.expr.kind
        );
    };
    assert!(
        matches!(&*body.kind, ExprKind::Reduce { .. }),
        "the inner reduce is left nested in the outer reduce's body, matching the documented gap"
    );
}
