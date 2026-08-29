use crate::ir;
use alpha_model::{registered_operation, ElementType, Multiplicity, RegisteredOperation};
use isl::{DimType, Map, MultiAff, Set};
use std::collections::HashMap;

pub struct ContinuityEdge {
    pub statement: String,
    pub input_variable: String,
    pub output_variable: String,
    pub relation: Map,
}

pub enum ResourceRootKind {
    SystemInput,
    OperationOutput(RegisteredOperation),
}

pub struct ResourceRoot {
    pub statement: Option<String>,
    pub variable: String,
    pub kind: ResourceRootKind,
    pub relation: Map,
}

pub enum ResourceSinkKind {
    SystemOutput,
    OperationInput(RegisteredOperation),
}

pub struct ResourceSink {
    pub statement: Option<String>,
    pub variable: String,
    pub kind: ResourceSinkKind,
    pub relation: Map,
}

pub struct ResourceFlow {
    pub edges: Vec<ContinuityEdge>,
    pub roots: Vec<ResourceRoot>,
    pub sinks: Vec<ResourceSink>,
}

#[derive(Debug)]
pub struct ResourceFlowError {
    pub message: String,
}

impl std::fmt::Display for ResourceFlowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResourceFlowError {}

impl From<isl::IslError> for ResourceFlowError {
    fn from(error: isl::IslError) -> Self {
        Self {
            message: error.message,
        }
    }
}

fn resource_domain(domain: &Set, parameters: &Set) -> Result<Set, ResourceFlowError> {
    Ok(domain.clone().intersect_params(parameters.clone())?)
}

fn identity(domain: &Set, parameters: &Set) -> Result<Map, ResourceFlowError> {
    let domain = resource_domain(domain, parameters)?;
    Ok(MultiAff::identity_on_domain_space(domain.space())?
        .into_map()?
        .intersect_domain(domain)?
        .reset_tuple_name(DimType::In)?
        .reset_tuple_name(DimType::OutOrSet)?)
}

fn statement_name(call: &ir::OperationCall, counters: &mut HashMap<String, usize>) -> String {
    let base = call
        .outputs
        .first()
        .map(|access| access.variable.as_str())
        .unwrap_or_else(|| call.operation.name());
    let counter = counters.entry(base.to_string()).or_default();
    let name = format!("{base}__call{counter}");
    *counter += 1;
    name
}

fn access_map(
    access: &ir::Access,
    domain: &Set,
    parameters: &Set,
) -> Result<Map, ResourceFlowError> {
    Ok(access
        .function
        .clone()
        .into_map()?
        .intersect_domain(resource_domain(domain, parameters)?)?
        .reset_tuple_name(DimType::In)?
        .reset_tuple_name(DimType::OutOrSet)?)
}

fn union_sets(sets: impl IntoIterator<Item = Set>) -> Result<Option<Set>, ResourceFlowError> {
    let mut sets = sets.into_iter();
    let Some(mut union) = sets.next() else {
        return Ok(None);
    };
    for set in sets {
        union = union.union(set)?;
    }
    Ok(Some(union))
}

pub fn analyze(system: &ir::System) -> Result<ResourceFlow, ResourceFlowError> {
    let quantum_variables: HashMap<_, _> = system
        .inputs
        .iter()
        .chain(&system.outputs)
        .chain(&system.locals)
        .filter(|variable| {
            variable.element_type == ElementType::Qubit
                && variable.multiplicity == Multiplicity::Linear
        })
        .map(|variable| (variable.name.as_str(), variable))
        .collect();

    let mut flow = ResourceFlow {
        edges: Vec::new(),
        roots: Vec::new(),
        sinks: Vec::new(),
    };
    for variable in &system.inputs {
        if quantum_variables.contains_key(variable.name.as_str()) {
            flow.roots.push(ResourceRoot {
                statement: None,
                variable: variable.name.clone(),
                kind: ResourceRootKind::SystemInput,
                relation: identity(&variable.domain, &system.parameter_domain)?,
            });
        }
    }
    for variable in &system.outputs {
        if quantum_variables.contains_key(variable.name.as_str()) {
            flow.sinks.push(ResourceSink {
                statement: None,
                variable: variable.name.clone(),
                kind: ResourceSinkKind::SystemOutput,
                relation: identity(&variable.domain, &system.parameter_domain)?,
            });
        }
    }

    let mut counters = HashMap::new();
    for body in &system.bodies {
        for equation in &body.equations {
            let ir::Equation::OperationCall(call) = equation else {
                continue;
            };
            let statement = statement_name(call, &mut counters);
            let signature = registered_operation(call.operation.name())
                .expect("IR operation must remain registered");
            for continuity in &signature.continuity {
                let input = &call.inputs[continuity.input];
                let output = &call.outputs[continuity.output];
                if !quantum_variables.contains_key(input.variable.as_str())
                    || !quantum_variables.contains_key(output.variable.as_str())
                {
                    return Err(ResourceFlowError {
                        message: format!(
                            "continuity for {statement} changes resource type from '{}' to '{}'",
                            input.variable, output.variable
                        ),
                    });
                }
                let relation = access_map(input, &call.domain, &system.parameter_domain)?
                    .reverse()?
                    .apply_range(access_map(output, &call.domain, &system.parameter_domain)?)?
                    .reset_tuple_name(DimType::In)?
                    .reset_tuple_name(DimType::OutOrSet)?;
                if !relation.is_injective()? {
                    return Err(ResourceFlowError {
                        message: format!("continuity for {statement} converges: {relation}"),
                    });
                }
                if !relation.clone().reverse()?.is_injective()? {
                    return Err(ResourceFlowError {
                        message: format!("continuity for {statement} branches: {relation}"),
                    });
                }
                flow.edges.push(ContinuityEdge {
                    statement: statement.clone(),
                    input_variable: input.variable.clone(),
                    output_variable: output.variable.clone(),
                    relation,
                });
            }
            for (index, output) in call.outputs.iter().enumerate() {
                if !signature
                    .continuity
                    .iter()
                    .any(|continuity| continuity.output == index)
                    && output.variable.as_str() != ""
                    && quantum_variables.contains_key(output.variable.as_str())
                {
                    flow.roots.push(ResourceRoot {
                        statement: Some(statement.clone()),
                        variable: output.variable.clone(),
                        kind: ResourceRootKind::OperationOutput(call.operation),
                        relation: access_map(output, &call.domain, &system.parameter_domain)?,
                    });
                }
            }
            for (index, input) in call.inputs.iter().enumerate() {
                if !signature
                    .continuity
                    .iter()
                    .any(|continuity| continuity.input == index)
                    && quantum_variables.contains_key(input.variable.as_str())
                {
                    flow.sinks.push(ResourceSink {
                        statement: Some(statement.clone()),
                        variable: input.variable.clone(),
                        kind: ResourceSinkKind::OperationInput(call.operation),
                        relation: access_map(input, &call.domain, &system.parameter_domain)?
                            .reverse()?,
                    });
                }
            }
        }
    }

    for variable in quantum_variables.values() {
        let variable_domain =
            resource_domain(&variable.domain, &system.parameter_domain)?.reset_tuple_name()?;
        let incoming = flow
            .edges
            .iter()
            .filter(|edge| edge.output_variable == variable.name)
            .map(|edge| edge.relation.clone().range())
            .chain(
                flow.roots
                    .iter()
                    .filter(|root| root.variable == variable.name)
                    .map(|root| root.relation.clone().range()),
            )
            .collect::<Result<Vec<_>, _>>()?;
        let outgoing = flow
            .edges
            .iter()
            .filter(|edge| edge.input_variable == variable.name)
            .map(|edge| edge.relation.clone().domain())
            .chain(
                flow.sinks
                    .iter()
                    .filter(|sink| sink.variable == variable.name)
                    .map(|sink| sink.relation.clone().domain()),
            )
            .collect::<Result<Vec<_>, _>>()?;
        for sets in [&incoming, &outgoing] {
            for (index, left) in sets.iter().enumerate() {
                for right in &sets[index + 1..] {
                    if !left.is_disjoint(right)? {
                        return Err(ResourceFlowError {
                            message: format!(
                                "resource flow for '{}' branches or converges on {}",
                                variable.name,
                                left.clone().intersect(right.clone())?
                            ),
                        });
                    }
                }
            }
        }
        for (direction, sets) in [("incoming", incoming), ("outgoing", outgoing)] {
            let covered = union_sets(sets)?.ok_or_else(|| ResourceFlowError {
                message: format!("resource '{}' has no {direction} flow", variable.name),
            })?;
            if !covered.is_equal(&variable_domain)? {
                return Err(ResourceFlowError {
                    message: format!(
                        "resource '{}' has incomplete {direction} flow: {}",
                        variable.name,
                        variable_domain.clone().subtract(covered)?
                    ),
                });
            }
        }
    }
    Ok(flow)
}
