use alpha_codegen::scheduled_ir::{self, IndexExpr, ScheduledNode, StatementId};
use alpha_model::Resolver;

fn normalized(source: &str) -> alpha_transform::ir::System {
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let system = parse.tree().systems().next().unwrap();
    let mut resolver = Resolver::new(isl::Context::new(), &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let (mut lowered, diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    alpha_transform::normalize_reduction::apply(&mut lowered);
    alpha_transform::normalize::apply(lowered, true)
}

fn invocations(node: &ScheduledNode) -> Vec<(StatementId, &[IndexExpr])> {
    let mut result = Vec::new();
    fn visit<'a>(node: &'a ScheduledNode, result: &mut Vec<(StatementId, &'a [IndexExpr])>) {
        match node {
            ScheduledNode::Loop { body, .. } => visit(body, result),
            ScheduledNode::If {
                then_body,
                else_body,
                ..
            } => {
                visit(then_body, result);
                if let Some(else_body) = else_body {
                    visit(else_body, result);
                }
            }
            ScheduledNode::Sequence(nodes) => {
                for node in nodes {
                    visit(node, result);
                }
            }
            ScheduledNode::Invoke { statement, indices } => {
                result.push((*statement, indices));
            }
        }
    }
    visit(node, &mut result);
    result
}

#[test]
fn identity_and_reverse_are_typed_index_expressions() {
    let system =
        normalized("affine Copy [N]->{:N>0}\ninputs X:[N]\noutputs Y:[N]\nlet Y[i]=X[i];\n.");
    let identity = scheduled_ir::build(&system, "").unwrap();
    let reverse = scheduled_ir::build(&system, "[N] -> { Y[i] -> [N-1-i]; }").unwrap();

    assert!(matches!(identity.root, ScheduledNode::Loop { .. }));
    assert_eq!(invocations(&identity.root)[0].0, StatementId(0));
    assert!(matches!(
        invocations(&identity.root)[0].1,
        [IndexExpr::Variable(_)]
    ));
    assert!(matches!(
        invocations(&reverse.root)[0].1,
        [IndexExpr::Sub(_, _)]
    ));
}

#[test]
fn skewed_two_dimensional_schedule_is_structured() {
    let system = normalized(
        "affine Grid [N]->{:N>0}\noutputs Y:{[i,j]:0<=i<N and 0<=j<N}\nlet Y[i,j]=0[];\n.",
    );
    let program = scheduled_ir::build(&system, "[N] -> { Y[i,j] -> [i,i+j]; }").unwrap();
    assert!(matches!(program.root, ScheduledNode::Loop { .. }));
    assert_eq!(invocations(&program.root)[0].0, StatementId(0));
    insta::assert_debug_snapshot!(program.root);
}

#[test]
fn reductions_resolve_to_stable_statement_ids() {
    let system = normalized(
        "affine Sum [N]->{:N>0}\ninputs X:[N]\noutputs Y:[N]\nlet Y[i]=reduce(+, [j], {:0<=j<=i}: X[j]);\n.",
    );
    let program = scheduled_ir::build(
        &system,
        "{ Y__init[i] -> [i,0,0]; Y__reduce[i,j] -> [i,1,j]; }",
    )
    .unwrap();
    let ids: Vec<_> = invocations(&program.root)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(ids.contains(&StatementId(0)));
    assert!(ids.contains(&StatementId(1)));
    insta::assert_debug_snapshot!(program.root);
}

#[test]
fn registered_operation_invocation_uses_statement_id() {
    let system = normalized(
        "affine Gate [N]->{:N>0}\ninputs linear Q0:{[i]:0<=i<N} of qubit;\noutputs linear Q1:{[i]:0<=i<N} of qubit;\nlet with [i] : (Q1[i]) = h(Q0[i]);\n."
    );
    let program = scheduled_ir::build(&system, "").unwrap();
    assert_eq!(invocations(&program.root)[0].0, StatementId(0));
    assert!(matches!(
        program.statements[0].kind,
        alpha_codegen::stmt::StatementKind::OperationCall(_)
    ));
    assert!(matches!(
        invocations(&program.root)[0].1,
        [IndexExpr::Variable(_)]
    ));
}

#[test]
fn affine_statement_guard_becomes_typed_if() {
    let system = normalized(
        "affine Guard [N]->{:N>0}\noutputs Y:[N]\nlocals W:{[i]:0<=i<N and i<N/2}\nlet Y[i]=0[]; W[i]=1[];\n.",
    );
    let program = scheduled_ir::build(&system, "").unwrap();
    fn contains_if(node: &ScheduledNode) -> bool {
        match node {
            ScheduledNode::If { .. } => true,
            ScheduledNode::Loop { body, .. } => contains_if(body),
            ScheduledNode::Sequence(nodes) => nodes.iter().any(contains_if),
            ScheduledNode::Invoke { .. } => false,
        }
    }
    assert!(contains_if(&program.root), "{:#?}", program.root);
}
