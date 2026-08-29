use crate::error::{CodegenError, Result};
use crate::scheduled_ir::{CompareOp, IndexExpr, Predicate};
use ::hugr::builder::{
    DFGBuilder, Dataflow, DataflowHugr, DataflowSubContainer, HugrBuilder, SubContainer,
};
use ::hugr::extension::prelude::{qb_t, ConstUsize};
use ::hugr::std_extensions::arithmetic::int_ops::IntOpDef;
use ::hugr::std_extensions::arithmetic::int_types::INT_TYPES;
use ::hugr::std_extensions::collections::borrow_array::{borrow_array_type, BArrayOpBuilder};
use ::hugr::std_extensions::logic::LogicOp;
use ::hugr::types::Signature;
use ::hugr::{ops::Value, Wire};
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
