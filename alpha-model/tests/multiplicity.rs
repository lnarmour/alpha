use alpha_model::{check_source, Diagnostic};

fn diagnostics(source: &str) -> Vec<Diagnostic> {
    check_source(source)
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect()
}

const LINEAR_TO_UNRESTRICTED: &str = r#"affine widened [N] -> {: N > 0}
    inputs linear X : {[i] : 0 <= i < N};
    outputs Y : {[i] : 0 <= i < N};
    let
        Y[i] = X[i];
.
"#;

#[test]
fn linear_value_cannot_flow_to_unrestricted_target() {
    let diagnostics = diagnostics(LINEAR_TO_UNRESTRICTED);

    assert!(diagnostics.iter().any(
        |diagnostic| matches!(diagnostic, Diagnostic::LinearValueWidened { target, .. } if target == "Y")
    ));
}

#[test]
fn unrestricted_value_can_be_restricted_to_a_linear_target() {
    let source = LINEAR_TO_UNRESTRICTED
        .replace("linear X", "X")
        .replace("outputs Y", "outputs linear Y");

    assert!(diagnostics(&source).is_empty());
}

#[test]
fn linear_value_can_flow_to_a_linear_target() {
    let source = LINEAR_TO_UNRESTRICTED.replace("outputs Y", "outputs linear Y");

    assert!(diagnostics(&source).is_empty());
}

#[test]
fn existing_operators_reject_linear_operands() {
    let source = LINEAR_TO_UNRESTRICTED.replace("X[i]", "-((i -> i) @ X)");
    let diagnostics = diagnostics(&source);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            Diagnostic::LinearArgumentToUnrestrictedPort { operator, .. } if operator == "-"
        )),
        "{diagnostics:#?}"
    );
}

#[test]
fn binary_operators_reject_linear_operands() {
    let source = LINEAR_TO_UNRESTRICTED.replace("X[i]", "X[i] + X[i]");
    let diagnostics = diagnostics(&source);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            Diagnostic::LinearArgumentToUnrestrictedPort { operator, .. } if operator == "+"
        )),
        "{diagnostics:#?}"
    );
}

#[test]
fn legacy_external_functions_reject_linear_operands() {
    let source = format!(
        "external f(2)\n{}",
        LINEAR_TO_UNRESTRICTED.replace("X[i]", "f(X[i], X[i])")
    );
    let diagnostics = diagnostics(&source);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            Diagnostic::LinearArgumentToUnrestrictedPort { operator, .. } if operator == "f"
        )),
        "{diagnostics:#?}"
    );
}

#[test]
fn reduction_with_linear_reference_is_explicitly_unsupported() {
    let source = r#"affine reduced [N] -> {: N > 0}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = reduce(+, [j], {: 0 <= j <= i} : X[j]);
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::LinearityUnsupportedHere { construct, .. } if construct == "reduce"
    )), "{diagnostics:#?}");
}