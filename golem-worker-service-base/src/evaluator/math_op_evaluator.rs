use crate::evaluator::EvaluationError;
use crate::primitive::{GetPrimitive, Primitive};
use golem_wasm_rpc::TypeAnnotatedValue;

pub(crate) fn evaluate_math_op<F>(
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
