//! End-to-end exercise of the safe wrapper's operation inventory against real isl, covering the
//! subset `docs/rust-port-design.md` §5/§6/§7 in the workspace root identifies as what
//! `alpha-model`/`alpha-codegen` actually need: parsing, set/map algebra, hulls, dependence
//! image/preimage, constraint construction, and the AST builder (the one isl earns its keep on).

use isl::{
    AstBuild, AstExprKind, AstNodeKind, Constraint, Context, DimType, Format, LocalSpace, Map,
    MultiAff, Set, UnionMap,
};

#[test]
fn set_parsing_and_boolean_algebra() {
    let ctx = Context::new();
    let a = Set::read_from_str(&ctx, "{ [i] : 0 <= i and i < 10 }").unwrap();
    let b = Set::read_from_str(&ctx, "{ [i] : 5 <= i and i < 15 }").unwrap();

    let intersection = a.clone().intersect(b.clone()).unwrap();
    assert!(!intersection.is_empty().unwrap());

    let union = a.clone().union(b.clone()).unwrap();
    assert!(!union.is_empty().unwrap());

    let subtracted = a.clone().subtract(b).unwrap();
    assert!(!subtracted.is_empty().unwrap());

    assert!(a.is_equal(&a.clone()).unwrap());
}

#[test]
fn set_read_from_str_reports_isl_errors_not_panics() {
    let ctx = Context::new();
    let err = match Set::read_from_str(&ctx, "not a valid isl set at all !!!") {
        Err(e) => e,
        Ok(_) => panic!("expected malformed isl syntax to be rejected"),
    };
    assert!(
        !err.message.is_empty(),
        "expected a non-empty isl error message"
    );
}

#[test]
fn hulls_and_gist() {
    let ctx = Context::new();
    let s = Set::read_from_str(&ctx, "{ [i,j] : 0 <= i < 10 and i <= j and j < 10 }").unwrap();
    let hull = s.clone().convex_hull().unwrap();
    assert!(!hull.to_string().is_empty());

    let context = Set::read_from_str(&ctx, "{ [i,j] : 0 <= i < 10 }").unwrap();
    let gisted = s.gist(context).unwrap();
    // Just confirm it round-trips through the printer without erroring; exact textual form is
    // isl's own simplification choice, not something we assert on.
    assert!(!gisted.to_string_fmt(Format::Isl).is_empty());
}

#[test]
fn unbounded_reduction_style_bound_check() {
    // Mirrors `UniquenessAndCompletenessCheck.inReduceExpression`'s unbounded-reduction check
    // (docs/rust-port-design.md §6): a reduction body domain must have both bounds on every
    // dimension being reduced over.
    let ctx = Context::new();
    let bounded = Set::read_from_str(&ctx, "{ [i] : 0 <= i < 10 }").unwrap();
    assert!(bounded.has_lower_bound(DimType::OutOrSet, 0).unwrap());
    assert!(bounded.has_upper_bound(DimType::OutOrSet, 0).unwrap());

    let unbounded = Set::read_from_str(&ctx, "{ [i] : 0 <= i }").unwrap();
    assert!(unbounded.has_lower_bound(DimType::OutOrSet, 0).unwrap());
    assert!(!unbounded.has_upper_bound(DimType::OutOrSet, 0).unwrap());
}

#[test]
fn map_apply_and_domain_range() {
    let ctx = Context::new();
    let set = Set::read_from_str(&ctx, "{ [i] : 0 <= i < 10 }").unwrap();
    let shift = Map::read_from_str(&ctx, "{ [i] -> [i+1] }").unwrap();

    let image = set.apply(shift).unwrap();
    // { [i] : 0 <= i < 10 } shifted by +1 should contain 10 but not 0.
    let contains_10 = Set::read_from_str(&ctx, "{ [10] }").unwrap();
    assert!(!image
        .clone()
        .intersect(contains_10)
        .unwrap()
        .is_empty()
        .unwrap());
    let contains_0 = Set::read_from_str(&ctx, "{ [0] }").unwrap();
    assert!(image.intersect(contains_0).unwrap().is_empty().unwrap());
}

#[test]
fn reduce_projection_preimage() {
    // Mirrors ExpressionDomainCalculator's rule for AbstractReduceExpression (§6): expression
    // domain = image of the reduce body's domain under the projection function. Here we go the
    // other way (preimage), matching ContextDomainCalculator's dual rule.
    let ctx = Context::new();
    let body_context = Set::read_from_str(&ctx, "{ [i] : 0 <= i < 5 }").unwrap();
    let projection = MultiAff::read_from_str(&ctx, "{ [i,j] -> [i] }").unwrap();
    assert_eq!(projection.n_out(), 1);

    let preimage = body_context.preimage_multi_aff(projection).unwrap();
    assert!(!preimage.is_empty().unwrap());
}

#[test]
fn constraint_construction() {
    // Mirrors WriteCExprConverter's createReduceLoopDomain (§7): build a basic set by directly
    // adding a constraint via coefficients, rather than parsing text.
    let ctx = Context::new();
    let universe = Set::read_from_str(&ctx, "{ [i,j] : 0 <= i < 10 and 0 <= j < 10 }").unwrap();
    let bset = universe.convex_hull().unwrap();
    let space = bset.clone().into_set().space();
    let ls = LocalSpace::from_space(space).unwrap();

    // Constrain i - j = 0, i.e. i == j.
    let c = Constraint::equality(ls)
        .unwrap()
        .set_coefficient(DimType::OutOrSet, 0, 1)
        .unwrap()
        .set_coefficient(DimType::OutOrSet, 1, -1)
        .unwrap()
        .set_constant(0)
        .unwrap();

    let constrained = bset.add_constraint(c).unwrap().into_set();
    assert!(!constrained.is_empty().unwrap());
    // Every point should have i == j: intersecting with i != j should be empty.
    let off_diagonal = Set::read_from_str(&ctx, "{ [i,j] : i != j }").unwrap();
    assert!(constrained
        .intersect(off_diagonal)
        .unwrap()
        .is_empty()
        .unwrap());
}

#[test]
fn ast_build_generates_a_for_loop() {
    // Mirrors LoopGenerator.generateLoops (§7): identity schedule over a domain, via isl's AST
    // builder — the demand-driven codegen path's one real "isl earns its keep" call.
    let ctx = Context::new();
    let context = Set::read_from_str(&ctx, "{ : }").unwrap();
    let domain = Map::read_from_str(&ctx, "{ [i] -> [i] : 0 <= i < 10 }").unwrap();

    let build = AstBuild::from_context(context).unwrap();
    let schedule = UnionMap::from_map(domain);
    let node = build.generate(schedule).unwrap();

    match node.kind() {
        AstNodeKind::For { .. } => {}
        other => panic!("expected a For node for a 1-d bounded domain, got {other:?}"),
    }

    let iterator = node.for_iterator().unwrap();
    assert_eq!(iterator.kind(), AstExprKind::Id);
    assert!(!iterator.id_name().unwrap().is_empty());

    let printed = node.to_string_fmt(Format::C);
    assert!(
        printed.contains("for"),
        "expected C-formatted output to contain 'for': {printed}"
    );
}

#[test]
fn ast_build_generates_nested_for_and_if() {
    let ctx = Context::new();
    let context = Set::read_from_str(&ctx, "{ : }").unwrap();
    let domain =
        Map::read_from_str(&ctx, "{ [i,j] -> [i,j] : 0 <= i < 5 and i <= j < 5 }").unwrap();

    let build = AstBuild::from_context(context).unwrap();
    let node = build.generate(UnionMap::from_map(domain)).unwrap();

    // Whatever shape isl chooses (nested fors, possibly with an if for the triangular
    // constraint), the printed C text should be well-formed and contain at least one loop.
    let printed = node.to_string_fmt(Format::C);
    assert!(printed.contains("for"));
}
