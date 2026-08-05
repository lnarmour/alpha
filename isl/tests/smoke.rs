//! End-to-end exercise of the safe wrapper's operation inventory against real isl, covering the
//! subset `alpha-model`/`alpha-codegen` actually need: parsing, set/map algebra, hulls, dependence
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
    // Mirrors `UniquenessAndCompletenessCheck.inReduceExpression`'s unbounded-reduction check:
    // a reduction body domain must have both bounds on every dimension being reduced over.
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
fn union_map_read_union_extract_round_trip() {
    // Mirrors a target mapping's own text format (§6 of docs/scheduled-codegen-design.md): one
    // union map, one fragment per statement, keyed by tuple name.
    let ctx = Context::new();
    let text = "{ S1[i] -> [i, 0]; S2[i,j] -> [i, 1, j]; }";
    let umap = UnionMap::read_from_str(&ctx, text).unwrap();

    // `extract_map` matches on the *whole* map space, domain and range alike — the query space
    // below must have the same input tuple name/dim count *and* the same output dim count as the
    // `S1[i] -> [i, 0]` fragment (1 input dim, 2 output dims) to find it.
    let s1_space = Map::read_from_str(&ctx, "{ S1[i] -> [x,y] }")
        .unwrap()
        .space();
    let s1 = umap.extract_map(s1_space).unwrap();
    assert!(!s1.is_empty().unwrap());
    assert!(s1.is_injective().unwrap());

    // A tuple name absent from the union map extracts as empty, not an error.
    let absent_space = Map::read_from_str(&ctx, "{ Absent[i] -> [x,y] }")
        .unwrap()
        .space();
    let absent = umap.extract_map(absent_space).unwrap();
    assert!(absent.is_empty().unwrap());

    // Unioning two fragments built independently reproduces both statements.
    let fragment_a = UnionMap::from_map(Map::read_from_str(&ctx, "{ A[i] -> [i, 0] }").unwrap());
    let fragment_b = UnionMap::from_map(Map::read_from_str(&ctx, "{ B[i] -> [i, 1] }").unwrap());
    let combined = fragment_a.union(fragment_b).unwrap();
    let a_space = Map::read_from_str(&ctx, "{ A[i] -> [x,y] }")
        .unwrap()
        .space();
    assert!(!combined.extract_map(a_space).unwrap().is_empty().unwrap());
}

#[test]
fn union_map_for_each_map_discovers_every_fragment_by_tuple_name() {
    let ctx = Context::new();
    let text = "{ S1[i] -> [i, 0]; S2[i,j] -> [i, 1, j]; }";
    let umap = UnionMap::read_from_str(&ctx, text).unwrap();

    let mut seen: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    umap.for_each_map(|m| {
        let space = m.space();
        let name = space.tuple_name(DimType::In).unwrap();
        seen.insert(name, space.dim(DimType::OutOrSet));
    })
    .unwrap();

    assert_eq!(seen.len(), 2);
    assert_eq!(seen["S1"], 2);
    assert_eq!(seen["S2"], 3);
}

#[test]
fn map_is_injective_and_lex_ge() {
    let ctx = Context::new();
    let injective = Map::read_from_str(&ctx, "{ [i] -> [i+1] }").unwrap();
    assert!(injective.is_injective().unwrap());

    let non_injective = Map::read_from_str(&ctx, "{ [i,j] -> [i] : 0 <= j < 10 }").unwrap();
    assert!(!non_injective.is_injective().unwrap());

    // The universal lex_ge relation on a 1-d space should contain [1] -> [0] (1 >=_lex 0) but not
    // [0] -> [1].
    let space = Set::read_from_str(&ctx, "{ [i] : }").unwrap().space();
    let lex_ge = Map::lex_ge_on_space(space).unwrap();
    let ordered = Set::read_from_str(&ctx, "{ [1] }")
        .unwrap()
        .apply(lex_ge.clone())
        .unwrap();
    assert!(!ordered
        .intersect(Set::read_from_str(&ctx, "{ [0] }").unwrap())
        .unwrap()
        .is_empty()
        .unwrap());
    let reversed = Set::read_from_str(&ctx, "{ [0] }")
        .unwrap()
        .apply(lex_ge)
        .unwrap();
    assert!(reversed
        .intersect(Set::read_from_str(&ctx, "{ [1] }").unwrap())
        .unwrap()
        .is_empty()
        .unwrap());
}

#[test]
fn set_tuple_name_round_trips() {
    let ctx = Context::new();
    let s = Set::read_from_str(&ctx, "{ [i,j] : 0 <= i < 10 and 0 <= j < 10 }")
        .unwrap()
        .set_tuple_name("Stmt")
        .unwrap();
    assert_eq!(s.space().dim(DimType::OutOrSet), 2);
    let printed = s.to_string_fmt(Format::Isl);
    assert!(
        printed.contains("Stmt"),
        "expected the tuple name to round-trip through the printer: {printed}"
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

/// isl's `ast_build_node_from_schedule_map` does **not** reliably preserve full lexicographic
/// order across statements whose schedule-space widths differ, even though the isl set/map text
/// itself parses and builds without error. Discovered while implementing scheduled codegen's
/// target-mapping validator (§6 of `docs/scheduled-codegen-design.md`): a 3-statement schedule
/// with widths (2, 3, 2) — `init[i] -> [i,0]`, `reduce[i,j] -> [i,1,j]`, `copy[i] -> [i,2]` —
/// generates code that runs every `init`/`copy` instance (for *every* `i`) before any `reduce`
/// instance, even though strict lex order interleaves them per-`i`
/// (`[0,0] < [0,1,0] < [0,2] < [1,0] < [1,1,0] < ...`). This is exactly why §6's "every statement
/// maps into one shared schedule space of *fixed width*" rule has to be enforced as a hard
/// rejection, not a soft preference — see [`uniform_width_schedule_preserves_lex_order_via_full_nesting`]
/// for the same statements padded to a uniform width, which *does* produce the correct nesting.
#[test]
fn heterogeneous_width_schedule_does_not_preserve_lex_order() {
    let ctx = Context::new();
    let text = "[N] -> { init[i] -> [i, 0] : 0 <= i < N; \
               reduce[i,j] -> [i, 1, j] : 0 <= j <= i < N; \
               copy[i] -> [i, 2] : 0 <= i < N; }";
    let umap = UnionMap::read_from_str(&ctx, text).unwrap();
    let context = Set::read_from_str(&ctx, "[N] -> { : N > 0 }").unwrap();
    let build = AstBuild::from_context(context).unwrap();
    let printed = build.generate(umap).unwrap().to_string_fmt(Format::C);
    // The mis-scheduled shape: two separate top-level `for` loops, the `reduce` one printed after
    // the `init`/`copy` one — i.e. every `init`/`copy` runs before any `reduce`, not interleaved
    // per-`i` the way strict lex order over [i,phase,j] would require.
    let init_pos = printed.find("init(").expect("init call present");
    let copy_pos = printed.find("copy(").expect("copy call present");
    let reduce_pos = printed.find("reduce(").expect("reduce call present");
    assert!(
        init_pos < reduce_pos && copy_pos < reduce_pos,
        "expected this known-bad heterogeneous-width case to (still) misorder init/copy before \
         reduce; if this now fails, isl's behavior has changed and §6's width-uniformity rule \
         may be revisitable: {printed}"
    );
}

/// The corrected counterpart to
/// [`heterogeneous_width_schedule_does_not_preserve_lex_order`]: the same three statements,
/// padded to a uniform 3-wide shared schedule space (§6's actual rule) instead of the doc's own
/// inconsistent (2,3,2)-width worked example. This produces exactly the fully-nested loop
/// structure §6's prose describes ("for each i, initialize, then accumulate over increasing j,
/// then copy out") — confirming uniform width is both necessary (previous test) and sufficient.
#[test]
fn uniform_width_schedule_preserves_lex_order_via_full_nesting() {
    let ctx = Context::new();
    let text = "[N] -> { init[i] -> [i, 0, 0] : 0 <= i < N; \
               reduce[i,j] -> [i, 1, j] : 0 <= j <= i < N; \
               copy[i] -> [i, 2, 0] : 0 <= i < N; }";
    let umap = UnionMap::read_from_str(&ctx, text).unwrap();
    let context = Set::read_from_str(&ctx, "[N] -> { : N > 0 }").unwrap();
    let build = AstBuild::from_context(context).unwrap();
    let printed = build.generate(umap).unwrap().to_string_fmt(Format::C);
    let init_pos = printed.find("init(").expect("init call present");
    let copy_pos = printed.find("copy(").expect("copy call present");
    let reduce_pos = printed.find("reduce(").expect("reduce call present");
    assert!(
        init_pos < reduce_pos && reduce_pos < copy_pos,
        "expected init, then reduce, then copy, fully nested under one shared loop: {printed}"
    );
    // Exactly one outer `for` — everything fused into a single loop nest, not split in two.
    assert_eq!(
        printed.matches("for (int c0").count(),
        1,
        "expected one shared outer loop, not separate loops per statement: {printed}"
    );
}
