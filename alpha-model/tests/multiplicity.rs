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

#[test]
fn identity_transfer_consumes_each_linear_point_once() {
    let source = LINEAR_TO_UNRESTRICTED.replace("outputs Y", "outputs linear Y");

    assert!(diagnostics(&source).is_empty());
}

#[test]
fn two_reads_of_the_same_linear_points_overlap() {
    let source = r#"affine duplicate [N] -> {: N > 0}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y, Z : {[i] : 0 <= i < N};
    let
        Y[i] = X[i];
        Z[i] = X[i];
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::LinearUsesOverlap { variable, .. } if variable == "X"
    )), "{diagnostics:#?}");
}

#[test]
fn broadcast_read_is_not_injective() {
    let source = r#"affine broadcast [N] -> {: N > 0}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i, j] : 0 <= i < N and 0 <= j < 2};
    let
        Y[i, j] = X[i];
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::LinearUseNotInjective { variable, .. } if variable == "X"
    )), "{diagnostics:#?}");
}

#[test]
fn partially_read_linear_input_reports_unconsumed_points() {
    let source = r#"affine partial [N] -> {: N > 1}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N - 1};
    let
        Y[i] = X[i];
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::LinearValueUnconsumed { variable, detail, .. }
            if variable == "X" && detail.contains("N")
    )), "{diagnostics:#?}");
}

#[test]
fn affine_permutation_consumes_each_linear_point_once() {
    let source = LINEAR_TO_UNRESTRICTED
        .replace("outputs Y", "outputs linear Y")
        .replace("X[i]", "X[N - i - 1]");

    assert!(diagnostics(&source).is_empty());
}

#[test]
fn runtime_if_branches_must_consume_the_same_linear_resources() {
    let source = r#"affine branch [N] -> {: N > 0}
    inputs C : {[i] : 0 <= i < N};
           linear X, A : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = if C[i] then X[i] else A[i];
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::LinearBranchMismatch { .. }
    )), "{diagnostics:#?}");
}

#[test]
fn runtime_if_counts_equal_branch_resources_once() {
    let source = r#"affine branch [N] -> {: N > 0}
    inputs C : {[i] : 0 <= i < N};
           linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = if C[i] then X[i] else X[i];
.
"#;

    let diagnostics = diagnostics(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn disjoint_case_branches_union_linear_uses() {
    let source = r#"affine pieces [N] -> {: N > 1}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = case {
            {: i < N - 1} : X[i];
            {: i >= N - 1} : X[i];
        };
.
"#;

    let diagnostics = diagnostics(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn overlapping_case_branches_report_overlapping_linear_uses() {
    let source = r#"affine overlapping_pieces [N] -> {: N > 1}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = case {
            {: i < N} : X[i];
            {: i >= N - 1} : X[i];
        };
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, Diagnostic::LinearUsesOverlap { .. })),
        "{diagnostics:#?}");
}

#[test]
fn runtime_if_compares_access_relations_not_only_variable_names() {
    let source = r#"affine branch_map [N] -> {: N > 1}
    inputs C : {[i] : 0 <= i < N};
           linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let
        Y[i] = if C[i] then X[i] else X[N - i - 1];
.
"#;
    let diagnostics = diagnostics(source);

    assert!(diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, Diagnostic::LinearBranchMismatch { .. })),
        "{diagnostics:#?}");
}