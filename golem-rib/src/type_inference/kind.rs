use crate::InferredType;
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::Display;

pub trait GetTypeKind {
    fn get_type_kind(&self) -> TypeKind;
}

#[derive(PartialEq, Clone, Debug)]
pub enum TypeKind {
    Record,
    Tuple,
    Flag,
    Str,
    Number,
    List,
    Boolean,
    Option,
    Enum,
    Char,
    Result,
    Resource,
    Variant,
    Unknown,
    Ambiguous { possibilities: Vec<TypeKind> },
    Range,
}

impl Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Record => write!(f, "record"),
            TypeKind::Tuple => write!(f, "tuple"),
            TypeKind::Flag => write!(f, "flag"),
            TypeKind::Str => write!(f, "str"),
            TypeKind::Number => write!(f, "number"),
            TypeKind::List => write!(f, "list"),
            TypeKind::Boolean => write!(f, "boolean"),
            TypeKind::Option => write!(f, "option"),
            TypeKind::Enum => write!(f, "enum"),
            TypeKind::Char => write!(f, "chr"),
            TypeKind::Result => write!(f, "result"),
            TypeKind::Resource => write!(f, "resource"),
            TypeKind::Variant => write!(f, "variant"),
            TypeKind::Unknown => write!(f, "unknown"),
            TypeKind::Range => write!(f, "range"),
            TypeKind::Ambiguous { possibilities } => {
                write!(f, "conflicting types: ")?;
                for (i, kind) in possibilities.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", kind)?;
                }
                Ok(())
            }
        }
    }
}

impl GetTypeKind for AnalysedType {
    fn get_type_kind(&self) -> TypeKind {
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
    fn get_type_kind(&self) -> TypeKind {
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
            InferredType::OneOf(possibilities) => internal::get_type_kind(possibilities),
            InferredType::AllOf(possibilities) => internal::get_type_kind(possibilities),
            InferredType::Unknown => TypeKind::Unknown,
            InferredType::Sequence(_) => TypeKind::Unknown,
            InferredType::Instance { .. } => TypeKind::Unknown,
            InferredType::Range { .. } => TypeKind::Range,
        }
    }
}

mod internal {
    use crate::type_inference::kind::{GetTypeKind, TypeKind};
    use crate::InferredType;

    pub(crate) fn get_type_kind(possibilities: &[InferredType]) -> TypeKind {
        if let Some(first) = possibilities.first() {
            let first = first.get_type_kind();
            if possibilities.iter().all(|p| p.get_type_kind() == first) {
                first
            } else {
                TypeKind::Ambiguous {
                    possibilities: possibilities.iter().map(|p| p.get_type_kind()).collect(),
                }
            }
        } else {
            TypeKind::Unknown
        }
    }
}
