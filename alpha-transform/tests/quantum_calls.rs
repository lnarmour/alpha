use alpha_model::{RegisteredOperation, Resolver};
use alpha_transform::{ir, lower};
use isl::{Context, DimType};

const GATES: &str = r#"affine gates [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;

fn lower_first(source: &str) -> ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let (lowered, diagnostics) = lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    lowered
}

#[test]
fn lowers_registered_call_to_explicit_operation_ir() {
    let system = lower_first(GATES);
    let ir::Equation::OperationCall(call) = &system.bodies[0].equations[0] else {
        panic!("expected operation call")
    };
    assert_eq!(call.operation, RegisteredOperation::Cx);
    assert_eq!(call.index_names, ["i"]);
    assert_eq!(call.domain.to_string(), "[N] -> { [i] : 0 <= i < N }");
    assert_eq!(call.outputs.len(), 2);
    assert_eq!(call.inputs.len(), 2);
    for access in call.outputs.iter().chain(&call.inputs) {
        assert_eq!(access.function.n_out(), 1);
        let affine = access.function.get_aff(0).unwrap();
        let coefficient = affine.coefficient(DimType::In, 0).unwrap();
        assert_eq!((coefficient.num_si(), coefficient.den_si()), (1, 1));
        assert!(affine.constant().unwrap().is_zero());
    }
}

#[test]
fn operation_calls_survive_normalization_and_printing() {
    let mut system = lower_first(GATES);
    alpha_transform::normalize_reduction::apply(&mut system);
    let system = alpha_transform::normalize::apply(system, true);
    assert!(matches!(
        system.bodies[0].equations[0],
        ir::Equation::OperationCall(_)
    ));
    for rendered in [
        alpha_transform::print::show(&system),
        alpha_transform::print::ashow(&system),
    ] {
        assert!(
            rendered.contains("(Q1[i], R1[i]) = cx(Q0[i], R0[i]);"),
            "{rendered}"
        );
        assert!(
            alpha_syntax::parse(&rendered).errors.is_empty(),
            "{rendered}"
        );
    }
}
