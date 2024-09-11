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

use std::cmp::Ordering;
use std::fmt::Display;

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub trait GetLiteralValue {
    fn get_literal(&self) -> Option<LiteralValue>;
}

impl GetLiteralValue for TypeAnnotatedValue {
    fn get_literal(&self) -> Option<LiteralValue> {
        match self {
            TypeAnnotatedValue::Str(value) => Some(LiteralValue::String(value.clone())),
            TypeAnnotatedValue::Bool(value) => Some(LiteralValue::Bool(*value)),
            TypeAnnotatedValue::Enum(value) => {
                // An enum can be turned into a simple literal and can be part of string concatenations
                Some(LiteralValue::String(value.value.clone()))
            }
            TypeAnnotatedValue::Variant(value) => {
                // A no arg variant can be turned into a simple literal and can be part of string concatenations
                if let None = value.case_value {
                    Some(LiteralValue::String(value.case_name.clone()))
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
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    pub(crate) fn get_numeric_value(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Option<CoercedNumericValue> {
        match type_annotated_value {
            TypeAnnotatedValue::S16(value) => Some(CoercedNumericValue::NegInt(*value as i64)),
            TypeAnnotatedValue::S32(value) => Some(CoercedNumericValue::NegInt(*value as i64)),
            TypeAnnotatedValue::S64(value) => Some(CoercedNumericValue::NegInt(*value)),
            TypeAnnotatedValue::U16(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            TypeAnnotatedValue::U32(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            TypeAnnotatedValue::U64(value) => Some(CoercedNumericValue::PosInt(*value)),
            TypeAnnotatedValue::F32(value) => Some(CoercedNumericValue::Float(*value as f64)),
            TypeAnnotatedValue::F64(value) => Some(CoercedNumericValue::Float(*value)),
            TypeAnnotatedValue::U8(value) => Some(CoercedNumericValue::PosInt(*value as u64)),
            _ => None,
        }
    }
}
