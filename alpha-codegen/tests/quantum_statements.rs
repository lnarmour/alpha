use alpha_model::Resolver;
use isl::Context;

fn normalized(source: &str) -> alpha_transform::ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    assert!(alpha_model::analyze_system(&mut resolver, &system).is_empty());
    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    alpha_transform::normalize_reduction::apply(&mut lowered);
    alpha_transform::normalize::apply(lowered, true)
}

#[test]
fn operation_statements_have_deterministic_names() {
    let source = r#"affine calls [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    let description = alpha_codegen::describe_normalized_system(&normalized(source), "").unwrap();
    assert!(description.contains("Q1__call0"), "{description}");
}

#[test]
fn zero_output_calls_use_the_operation_name() {
    let source = r#"affine sink [N] -> {:N>0}
    inputs linear Q : {[i] : 0 <= i < N} of qubit;
    let with [i] : () = discard(Q[i]);
.
"#;
    let description = alpha_codegen::describe_normalized_system(&normalized(source), "").unwrap();
    assert!(description.contains("discard__call0"), "{description}");
}

#[test]
fn same_output_base_gets_source_order_suffixes() {
    let source = r#"affine split [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < 2*N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < 2*N} of qubit;
    let
        over {[i] : 0 <= i < N} with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
        over {[i] : 0 <= i < N} with [i] : (Q1[N+i], R1[N+i]) = cx(Q0[N+i], R0[N+i]);
.
"#;
    let description = alpha_codegen::describe_normalized_system(&normalized(source), "").unwrap();
    assert!(description.contains("Q1__call0"), "{description}");
    assert!(description.contains("Q1__call1"), "{description}");
}
