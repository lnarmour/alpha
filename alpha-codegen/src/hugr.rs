use crate::error::{CodegenError, Result};
use crate::realize::{EntryOccupancy, ExitOccupancy, Realization};
use crate::scheduled_ir::{CompareOp, IndexExpr, Predicate, ScheduledNode, StatementId};
use crate::specialize::{self, ParameterBindings, SpecializedProgram};
use crate::stmt::StatementKind;
use ::hugr::builder::{
    DFGBuilder, Dataflow, DataflowHugr, DataflowSubContainer, HugrBuilder, SubContainer,
};
use ::hugr::envelope::EnvelopeConfig;
use ::hugr::extension::prelude::{bool_t, either_type, qb_t, ConstUsize, UnwrapBuilder};
use ::hugr::std_extensions::arithmetic::conversions::ConvertOpDef;
use ::hugr::std_extensions::arithmetic::int_ops::IntOpDef;
use ::hugr::std_extensions::arithmetic::int_types::INT_TYPES;
use ::hugr::std_extensions::collections::array::{array_type, ArrayOpBuilder};
use ::hugr::std_extensions::collections::borrow_array::{borrow_array_type, BArrayOpBuilder};
use ::hugr::std_extensions::collections::borrow_array::{BArrayFromArray, BArrayToArray};
use ::hugr::std_extensions::logic::LogicOp;
use ::hugr::types::{Signature, Type};
use ::hugr::{ops::Value, HugrView, Wire};
use alpha_transform::{ir, resource_flow};
use isl::{DimType, MultiAff};
use std::collections::HashMap;
use tket::extension::measurement::MeasurementOp;
use tket::TketOp;

fn build_error(error: impl std::fmt::Display) -> CodegenError {
    CodegenError::Hugr(error.to_string())
}

fn binary_int(builder: &mut impl Dataflow, op: IntOpDef, lhs: Wire, rhs: Wire) -> Result<Wire> {
    Ok(builder
        .add_dataflow_op(op.with_log_width(6), [lhs, rhs])
        .map_err(build_error)?
        .out_wire(0))
}

fn select_int(
    builder: &mut impl Dataflow,
    condition: Wire,
    when_false: Wire,
    when_true: Wire,
) -> Result<Wire> {
    let integer_type = INT_TYPES[6].clone();
    let mut conditional = builder
        .conditional_builder(
            ([Vec::new().into(), Vec::new().into()], condition),
            [
                (integer_type.clone(), when_false),
                (integer_type.clone(), when_true),
            ],
            [integer_type].into(),
        )
        .map_err(build_error)?;
    for case in 0..2 {
        let branch = conditional.case_builder(case).map_err(build_error)?;
        let [when_false, when_true] = branch.input_wires_arr();
        branch
            .finish_with_outputs([if case == 0 { when_false } else { when_true }])
            .map_err(build_error)?;
    }
    Ok(conditional
        .finish_sub_container()
        .map_err(build_error)?
        .out_wire(0))
}

fn rounded_div(builder: &mut impl Dataflow, lhs: Wire, rhs: Wire, ceiling: bool) -> Result<Wire> {
    let quotient = binary_int(builder, IntOpDef::idiv_s, lhs, rhs)?;
    let remainder = binary_int(builder, IntOpDef::imod_s, lhs, rhs)?;
    let zero = builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, 0)
            .map_err(build_error)?,
    );
    let one = builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, 1)
            .map_err(build_error)?,
    );
    let has_remainder = binary_int(builder, IntOpDef::ine, remainder, zero)?;
    let remainder_negative = binary_int(builder, IntOpDef::ilt_s, remainder, zero)?;
    let divisor_negative = binary_int(builder, IntOpDef::ilt_s, rhs, zero)?;
    let signs_differ = builder
        .add_dataflow_op(LogicOp::Xor, [remainder_negative, divisor_negative])
        .map_err(build_error)?
        .out_wire(0);
    let adjust_direction = if ceiling {
        builder
            .add_dataflow_op(LogicOp::Not, [signs_differ])
            .map_err(build_error)?
            .out_wire(0)
    } else {
        signs_differ
    };
    let should_adjust = builder
        .add_dataflow_op(LogicOp::And, [has_remainder, adjust_direction])
        .map_err(build_error)?
        .out_wire(0);
    let adjusted = binary_int(
        builder,
        if ceiling {
            IntOpDef::iadd
        } else {
            IntOpDef::isub
        },
        quotient,
        one,
    )?;
    select_int(builder, should_adjust, quotient, adjusted)
}

fn lower_index_expr(
    builder: &mut impl Dataflow,
    expression: &IndexExpr,
    variables: &HashMap<String, Wire>,
) -> Result<Wire> {
    let operands = |builder: &mut _, lhs: &IndexExpr, rhs: &IndexExpr| -> Result<(Wire, Wire)> {
        Ok((
            lower_index_expr(builder, lhs, variables)?,
            lower_index_expr(builder, rhs, variables)?,
        ))
    };
    match expression {
        IndexExpr::Constant(value) => Ok(builder.add_load_value(
            ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, *value)
                .map_err(build_error)?,
        )),
        IndexExpr::Variable(name) => variables.get(name).copied().ok_or_else(|| {
            CodegenError::Hugr(format!("scheduled index variable '{name}' is not in scope"))
        }),
        IndexExpr::Add(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            binary_int(builder, IntOpDef::iadd, lhs, rhs)
        }
        IndexExpr::Sub(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            binary_int(builder, IntOpDef::isub, lhs, rhs)
        }
        IndexExpr::Mul(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            binary_int(builder, IntOpDef::imul, lhs, rhs)
        }
        IndexExpr::Div(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            binary_int(builder, IntOpDef::idiv_s, lhs, rhs)
        }
        IndexExpr::FloorDiv(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            rounded_div(builder, lhs, rhs, false)
        }
        IndexExpr::CeilDiv(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            rounded_div(builder, lhs, rhs, true)
        }
        IndexExpr::Mod(lhs, rhs) => {
            let (lhs, rhs) = operands(builder, lhs, rhs)?;
            binary_int(builder, IntOpDef::imod_s, lhs, rhs)
        }
        IndexExpr::Min(values) | IndexExpr::Max(values) => {
            let op = if matches!(expression, IndexExpr::Min(_)) {
                IntOpDef::imin_s
            } else {
                IntOpDef::imax_s
            };
            let mut values = values.iter();
            let first = values.next().ok_or_else(|| {
                CodegenError::Hugr("min/max requires at least one operand".to_string())
            })?;
            let mut result = lower_index_expr(builder, first, variables)?;
            for value in values {
                let value = lower_index_expr(builder, value, variables)?;
                result = binary_int(builder, op, result, value)?;
            }
            Ok(result)
        }
    }
}

fn lower_predicate(
    builder: &mut impl Dataflow,
    predicate: &Predicate,
    variables: &HashMap<String, Wire>,
) -> Result<Wire> {
    match predicate {
        Predicate::Compare { op, lhs, rhs } => {
            let lhs = lower_index_expr(builder, lhs, variables)?;
            let rhs = lower_index_expr(builder, rhs, variables)?;
            let op = match op {
                CompareOp::Eq => IntOpDef::ieq,
                CompareOp::Le => IntOpDef::ile_s,
                CompareOp::Lt => IntOpDef::ilt_s,
                CompareOp::Ge => IntOpDef::ige_s,
                CompareOp::Gt => IntOpDef::igt_s,
            };
            binary_int(builder, op, lhs, rhs)
        }
        Predicate::And(values) | Predicate::Or(values) => {
            let is_and = matches!(predicate, Predicate::And(_));
            let op = if is_and { LogicOp::And } else { LogicOp::Or };
            let mut result = builder.add_load_value(if is_and {
                Value::true_val()
            } else {
                Value::false_val()
            });
            for value in values {
                let value = lower_predicate(builder, value, variables)?;
                result = builder
                    .add_dataflow_op(op, [result, value])
                    .map_err(build_error)?
                    .out_wire(0);
            }
            Ok(result)
        }
        Predicate::Not(value) => {
            let value = lower_predicate(builder, value, variables)?;
            Ok(builder
                .add_dataflow_op(LogicOp::Not, [value])
                .map_err(build_error)?
                .out_wire(0))
        }
        Predicate::Constant(value) => Ok(builder.add_load_value(if *value {
            Value::true_val()
        } else {
            Value::false_val()
        })),
    }
}

#[derive(Clone)]
struct StateSlot {
    ty: Type,
    size: u64,
    wire: Wire,
}

fn signed_constant(builder: &mut impl Dataflow, value: i64) -> Result<Wire> {
    Ok(builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, value)
            .map_err(build_error)?,
    ))
}

fn lower_affine_components(
    builder: &mut impl Dataflow,
    function: &MultiAff,
    arguments: &[IndexExpr],
    variables: &HashMap<String, Wire>,
) -> Result<Vec<Wire>> {
    if function.dim(DimType::Div) != 0 {
        return Err(CodegenError::Hugr(format!(
            "resolved lane expression contains {} unsupported local dimensions: {function}",
            function.dim(DimType::Div)
        )));
    }
    let inputs = arguments
        .iter()
        .map(|argument| lower_index_expr(builder, argument, variables))
        .collect::<Result<Vec<_>>>()?;
    if function.dim(DimType::In) as usize != inputs.len() {
        return Err(CodegenError::Hugr(format!(
            "resolved lane expects {} indices but invocation supplies {}",
            function.dim(DimType::In),
            inputs.len()
        )));
    }
    (0..function.n_out())
        .map(|output| {
            let affine = function.get_aff(output)?;
            if affine.denominator()?.num_si() != 1 {
                return Err(CodegenError::Hugr(
                    "resolved lane expression is not integral affine".into(),
                ));
            }
            let mut value = signed_constant(builder, affine.constant()?.num_si())?;
            for (position, input) in inputs.iter().enumerate() {
                let coefficient = affine.coefficient(DimType::In, position as u32)?.num_si();
                if coefficient == 0 {
                    continue;
                }
                let term = if coefficient == 1 {
                    *input
                } else {
                    let coefficient = signed_constant(builder, coefficient)?;
                    binary_int(builder, IntOpDef::imul, *input, coefficient)?
                };
                value = binary_int(builder, IntOpDef::iadd, value, term)?;
            }
            for position in 0..function.dim(DimType::Param) {
                let coefficient = affine.coefficient(DimType::Param, position)?.num_si();
                if coefficient == 0 {
                    continue;
                }
                let name = function
                    .space()
                    .dim_name(DimType::Param, position)
                    .ok_or_else(|| {
                        CodegenError::Hugr(format!(
                            "resolved lane parameter at position {position} has no name"
                        ))
                    })?;
                let parameter = variables.get(&name).copied().ok_or_else(|| {
                    CodegenError::Hugr(format!(
                        "resolved lane parameter '{name}' is not specialized"
                    ))
                })?;
                let term = if coefficient == 1 {
                    parameter
                } else {
                    let coefficient = signed_constant(builder, coefficient)?;
                    binary_int(builder, IntOpDef::imul, parameter, coefficient)?
                };
                value = binary_int(builder, IntOpDef::iadd, value, term)?;
            }
            Ok(value)
        })
        .collect()
}

fn flatten_lane(builder: &mut impl Dataflow, components: Vec<Wire>, shape: &[u64]) -> Result<Wire> {
    if components.len() != shape.len() {
        return Err(CodegenError::Hugr(format!(
            "lane rank {} does not match resource shape rank {}",
            components.len(),
            shape.len()
        )));
    }
    let mut flat = signed_constant(builder, 0)?;
    for (position, component) in components.into_iter().enumerate() {
        let stride = shape[position + 1..]
            .iter()
            .try_fold(1_u64, |product, extent| product.checked_mul(*extent))
            .ok_or_else(|| CodegenError::Hugr("lane stride overflows u64".into()))?;
        let term = if stride == 1 {
            component
        } else {
            let stride = i64::try_from(stride)
                .map_err(|_| CodegenError::Hugr("lane stride exceeds i64".into()))?;
            let stride = signed_constant(builder, stride)?;
            binary_int(builder, IntOpDef::imul, component, stride)?
        };
        flat = binary_int(builder, IntOpDef::iadd, flat, term)?;
    }
    Ok(builder
        .add_dataflow_op(ConvertOpDef::itousize.without_log_width(), [flat])
        .map_err(build_error)?
        .out_wire(0))
}

fn with_wires(types: &[StateSlot], wires: impl IntoIterator<Item = Wire>) -> Vec<StateSlot> {
    types
        .iter()
        .zip(wires)
        .map(|(slot, wire)| StateSlot {
            ty: slot.ty.clone(),
            size: slot.size,
            wire,
        })
        .collect()
}

fn evaluate_index(expression: &IndexExpr, bindings: &ParameterBindings) -> Option<i64> {
    let binary = |lhs: &IndexExpr, rhs: &IndexExpr| {
        Some((
            evaluate_index(lhs, bindings)?,
            evaluate_index(rhs, bindings)?,
        ))
    };
    match expression {
        IndexExpr::Constant(value) => Some(*value),
        IndexExpr::Variable(name) => bindings.get(name).copied(),
        IndexExpr::Add(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            lhs.checked_add(rhs)
        }
        IndexExpr::Sub(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            lhs.checked_sub(rhs)
        }
        IndexExpr::Mul(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            lhs.checked_mul(rhs)
        }
        IndexExpr::Div(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            lhs.checked_div(rhs)
        }
        IndexExpr::FloorDiv(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            Some(lhs.div_euclid(rhs))
        }
        IndexExpr::CeilDiv(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            Some(-(-lhs).div_euclid(rhs))
        }
        IndexExpr::Mod(lhs, rhs) => {
            let (lhs, rhs) = binary(lhs, rhs)?;
            Some(lhs.rem_euclid(rhs))
        }
        IndexExpr::Min(values) => values
            .iter()
            .map(|value| evaluate_index(value, bindings))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min(),
        IndexExpr::Max(values) => values
            .iter()
            .map(|value| evaluate_index(value, bindings))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max(),
    }
}

fn evaluate_predicate(predicate: &Predicate, bindings: &ParameterBindings) -> Option<bool> {
    match predicate {
        Predicate::Compare { op, lhs, rhs } => {
            let lhs = evaluate_index(lhs, bindings)?;
            let rhs = evaluate_index(rhs, bindings)?;
            Some(match op {
                CompareOp::Eq => lhs == rhs,
                CompareOp::Le => lhs <= rhs,
                CompareOp::Lt => lhs < rhs,
                CompareOp::Ge => lhs >= rhs,
                CompareOp::Gt => lhs > rhs,
            })
        }
        Predicate::And(values) => values.iter().try_fold(true, |result, value| {
            Some(result && evaluate_predicate(value, bindings)?)
        }),
        Predicate::Or(values) => values.iter().try_fold(false, |result, value| {
            Some(result || evaluate_predicate(value, bindings)?)
        }),
        Predicate::Not(value) => Some(!evaluate_predicate(value, bindings)?),
        Predicate::Constant(value) => Some(*value),
    }
}

fn access_index(
    builder: &mut impl Dataflow,
    access: &crate::realize::ResolvedAccess,
    realization: &Realization,
    indices: &[IndexExpr],
    variables: &HashMap<String, Wire>,
) -> Result<Wire> {
    let group = &realization.groups[access.group.0];
    let components = lower_affine_components(builder, &access.lane, indices, variables)?;
    flatten_lane(builder, components, &group.shape)
}

fn borrow_access(
    builder: &mut impl Dataflow,
    state: &mut [StateSlot],
    access: &crate::realize::ResolvedAccess,
    realization: &Realization,
    indices: &[IndexExpr],
    variables: &HashMap<String, Wire>,
) -> Result<(Wire, Wire)> {
    let index = access_index(builder, access, realization, indices, variables)?;
    let group = &realization.groups[access.group.0];
    let (array, qubit) = builder
        .add_borrow_array_borrow(qb_t(), group.size, state[access.group.0].wire, index)
        .map_err(build_error)?;
    state[access.group.0].wire = array;
    Ok((index, qubit))
}

fn return_access(
    builder: &mut impl Dataflow,
    state: &mut [StateSlot],
    access: &crate::realize::ResolvedAccess,
    realization: &Realization,
    index: Wire,
    qubit: Wire,
) -> Result<()> {
    let group = &realization.groups[access.group.0];
    state[access.group.0].wire = builder
        .add_borrow_array_return(qb_t(), group.size, state[access.group.0].wire, index, qubit)
        .map_err(build_error)?;
    Ok(())
}

fn bool_output_shape(program: &SpecializedProgram<'_>, name: &str) -> Result<Vec<u64>> {
    let variable = program
        .program
        .system
        .outputs
        .iter()
        .find(|variable| variable.name == name)
        .ok_or_else(|| CodegenError::Hugr(format!("unknown classical output '{name}'")))?;
    let domain = variable
        .domain
        .clone()
        .intersect_params(program.parameter_point.clone())?
        .project_out(DimType::Param, 0, variable.domain.dim(DimType::Param))?;
    if !domain.is_box()? {
        return Err(CodegenError::Hugr(format!(
            "classical output '{name}' domain is not rectangular"
        )));
    }
    (0..domain.dim(DimType::OutOrSet))
        .map(|position| {
            let minimum = domain.dim_min(position)?.constant_value()?.ok_or_else(|| {
                CodegenError::Hugr(format!(
                    "classical output '{name}' has symbolic lower bound"
                ))
            })?;
            let maximum = domain.dim_max(position)?.constant_value()?.ok_or_else(|| {
                CodegenError::Hugr(format!(
                    "classical output '{name}' has symbolic upper bound"
                ))
            })?;
            if minimum != 0 || maximum < 0 {
                return Err(CodegenError::Hugr(format!(
                    "classical output '{name}' domain is not zero-based"
                )));
            }
            Ok((maximum as u64) + 1)
        })
        .collect()
}

fn set_bool_output(
    builder: &mut impl Dataflow,
    state: &mut [StateSlot],
    group_count: usize,
    program: &SpecializedProgram<'_>,
    call: &ir::OperationCall,
    indices: &[IndexExpr],
    variables: &HashMap<String, Wire>,
    value: Wire,
) -> Result<()> {
    let output = call.outputs.first().ok_or_else(|| {
        CodegenError::Hugr("measurement operation has no classical output".into())
    })?;
    let output_position = program
        .program
        .system
        .outputs
        .iter()
        .filter(|variable| variable.element_type == alpha_model::ElementType::Bool)
        .position(|variable| variable.name == output.variable)
        .ok_or_else(|| {
            CodegenError::Hugr(format!(
                "measurement output '{}' is not a system bool output",
                output.variable
            ))
        })?;
    let shape = bool_output_shape(program, &output.variable)?;
    let components = lower_affine_components(builder, &output.function, indices, variables)?;
    let index = flatten_lane(builder, components, &shape)?;
    let slot = &mut state[group_count + output_position];
    let result = builder
        .add_array_set(bool_t(), slot.size, slot.wire, index, value)
        .map_err(build_error)?;
    let [_, array] = builder
        .build_unwrap_sum(
            1,
            either_type(
                [bool_t(), array_type(slot.size, bool_t())],
                [bool_t(), array_type(slot.size, bool_t())],
            ),
            result,
        )
        .map_err(build_error)?;
    slot.wire = array;
    Ok(())
}

fn emit_invoke(
    builder: &mut impl Dataflow,
    statement: StatementId,
    indices: &[IndexExpr],
    mut state: Vec<StateSlot>,
    program: &SpecializedProgram<'_>,
    realization: &Realization,
    variables: &HashMap<String, Wire>,
) -> Result<Vec<StateSlot>> {
    let StatementKind::OperationCall(call) = &program.program.statements[statement.0].kind else {
        return Err(CodegenError::Hugr(format!(
            "scheduled statement '{}' is not a quantum operation",
            program.program.statements[statement.0].name
        )));
    };
    let operation = realization
        .operations
        .iter()
        .find(|operation| operation.statement == statement)
        .ok_or_else(|| CodegenError::Hugr("scheduled operation has no realization".into()))?;
    use alpha_model::RegisteredOperation;
    match call.operation {
        RegisteredOperation::QAlloc => {
            let output = operation.outputs.first().ok_or_else(|| {
                CodegenError::Hugr("qalloc operation has no realized output".into())
            })?;
            let [qubit] = builder
                .add_dataflow_op(TketOp::QAlloc, [])
                .map_err(build_error)?
                .outputs_arr();
            let index = access_index(builder, output, realization, indices, variables)?;
            return_access(builder, &mut state, output, realization, index, qubit)?;
        }
        RegisteredOperation::H => {
            let (index, qubit) = borrow_access(
                builder,
                &mut state,
                &operation.inputs[0],
                realization,
                indices,
                variables,
            )?;
            let [qubit] = builder
                .add_dataflow_op(TketOp::H, [qubit])
                .map_err(build_error)?
                .outputs_arr();
            return_access(
                builder,
                &mut state,
                &operation.outputs[0],
                realization,
                index,
                qubit,
            )?;
        }
        RegisteredOperation::Cx => {
            let (left_index, left) = borrow_access(
                builder,
                &mut state,
                &operation.inputs[0],
                realization,
                indices,
                variables,
            )?;
            let (right_index, right) = borrow_access(
                builder,
                &mut state,
                &operation.inputs[1],
                realization,
                indices,
                variables,
            )?;
            let [left, right] = builder
                .add_dataflow_op(TketOp::CX, [left, right])
                .map_err(build_error)?
                .outputs_arr();
            return_access(
                builder,
                &mut state,
                &operation.outputs[0],
                realization,
                left_index,
                left,
            )?;
            return_access(
                builder,
                &mut state,
                &operation.outputs[1],
                realization,
                right_index,
                right,
            )?;
        }
        RegisteredOperation::Measure | RegisteredOperation::Discard => {
            let (_, qubit) = borrow_access(
                builder,
                &mut state,
                &operation.inputs[0],
                realization,
                indices,
                variables,
            )?;
            if call.operation == RegisteredOperation::Measure {
                let measurement = builder
                    .add_dataflow_op(TketOp::MeasureFree, [qubit])
                    .map_err(build_error)?
                    .out_wire(0);
                let value = builder
                    .add_dataflow_op(MeasurementOp::Read, [measurement])
                    .map_err(build_error)?
                    .out_wire(0);
                set_bool_output(
                    builder,
                    &mut state,
                    realization.groups.len(),
                    program,
                    call,
                    indices,
                    variables,
                    value,
                )?;
            } else {
                builder
                    .add_dataflow_op(TketOp::QFree, [qubit])
                    .map_err(build_error)?;
            }
        }
    }
    Ok(state)
}

fn emit_node(
    builder: &mut impl Dataflow,
    node: &ScheduledNode,
    mut state: Vec<StateSlot>,
    program: &SpecializedProgram<'_>,
    realization: &Realization,
    variables: &HashMap<String, Wire>,
) -> Result<Vec<StateSlot>> {
    match node {
        ScheduledNode::Sequence(nodes) => {
            for node in nodes {
                state = emit_node(builder, node, state, program, realization, variables)?;
            }
            Ok(state)
        }
        ScheduledNode::Invoke { statement, indices } => emit_invoke(
            builder,
            *statement,
            indices,
            state,
            program,
            realization,
            variables,
        ),
        ScheduledNode::If {
            condition,
            then_body,
            else_body,
        } => {
            if let Some(condition) = evaluate_predicate(condition, &program.bindings) {
                return if condition {
                    emit_node(builder, then_body, state, program, realization, variables)
                } else if let Some(else_body) = else_body {
                    emit_node(builder, else_body, state, program, realization, variables)
                } else {
                    Ok(state)
                };
            }
            let condition = lower_predicate(builder, condition, variables)?;
            let types = state.clone();
            let output_count = state.len();
            let mut conditional = builder
                .conditional_builder(
                    ([Vec::new().into(), Vec::new().into()], condition),
                    state.iter().map(|slot| (slot.ty.clone(), slot.wire)),
                    state
                        .iter()
                        .map(|slot| slot.ty.clone())
                        .collect::<Vec<_>>()
                        .into(),
                )
                .map_err(build_error)?;
            {
                let mut false_case = conditional.case_builder(0).map_err(build_error)?;
                let inputs = with_wires(&types, false_case.input_wires());
                let outputs = if let Some(else_body) = else_body {
                    emit_node(
                        &mut false_case,
                        else_body,
                        inputs,
                        program,
                        realization,
                        variables,
                    )?
                } else {
                    inputs
                };
                false_case
                    .finish_with_outputs(outputs.iter().map(|slot| slot.wire))
                    .map_err(build_error)?;
            }
            {
                let mut true_case = conditional.case_builder(1).map_err(build_error)?;
                let inputs = with_wires(&types, true_case.input_wires());
                let outputs = emit_node(
                    &mut true_case,
                    then_body,
                    inputs,
                    program,
                    realization,
                    variables,
                )?;
                true_case
                    .finish_with_outputs(outputs.iter().map(|slot| slot.wire))
                    .map_err(build_error)?;
            }
            let handle = conditional.finish_sub_container().map_err(build_error)?;
            Ok(with_wires(
                &types,
                (0..output_count).map(|position| handle.out_wire(position)),
            ))
        }
        ScheduledNode::Loop {
            iterator,
            init,
            condition,
            step,
            body,
        } => {
            let initial = lower_index_expr(builder, init, variables)?;
            let types = state.clone();
            let output_count = state.len();
            let mut tail = builder
                .tail_loop_builder(
                    [(INT_TYPES[6].clone(), initial)],
                    state.iter().map(|slot| (slot.ty.clone(), slot.wire)),
                    Vec::<Type>::new().into(),
                )
                .map_err(build_error)?;
            let signature = tail.loop_signature().map_err(build_error)?.clone();
            let inputs = tail.input_wires().collect::<Vec<_>>();
            let iterator_wire = inputs[0];
            let mut loop_variables = variables.clone();
            loop_variables.insert(iterator.0.clone(), iterator_wire);
            let condition = lower_predicate(&mut tail, condition, &loop_variables)?;
            let output_row = tail.internal_output_row().map_err(build_error)?;
            let mut conditional = tail
                .conditional_builder(
                    ([Vec::new().into(), Vec::new().into()], condition),
                    std::iter::once((INT_TYPES[6].clone(), iterator_wire)).chain(
                        types
                            .iter()
                            .zip(inputs.iter().skip(1))
                            .map(|(slot, wire)| (slot.ty.clone(), *wire)),
                    ),
                    output_row,
                )
                .map_err(build_error)?;
            {
                let mut break_case = conditional.case_builder(0).map_err(build_error)?;
                let inputs = break_case.input_wires().collect::<Vec<_>>();
                let control = break_case
                    .make_break(signature.clone(), [])
                    .map_err(build_error)?;
                break_case
                    .finish_with_outputs(std::iter::once(control).chain(inputs.into_iter().skip(1)))
                    .map_err(build_error)?;
            }
            {
                let mut continue_case = conditional.case_builder(1).map_err(build_error)?;
                let inputs = continue_case.input_wires().collect::<Vec<_>>();
                let body_state = with_wires(&types, inputs.iter().skip(1).copied());
                let body_state = emit_node(
                    &mut continue_case,
                    body,
                    body_state,
                    program,
                    realization,
                    &loop_variables,
                )?;
                let increment = lower_index_expr(&mut continue_case, step, &loop_variables)?;
                let next = binary_int(&mut continue_case, IntOpDef::iadd, inputs[0], increment)?;
                let control = continue_case
                    .make_continue(signature, [next])
                    .map_err(build_error)?;
                continue_case
                    .finish_with_outputs(
                        std::iter::once(control).chain(body_state.iter().map(|slot| slot.wire)),
                    )
                    .map_err(build_error)?;
            }
            let conditional = conditional.finish_sub_container().map_err(build_error)?;
            let handle = tail
                .finish_with_outputs(
                    conditional.out_wire(0),
                    (1..=output_count).map(|position| conditional.out_wire(position)),
                )
                .map_err(build_error)?;
            Ok(with_wires(
                &types,
                (0..output_count).map(|position| handle.out_wire(position)),
            ))
        }
    }
}

pub fn generate_hugr(
    system: &ir::System,
    schedule_text: &str,
    bindings: &ParameterBindings,
) -> Result<::hugr::Hugr> {
    let scheduled = crate::scheduled_ir::build(system, schedule_text)?;
    let specialized = specialize::apply(&scheduled, bindings)?;
    let flow = resource_flow::analyze(system)
        .map_err(|error| CodegenError::Realization(error.to_string()))?;
    let realization = crate::realize::infer(&specialized, &flow)?;
    let bool_outputs = system
        .outputs
        .iter()
        .filter(|variable| variable.element_type == alpha_model::ElementType::Bool)
        .map(|variable| {
            let shape = bool_output_shape(&specialized, &variable.name)?;
            let size = shape.iter().try_fold(1_u64, |product, extent| {
                product
                    .checked_mul(*extent)
                    .ok_or_else(|| CodegenError::Hugr("classical output size overflows u64".into()))
            })?;
            Ok((variable.name.clone(), size))
        })
        .collect::<Result<Vec<_>>>()?;
    let input_types = realization
        .groups
        .iter()
        .filter(|group| group.entry == EntryOccupancy::Occupied)
        .map(|group| array_type(group.size, qb_t()))
        .collect::<Vec<_>>();
    let output_types = bool_outputs
        .iter()
        .map(|(_, size)| array_type(*size, bool_t()))
        .chain(
            realization
                .groups
                .iter()
                .filter(|group| group.exit == ExitOccupancy::Occupied)
                .map(|group| array_type(group.size, qb_t())),
        )
        .collect::<Vec<_>>();
    let mut builder =
        DFGBuilder::new(Signature::new(input_types, output_types)).map_err(build_error)?;
    let mut inputs = builder.input_wires();
    let mut state = Vec::new();
    for group in &realization.groups {
        let wire = match group.entry {
            EntryOccupancy::Empty => builder
                .add_new_all_borrowed(qb_t(), group.size)
                .map_err(build_error)?,
            EntryOccupancy::Occupied => builder
                .add_dataflow_op(
                    BArrayFromArray::new(qb_t(), group.size),
                    [inputs
                        .next()
                        .expect("input type and resource group count agree")],
                )
                .map_err(build_error)?
                .out_wire(0),
        };
        state.push(StateSlot {
            ty: borrow_array_type(group.size, qb_t()),
            size: group.size,
            wire,
        });
    }
    for (_, size) in &bool_outputs {
        let false_value = builder.add_load_value(Value::false_val());
        let wire = builder
            .add_new_array(bool_t(), std::iter::repeat_n(false_value, *size as usize))
            .map_err(build_error)?;
        state.push(StateSlot {
            ty: array_type(*size, bool_t()),
            size: *size,
            wire,
        });
    }
    let variables = specialized
        .bindings
        .iter()
        .map(|(name, value)| Ok((name.clone(), signed_constant(&mut builder, *value)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let state = emit_node(
        &mut builder,
        &specialized.program.root,
        state,
        &specialized,
        &realization,
        &variables,
    )?;
    let mut outputs = state[realization.groups.len()..]
        .iter()
        .map(|slot| slot.wire)
        .collect::<Vec<_>>();
    for group in &realization.groups {
        let slot = &state[group.id.0];
        match group.exit {
            ExitOccupancy::Empty => builder
                .add_discard_all_borrowed(qb_t(), group.size, slot.wire)
                .map_err(build_error)?,
            ExitOccupancy::Occupied => outputs.push(
                builder
                    .add_dataflow_op(BArrayToArray::new(qb_t(), group.size), [slot.wire])
                    .map_err(build_error)?
                    .out_wire(0),
            ),
        }
    }
    let hugr = builder
        .finish_hugr_with_outputs(outputs)
        .map_err(build_error)?;
    hugr.validate().map_err(build_error)?;
    Ok(hugr)
}

pub fn generate_hugr_system(
    system: &ir::System,
    schedule_text: &str,
    bindings: &ParameterBindings,
) -> Result<String> {
    generate_hugr(system, schedule_text, bindings)?
        .store_str(EnvelopeConfig::text())
        .map_err(build_error)
}

#[doc(hidden)]
pub fn build_borrow_h_primitive(size: u64, index: u64) -> Result<::hugr::Hugr> {
    if index >= size {
        return Err(CodegenError::Hugr(format!(
            "borrow index {index} is outside array size {size}"
        )));
    }
    let element_type = qb_t();
    let array_type = borrow_array_type(size, element_type.clone());
    let mut builder = DFGBuilder::new(Signature::new_endo([array_type])).map_err(build_error)?;
    let [array] = builder.input_wires_arr();
    let borrow_index = builder.add_load_value(ConstUsize::new(index));
    let (array, qubit) = builder
        .add_borrow_array_borrow(element_type.clone(), size, array, borrow_index)
        .map_err(build_error)?;
    let [qubit] = builder
        .add_dataflow_op(TketOp::H, [qubit])
        .map_err(build_error)?
        .outputs_arr();
    let return_index = builder.add_load_value(ConstUsize::new(index));
    let array = builder
        .add_borrow_array_return(element_type, size, array, return_index, qubit)
        .map_err(build_error)?;
    builder
        .finish_hugr_with_outputs([array])
        .map_err(build_error)
}

#[doc(hidden)]
pub fn build_counted_loop_primitive(size: u64) -> Result<::hugr::Hugr> {
    let element_type = qb_t();
    let array_type = borrow_array_type(size, element_type);
    let mut builder =
        DFGBuilder::new(Signature::new_endo([array_type.clone()])).map_err(build_error)?;
    let [array] = builder.input_wires_arr();
    let zero = builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, 0)
            .map_err(build_error)?,
    );
    let one = builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, 1)
            .map_err(build_error)?,
    );
    let limit = builder.add_load_value(
        ::hugr::std_extensions::arithmetic::int_types::ConstInt::new_s(6, size as i64)
            .map_err(build_error)?,
    );
    let mut tail = builder
        .tail_loop_builder(
            [(INT_TYPES[6].clone(), zero)],
            [(array_type.clone(), array)],
            [].into(),
        )
        .map_err(build_error)?;
    let [iterator, array] = tail.input_wires_arr();
    let [condition] = tail
        .add_dataflow_op(IntOpDef::ilt_s.with_log_width(6), [iterator, limit])
        .map_err(build_error)?
        .outputs_arr();
    let [next] = tail
        .add_dataflow_op(IntOpDef::iadd.with_log_width(6), [iterator, one])
        .map_err(build_error)?
        .outputs_arr();
    let loop_signature = tail.loop_signature().map_err(build_error)?.clone();
    let output_row = tail.internal_output_row().map_err(build_error)?;
    let mut conditional = tail
        .conditional_builder(
            ([Vec::new().into(), Vec::new().into()], condition),
            [(INT_TYPES[6].clone(), next), (array_type.clone(), array)],
            output_row,
        )
        .map_err(build_error)?;
    {
        let mut branch = conditional.case_builder(0).map_err(build_error)?;
        let [_next, array] = branch.input_wires_arr();
        let control = branch
            .make_break(loop_signature.clone(), [])
            .map_err(build_error)?;
        branch
            .finish_with_outputs([control, array])
            .map_err(build_error)?;
    }
    {
        let mut branch = conditional.case_builder(1).map_err(build_error)?;
        let [next, array] = branch.input_wires_arr();
        let control = branch
            .make_continue(loop_signature, [next])
            .map_err(build_error)?;
        branch
            .finish_with_outputs([control, array])
            .map_err(build_error)?;
    }
    let [control, array] = conditional
        .finish_sub_container()
        .map_err(build_error)?
        .outputs_arr();
    let handle = tail
        .finish_with_outputs(control, [array])
        .map_err(build_error)?;
    let [array] = handle.outputs_arr();
    builder
        .finish_hugr_with_outputs([array])
        .map_err(build_error)
}

#[doc(hidden)]
pub fn build_index_lowering_primitive() -> Result<::hugr::Hugr> {
    let mut builder =
        DFGBuilder::new(Signature::new([INT_TYPES[6].clone()], [])).map_err(build_error)?;
    let [variable] = builder.input_wires_arr();
    let variables = HashMap::from([("i".to_string(), variable)]);
    let constant = || IndexExpr::Constant(2);
    let expressions = [
        IndexExpr::Constant(-1),
        IndexExpr::Variable("i".to_string()),
        IndexExpr::Add(Box::new(constant()), Box::new(constant())),
        IndexExpr::Sub(Box::new(constant()), Box::new(constant())),
        IndexExpr::Mul(Box::new(constant()), Box::new(constant())),
        IndexExpr::Div(Box::new(constant()), Box::new(constant())),
        IndexExpr::FloorDiv(Box::new(constant()), Box::new(constant())),
        IndexExpr::CeilDiv(Box::new(constant()), Box::new(constant())),
        IndexExpr::Mod(Box::new(constant()), Box::new(constant())),
        IndexExpr::Min(vec![constant(), constant()]),
        IndexExpr::Max(vec![constant(), constant()]),
    ];
    for expression in expressions {
        let _ = lower_index_expr(&mut builder, &expression, &variables)?;
    }
    for op in [
        CompareOp::Eq,
        CompareOp::Le,
        CompareOp::Lt,
        CompareOp::Ge,
        CompareOp::Gt,
    ] {
        let _ = lower_predicate(
            &mut builder,
            &Predicate::Compare {
                op,
                lhs: constant(),
                rhs: constant(),
            },
            &variables,
        )?;
    }
    for predicate in [
        Predicate::And(vec![Predicate::Constant(true)]),
        Predicate::Or(vec![Predicate::Constant(false)]),
        Predicate::Not(Box::new(Predicate::Constant(true))),
        Predicate::Constant(false),
    ] {
        let _ = lower_predicate(&mut builder, &predicate, &variables)?;
    }
    builder.finish_hugr().map_err(build_error)
}

#[doc(hidden)]
pub fn build_quantum_lifecycle_primitive() -> Result<::hugr::Hugr> {
    let mut builder = DFGBuilder::new(Signature::new([], [::hugr::extension::prelude::bool_t()]))
        .map_err(build_error)?;
    let array = builder
        .add_new_all_borrowed(qb_t(), 1)
        .map_err(build_error)?;
    let [qubit] = builder
        .add_dataflow_op(TketOp::QAlloc, [])
        .map_err(build_error)?
        .outputs_arr();
    let index = builder.add_load_value(ConstUsize::new(0));
    let array = builder
        .add_borrow_array_return(qb_t(), 1, array, index, qubit)
        .map_err(build_error)?;
    let index = builder.add_load_value(ConstUsize::new(0));
    let (array, qubit) = builder
        .add_borrow_array_borrow(qb_t(), 1, array, index)
        .map_err(build_error)?;
    builder
        .add_discard_all_borrowed(qb_t(), 1, array)
        .map_err(build_error)?;
    let measurement = builder
        .add_dataflow_op(TketOp::MeasureFree, [qubit])
        .map_err(build_error)?
        .out_wire(0);
    let result = builder
        .add_dataflow_op(MeasurementOp::Read, [measurement])
        .map_err(build_error)?
        .out_wire(0);

    let [left] = builder
        .add_dataflow_op(TketOp::QAlloc, [])
        .map_err(build_error)?
        .outputs_arr();
    let [right] = builder
        .add_dataflow_op(TketOp::QAlloc, [])
        .map_err(build_error)?
        .outputs_arr();
    let [left, right] = builder
        .add_dataflow_op(TketOp::CX, [left, right])
        .map_err(build_error)?
        .outputs_arr();
    builder
        .add_dataflow_op(TketOp::QFree, [left])
        .map_err(build_error)?;
    builder
        .add_dataflow_op(TketOp::QFree, [right])
        .map_err(build_error)?;
    builder
        .finish_hugr_with_outputs([result])
        .map_err(build_error)
}
