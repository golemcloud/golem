use crate::evaluator::{EvaluationError, ExprEvaluationResult};
use crate::primitive::{GetPrimitive, Primitive};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub(crate) fn compare_typed_value<F>(
    left: &TypeAnnotatedValue,
    right: &TypeAnnotatedValue,
    compare: F,
) -> Result<TypeAnnotatedValue, EvaluationError>
where
    F: Fn(Primitive, Primitive) -> bool,
{
    match (left.get_primitive(), right.get_primitive()) {
        (Some(left), Some(right)) => {
            let result = compare(left, right);
            Ok(TypeAnnotatedValue::Bool(result))
        }
        _ => Err(EvaluationError::Message(
            "Unsupported type to compare".to_string(),
        )),
    }
}

pub(crate) fn compare_eval_result<F>(
    left: &ExprEvaluationResult,
    right: &ExprEvaluationResult,
    compare: F,
) -> Result<ExprEvaluationResult, EvaluationError>
where
    F: Fn(Primitive, Primitive) -> bool,
{
    if left.is_unit() && right.is_unit() {
        Ok(TypeAnnotatedValue::Bool(true).into())
    } else {
        match (left.get_value(), right.get_value()) {
            (Some(left), Some(right)) => {
                let result = compare_typed_value(&left, &right, compare)?;
                Ok(ExprEvaluationResult::Value(result))
            }
            _ => Err(EvaluationError::Message(
                "Unsupported type to compare".to_string(),
            )),
        }
    }
}