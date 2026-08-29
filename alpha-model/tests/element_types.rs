use alpha_model::{check_source, Diagnostic, ElementType, Resolver};
use isl::Context;

fn diagnostics(source: &str) -> Vec<Diagnostic> {
    check_source(source)
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect()
}

#[test]
fn explicitly_linear_qubits_are_valid() {
    let source = r#"affine q [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    outputs linear R : {[i] : 0 <= i < 2} of qubit;
    let R[i] = Q[i];
.
"#;
    assert!(diagnostics(source).is_empty());
}

#[test]
fn unrestricted_qubits_are_rejected() {
    let source = r#"affine q [] -> {:}
    inputs Q : {[i] : 0 <= i < 2} of qubit;
    outputs Y : {[i] : 0 <= i < 2};
    let Y[i] = 0;
.
"#;
    let diagnostics = diagnostics(source);
    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::QubitMustBeLinear { variable, .. } if variable == "Q"
    )));
}

#[test]
fn comma_group_inherits_terminating_element_type() {
    let source = r#"affine q [N] -> {:N>0}
    inputs linear Q, R : {[i] : 0 <= i < N} of qubit;
    outputs linear S, T : {[i] : 0 <= i < N} of qubit;
    let S[i] = Q[i]; T[i] = R[i];
.
"#;
    assert!(diagnostics(source).is_empty());

    let parse = alpha_syntax::parse(source);
    let system = parse.tree().systems().next().unwrap();
    let resolver = Resolver::new(Context::new(), &system);
    assert_eq!(resolver.variable_type("Q"), Some(ElementType::Qubit));
    assert_eq!(resolver.variable_type("R"), Some(ElementType::Qubit));
}

#[test]
fn untyped_variables_resolve_as_unspecified() {
    let source = "affine id [N] -> {:N>0} inputs X:[N] outputs Y:[N] let Y[i]=X[i];.";
    assert!(diagnostics(source).is_empty());

    let parse = alpha_syntax::parse(source);
    let system = parse.tree().systems().next().unwrap();
    let resolver = Resolver::new(Context::new(), &system);
    assert_eq!(resolver.variable_type("X"), Some(ElementType::Unspecified));
}
