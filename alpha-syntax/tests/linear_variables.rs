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