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
use crate::CoercedNumericValue;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;
use poem_openapi::types::ToJSON;
use std::fmt;

// A result of a function can be unit, which is not representable using type_annotated_value
// A result can be a type_annotated_value
// A result can be a sink where it collects only the required elements from a possible iterable
// A result can also be stored as an iterator, that its easy to stream through any iterables, given a sink is following it.
pub enum RibInterpreterStackValue {
    Unit,
    Val(TypeAnnotatedValue),
    Iterator(Box<dyn Iterator<Item = TypeAnnotatedValue> + Send>),
    Sink(Vec<TypeAnnotatedValue>, AnalysedType),
}

impl RibInterpreterStackValue {
    pub fn is_sink(&self) -> bool {
        matches!(self, RibInterpreterStackValue::Sink(_, _))
    }
    pub fn is_iterator(&self) -> bool {
        matches!(self, RibInterpreterStackValue::Iterator(_))
    }

    pub fn evaluate_math_op<F>(
        &self,
        right: &RibInterpreterStackValue,
        op: F,
    ) -> Result<CoercedNumericValue, String>
    where
        F: Fn(CoercedNumericValue, CoercedNumericValue) -> CoercedNumericValue,
    {
        match (self.get_val(), right.get_val()) {
            (Some(left), Some(right)) => {
                if let (Some(left_lit), Some(right_lit)) = (
                    left.get_literal().and_then(|x| x.get_number()),
                    right.get_literal().and_then(|x| x.get_number()),
                ) {
                    Ok(op(left_lit, right_lit))
                } else {
                    Err(format!(
                        "Unable to complete the math operation on {}, {}",
                        left.to_json_string(),
                        right.to_json_string()
                    ))
                }
            }
            _ => Err("Failed to obtain values to complete the math operation".to_string()),
        }
    }

    pub fn compare<F>(
        &self,
        right: &RibInterpreterStackValue,
        compare: F,
    ) -> Result<RibInterpreterStackValue, String>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if self.is_unit() && right.is_unit() {
            Ok(RibInterpreterStackValue::Val(TypeAnnotatedValue::Bool(
                true,
            )))
        } else {
            match (self.get_val(), right.get_val()) {
                (Some(left), Some(right)) => {
                    let result = internal::compare_typed_value(&left, &right, compare)?;
                    Ok(RibInterpreterStackValue::Val(result))
                }
                _ => Err("Values are not literals and cannot be compared".to_string()),
            }
        }
    }

    pub fn get_bool(&self) -> Option<bool> {
        match self {
            RibInterpreterStackValue::Val(TypeAnnotatedValue::Bool(bool)) => Some(*bool),
            RibInterpreterStackValue::Val(_) => None,
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }
    pub fn get_val(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RibInterpreterStackValue::Val(val) => Some(val.clone()),
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }

    pub fn get_literal(&self) -> Option<LiteralValue> {
        match self {
            RibInterpreterStackValue::Val(val) => val.get_literal(),
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, RibInterpreterStackValue::Unit)
    }

    pub fn val(val: TypeAnnotatedValue) -> Self {
        RibInterpreterStackValue::Val(val)
    }

    pub fn unwrap(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RibInterpreterStackValue::Val(val) => match val {
                TypeAnnotatedValue::Option(option) => option
                    .value
                    .as_deref()
                    .and_then(|x| x.type_annotated_value.clone()),
                TypeAnnotatedValue::Result(result) => {
                    let result = match &result.result_value {
                        Some(ResultValue::OkValue(ok)) => Some(ok.clone()),
                        Some(ResultValue::ErrorValue(err)) => Some(err.clone()),
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
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }
}

impl fmt::Debug for RibInterpreterStackValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RibInterpreterStackValue::Unit => write!(f, "Unit"),
            RibInterpreterStackValue::Val(value) => write!(f, "val:{:?}", value),
            RibInterpreterStackValue::Iterator(_) => write!(f, "Iterator:(...)"),
            RibInterpreterStackValue::Sink(value, _) => write!(f, "sink:{}", value.len()),
        }
    }
}

mod internal {
    use crate::interpreter::literal::{GetLiteralValue, LiteralValue};
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{TypedEnum, TypedFlags, TypedVariant};

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
        } else if let (TypeAnnotatedValue::Enum(left), TypeAnnotatedValue::Enum(right)) =
            (left, right)
        {
            compare_enums(left, right)
        } else if let (TypeAnnotatedValue::Flags(left), TypeAnnotatedValue::Flags(right)) =
            (left, right)
        {
            compare_flags(left, right)
        } else {
            Err(unsupported_type_error(left, right))
        }
    }

    fn compare_flags(left: &TypedFlags, right: &TypedFlags) -> Result<TypeAnnotatedValue, String> {
        if left.values == right.values {
            Ok(TypeAnnotatedValue::Bool(true))
        } else {
            Ok(TypeAnnotatedValue::Bool(false))
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

    fn compare_enums(left: &TypedEnum, right: &TypedEnum) -> Result<TypeAnnotatedValue, String> {
        if left.value == right.value {
            Ok(TypeAnnotatedValue::Bool(true))
        } else {
            Ok(TypeAnnotatedValue::Bool(false))
        }
    }

    fn unsupported_type_error(left: &TypeAnnotatedValue, right: &TypeAnnotatedValue) -> String {
        let left = AnalysedType::try_from(left);
        let right = AnalysedType::try_from(right);

        match (left, right) {
            (Ok(left), Ok(right)) => {
                format!("Unsupported op {:?}, {:?}", left, right)
            }
            _ => "Unsupported types. Un-identified types".to_string(),
        }
    }
}
