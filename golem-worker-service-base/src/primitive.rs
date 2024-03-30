use golem_wasm_rpc::TypeAnnotatedValue;
use std::fmt::Display;

pub trait GetPrimitive {
    fn get_primitive(&self) -> Option<Primitive>;
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Primitive {
    Num(Number),
    String(String),
    Bool(bool),
}

impl From<String> for Primitive {
    fn from(value: String) -> Self {
        if let Ok(u64) = value.parse::<u64>() {
            Primitive::Num(Number::PosInt(u64))
        } else if let Ok(i64_value) = value.parse::<i64>() {
            Primitive::Num(Number::NegInt(i64_value))
        } else if let Ok(f64_value) = value.parse::<f64>() {
            Primitive::Num(Number::Float(f64_value))
        } else if let Ok(bool) = value.parse::<bool>() {
            Primitive::Bool(bool)
        } else {
            Primitive::String(value.to_string())
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Number {
    PosInt(u64),
    NegInt(i64),
    Float(f64),
}

impl Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Number::PosInt(value) => write!(f, "{}", value),
            Number::NegInt(value) => write!(f, "{}", value),
            Number::Float(value) => write!(f, "{}", value),
        }
    }
}

impl Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Primitive::Num(number) => write!(f, "{}", number),
            Primitive::String(value) => write!(f, "{}", value),
            Primitive::Bool(value) => write!(f, "{}", value),
        }
    }
}

impl GetPrimitive for TypeAnnotatedValue {
    fn get_primitive(&self) -> Option<Primitive> {
        let optional_number = get_number(self);

        match optional_number {
            Some(number) => Some(Primitive::Num(number)),
            None => match self {
                TypeAnnotatedValue::Str(value) => Some(Primitive::String(value.clone())),
                TypeAnnotatedValue::Bool(value) => Some(Primitive::Bool(*value)),
                _ => None,
            },
        }
    }
}

fn get_number(type_annotated_value: &TypeAnnotatedValue) -> Option<Number> {
    match type_annotated_value {
        TypeAnnotatedValue::S16(value) => Some(Number::NegInt(*value as i64)),
        TypeAnnotatedValue::S32(value) => Some(Number::NegInt(*value as i64)),
        TypeAnnotatedValue::S64(value) => Some(Number::NegInt(*value)),
        TypeAnnotatedValue::U16(value) => Some(Number::PosInt(*value as u64)),
        TypeAnnotatedValue::U32(value) => Some(Number::PosInt(*value as u64)),
        TypeAnnotatedValue::U64(value) => Some(Number::PosInt(*value)),
        TypeAnnotatedValue::F32(value) => Some(Number::Float(*value as f64)),
        TypeAnnotatedValue::F64(value) => Some(Number::Float(*value)),
        _ => None,
    }
}
