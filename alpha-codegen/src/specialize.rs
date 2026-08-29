use crate::error::{CodegenError, Result};
use crate::scheduled_ir::ScheduledProgram;
use isl::{DimType, Set, UnionMap};
use std::collections::{BTreeMap, BTreeSet};

pub type ParameterBindings = BTreeMap<String, i64>;

pub struct SpecializedProgram<'a> {
    pub program: &'a ScheduledProgram<'a>,
    pub bindings: ParameterBindings,
    pub parameter_point: Set,
    pub statement_domains: Vec<Set>,
    pub schedule: UnionMap,
}

pub fn apply<'a>(
    program: &'a ScheduledProgram<'a>,
    bindings: &ParameterBindings,
) -> Result<SpecializedProgram<'a>> {
    let parameter_space = program.system.parameter_domain.space();
    let parameter_count = parameter_space.dim(DimType::Param);
    let parameter_names: Vec<_> = (0..parameter_count)
        .map(|position| {
            parameter_space
                .dim_name(DimType::Param, position)
                .ok_or_else(|| {
                    CodegenError::Specialization(format!(
                        "parameter at position {position} has no name"
                    ))
                })
        })
        .collect::<Result<_>>()?;
    let known: BTreeSet<_> = parameter_names.iter().map(String::as_str).collect();

    for name in bindings.keys() {
        if !known.contains(name.as_str()) {
            return Err(CodegenError::Specialization(format!(
                "unknown parameter '{name}'"
            )));
        }
    }
    for name in &parameter_names {
        if !bindings.contains_key(name) {
            return Err(CodegenError::Specialization(format!(
                "missing parameter '{name}'"
            )));
        }
    }

    let parameter_list = parameter_names.join(",");
    let constraints = parameter_names
        .iter()
        .map(|name| format!("{name} = {}", bindings[name]))
        .collect::<Vec<_>>()
        .join(" and ");
    let point_text = if parameter_names.is_empty() {
        "{ : }".to_string()
    } else {
        format!("[{parameter_list}] -> {{ : {constraints} }}")
    };
    let point = Set::read_from_str(&program.system.parameter_domain.ctx(), &point_text)?;
    if !point.is_subset(&program.system.parameter_domain)? {
        return Err(CodegenError::Specialization(
            "bindings are outside the parameter domain".to_string(),
        ));
    }

    let specialize_set = |domain: &Set| -> Result<Set> {
        Ok(domain
            .clone()
            .intersect_params(point.clone())?
            .project_out(DimType::Param, 0, parameter_count)?)
    };
    let statement_domains = program
        .statements
        .iter()
        .map(|statement| specialize_set(&statement.domain))
        .collect::<Result<Vec<_>>>()?;
    let schedule = program
        .schedule
        .clone()
        .intersect_params(point.clone())?
        .project_out(DimType::Param, 0, parameter_count)?;

    Ok(SpecializedProgram {
        program,
        bindings: bindings.clone(),
        parameter_point: point,
        statement_domains,
        schedule,
    })
}
