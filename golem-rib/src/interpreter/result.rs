// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
                _ => Err("Values are not literals and cannot be compared".to_string()),
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
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::TypedVariant;

    pub(crate) fn compare_typed_value<F>(
        left: &TypeAnnotatedValue,
        right: &TypeAnnotatedValue,
        compare: F,
    ) -> Result<TypeAnnotatedValue, String>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if let (Some(left_lit), Some(right_lit)) = (left.get_literal(), right.get_literal()) {
            Ok(TypeAnnotatedValue::Bool(compare(left_lit, right_lit)))
        } else if let (TypeAnnotatedValue::Variant(left), TypeAnnotatedValue::Variant(right)) =
            (left, right)
        {
            compare_variants(left.as_ref(), right.as_ref(), compare)
        } else {
            Err(unsupported_type_error(left, right))
        }
    }

    fn compare_variants<F>(
        left: &TypedVariant,
        right: &TypedVariant,
        compare: F,
    ) -> Result<TypeAnnotatedValue, String>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if left.case_name == right.case_name {
            match (
                left.case_value.clone().and_then(|x| x.type_annotated_value),
                right
                    .case_value
                    .clone()
                    .and_then(|x| x.type_annotated_value),
            ) {
                (Some(left_val), Some(right_val)) => {
                    compare_typed_value(&left_val, &right_val, compare)
                }
                _ => Ok(TypeAnnotatedValue::Bool(true)),
            }
        } else {
            Ok(TypeAnnotatedValue::Bool(false))
        }
    }

    fn unsupported_type_error(left: &TypeAnnotatedValue, right: &TypeAnnotatedValue) -> String {
        let left = AnalysedType::try_from(left);
        let right = AnalysedType::try_from(right);

        match (left, right) {
            (Ok(left), Ok(right)) => {
                format!("Unsupported type to compare {:?}, {:?}", left, right)
            }
            _ => "Unsupported type to compare. Un-identified types".to_string(),
        }
    }
}
