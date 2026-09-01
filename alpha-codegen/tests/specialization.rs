use alpha_codegen::scheduled_ir;
use alpha_codegen::specialize::{self, ParameterBindings};
use alpha_model::Resolver;

fn program() -> (alpha_transform::ir::System, String) {
    let source = "affine Copy [N]->{:N>0}\ninputs X:[N]\noutputs Y:[N]\nlet Y[i]=X[i];\n.";
    let parse = alpha_syntax::parse(source);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(isl::Context::new(), &system);
    assert!(alpha_model::analyze_system(&mut resolver, &system).is_empty());
    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty());
    alpha_transform::normalize_reduction::apply(&mut lowered);
    let normalized = alpha_transform::normalize::apply(lowered, true);
    (normalized, String::new())
}

#[test]
fn validates_and_restricts_parameter_bindings() {
    let (system, schedule) = program();
    let scheduled = scheduled_ir::build(&system, &schedule).unwrap();

    let missing = specialize::apply(&scheduled, &ParameterBindings::new())
        .err()
        .unwrap();
    assert!(missing.to_string().contains("missing parameter 'N'"));

    let mut outside = ParameterBindings::new();
    outside.insert("N".to_string(), 0);
    let outside = specialize::apply(&scheduled, &outside).err().unwrap();
    assert!(outside.to_string().contains("outside the parameter domain"));

    let mut unknown = ParameterBindings::new();
    unknown.insert("N".to_string(), 4);
    unknown.insert("M".to_string(), 2);
    let unknown = specialize::apply(&scheduled, &unknown).err().unwrap();
    assert!(unknown.to_string().contains("unknown parameter 'M'"));

    let mut bindings = ParameterBindings::new();
    bindings.insert("N".to_string(), 4);
    let specialized = specialize::apply(&scheduled, &bindings).unwrap();
    assert_eq!(specialized.bindings["N"], 4);
    let expected =
        isl::Set::read_from_str(&system.parameter_domain.ctx(), "{ [i] : 0 <= i <= 3 }").unwrap();
    assert!(specialized.statement_domains[0]
        .is_equal(&expected)
        .unwrap());
}
