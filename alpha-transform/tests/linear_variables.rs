use alpha_model::{ElementType, Multiplicity, Resolver};
use alpha_transform::{lower, print};
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
    assert_eq!(lowered.outputs[0].multiplicity, Multiplicity::Unrestricted);
}

#[test]
fn lowering_and_printing_preserve_element_types() {
    let source = r#"affine typed [N] -> {:N>0}
    inputs linear Q:[N] of qubit; B:[N] of bool; I:[N] of int; R:[N] of real;
.
"#;
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    let (lowered, diagnostics) = lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    assert_eq!(lowered.inputs[0].element_type, ElementType::Qubit);
    assert_eq!(lowered.inputs[1].element_type, ElementType::Bool);
    assert_eq!(lowered.inputs[2].element_type, ElementType::Int);
    assert_eq!(lowered.inputs[3].element_type, ElementType::Real);

    for rendered in [print::show(&lowered), print::ashow(&lowered)] {
        assert!(rendered.contains("of qubit"), "{rendered}");
        assert!(rendered.contains("of bool"), "{rendered}");
        assert!(rendered.contains("of int"), "{rendered}");
        assert!(rendered.contains("of real"), "{rendered}");
        assert!(
            alpha_syntax::parse(&rendered).errors.is_empty(),
            "{rendered}"
        );
    }
}
