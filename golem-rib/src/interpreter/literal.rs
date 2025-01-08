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

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
use std::cmp::Ordering;
use std::fmt::Display;
use std::ops::{Add, Div, Mul, Sub};

pub trait GetLiteralValue {
    fn get_literal(&self) -> Option<LiteralValue>;
}

impl GetLiteralValue for ValueAndType {
    fn get_literal(&self) -> Option<LiteralValue> {
        match self {
            ValueAndType {
                value: Value::String(value),
                ..
            } => Some(LiteralValue::String(value.clone())),
            ValueAndType {
                value: Value::Char(code_point),
                ..
            } => char::from_u32(*code_point as u32)
                .map(|c| c.to_string())
                .map(LiteralValue::String),
            ValueAndType {
                value: Value::Bool(value),
                ..
            } => Some(LiteralValue::Bool(*value)),
            ValueAndType {
                value: Value::Enum(idx),
                typ: AnalysedType::Enum(typ),
            } => {
                // An enum can be turned into a simple literal and can be part of string concatenations
                Some(LiteralValue::String(typ.cases[*idx as usize].clone()))
            }
            ValueAndType {
                value:
                    Value::Variant {
                        case_idx,
                        case_value,
                    },
                typ: AnalysedType::Variant(typ),
            } => {
                // A no arg variant can be turned into a simple literal and can be part of string concatenations
                if case_value.is_none() {
                    Some(LiteralValue::String(
                        typ.cases[*case_idx as usize].name.clone(),
                    ))
                } else {
                    None
                }
            }
            other => internal::get_numeric_value(other).map(LiteralValue::Num),
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum LiteralValue {
    Num(CoercedNumericValue),
    String(String),
    Bool(bool),
}

impl LiteralValue {
    pub fn get_bool(&self) -> Option<bool> {
        match self {
            LiteralValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn get_number(&self) -> Option<CoercedNumericValue> {
        match self {
            LiteralValue::Num(num) => Some(num.clone()),
            _ => None,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            LiteralValue::Num(number) => number.to_string(),
            LiteralValue::String(value) => value.clone(),
            LiteralValue::Bool(value) => value.to_string(),
        }
    }
}

impl From<String> for LiteralValue {
    fn from(value: String) -> Self {
        if let Ok(u64) = value.parse::<u64>() {
            LiteralValue::Num(CoercedNumericValue::PosInt(u64))
        } else if let Ok(i64_value) = value.parse::<i64>() {
            LiteralValue::Num(CoercedNumericValue::NegInt(i64_value))
        } else if let Ok(f64_value) = value.parse::<f64>() {
            LiteralValue::Num(CoercedNumericValue::Float(f64_value))
        } else if let Ok(bool) = value.parse::<bool>() {
            LiteralValue::Bool(bool)
        } else {
            LiteralValue::String(value.to_string())
        }
    }
}

// A coerced representation of numeric wasm types, simplifying finer-grained TypeAnnotatedValue types into u64, i64, and f64.
#[derive(Clone, Debug)]
pub enum CoercedNumericValue {
    PosInt(u64),
    NegInt(i64),
    Float(f64),
}

impl CoercedNumericValue {
    pub fn cast_to(&self, analysed_type: &AnalysedType) -> Option<ValueAndType> {
        match (self, analysed_type) {
            (CoercedNumericValue::PosInt(val), AnalysedType::U8(_)) if *val <= u8::MAX as u64 => {
                Some((*val as u8).into_value_and_type())
            }
            (CoercedNumericValue::PosInt(val), AnalysedType::U16(_)) if *val <= u16::MAX as u64 => {
                Some((*val as u16).into_value_and_type())
            }
            (CoercedNumericValue::PosInt(val), AnalysedType::U32(_)) if *val <= u32::MAX as u64 => {
                Some((*val as u32).into_value_and_type())
            }
            (CoercedNumericValue::PosInt(val), AnalysedType::U64(_)) => {
                Some((*val).into_value_and_type())
            }

            (CoercedNumericValue::NegInt(val), AnalysedType::S8(_))
                if *val >= i8::MIN as i64 && *val <= i8::MAX as i64 =>
            {
                Some((*val as i8).into_value_and_type())
            }
            (CoercedNumericValue::NegInt(val), AnalysedType::S16(_))
                if *val >= i16::MIN as i64 && *val <= i16::MAX as i64 =>
            {
                Some((*val as i16).into_value_and_type())
            }
            (CoercedNumericValue::NegInt(val), AnalysedType::S32(_))
                if *val >= i32::MIN as i64 && *val <= i32::MAX as i64 =>
            {
                Some((*val as i32).into_value_and_type())
            }
            (CoercedNumericValue::NegInt(val), AnalysedType::S64(_)) => {
                Some((*val).into_value_and_type())
            }

            (CoercedNumericValue::Float(val), AnalysedType::F64(_)) => {
                Some((*val).into_value_and_type())
            }
            (CoercedNumericValue::Float(val), AnalysedType::F32(_))
                if *val >= f32::MIN as f64 && *val <= f32::MAX as f64 =>
            {
                Some((*val as f32).into_value_and_type())
            }

            _ => None,
        }
    }
}

macro_rules! impl_ops {
    ($trait:ident, $method:ident) => {
        impl $trait for CoercedNumericValue {
            type Output = Self;

            fn $method(self, rhs: Self) -> Self::Output {
                match (self, rhs) {
                    (CoercedNumericValue::Float(a), CoercedNumericValue::Float(b)) => {
                        CoercedNumericValue::Float(a.$method(b))
                    }
                    (CoercedNumericValue::Float(a), CoercedNumericValue::PosInt(b)) => {
                        CoercedNumericValue::Float(a.$method(b as f64))
                    }
                    (CoercedNumericValue::Float(a), CoercedNumericValue::NegInt(b)) => {
                        CoercedNumericValue::Float(a.$method(b as f64))
                    }
                    (CoercedNumericValue::PosInt(a), CoercedNumericValue::Float(b)) => {
                        CoercedNumericValue::Float((a as f64).$method(b))
                    }
                    (CoercedNumericValue::NegInt(a), CoercedNumericValue::Float(b)) => {
                        CoercedNumericValue::Float((a as f64).$method(b))
                    }
                    (CoercedNumericValue::PosInt(a), CoercedNumericValue::PosInt(b)) => {
                        CoercedNumericValue::PosInt(a.$method(b))
                    }
                    (CoercedNumericValue::NegInt(a), CoercedNumericValue::NegInt(b)) => {
                        CoercedNumericValue::NegInt(a.$method(b))
                    }
                    (CoercedNumericValue::PosInt(a), CoercedNumericValue::NegInt(b)) => {
                        CoercedNumericValue::NegInt((a as i64).$method(b))
                    }
                    (CoercedNumericValue::NegInt(a), CoercedNumericValue::PosInt(b)) => {
                        CoercedNumericValue::NegInt(a.$method(b as i64))
                    }
                }
            }
        }
    };
}

impl_ops!(Add, add);
impl_ops!(Sub, sub);
impl_ops!(Mul, mul);
impl_ops!(Div, div);

// Auto-derived PartialOrd fails if types don't match
// and therefore custom impl.
impl PartialOrd for CoercedNumericValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use CoercedNumericValue::*;
        match (self, other) {
            (PosInt(a), PosInt(b)) => a.partial_cmp(b),
            (NegInt(a), NegInt(b)) => a.partial_cmp(b),
            (Float(a), Float(b)) => a.partial_cmp(b),

            (PosInt(a), NegInt(b)) => {
                if let Ok(b_as_u64) = u64::try_from(*b) {
                    a.partial_cmp(&b_as_u64)
                } else {
                    Some(Ordering::Greater) // Positive numbers are greater than negative numbers
                }
            }

            (NegInt(a), PosInt(b)) => {
                if let Ok(a_as_u64) = u64::try_from(*a) {
                    a_as_u64.partial_cmp(b)
                } else {
                    Some(Ordering::Less) // Negative numbers are less than positive numbers
                }
            }

            (PosInt(a), Float(b)) => (*a as f64).partial_cmp(b),

            (Float(a), PosInt(b)) => a.partial_cmp(&(*b as f64)),

            (NegInt(a), Float(b)) => (*a as f64).partial_cmp(b),

            (Float(a), NegInt(b)) => a.partial_cmp(&(*b as f64)),
        }
    }
}

// Similarly, auto-derived PartialEq fails if types don't match
// and therefore custom impl
// There is a high chance two variables can be inferred S32(1) and U32(1)
impl PartialEq for CoercedNumericValue {
    fn eq(&self, other: &Self) -> bool {
        use CoercedNumericValue::*;
        match (self, other) {
            (PosInt(a), PosInt(b)) => a == b,
            (NegInt(a), NegInt(b)) => a == b,
            (Float(a), Float(b)) => a == b,

            // Comparing PosInt with NegInt
            (PosInt(a), NegInt(b)) => {
                if let Ok(b_as_u64) = u64::try_from(*b) {
                    a == &b_as_u64
                } else {
                    false
                }
            }

            // Comparing NegInt with PosInt
            (NegInt(a), PosInt(b)) => {
                if let Ok(a_as_u64) = u64::try_from(*a) {
                    &a_as_u64 == b
                } else {
                    false
                }
            }

            // Comparing PosInt with Float
            (PosInt(a), Float(b)) => (*a as f64) == *b,

            // Comparing Float with PosInt
            (Float(a), PosInt(b)) => *a == (*b as f64),

            // Comparing NegInt with Float
            (NegInt(a), Float(b)) => (*a as f64) == *b,

            // Comparing Float with NegInt
            (Float(a), NegInt(b)) => *a == (*b as f64),
        }
    }
}

impl Display for CoercedNumericValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoercedNumericValue::PosInt(value) => write!(f, "{}", value),
            CoercedNumericValue::NegInt(value) => write!(f, "{}", value),
            CoercedNumericValue::Float(value) => write!(f, "{}", value),
        }
    }
}

impl Display for LiteralValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiteralValue::Num(number) => write!(f, "{}", number),
            LiteralValue::String(value) => write!(f, "{}", value),
            LiteralValue::Bool(value) => write!(f, "{}", value),
        }
    }
}

mod internal {
    use crate::interpreter::literal::CoercedNumericValue;
    use golem_wasm_rpc::{Value, ValueAndType};

    pub(crate) fn get_numeric_value(
        type_annotated_value: &ValueAndType,
    ) -> Option<CoercedNumericValue> {
        match &type_annotated_value.value {
            Value::S8(value) => Some(CoercedNumericValue::NegInt(*value as i64)),
            Value::S16(value) => Some(CoercedNumericValue::NegInt(*value as i64)),
            Value::S32(value) => Some(CoercedNumericValue::NegInt(*value as i64)),
            Value::S64(value) => Some(CoercedNumericValue::NegInt(*value)),
            Value::U8(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            Value::U16(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            Value::U32(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            Value::U64(value) => Some(CoercedNumericValue::PosInt(*value)),
            Value::F32(value) => Some(CoercedNumericValue::Float(*value as f64)),
            Value::F64(value) => Some(CoercedNumericValue::Float(*value)),
            _ => None,
        }
    }
}
