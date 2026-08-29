use alpha_model::{check_source, Continuity, RegisteredOperation};

fn diagnostics(source: &str) -> Vec<alpha_model::Diagnostic> {
    check_source(source)
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect()
}

#[test]
fn registry_records_quantum_continuity() {
    let cx = alpha_model::registered_operation("cx").unwrap();
    assert_eq!(cx.operation, RegisteredOperation::Cx);
    assert_eq!(
        cx.continuity,
        vec![
            Continuity {
                input: 0,
                output: 0
            },
            Continuity {
                input: 1,
                output: 1
            },
        ]
    );
}

#[test]
fn typed_quantum_calls_are_valid_without_external_declarations() {
    let source = r#"affine gates [] -> {:}
    inputs linear Q0, R0 : {[i] : 0 <= i < 2} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < 2} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    assert!(diagnostics(source).is_empty(), "{:#?}", diagnostics(source));
}

#[test]
fn allocation_gates_measurement_and_discard_are_valid() {
    let source = r#"affine circuit [] -> {:}
    outputs M : {[i] : 0 <= i < 2} of bool;
    locals linear Q0, Q1, Q2 : {[i] : 0 <= i < 2} of qubit;
    let
        with [i] : (Q0[i]) = qalloc();
        with [i] : (Q1[i]) = h(Q0[i]);
        with [i] : (Q2[i]) = h(Q1[i]);
        with [i] : (M[i]) = measure(Q2[i]);
.
affine sink [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    let with [i] : () = discard(Q[i]);
.
"#;
    assert!(diagnostics(source).is_empty(), "{:#?}", diagnostics(source));
}

#[test]
fn wrong_operation_arity_is_rejected() {
    let source = r#"affine bad [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    outputs linear R : {[i] : 0 <= i < 2} of qubit;
    let with [i] : (R[i]) = cx(Q[i]);
.
"#;
    assert!(diagnostics(source).iter().any(|diagnostic| matches!(
        diagnostic,
        alpha_model::Diagnostic::OperationArityMismatch { operation, .. } if operation == "cx"
    )));
}

#[test]
fn operation_port_type_mismatch_is_rejected() {
    let source = r#"affine bad [] -> {:}
    inputs B : {[i] : 0 <= i < 2} of bool;
    outputs linear Q : {[i] : 0 <= i < 2} of qubit;
    let with [i] : (Q[i]) = h(B[i]);
.
"#;
    assert!(diagnostics(source).iter().any(|diagnostic| matches!(
        diagnostic,
        alpha_model::Diagnostic::OperationPortTypeMismatch { operation, .. } if operation == "h"
    )));
}

#[test]
fn aliased_linear_gate_operands_are_rejected() {
    let source = r#"affine bad [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    outputs linear R, S : {[i] : 0 <= i < 2} of qubit;
    let with [i] : (R[i], S[i]) = cx(Q[i], Q[i]);
.
"#;
    let diagnostics = diagnostics(source);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            alpha_model::Diagnostic::OperationOperandAliased { operation, .. } if operation == "cx"
        )),
        "{diagnostics:#?}"
    );
}

#[test]
fn disjoint_operands_from_one_linear_variable_are_valid() {
    let source = r#"affine pairs [N] -> {:N>0}
    inputs linear Q : {[i] : 0 <= i < 2*N} of qubit;
    outputs linear R, S : {[i] : 0 <= i < N} of qubit;
    let with [i] : (R[i], S[i]) = cx(Q[2*i], Q[2*i+1]);
.
"#;
    assert!(diagnostics(source).is_empty(), "{:#?}", diagnostics(source));
}

#[test]
fn registered_operation_names_are_reserved() {
    let source = r#"external h(1)
affine qalloc [] -> {:}.
"#;
    let diagnostics = diagnostics(source);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                alpha_model::Diagnostic::ReservedOperationName { .. }
            ))
            .count(),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn rejects_measurement_controlled_gate_expression() {
    let source = r#"affine bad [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    M : {[i] : 0 <= i < 2} of bool;
    outputs linear R : {[i] : 0 <= i < 2} of qubit;
    let R[i] = if M[i] then h(Q[i]) else Q[i];
.
"#;
    let diagnostics = diagnostics(source);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            alpha_model::Diagnostic::InvalidOperationContext { operation, .. } if operation == "h"
        )),
        "{diagnostics:#?}"
    );
}

#[test]
fn implicit_call_domains_must_agree() {
    let source = r#"affine bad [N] -> {:N>1}
    inputs linear Q : {[i] : 0 <= i < N} of qubit;
    linear R : {[i] : 0 <= i < N-1} of qubit;
    outputs linear Q1 : {[i] : 0 <= i < N} of qubit;
    linear R1 : {[i] : 0 <= i < N-1} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q[i], R[i]);
.
"#;
    let diagnostics = diagnostics(source);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            alpha_model::Diagnostic::CallDomainMismatch { operation, .. } if operation == "cx"
        )),
        "{diagnostics:#?}"
    );
}
