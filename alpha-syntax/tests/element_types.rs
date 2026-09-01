use alpha_syntax::ast::ElementType;

#[test]
fn parses_explicit_element_types_losslessly() {
    let source = r#"affine circuit [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 4} of qubit;
    outputs B : {[i] : 0 <= i < 4} of bool;
    locals I : {[i] : 0 <= i < 4} of int;
    R : {[i] : 0 <= i < 4} of real;
    let B[i] = false;
.
"#;
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    assert_eq!(parse.syntax_node().text().to_string(), source);

    let system = parse.tree().systems().next().unwrap();
    assert_eq!(
        system
            .inputs()
            .unwrap()
            .variables()
            .next()
            .unwrap()
            .element_type(),
        Some(ElementType::Qubit)
    );
    assert_eq!(
        system
            .outputs()
            .unwrap()
            .variables()
            .next()
            .unwrap()
            .element_type(),
        Some(ElementType::Bool)
    );
    let local_types: Vec<_> = system
        .locals()
        .unwrap()
        .variables()
        .map(|variable| variable.element_type())
        .collect();
    assert_eq!(
        local_types,
        vec![Some(ElementType::Int), Some(ElementType::Real)]
    );
}

#[test]
fn untyped_variables_remain_valid() {
    let parse =
        alpha_syntax::parse("affine id [N] -> {:N>0} inputs X:[N] outputs Y:[N] let Y[i]=X[i];.");
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let variable = parse
        .tree()
        .systems()
        .next()
        .unwrap()
        .inputs()
        .unwrap()
        .variables()
        .next()
        .unwrap();
    assert_eq!(variable.element_type(), None);
}

#[test]
fn comma_group_type_belongs_to_terminating_declaration() {
    let source =
        "affine id [N] -> {:N>0} inputs linear A, B:[N] of qubit outputs C:[N] let C[i]=0;.";
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    assert_eq!(parse.syntax_node().text().to_string(), source);

    let variables: Vec<_> = parse
        .tree()
        .systems()
        .next()
        .unwrap()
        .inputs()
        .unwrap()
        .variables()
        .collect();
    assert_eq!(variables[0].element_type(), None);
    assert_eq!(variables[1].element_type(), Some(ElementType::Qubit));
}

#[test]
fn malformed_element_type_is_diagnosed_losslessly() {
    let source = "affine bad [N] -> {:N>0} inputs X:[N] of; outputs Y:[N] let Y[i]=0;.";
    let parse = alpha_syntax::parse(source);
    assert!(
        parse
            .errors
            .iter()
            .any(|error| error.message.contains("after 'of'")),
        "{:?}",
        parse.errors
    );
    assert_eq!(parse.syntax_node().text().to_string(), source);
}
