const LINEAR_GROUP: &str = r#"affine transfer [N] -> {: N > 0}
    inputs linear X, Y : {[i] : 0 <= i < N};
    outputs Z : {[i] : 0 <= i < N};
    let
        Z[i] = X[i];
.
"#;

#[test]
fn linear_modifier_marks_the_first_variable_in_a_group() {
    let parse = alpha_syntax::parse(LINEAR_GROUP);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    assert_eq!(parse.syntax_node().text().to_string(), LINEAR_GROUP);

    let system = parse.tree().systems().next().expect("one system");
    let variables: Vec<_> = system.inputs().expect("inputs").variables().collect();

    assert_eq!(variables.len(), 2);
    assert!(variables[0].is_linear());
    assert!(!variables[1].is_linear());
}

#[test]
fn ordinary_variable_groups_remain_valid() {
    let source = LINEAR_GROUP.replace("linear X, Y", "X, Y");
    let parse = alpha_syntax::parse(&source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    assert!(parse
        .tree()
        .systems()
        .next()
        .unwrap()
        .inputs()
        .unwrap()
        .variables()
        .all(|variable| !variable.is_linear()));
}

#[test]
fn linear_modifier_is_rejected_inside_a_comma_group() {
    let source = LINEAR_GROUP.replace("linear X, Y", "X, linear Y");
    let parse = alpha_syntax::parse(&source);
    assert!(!parse.errors.is_empty());
}

#[test]
fn external_functions_accept_explicit_multiplicity_signatures() {
    for source in [
        "external move(linear) -> linear\n",
        "external observe(linear) -> unrestricted\n",
        "external destroy(linear) -> ()\n",
    ] {
        let parse = alpha_syntax::parse(source);
        assert!(parse.errors.is_empty(), "{source}: {:?}", parse.errors);
        assert_eq!(parse.syntax_node().text().to_string(), source);

        let external = parse.tree().external_functions().next().unwrap();
        assert!(external.cardinality().is_none());
        assert_eq!(external.input_multiplicities().count(), 1);
    }
}

#[test]
fn legacy_external_cardinality_remains_valid() {
    let source = "external f(2)\n";
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);

    let external = parse.tree().external_functions().next().unwrap();
    assert_eq!(external.cardinality().unwrap().text(), "2");
    assert_eq!(external.input_multiplicities().count(), 0);
}
