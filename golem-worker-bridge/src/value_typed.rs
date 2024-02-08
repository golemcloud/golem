use std::fmt::Display;

use serde_json::Value;

// More of a typed serde_json::Value but typed for its primitives
// Anything other than primitives are just represented using JSON (i.e, non-recursive) itself
// This is to ensure that there exists a reasonable safety when evaluating
// expressions such as `response.x > response.y`, that we don't want to
// consider x and y as strings if they were actually numbers, while keeping it performant enough.
// Conversion to ValueTyped should ideally be used only if type information is required (such as only when the node has comparison operator for instance)
#[derive(PartialEq, Debug, Clone)]
pub enum ValueTyped {
    Boolean(bool),
    Float(f64),
    U64(u64),
    I64(i64),
    String(String),
    ComplexJson(Value),
}

impl Display for ValueTyped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueTyped::Boolean(bool) => {
                write!(f, "{}", bool)
            }
            ValueTyped::Float(float) => {
                write!(f, "{}", float)
            }
            ValueTyped::U64(u64) => {
                write!(f, "{}", u64)
            }
            ValueTyped::I64(i64) => {
                write!(f, "{}", i64)
            }
            ValueTyped::ComplexJson(json) => {
                write!(f, "{}", json)
            }
            ValueTyped::String(string) => {
                write!(f, "{}", string)
            }
        }
    }
}

pub enum VariantComparisonError {
    UnrelatedTypes(ValueTyped, ValueTyped, ComparisonOp),
    ComplexTypeComparison(ValueTyped, ComparisonOp),
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

impl ValueTyped {
    pub fn get_primitive_variant(input: &str) -> ValueTyped {
        if let Ok(u64) = input.parse::<u64>() {
            ValueTyped::U64(u64)
        } else if let Ok(i64) = input.parse::<i64>() {
            ValueTyped::I64(i64)
        } else if let Ok(f64) = input.parse::<f64>() {
            ValueTyped::Float(f64)
        } else if let Ok(bool) = input.parse::<bool>() {
            ValueTyped::Boolean(bool)
        } else {
            ValueTyped::String(input.to_string())
        }
    }

    pub fn get_primitive_string(&self) -> Option<String> {
        match self {
            ValueTyped::Boolean(bool) => Some(bool.to_string()),
            ValueTyped::Float(f) => Some(f.to_string()),
            ValueTyped::String(string) => Some(string.clone()),
            ValueTyped::U64(u64) => Some(u64.to_string()),
            ValueTyped::I64(i64) => Some(i64.to_string()),
            ValueTyped::ComplexJson(value) => match value {
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
            ValueTyped::Boolean(bool) => Some(*bool),
            ValueTyped::String(value) => value.parse().ok(),
            _ => None,
        }
    }

    pub fn get_json(&self) -> Option<Value> {
        match self {
            ValueTyped::ComplexJson(json) => Some(json.clone()),
            _ => None,
        }
    }

    pub fn convert_to_json(&self) -> Value {
        match self {
            ValueTyped::Boolean(bool) => Value::Bool(*bool),
            ValueTyped::String(string) => Value::String(string.clone()),
            ValueTyped::Float(float) => serde_json::Number::from_f64(*float)
                .map(Value::Number)
                .unwrap_or(Value::String(float.to_string())),
            ValueTyped::I64(i64) => Value::Number(serde_json::Number::from(*i64)),
            ValueTyped::U64(u64) => Value::Number(serde_json::Number::from(*u64)),
            ValueTyped::ComplexJson(json) => json.clone(),
        }
    }

    pub fn greater_than(&self, that: ValueTyped) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (ValueTyped::Float(f1), ValueTyped::Float(f2)) => Ok(f1 > &f2),
            (ValueTyped::U64(i1), ValueTyped::U64(i2)) => Ok(i1 > &i2),
            (ValueTyped::I64(i1), ValueTyped::I64(f2)) => Ok(i1 > &f2),
            (ValueTyped::String(s1), ValueTyped::String(s2)) => Ok(s1 > &s2),
            (ValueTyped::Boolean(left), ValueTyped::Boolean(right)) => Ok(*left & !right),
            (ValueTyped::ComplexJson(_), t2 @ ValueTyped::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::GreaterThan),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::GreaterThan,
            )),
        }
    }

    pub fn greater_than_or_equal_to(
        &self,
        that: ValueTyped,
    ) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (ValueTyped::Float(f1), ValueTyped::Float(f2)) => Ok(f1 >= &f2),
            (ValueTyped::U64(i1), ValueTyped::U64(i2)) => Ok(i1 >= &i2),
            (ValueTyped::I64(i1), ValueTyped::I64(f2)) => Ok(i1 >= &f2),
            (ValueTyped::String(s1), ValueTyped::String(s2)) => Ok(s1 >= &s2),
            (ValueTyped::Boolean(left), ValueTyped::Boolean(right)) => Ok(*left >= right),
            (ValueTyped::ComplexJson(_), t2 @ ValueTyped::ComplexJson(_)) => {
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

    pub fn equal_to(&self, that: ValueTyped) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            // There won't be coercsions at primitve level
            (ValueTyped::Float(f1), ValueTyped::Float(f2)) => Ok(*f1 == f2),
            (ValueTyped::U64(i1), ValueTyped::U64(i2)) => Ok(*i1 == i2),
            (ValueTyped::I64(i1), ValueTyped::I64(f2)) => Ok(*i1 == f2),
            (ValueTyped::String(s1), ValueTyped::String(s2)) => Ok(*s1 == s2),
            (ValueTyped::Boolean(left), ValueTyped::Boolean(right)) => Ok(*left == right),
            (ValueTyped::ComplexJson(_), t2 @ ValueTyped::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::EqualTo),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::EqualTo,
            )),
        }
    }

    pub fn less_than(&self, that: ValueTyped) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            (ValueTyped::Float(f1), ValueTyped::Float(f2)) => Ok(f1 < &f2),
            (ValueTyped::U64(i1), ValueTyped::U64(i2)) => Ok(i1 < &i2),
            (ValueTyped::I64(i1), ValueTyped::I64(f2)) => Ok(i1 < &f2),
            (ValueTyped::String(s1), ValueTyped::String(s2)) => Ok(s1 < &s2),
            (ValueTyped::Boolean(left), ValueTyped::Boolean(right)) => Ok(*left == right),
            (ValueTyped::ComplexJson(_), t2 @ ValueTyped::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::LessThan),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::LessThan,
            )),
        }
    }

    pub fn less_than_or_equal_to(&self, that: ValueTyped) -> Result<bool, VariantComparisonError> {
        match (self, that) {
            (ValueTyped::Float(f1), ValueTyped::Float(f2)) => Ok(f1 <= &f2),
            (ValueTyped::U64(i1), ValueTyped::U64(i2)) => Ok(i1 <= &i2),
            (ValueTyped::I64(i1), ValueTyped::I64(f2)) => Ok(i1 <= &f2),
            (ValueTyped::String(s1), ValueTyped::String(s2)) => Ok(s1 <= &s2),
            (ValueTyped::Boolean(left), ValueTyped::Boolean(right)) => Ok(*left <= right),
            (ValueTyped::ComplexJson(_), t2 @ ValueTyped::ComplexJson(_)) => Err(
                VariantComparisonError::ComplexTypeComparison(t2, ComparisonOp::LessThanOrEqualTo),
            ),
            (t1, t2) => Err(VariantComparisonError::UnrelatedTypes(
                t1.clone(),
                t2,
                ComparisonOp::LessThanOrEqualTo,
            )),
        }
    }

    pub fn from_json(input: &serde_json::Value) -> ValueTyped {
        match input {
            array @ Value::Array(_) => ValueTyped::ComplexJson(array.clone()),
            Value::Bool(bool) => ValueTyped::Boolean(*bool),
            Value::String(string) => ValueTyped::from_string(string.as_str()),
            Value::Number(number) => ValueTyped::from_string(number.to_string().as_str()),
            map @ Value::Object(_) => ValueTyped::ComplexJson(map.clone()),
            null @ Value::Null => ValueTyped::ComplexJson(null.clone()),
        }
    }

    pub fn from_string(input: &str) -> ValueTyped {
        if let Ok(u64) = input.parse::<u64>() {
            return ValueTyped::U64(u64);
        } else if let Ok(i64_value) = input.parse::<i64>() {
            return ValueTyped::I64(i64_value);
        } else if let Ok(f64_value) = input.parse::<f64>() {
            return ValueTyped::Float(f64_value);
        }

        // If parsing as a number fails, treat it as a string
        ValueTyped::String(input.to_string())
    }
}
