use alpha_model::{RegisteredOperation, Resolver};
use alpha_transform::{ir, lower, resource_flow};
use isl::{Context, Map};

fn normalized(source: &str) -> ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let (mut lowered, diagnostics) = lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    alpha_transform::normalize_reduction::apply(&mut lowered);
    alpha_transform::normalize::apply(lowered, true)
}

#[test]
fn extracts_allocation_continuity_and_measurement() {
    let source = r#"affine chain [T,N] -> {:T>0 and N>0}
    outputs M : {[i] : 0 <= i < N} of bool;
    locals linear Q : {[t,i] : 0 <= t < T and 0 <= i < N} of qubit;
    let
        over {[t,i] : t=0 and 0<=i<N} with [t,i] : (Q[t,i]) = qalloc();
        over {[t,i] : 0<t<T and 0<=i<N} with [t,i] : (Q[t,i]) = h(Q[t-1,i]);
        with [i] : (M[i]) = measure(Q[T-1,i]);
.
"#;
    let system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();

    assert_eq!(flow.edges.len(), 1);
    assert_eq!(flow.roots.len(), 1);
    assert!(matches!(
        flow.roots[0].kind,
        resource_flow::ResourceRootKind::OperationOutput(RegisteredOperation::QAlloc)
    ));
    assert_eq!(flow.sinks.len(), 1);
    assert!(matches!(
        flow.sinks[0].kind,
        resource_flow::ResourceSinkKind::OperationInput(RegisteredOperation::Measure)
    ));

    let expected = Map::read_from_str(
        &system.parameter_domain.ctx(),
        "[T,N] -> { [t,i] -> [1+t,i] : 0 <= t < T-1 and 0 <= i < N }",
    )
    .unwrap();
    assert!(
        flow.edges[0].relation.is_equal(&expected).unwrap(),
        "{}",
        flow.edges[0].relation
    );
}

#[test]
fn cx_has_two_independent_continuity_edges() {
    let source = r#"affine gates [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    let flow = resource_flow::analyze(&normalized(source)).unwrap();
    assert_eq!(flow.edges.len(), 2);
    assert_eq!(flow.roots.len(), 2);
    assert_eq!(flow.sinks.len(), 2);
    assert_eq!(flow.edges[0].input_variable, "Q0");
    assert_eq!(flow.edges[0].output_variable, "Q1");
    assert_eq!(flow.edges[1].input_variable, "R0");
    assert_eq!(flow.edges[1].output_variable, "R1");
}

#[test]
fn connects_system_input_to_system_output() {
    let source = r#"affine boundary [N] -> {:N>0}
    inputs linear Q0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i]) = h(Q0[i]);
.
"#;
    let system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();
    assert_eq!(flow.edges.len(), 1);
    assert_eq!(flow.roots.len(), 1);
    assert!(matches!(
        flow.roots[0].kind,
        resource_flow::ResourceRootKind::SystemInput
    ));
    assert_eq!(flow.sinks.len(), 1);
    assert!(matches!(
        flow.sinks[0].kind,
        resource_flow::ResourceSinkKind::SystemOutput
    ));

    let expected = Map::read_from_str(
        &system.parameter_domain.ctx(),
        "[N] -> { [i] -> [i] : 0 <= i < N }",
    )
    .unwrap();
    assert!(flow.edges[0].relation.is_equal(&expected).unwrap());
}
