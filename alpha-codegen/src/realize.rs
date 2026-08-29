use crate::error::{CodegenError, Result};
use crate::scheduled_ir::StatementId;
use crate::specialize::SpecializedProgram;
use alpha_model::{ElementType, Multiplicity};
use alpha_transform::resource_flow::{ResourceFlow, ResourceRootKind, ResourceSinkKind};
use isl::{DimType, Map, MultiAff, Set};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceGroupId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOccupancy {
    Empty,
    Occupied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitOccupancy {
    Empty,
    Occupied,
}

pub struct ResourceGroup {
    pub id: ResourceGroupId,
    pub element_type: ElementType,
    pub shape: Vec<u64>,
    pub size: u64,
    pub entry: EntryOccupancy,
    pub exit: ExitOccupancy,
}

pub struct LogicalLaneMap {
    pub variable: String,
    pub group: ResourceGroupId,
    pub relation: Map,
}

pub struct ResolvedAccess {
    pub group: ResourceGroupId,
    pub lane: MultiAff,
}

pub struct ResolvedOperation {
    pub statement: StatementId,
    pub inputs: Vec<ResolvedAccess>,
    pub outputs: Vec<ResolvedAccess>,
}

pub struct Realization {
    pub groups: Vec<ResourceGroup>,
    pub logical_to_lane: Vec<LogicalLaneMap>,
    pub operations: Vec<ResolvedOperation>,
}

impl Realization {
    pub fn logical_lane_map(&self, variable: &str) -> Option<&Map> {
        self.logical_to_lane
            .iter()
            .find(|entry| entry.variable == variable)
            .map(|entry| &entry.relation)
    }
}

fn specialize_set(program: &SpecializedProgram<'_>, set: Set) -> Result<Set> {
    let parameter_count = set.dim(DimType::Param);
    Ok(set
        .intersect_params(program.parameter_point.clone())?
        .project_out(DimType::Param, 0, parameter_count)?)
}

fn specialize_map(program: &SpecializedProgram<'_>, map: Map) -> Result<Map> {
    let parameter_count = map.dim(DimType::Param);
    Ok(map
        .intersect_params(program.parameter_point.clone())?
        .project_out(DimType::Param, 0, parameter_count)?)
}

fn root_lane_map(domain: &Set) -> Result<(Map, Vec<u64>)> {
    if !domain.is_box()? {
        return Err(CodegenError::Realization(format!(
            "root domain is not rectangular: {domain}"
        )));
    }
    let dimensions = domain.dim(DimType::OutOrSet);
    let mut singleton_dimensions = Vec::new();
    let mut shape = Vec::new();
    for position in 0..dimensions {
        let minimum = domain.dim_min(position)?.constant_value()?.ok_or_else(|| {
            CodegenError::Realization(format!("non-constant lower bound: {domain}"))
        })?;
        let maximum = domain.dim_max(position)?.constant_value()?.ok_or_else(|| {
            CodegenError::Realization(format!("non-constant upper bound: {domain}"))
        })?;
        if minimum == maximum {
            singleton_dimensions.push(position);
        } else {
            if minimum != 0 || maximum < 0 {
                return Err(CodegenError::Realization(format!(
                    "root domain is not zero-based: {domain}"
                )));
            }
            shape.push((maximum as u64).checked_add(1).ok_or_else(|| {
                CodegenError::Realization("resource extent overflows u64".to_string())
            })?);
        }
    }
    let mut relation = MultiAff::identity_on_domain_space(domain.space())?
        .into_map()?
        .intersect_domain(domain.clone())?;
    for position in singleton_dimensions.into_iter().rev() {
        relation = relation.project_out(DimType::OutOrSet, position, 1)?;
    }
    if !relation.is_injective()? {
        return Err(CodegenError::Realization(format!(
            "root-to-lane map is not injective: {relation}"
        )));
    }
    Ok((relation, shape))
}

fn propagate(
    program: &SpecializedProgram<'_>,
    flow: &ResourceFlow,
    root_variable: &str,
    root_map: Map,
) -> Result<HashMap<String, Map>> {
    let mut maps = HashMap::from([(root_variable.to_string(), root_map)]);
    let mut changed = true;
    while changed {
        changed = false;
        for edge in &flow.edges {
            let Some(input_map) = maps.get(&edge.input_variable).cloned() else {
                continue;
            };
            let mut relation = specialize_map(program, edge.relation.clone())?;
            if edge.input_variable == edge.output_variable {
                let (closure, exact) = relation.transitive_closure()?;
                if !exact {
                    return Err(CodegenError::Realization(format!(
                        "continuity closure for '{}' is not exact",
                        edge.statement
                    )));
                }
                relation = closure;
            }
            let propagated = relation.reverse()?.apply_range(input_map)?;
            let combined = match maps.get(&edge.output_variable) {
                Some(existing) => {
                    let overlap = existing
                        .clone()
                        .domain()?
                        .intersect(propagated.clone().domain()?)?;
                    if !overlap.is_empty()?
                        && !existing
                            .clone()
                            .intersect_domain(overlap.clone())?
                            .is_equal(&propagated.clone().intersect_domain(overlap)?)?
                    {
                        return Err(CodegenError::Realization(format!(
                            "continuity join for '{}' assigns conflicting lanes",
                            edge.output_variable
                        )));
                    }
                    existing.clone().union(propagated)?
                }
                None => propagated,
            };
            if !combined.clone().reverse()?.is_injective()? {
                return Err(CodegenError::Realization(format!(
                    "resource '{}' maps one logical point to multiple lanes",
                    edge.output_variable
                )));
            }
            let differs = maps
                .get(&edge.output_variable)
                .map(|existing| existing.is_equal(&combined))
                .transpose()?
                != Some(true);
            if differs {
                maps.insert(edge.output_variable.clone(), combined);
                changed = true;
            }
        }
    }
    Ok(maps)
}

fn resolve_operations(
    program: &SpecializedProgram<'_>,
    realization: &Realization,
) -> Result<Vec<ResolvedOperation>> {
    let mut operations = Vec::new();
    for (statement_index, statement) in program.program.statements.iter().enumerate() {
        let crate::stmt::StatementKind::OperationCall(call) = &statement.kind else {
            continue;
        };
        let call_domain = specialize_set(program, call.domain.clone())?;
        let resolve =
            |access: &alpha_transform::ir::Access| -> Result<Option<(ResolvedAccess, Map)>> {
                let mut resolved = Vec::new();
                for logical_map in realization
                    .logical_to_lane
                    .iter()
                    .filter(|mapping| mapping.variable == access.variable)
                {
                    let lane_map = specialize_map(
                        program,
                        access
                            .function
                            .clone()
                            .into_map()?
                            .intersect_domain(call_domain.clone())?
                            .reset_tuple_name(DimType::In)?
                            .reset_tuple_name(DimType::OutOrSet)?,
                    )?
                    .apply_range(logical_map.relation.clone())?;
                    if lane_map.is_empty()? {
                        continue;
                    }
                    let lane = lane_map.clone().as_multi_aff()?.ok_or_else(|| {
                        CodegenError::Realization(format!(
                            "operation '{}' has a piecewise lane expression",
                            call.operation.name()
                        ))
                    })?;
                    resolved.push((
                        ResolvedAccess {
                            group: logical_map.group,
                            lane,
                        },
                        lane_map,
                    ));
                }
                if resolved.is_empty() {
                    return Ok(None);
                }
                if resolved.len() != 1 {
                    return Err(CodegenError::Realization(format!(
                        "operation '{}' access to '{}' resolves to {} trajectories",
                        call.operation.name(),
                        access.variable,
                        resolved.len()
                    )));
                }
                Ok(resolved.pop())
            };
        let inputs: Vec<_> = call
            .inputs
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        for (index, left) in inputs.iter().enumerate() {
            for right in &inputs[index + 1..] {
                if left.0.group == right.0.group
                    && !left.1.clone().intersect(right.1.clone())?.is_empty()?
                {
                    return Err(CodegenError::Realization(format!(
                        "operation '{}' has aliased resource operands",
                        call.operation.name()
                    )));
                }
            }
        }
        let outputs: Vec<_> = call
            .outputs
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        operations.push(ResolvedOperation {
            statement: StatementId(statement_index),
            inputs: inputs.into_iter().map(|access| access.0).collect(),
            outputs: outputs.into_iter().map(|access| access.0).collect(),
        });
    }
    Ok(operations)
}

pub fn infer(program: &SpecializedProgram<'_>, flow: &ResourceFlow) -> Result<Realization> {
    let mut realization = Realization {
        groups: Vec::new(),
        logical_to_lane: Vec::new(),
        operations: Vec::new(),
    };
    let mut covered: HashMap<String, Set> = HashMap::new();

    for root in &flow.roots {
        let root_relation = specialize_map(program, root.relation.clone())?;
        let root_domain = root_relation.range()?;
        let (root_map, shape) = root_lane_map(&root_domain)?;
        let trajectory = propagate(program, flow, &root.variable, root_map)?;
        let group = ResourceGroupId(realization.groups.len());
        let entry = match root.kind {
            ResourceRootKind::SystemInput => EntryOccupancy::Occupied,
            ResourceRootKind::OperationOutput(_) => EntryOccupancy::Empty,
        };
        let mut exit = None;
        for sink in &flow.sinks {
            let Some(lane_map) = trajectory.get(&sink.variable) else {
                continue;
            };
            let sink_domain = specialize_map(program, sink.relation.clone())?.domain()?;
            if lane_map.clone().domain()?.is_disjoint(&sink_domain)? {
                continue;
            }
            let occupancy = match sink.kind {
                ResourceSinkKind::SystemOutput => ExitOccupancy::Occupied,
                ResourceSinkKind::OperationInput(_) => ExitOccupancy::Empty,
            };
            if exit
                .replace(occupancy)
                .is_some_and(|existing| existing != occupancy)
            {
                return Err(CodegenError::Realization(
                    "trajectory has mixed sink occupancy".to_string(),
                ));
            }
        }
        let exit = exit.ok_or_else(|| {
            CodegenError::Realization(format!(
                "trajectory rooted at '{}' has no sink",
                root.variable
            ))
        })?;
        let size = shape.iter().try_fold(1_u64, |size, extent| {
            size.checked_mul(*extent).ok_or_else(|| {
                CodegenError::Realization("resource group size overflows u64".to_string())
            })
        })?;
        realization.groups.push(ResourceGroup {
            id: group,
            element_type: ElementType::Qubit,
            shape,
            size,
            entry,
            exit,
        });
        for (variable, relation) in trajectory {
            let domain = relation.clone().domain()?;
            if let Some(previous) = covered.get(&variable) {
                if !previous.is_disjoint(&domain)? {
                    return Err(CodegenError::Realization(format!(
                        "multiple trajectories cover resource '{variable}'"
                    )));
                }
                covered.insert(variable.clone(), previous.clone().union(domain)?);
            } else {
                covered.insert(variable.clone(), domain);
            }
            realization.logical_to_lane.push(LogicalLaneMap {
                variable,
                group,
                relation,
            });
        }
    }

    for variable in program
        .program
        .system
        .inputs
        .iter()
        .chain(&program.program.system.outputs)
        .chain(&program.program.system.locals)
        .filter(|variable| {
            variable.element_type == ElementType::Qubit
                && variable.multiplicity == Multiplicity::Linear
        })
    {
        let expected = specialize_set(program, variable.domain.clone())?.reset_tuple_name()?;
        let actual = covered.get(&variable.name).ok_or_else(|| {
            CodegenError::Realization(format!("resource '{}' is unreachable", variable.name))
        })?;
        if !actual.is_equal(&expected)? {
            return Err(CodegenError::Realization(format!(
                "resource '{}' has incomplete trajectory coverage: {}",
                variable.name,
                expected.subtract(actual.clone())?
            )));
        }
    }
    realization.operations = resolve_operations(program, &realization)?;
    Ok(realization)
}
