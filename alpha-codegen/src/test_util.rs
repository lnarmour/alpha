//! Shared test scaffolding for `crate::stmt`/`crate::schedule`'s own unit tests — parses,
//! analyzes, lowers, and fully normalizes a small inline Alpha source string, since neither
//! module has (or needs) a fixture-file corpus of its own yet.

use alpha_transform::ir;

pub(crate) fn normalized_system(src: &str) -> ir::System {
    let ctx = isl::Context::new();
    let parse = alpha_syntax::parse(src);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system in fixture");
    let mut resolver = alpha_model::Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (mut ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
    alpha_transform::normalize_reduction::apply(&mut ir_system);
    alpha_transform::normalize::apply(ir_system, true)
}

pub(crate) const PREFIX_SCAN: &str = "affine PrefixScan [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.";

pub(crate) const PLAIN_COPY: &str = "affine Copy [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = X[i];
.";
