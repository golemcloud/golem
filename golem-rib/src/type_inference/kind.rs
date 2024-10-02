use std::fmt::Display;
use golem_wasm_ast::analysis::AnalysedType;
use crate::{Expr, InferredType};

pub trait GetTypeKind {
    fn get_kind(&self) -> TypeKind;

}

#[derive(PartialEq)]
pub enum TypeKind {
    Record,
    Tuple,
    Flag,
    Str,
    Number,
    List,
    Boolean,
    FunctionCall,
    Option,
    Enum,
    Char,
    Result,
    Resource,
    Variant,
    Unknown,
}

impl Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Record => write!(f, "Record"),
            TypeKind::Tuple => write!(f, "Tuple"),
            TypeKind::Flag => write!(f, "Flag"),
            TypeKind::Str => write!(f, "Str"),
            TypeKind::Number => write!(f, "Number"),
            TypeKind::List => write!(f, "List"),
            TypeKind::Boolean => write!(f, "Boolean"),
            TypeKind::FunctionCall => write!(f, "FunctionCall"),
            TypeKind::Option => write!(f, "Option"),
            TypeKind::Enum => write!(f, "Enum"),
            TypeKind::Char => write!(f, "Char"),
            TypeKind::Result => write!(f, "Result"),
            TypeKind::Resource => write!(f, "Resource"),
            TypeKind::Variant => write!(f, "Variant"),
            TypeKind::Unknown => write!(f, "Unknown"),
        }
    }
}

impl GetTypeKind for AnalysedType {
    fn get_kind(&self) -> TypeKind {
        match self {
            AnalysedType::Record(_) => TypeKind::Record,
            AnalysedType::Tuple(_) => TypeKind::Tuple,
            AnalysedType::Flags(_) => TypeKind::Flag,
            AnalysedType::Str(_) => TypeKind::Str,
            AnalysedType::S8(_) => TypeKind::Number,
            AnalysedType::U8(_) => TypeKind::Number,
            AnalysedType::S16(_) => TypeKind::Number,
            AnalysedType::U16(_) => TypeKind::Number,
            AnalysedType::S32(_) => TypeKind::Number,
            AnalysedType::U32(_) => TypeKind::Number,
            AnalysedType::S64(_) => TypeKind::Number,
            AnalysedType::U64(_) => TypeKind::Number,
            AnalysedType::F32(_) => TypeKind::Number,
            AnalysedType::F64(_) => TypeKind::Number,
            AnalysedType::Chr(_) => TypeKind::Char,
            AnalysedType::List(_) => TypeKind::List,
            AnalysedType::Bool(_) => TypeKind::Boolean,
            AnalysedType::Option(_) => TypeKind::Option,
            AnalysedType::Enum(_) => TypeKind::Enum,
            AnalysedType::Result(_) => TypeKind::Result,
            AnalysedType::Handle(_) => TypeKind::Resource,
            AnalysedType::Variant(_) => TypeKind::Variant,
        }
    }
}

impl GetTypeKind for InferredType {
    fn get_kind(&self) -> TypeKind {
        match self {
            InferredType::Bool => TypeKind::Boolean,
            InferredType::S8 => TypeKind::Number,
            InferredType::U8 => TypeKind::Number,
            InferredType::S16 => TypeKind::Number,
            InferredType::U16 => TypeKind::Number,
            InferredType::S32 => TypeKind::Number,
            InferredType::U32 => TypeKind::Number,
            InferredType::S64 => TypeKind::Number,
            InferredType::U64 => TypeKind::Number,
            InferredType::F32 => TypeKind::Number,
            InferredType::F64 => TypeKind::Number,
            InferredType::Chr => TypeKind::Char,
            InferredType::Str => TypeKind::Str,
            InferredType::List(_) => TypeKind::List,
            InferredType::Tuple(_) => TypeKind::Tuple,
            InferredType::Record(_) => TypeKind::Record,
            InferredType::Flags(_) => TypeKind::Flag,
            InferredType::Enum(_) => TypeKind::Enum,
            InferredType::Option(_) => TypeKind::Option,
            InferredType::Result { .. } => TypeKind::Result,
            InferredType::Variant(_) => TypeKind::Variant,
            InferredType::Resource { .. } => TypeKind::Resource,
            InferredType::OneOf(possibilities) => {
                if let Some(first) = possibilities.first() {
                    let first = first.get_kind();
                    if possibilities.iter().all(|p| p.get_kind() == first) {
                        first
                    } else {
                        TypeKind::Unknown
                    }
                } else {
                    TypeKind::Unknown
                }
            }
            InferredType::AllOf(possibilities) => {
                if let Some(first) = possibilities.first() {
                    let first = first.get_kind();
                    if possibilities.iter().all(|p| p.get_kind() == first) {
                        first
                    } else {
                        TypeKind::Unknown
                    }
                } else {
                    TypeKind::Unknown
                }
            }
            InferredType::Unknown => TypeKind::Unknown,
            InferredType::Sequence(_) => TypeKind::Unknown
        }
    }
}