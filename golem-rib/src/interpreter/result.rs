use crate::interpreter::literal::{GetLiteralValue, LiteralValue};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;

#[derive(Debug, Clone, PartialEq)]
pub enum RibInterpreterResult {
    Unit,
    Val(TypeAnnotatedValue),
}

impl RibInterpreterResult {
    pub fn compare<F>(
        &self,
        right: &RibInterpreterResult,
        compare: F,
    ) -> Result<RibInterpreterResult, String>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if self.is_unit() && right.is_unit() {
            Ok(RibInterpreterResult::Val(TypeAnnotatedValue::Bool(true)))
        } else {
            match (self.get_val(), right.get_val()) {
                (Some(left), Some(right)) => {
                    let result = internal::compare_typed_value(&left, &right, compare)?;
                    Ok(RibInterpreterResult::Val(result))
                }
                _ => Err("Unsupported type to compare".to_string()),
            }
        }
    }

    pub fn get_bool(&self) -> Option<bool> {
        match self {
            RibInterpreterResult::Val(TypeAnnotatedValue::Bool(bool)) => Some(*bool),
            _ => None,
        }
    }
    pub fn get_val(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RibInterpreterResult::Val(val) => Some(val.clone()),
            _ => None,
        }
    }

    pub fn get_literal(&self) -> Option<LiteralValue> {
        match self {
            RibInterpreterResult::Val(val) => val.get_literal(),
            _ => None,
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, RibInterpreterResult::Unit)
    }

    pub fn val(val: TypeAnnotatedValue) -> Self {
        RibInterpreterResult::Val(val)
    }

    pub fn unwrap(self) -> Option<TypeAnnotatedValue> {
        match self {
            RibInterpreterResult::Val(val) => match val {
                TypeAnnotatedValue::Option(option) => option
                    .value
                    .as_deref()
                    .and_then(|x| x.type_annotated_value.clone()),
                TypeAnnotatedValue::Result(result) => {
                    let result = match result.result_value {
                        Some(ResultValue::OkValue(ok)) => Some(*ok),
                        Some(ResultValue::ErrorValue(err)) => Some(*err),
                        None => None,
                    };

                    // GRPC wrapper
                    result.and_then(|x| x.type_annotated_value)
                }

                TypeAnnotatedValue::Variant(variant) => variant
                    .case_value
                    .as_deref()
                    .and_then(|x| x.type_annotated_value.clone()),
                _ => None,
            },
            RibInterpreterResult::Unit => None,
        }
    }
}

mod internal {
    use crate::interpreter::literal::{GetLiteralValue, LiteralValue};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    pub(crate) fn compare_typed_value<F>(
        left: &TypeAnnotatedValue,
        right: &TypeAnnotatedValue,
        compare: F,
    ) -> Result<TypeAnnotatedValue, String>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        match (left.get_literal(), right.get_literal()) {
            (Some(left), Some(right)) => {
                let result = compare(left, right);
                Ok(TypeAnnotatedValue::Bool(result))
            }
            _ => Err("Unsupported type to compare".to_string()),
        }
    }
}
