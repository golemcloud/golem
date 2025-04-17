// Copyright 2024-2025 Golem Cloud
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
use crate::interpreter::rib_runtime_error::{
    arithmetic_error, invalid_comparison, invalid_type_with_value,
};
use crate::{corrupted_state, CoercedNumericValue, RibInterpreterResult};
use anyhow::anyhow;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::ops::Deref;

// A result of a function can be unit, which is not representable using value_and_type
// A result can be a value_and_type
// A result can be a sink where it collects only the required elements from a possible iterable
// A result can also be stored as an iterator, that its easy to stream through any iterables, given a sink is following it.
pub enum RibInterpreterStackValue {
    Unit,
    Val(ValueAndType),
    Iterator(Box<dyn Iterator<Item = ValueAndType> + Send + Sync>),
    Sink(Vec<ValueAndType>, AnalysedType),
}

impl TryFrom<RibInterpreterStackValue> for String {
    type Error = String;
    fn try_from(value: RibInterpreterStackValue) -> Result<Self, Self::Error> {
        match value {
            RibInterpreterStackValue::Val(val) => Ok(val.to_string()),
            RibInterpreterStackValue::Unit => Ok("unit".to_string()),
            _ => Ok("unknown".to_string()),
        }
    }
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
    ) -> RibInterpreterResult<CoercedNumericValue>
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
                    Err(arithmetic_error(
                        "values are not numeric and cannot be used in math operation",
                        None,
                        None,
                    ))
                }
            }
            _ => Err(corrupted_state!(
                "failed to obtain values to complete the math operation"
            )),
        }
    }

    pub fn compare<F>(
        &self,
        right: &RibInterpreterStackValue,
        compare: F,
    ) -> RibInterpreterResult<RibInterpreterStackValue>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if self.is_unit() && right.is_unit() {
            Ok(RibInterpreterStackValue::Val(true.into_value_and_type()))
        } else {
            match (self.get_val(), right.get_val()) {
                (Some(left), Some(right)) => {
                    let result = internal::compare_typed_value(&left, &right, compare)?;
                    Ok(RibInterpreterStackValue::Val(result))
                }
                _ => Err(invalid_comparison(
                    "values are not literals and cannot be compared,",
                    None,
                    None,
                )),
            }
        }
    }

    pub fn get_bool(&self) -> Option<bool> {
        match self {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::Bool(bool),
                ..
            }) => Some(*bool),
            RibInterpreterStackValue::Val(_) => None,
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }
    pub fn get_val(&self) -> Option<ValueAndType> {
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

    pub fn val(val: ValueAndType) -> Self {
        RibInterpreterStackValue::Val(val)
    }

    pub fn unwrap(&self) -> Option<ValueAndType> {
        match self {
            RibInterpreterStackValue::Val(val) => match (val.value.clone(), val.typ.clone()) {
                (Value::Option(Some(option)), AnalysedType::Option(option_type)) => {
                    let inner_value = option.deref().clone();
                    let inner_type = option_type.inner.deref().clone();
                    Some(ValueAndType {
                        value: inner_value,
                        typ: inner_type,
                    })
                }

                (Value::Result(Ok(Some(ok))), AnalysedType::Result(result_type)) => {
                    let ok_value = ok.deref().clone();
                    let ok_type = result_type.ok.as_ref()?.deref().clone();
                    Some(ValueAndType {
                        value: ok_value,
                        typ: ok_type,
                    })
                }

                (Value::Result(Err(Some(err))), AnalysedType::Result(result_type)) => {
                    let err_value = err.deref().clone();
                    let err_type = result_type.err.as_ref()?.deref().clone();
                    Some(ValueAndType {
                        value: err_value,
                        typ: err_type,
                    })
                }

                (
                    Value::Variant {
                        case_value: Some(case_value),
                        case_idx,
                    },
                    AnalysedType::Variant(variant_type),
                ) => {
                    let case_type = variant_type
                        .cases
                        .get(case_idx as usize)?
                        .typ
                        .as_ref()?
                        .clone();
                    Some(ValueAndType {
                        value: case_value.deref().clone(),
                        typ: case_type,
                    })
                }

                _ => None,
            },
            RibInterpreterStackValue::Unit => None,
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }
}

impl Display for RibInterpreterStackValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RibInterpreterStackValue::Unit => write!(f, "unit"),
            RibInterpreterStackValue::Val(value) => write!(f, "{}", value),
            RibInterpreterStackValue::Iterator(_) => write!(f, "iterator:(...)"),
            RibInterpreterStackValue::Sink(value, _) => write!(f, "sink:{}", value.len()),
        }
    }
}

impl fmt::Debug for RibInterpreterStackValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

mod internal {
    use crate::interpreter::literal::{GetLiteralValue, LiteralValue};
    use crate::{corrupted_state, RibInterpreterResult};
    use anyhow::anyhow;
    use golem_wasm_ast::analysis::{AnalysedType, TypeVariant};
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};

    pub fn unable_to_complete_math_operation(left: &ValueAndType, right: &ValueAndType) -> String {
        format!(
            "Unable to complete math operation for operands {}, {}",
            left.to_string(),
            right.to_string()
        )
    }

    pub(crate) fn compare_typed_value<F>(
        left: &ValueAndType,
        right: &ValueAndType,
        compare: F,
    ) -> RibInterpreterResult<ValueAndType>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if let (Some(left_lit), Some(right_lit)) = (left.get_literal(), right.get_literal()) {
            Ok(compare(left_lit, right_lit).into_value_and_type())
        } else if let (
            ValueAndType {
                value:
                    Value::Variant {
                        case_idx: left_case_idx,
                        case_value: left_case_value,
                    },
                typ: AnalysedType::Variant(left_typ),
            },
            ValueAndType {
                value:
                    Value::Variant {
                        case_idx: right_cast_idx,
                        case_value: right_case_value,
                    },
                typ: AnalysedType::Variant(right_typ),
            },
        ) = (left, right)
        {
            compare_variants(
                *left_case_idx,
                left_case_value,
                left_typ,
                *right_cast_idx,
                right_case_value,
                right_typ,
                compare,
            )
        } else if let (
            ValueAndType {
                value: Value::Enum(left_idx),
                ..
            },
            ValueAndType {
                value: Value::Enum(right_idx),
                ..
            },
        ) = (left, right)
        {
            Ok((left_idx == right_idx).into_value_and_type())
        } else if let (
            ValueAndType {
                value: Value::Flags(left_bitmap),
                ..
            },
            ValueAndType {
                value: Value::Flags(right_bitmap),
                ..
            },
        ) = (left, right)
        {
            Ok((left == right).into_value_and_type())
        } else {
            Err(anyhow!(unsupported_type_error(left, right)))
        }
    }

    fn compare_flags(left: &[bool], right: &[bool]) -> ValueAndType {
        (left == right).into_value_and_type()
    }

    fn compare_variants<F>(
        left_case_idx: u32,
        left_case_value: &Option<Box<Value>>,
        left_type: &TypeVariant,
        right_case_idx: u32,
        right_case_value: &Option<Box<Value>>,
        right_type: &TypeVariant,
        compare: F,
    ) -> RibInterpreterResult<ValueAndType>
    where
        F: Fn(LiteralValue, LiteralValue) -> bool,
    {
        if left_case_idx == right_case_idx {
            match (left_case_value, right_case_value) {
                (Some(left_val), Some(right_val)) => {
                    let left_typ = left_type
                        .cases
                        .get(left_case_idx as usize)
                        .ok_or(corrupted_state!("unknown variant index"))?
                        .typ
                        .clone();
                    let right_typ = right_type
                        .cases
                        .get(right_case_idx as usize)
                        .ok_or(corrupted_state!("unknown variant index"))?
                        .typ
                        .clone();
                    match (left_typ, right_typ) {
                        (Some(left_typ), Some(right_typ)) => compare_typed_value(
                            &ValueAndType::new(*left_val.clone(), left_typ),
                            &ValueAndType::new(*right_val.clone(), right_typ),
                            compare,
                        ),
                        _ => Ok(true.into_value_and_type()),
                    }
                }
                _ => Ok(true.into_value_and_type()),
            }
        } else {
            Ok(false.into_value_and_type())
        }
    }

    fn compare_enums(left_idx: u32, right_idx: u32) -> ValueAndType {
        (left_idx == right_idx).into_value_and_type()
    }

    fn unsupported_type_error(left: &ValueAndType, right: &ValueAndType) -> String {
        format!("Unsupported op {:?}, {:?}", left.typ, right.typ)
    }
}
