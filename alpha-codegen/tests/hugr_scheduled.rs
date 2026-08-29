use alpha_codegen::specialize::ParameterBindings;
use alpha_model::Resolver;
use alpha_transform::ir;
use hugr::extension::prelude::{bool_t, qb_t};
use hugr::ops::OpTrait;
use hugr::std_extensions::collections::array::array_type;
use hugr::HugrView;
use isl::Context;
use tket::TketOp;

fn normalized(source: &str) -> ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(Context::new(), &system);
    assert!(alpha_model::analyze_system(&mut resolver, &system).is_empty());
    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty());
    alpha_transform::normalize_reduction::apply(&mut lowered);
    alpha_transform::normalize::apply(lowered, true)
}

fn count_tket(hugr: &hugr::Hugr, op: TketOp) -> usize {
    hugr.nodes()
        .filter(|node| hugr.get_optype(*node).cast::<TketOp>() == Some(op))
        .count()
}

#[test]
fn emits_scheduled_allocation_chain() {
    let system = normalized(include_str!("src/quantum_chain.alpha"));
    let schedule =
        "[T,N] -> { Q__call0[t,i] -> [t,0,i]; Q__call1[t,i] -> [t,1,i]; M__call0[i] -> [T,2,i] }";
    let bindings = ParameterBindings::from([("T".into(), 3), ("N".into(), 4)]);
    let hugr = alpha_codegen::generate_hugr(&system, schedule, &bindings).unwrap();
    hugr.validate().unwrap();
    let signature = hugr
        .get_optype(hugr.entrypoint())
        .dataflow_signature()
        .unwrap();
    assert_eq!(signature.input_count(), 0);
    assert_eq!(signature.output_count(), 1);
    assert_eq!(signature.out_port_type(0), Some(&array_type(4, bool_t())));
    assert!(hugr
        .nodes()
        .any(|node| hugr.get_optype(node).is_tail_loop()));
    assert_eq!(count_tket(&hugr, TketOp::QAlloc), 1);
    assert_eq!(count_tket(&hugr, TketOp::H), 1);
    assert_eq!(count_tket(&hugr, TketOp::MeasureFree), 1);
}

#[test]
fn emits_scheduled_cx_boundaries() {
    let system = normalized(include_str!("src/quantum_cx.alpha"));
    let bindings = ParameterBindings::from([("N".into(), 4)]);
    let hugr = alpha_codegen::generate_hugr(&system, "", &bindings).unwrap();
    hugr.validate().unwrap();
    let signature = hugr
        .get_optype(hugr.entrypoint())
        .dataflow_signature()
        .unwrap();
    assert_eq!(signature.input_count(), 2);
    assert_eq!(signature.output_count(), 2);
    assert_eq!(signature.in_port_type(0), Some(&array_type(4, qb_t())));
    assert_eq!(signature.out_port_type(0), Some(&array_type(4, qb_t())));
    assert_eq!(count_tket(&hugr, TketOp::CX), 1);
    let envelope = alpha_codegen::generate_hugr_system(&system, "", &bindings).unwrap();
    assert!(envelope.contains("HUGRiHJ"));
}

#[test]
fn rejects_missing_specialization() {
    let system = normalized(include_str!("src/quantum_cx.alpha"));
    let error = alpha_codegen::generate_hugr(&system, "", &ParameterBindings::new())
        .err()
        .unwrap();
    assert!(error.to_string().contains("missing parameter 'N'"));
}

#[test]
fn rejects_triangular_domain() {
    let system = normalized(
        r#"affine triangular [N] -> {:N>0}
inputs linear Q0 : {[i,j] : 0 <= i < N and 0 <= j <= i} of qubit;
outputs linear Q1 : {[i,j] : 0 <= i < N and 0 <= j <= i} of qubit;
let with [i,j] : (Q1[i,j]) = h(Q0[i,j]);
.
"#,
    );
    let error =
        alpha_codegen::generate_hugr(&system, "", &ParameterBindings::from([("N".into(), 3)]))
            .err()
            .unwrap();
    assert!(error.to_string().contains("not rectangular"), "{error}");
}

#[test]
fn rejects_unproved_compact_realization() {
    let system = normalized(
        r#"affine offset [N] -> {:N>0}
inputs linear Q0 : {[i] : 1 <= i <= N} of qubit;
outputs linear Q1 : {[i] : 1 <= i <= N} of qubit;
let with [i] : (Q1[i]) = h(Q0[i]);
.
"#,
    );
    let error =
        alpha_codegen::generate_hugr(&system, "", &ParameterBindings::from([("N".into(), 3)]))
            .err()
            .unwrap();
    assert!(error.to_string().contains("not zero-based"), "{error}");
}
