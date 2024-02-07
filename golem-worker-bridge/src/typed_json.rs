use std::fmt::Display;

use http;
use nom::ParseTo;
use serde_json::Value;

// More of a typed JSON, for its primitives
// Anything other than primitives are just represented using JSON itself
// This is to ensure that there exists a reasonable safety when evaluating
// expressions such as `response.x > response.y`, that we don't want to
// consider x and y as strings if they were actually numbers
#[derive(PartialEq, Debug, Clone)]
pub enum TypedJson {
    Boolean(bool),
    Float(f64),
    U64(u64),
    I64(i64),
    String(String),
    ComplexJson(Value),
}

impl Display for TypedJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypedJson::Boolean(bool) => {
                write!(f, "{}", bool)
            }
            TypedJson::Float(float) => {
                write!(f, "{}", float)
            }
            TypedJson::U64(u64) => {
                write!(f, "{}", u64)
            }
            TypedJson::I64(i64) => {
                write!(f, "{}", i64)
            }
            TypedJson::ComplexJson(json) => {
                write!(f, "{}", json)
            }
            TypedJson::String(string) => {
                write!(f, "{}", string)
            }
        }
    }
}

pub enum VariantComparisonError {
    UnrelatedTypes(TypedJson, TypedJson, ComparisonOp),
    ComplexTypeComparison(TypedJson, ComparisonOp),
}

impl Display for VariantComparisonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantComparisonError::UnrelatedTypes(left, right, comparison_op) => {
                write!(
                    f,
                    "Comparing ({}) unrelated types: {:?} {:?}. If you want to refer to variables in request, make sure you are prefixing it with request. Examples: request.path.user_id, request.body.user_id, request.headers.user_id",
                    comparison_op, left, right
                )
            }
            VariantComparisonError::ComplexTypeComparison(right, comparison_op) => {
                write!(f, "Comparing ({}) complex types: {}. If you want to refer to variables in request, make sure you are prefixing it with request. Examples: request.path.user_id, request.body.user_id, request.headers.user_id", comparison_op, right)
            }
        }
    }
}

pub enum ComparisonOp {
    GreaterThan,
    GreaterThanOrEqualTo,
    LessThanOrEqualTo,
    LessThan,
    EqualTo,
}

impl Display for ComparisonOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComparisonOp::GreaterThan => {
                write!(f, ">")
            }
            ComparisonOp::LessThan => {
                write!(f, "<")
            }
            ComparisonOp::EqualTo => {
                write!(f, "=")
            }
            ComparisonOp::GreaterThanOrEqualTo => {
                write!(f, ">=")
            }
            ComparisonOp::LessThanOrEqualTo => {
                write!(f, "<=")
            }
        }
    }
}

impl TypedJson {
    pub fn get_http_status_code(&self) -> Option<http::status::StatusCode> {
        let variant = TypedJson::get_primitive_string(self)?;
        let possible_status_code = variant.parse::<u16>().ok()?;
        http::status::StatusCode::from_u16(possible_status_code).ok()
    }

    pub fn get_primitive_variant(input: &str) -> TypedJson {
        if let Ok(u64) = input.parse::<u64>() {
            TypedJson::U64(u64)
        } else if let Ok(i64) = input.parse::<i64>() {
            TypedJson::I64(i64)
        } else if let Ok(f64) = input.parse::<f64>() {
            TypedJson::Float(f64)
        } else if let Ok(bool) = input.parse::<bool>() {
            TypedJson::Boolean(bool)
        } else {
            TypedJson::String(input.to_string())
        }
    }

    pub fn get_primitive_string(&self) -> Option<String> {
        match self {
            TypedJson::Boolean(bool) => Some(bool.to_string()),
            TypedJson::Float(f) => Some(f.to_string()),
            TypedJson::String(string) => Some(string.clone()),
            TypedJson::U64(u64) => Some(u64.to_string()),
            TypedJson::I64(i64) => Some(i64.to_string()),
            TypedJson::ComplexJson(value) => match value {
                Value::Number(number) => Some(number.to_string()),
                Value::String(string) => Some(string.clone()),
                Value::Bool(bool) => Some(bool.to_string()),
                Value::Object(_) => None,
                Value::Array(_) => None,
                Value::Null => None,
            },
        }
    }

    pub fn get_primitive_bool(&self) -> Option<bool> {
        match self {
            TypedJson::Boolean(bool) => Some(*bool),
            TypedJson::String(value) => value.parse().ok(),
            _ => None,
        }
    }

    pub fn get_json(&self) -> Option<Value> {
        match self {
            TypedJson::ComplexJson(json) => Some(json.clone()),
            _ => None,
        }
    }

    pub fn convert_to_json(&self) -> Value {
        match self {
            TypedJson::Boolean(bool) => Value::Bool(*bool),
            TypedJson::String(string) => Value::String(string.clone()),
            TypedJson::Float(float) => serde_json::Number::from_f64(*float)
                .map(Value::Number)
                .unwrap_or(Value::String(float.to_string())),
            TypedJson::I64(i64) => Value::Number(serde_json::Number::from(*i64)),
            TypedJson::U64(u64) => Value::Number(serde_json::Number::from(*u64)),
            TypedJson::ComplexJson(json) => json.clone(),
        }
    }

    pub fn greater_than(&self, that: TypedJson) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (TypedJson::Float(f1), TypedJson::Float(f2)) => Ok(f1 > &f2),
            (TypedJson::U64(i1), TypedJson::U64(i2)) => Ok(i1 > &i2),
            (TypedJson::I64(i1), TypedJson::I64(f2)) => Ok(i1 > &f2),
            (TypedJson::String(s1), TypedJson::String(s2)) => Ok(s1 > &s2),
            (TypedJson::Boolean(left), TypedJson::Boolean(right)) => Ok(*left & !right),
            (TypedJson::ComplexJson(_), t2 @ TypedJson::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::GreaterThan),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::GreaterThan,
            )),
        }
    }

    pub fn greater_than_or_equal_to(&self, that: TypedJson) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (TypedJson::Float(f1), TypedJson::Float(f2)) => Ok(f1 >= &f2),
            (TypedJson::U64(i1), TypedJson::U64(i2)) => Ok(i1 >= &i2),
            (TypedJson::I64(i1), TypedJson::I64(f2)) => Ok(i1 >= &f2),
            (TypedJson::String(s1), TypedJson::String(s2)) => Ok(s1 >= &s2),
            (TypedJson::Boolean(left), TypedJson::Boolean(right)) => Ok(*left >= right),
            (TypedJson::ComplexJson(_), t2 @ TypedJson::ComplexJson(_)) => {
                Err(VariantComparisonError::ComplexTypeComparison(
                    t2,
                    ComparisonOp::GreaterThanOrEqualTo,
                ))
            }
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::GreaterThanOrEqualTo,
            )),
        }
    }

    pub fn equal_to(&self, that: TypedJson) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (TypedJson::Float(f1), TypedJson::Float(f2)) => Ok(*f1 == f2),
            (TypedJson::U64(i1), TypedJson::U64(i2)) => Ok(*i1 == i2),
            (TypedJson::I64(i1), TypedJson::I64(f2)) => Ok(*i1 == f2),
            (TypedJson::String(s1), TypedJson::String(s2)) => Ok(*s1 == s2),
            (TypedJson::Boolean(left), TypedJson::Boolean(right)) => Ok(*left == right),
            (TypedJson::ComplexJson(_), t2 @ TypedJson::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::EqualTo),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::EqualTo,
            )),
        }
    }

    pub fn less_than(&self, that: TypedJson) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            (TypedJson::Float(f1), TypedJson::Float(f2)) => Ok(f1 < &f2),
            (TypedJson::U64(i1), TypedJson::U64(i2)) => Ok(i1 < &i2),
            (TypedJson::I64(i1), TypedJson::I64(f2)) => Ok(i1 < &f2),
            (TypedJson::String(s1), TypedJson::String(s2)) => Ok(s1 < &s2),
            (TypedJson::Boolean(left), TypedJson::Boolean(right)) => Ok(*left == right),
            (TypedJson::ComplexJson(_), t2 @ TypedJson::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::LessThan),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::LessThan,
            )),
        }
    }

    pub fn less_than_or_equal_to(&self, that: TypedJson) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            (TypedJson::Float(f1), TypedJson::Float(f2)) => Ok(f1 <= &f2),
            (TypedJson::U64(i1), TypedJson::U64(i2)) => Ok(i1 <= &i2),
            (TypedJson::I64(i1), TypedJson::I64(f2)) => Ok(i1 <= &f2),
            (TypedJson::String(s1), TypedJson::String(s2)) => Ok(s1 <= &s2),
            (TypedJson::Boolean(left), TypedJson::Boolean(right)) => Ok(*left <= right),
            (TypedJson::ComplexJson(_), t2 @ TypedJson::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::LessThanOrEqualTo),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::LessThanOrEqualTo,
            )),
        }
    }

    pub fn from_json(input: &serde_json::Value) -> TypedJson {
        match input {
            array @ Value::Array(_) => TypedJson::ComplexJson(array.clone()),
            Value::Bool(bool) => TypedJson::Boolean(*bool),
            Value::String(string) => TypedJson::from_string(string.as_str()),
            Value::Number(number) => TypedJson::from_string(number.to_string().as_str()),
            map @ Value::Object(_) => TypedJson::ComplexJson(map.clone()),
            null @ Value::Null => TypedJson::ComplexJson(null.clone()),
        }
    }

    pub fn from_string(input: &str) -> TypedJson {
        if let Ok(u64) = input.parse::<u64>() {
            return TypedJson::U64(u64);
        } else if let Ok(i64_value) = input.parse::<i64>() {
            return TypedJson::I64(i64_value);
        } else if let Ok(f64_value) = input.parse::<f64>() {
            return TypedJson::Float(f64_value);
        }

        // If parsing as a number fails, treat it as a string
        TypedJson::String(input.to_string())
    }
}
