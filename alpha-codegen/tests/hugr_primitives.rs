use hugr::HugrView;

#[test]
fn borrow_gate_return_validates() {
    let hugr = alpha_codegen::hugr::build_borrow_h_primitive(4, 1).unwrap();
    hugr.validate().unwrap();
}

#[test]
fn counted_tail_loop_validates() {
    let hugr = alpha_codegen::hugr::build_counted_loop_primitive(4).unwrap();
    hugr.validate().unwrap();
    assert!(hugr
        .nodes()
        .any(|node| hugr.get_optype(node).is_tail_loop()));
    assert!(hugr
        .nodes()
        .any(|node| hugr.get_optype(node).is_conditional()));
}

#[test]
fn scheduled_index_and_predicate_variants_validate() {
    let hugr = alpha_codegen::hugr::build_index_lowering_primitive().unwrap();
    hugr.validate().unwrap();
    assert!(
        hugr.nodes()
            .filter(|node| hugr.get_optype(*node).is_conditional())
            .count()
            >= 2
    );
}

#[test]
fn quantum_lifecycle_primitives_validate() {
    let hugr = alpha_codegen::hugr::build_quantum_lifecycle_primitive().unwrap();
    hugr.validate().unwrap();
}
