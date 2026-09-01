use alpha_model::Resolver;

const LINEAR_TRANSFER: &str = r#"affine Transfer [N] -> {: N > 0}
    inputs linear X : {[i] : 0 <= i < N};
    outputs linear Y : {[i] : 0 <= i < N};
    let Y[i] = X[i];
.
"#;

#[test]
fn legal_schedules_do_not_change_linear_resource_validation() {
    let diagnostics = alpha_model::check_source(LINEAR_TRANSFER);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let parse = alpha_syntax::parse(LINEAR_TRANSFER);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().expect("one system");
    let mut resolver = Resolver::new(isl::Context::new(), &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    alpha_transform::normalize_reduction::apply(&mut lowered);
    let normalized = alpha_transform::normalize::apply(lowered, true);

    alpha_codegen::validate_scheduled_system(&normalized, "").unwrap();
    alpha_codegen::validate_scheduled_system(&normalized, "[N] -> { Y[i] -> [N - 1 - i]; }")
        .unwrap();
}
