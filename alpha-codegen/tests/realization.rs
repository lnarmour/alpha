use alpha_codegen::realize::{self, EntryOccupancy, ExitOccupancy};
use alpha_codegen::scheduled_ir;
use alpha_codegen::specialize::{self, ParameterBindings};
use alpha_model::{RegisteredOperation, Resolver};
use alpha_transform::{ir, resource_flow};
use isl::{Context, Map};

fn normalized(source: &str) -> ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    alpha_transform::normalize_reduction::apply(&mut lowered);
    alpha_transform::normalize::apply(lowered, true)
}

#[test]
fn repeated_chain_uses_one_lane_per_root_point() {
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
    let schedule = "[T,N] -> { Q__call0[t,i] -> [t,0,i]; \
        Q__call1[t,i] -> [t,1,i]; M__call0[i] -> [T,2,i] }";
    let scheduled = scheduled_ir::build(&system, schedule).unwrap();
    let bindings = ParameterBindings::from([("T".to_string(), 3), ("N".to_string(), 4)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();

    let realization = realize::infer(&specialized, &flow).unwrap();
    assert_eq!(realization.groups.len(), 1);
    assert_eq!(realization.groups[0].shape, vec![4]);
    assert_eq!(realization.groups[0].size, 4);
    assert_eq!(realization.groups[0].entry, EntryOccupancy::Empty);
    assert_eq!(realization.groups[0].exit, ExitOccupancy::Empty);
    let expected = Map::read_from_str(
        &system.parameter_domain.ctx(),
        "{ [t,i] -> [i] : 0 <= t <= 2 and 0 <= i <= 3 }",
    )
    .unwrap();
    assert!(realization
        .logical_lane_map("Q")
        .unwrap()
        .is_equal(&expected)
        .unwrap());
}

#[test]
fn rejects_non_rectangular_root_domains() {
    let source = r#"affine triangular [N] -> {:N>0}
    inputs linear Q0 : {[i,j] : 0 <= i < N and 0 <= j <= i} of qubit;
    outputs linear Q1 : {[i,j] : 0 <= i < N and 0 <= j <= i} of qubit;
    let with [i,j] : (Q1[i,j]) = h(Q0[i,j]);
.
"#;
    let system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();
    let scheduled = scheduled_ir::build(&system, "").unwrap();
    let bindings = ParameterBindings::from([("N".to_string(), 3)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    let error = realize::infer(&specialized, &flow).err().unwrap();
    assert!(error.to_string().contains("not rectangular"), "{error}");
}

#[test]
fn system_boundaries_remain_occupied() {
    let source = r#"affine boundary [N] -> {:N>0}
    inputs linear Q0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i]) = h(Q0[i]);
.
"#;
    let system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();
    let scheduled = scheduled_ir::build(&system, "").unwrap();
    let bindings = ParameterBindings::from([("N".to_string(), 3)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    let realization = realize::infer(&specialized, &flow).unwrap();
    assert_eq!(realization.groups.len(), 1);
    assert_eq!(realization.groups[0].shape, vec![3]);
    assert_eq!(realization.groups[0].entry, EntryOccupancy::Occupied);
    assert_eq!(realization.groups[0].exit, ExitOccupancy::Occupied);
}

#[test]
fn cx_preserves_two_distinct_resource_groups() {
    let source = r#"affine gates [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    let system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();
    let scheduled = scheduled_ir::build(&system, "").unwrap();
    let bindings = ParameterBindings::from([("N".to_string(), 2)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    let realization = realize::infer(&specialized, &flow).unwrap();
    assert_eq!(realization.groups.len(), 2);
    assert_eq!(realization.operations.len(), 1);
    assert_eq!(realization.operations[0].inputs.len(), 2);
    assert_eq!(realization.operations[0].outputs.len(), 2);
    assert!(realization
        .groups
        .iter()
        .all(|group| group.shape == vec![2] && group.size == 2));
    let q_group = realization
        .logical_to_lane
        .iter()
        .find(|mapping| mapping.variable == "Q0")
        .unwrap()
        .group;
    let r_group = realization
        .logical_to_lane
        .iter()
        .find(|mapping| mapping.variable == "R0")
        .unwrap()
        .group;
    assert_ne!(q_group, r_group);
}

#[test]
fn rejects_operands_that_realize_to_the_same_lane() {
    let source = r#"affine gates [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    let mut system = normalized(source);
    let flow = resource_flow::analyze(&system).unwrap();
    let ir::Equation::OperationCall(call) = &mut system.bodies[0].equations[0] else {
        panic!("expected operation call");
    };
    call.inputs[1] = call.inputs[0].clone();

    let scheduled = scheduled_ir::build(&system, "").unwrap();
    let bindings = ParameterBindings::from([("N".to_string(), 2)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    let error = realize::infer(&specialized, &flow).err().unwrap();
    assert!(error.to_string().contains("aliased resource operands"));
}

#[test]
fn rejects_mixed_sink_occupancy_within_a_group() {
    let source = r#"affine boundary [N] -> {:N>0}
    inputs linear Q0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i]) = h(Q0[i]);
.
"#;
    let system = normalized(source);
    let mut flow = resource_flow::analyze(&system).unwrap();
    flow.sinks.push(resource_flow::ResourceSink {
        statement: Some("synthetic_measure".to_string()),
        variable: "Q1".to_string(),
        kind: resource_flow::ResourceSinkKind::OperationInput(RegisteredOperation::Measure),
        relation: flow.sinks[0].relation.clone(),
    });
    let scheduled = scheduled_ir::build(&system, "").unwrap();
    let bindings = ParameterBindings::from([("N".to_string(), 2)]);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    let error = realize::infer(&specialized, &flow).err().unwrap();
    assert!(error.to_string().contains("mixed sink occupancy"));
}
