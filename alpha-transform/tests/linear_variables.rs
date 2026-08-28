use alpha_model::{Multiplicity, Resolver};
use alpha_transform::lower;
use isl::Context;

const LINEAR_GROUP: &str = r#"affine transfer [N] -> {: N > 0}
    inputs linear X, Y : {[i] : 0 <= i < N};
    outputs Z : {[i] : 0 <= i < N};
    let
        Z[i] = X[i];
.
"#;

#[test]
fn lowering_propagates_group_multiplicity() {
    let parse = alpha_syntax::parse(LINEAR_GROUP);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system");
    let mut resolver = Resolver::new(Context::new(), &system);

    let (lowered, diagnostics) = lower::lower_system(&mut resolver, &system).expect("lowering");

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(lowered.inputs[0].multiplicity, Multiplicity::Linear);
    assert_eq!(lowered.inputs[1].multiplicity, Multiplicity::Linear);
    assert_eq!(
        lowered.outputs[0].multiplicity,
        Multiplicity::Unrestricted
    );
}