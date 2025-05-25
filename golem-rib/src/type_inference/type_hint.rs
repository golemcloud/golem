// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{InferredType, TypeInternal};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt;
use std::ops::Deref;

// `TypeHint` is a simplified form of `InferredType`
// It can capture partial type information (e.g., `List(None)` all the  way full type information such
// as `List(Some(Number))`).
// It supports early checks like `inferred_type.get_type_hint() == analysed_type.get_type_hint()`.
//
// As compilation progresses, `TypeHint` may get refined and can help with error reporting at various
// stages even if the type information is not fully available.
pub trait GetTypeHint {
    fn get_type_hint(&self) -> TypeHint;
}

#[derive(PartialEq, Clone, Debug)]
pub enum TypeHint {
    Record(Option<Vec<(String, TypeHint)>>),
    Tuple(Option<Vec<TypeHint>>),
    Flag(Option<Vec<String>>),
    Str,
    Number,
    List(Option<Box<TypeHint>>),
    Boolean,
    Option(Option<Box<TypeHint>>),
    Enum(Option<Vec<String>>),
    Char,
    Result {
        ok: Option<Box<TypeHint>>,
        err: Option<Box<TypeHint>>,
    },
    Resource,
    Variant(Option<Vec<(String, Option<TypeHint>)>>),
    Unknown,
    Ambiguous {
        possibilities: Vec<TypeHint>,
    },
    Range,
}

impl TypeHint {
    pub fn get_type_kind(&self) -> String {
        match self {
            TypeHint::Record(_) => "record".to_string(),
            TypeHint::Tuple(_) => "tuple".to_string(),
            TypeHint::Flag(_) => "flag".to_string(),
            TypeHint::Str => "str".to_string(),
            TypeHint::Number => "number".to_string(),
            TypeHint::List(_) => "list".to_string(),
            TypeHint::Boolean => "boolean".to_string(),
            TypeHint::Option(_) => "option".to_string(),
            TypeHint::Enum(_) => "enum".to_string(),
            TypeHint::Char => "char".to_string(),
            TypeHint::Result { .. } => "result".to_string(),
            TypeHint::Resource => "resource".to_string(),
            TypeHint::Variant(_) => "variant".to_string(),
            TypeHint::Unknown => "unknown".to_string(),
            TypeHint::Ambiguous { .. } => "ambiguous".to_string(),
            TypeHint::Range => "range".to_string(),
        }
    }
}

impl fmt::Display for TypeHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeHint::Record(Some(fields)) => {
                write!(f, "record{{")?;
                for (i, (name, kind)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, kind)?;
                }
                write!(f, "}}")
            }
            TypeHint::Record(None) => write!(f, "record"),

            TypeHint::Tuple(Some(types)) => {
                write!(f, "tuple<")?;
                for (i, kind) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", kind)?;
                }
                write!(f, ">")
            }
            TypeHint::Tuple(None) => write!(f, "tuple"),

            TypeHint::Flag(Some(flags)) => {
                write!(f, "{{")?;
                for (i, flag) in flags.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", flag)?;
                }
                write!(f, "}}")
            }
            TypeHint::Flag(None) => write!(f, "flag"),

            TypeHint::Str => write!(f, "string"),
            TypeHint::Number => write!(f, "number"),
            TypeHint::List(None) => write!(f, "list"),
            TypeHint::List(Some(typ)) => {
                write!(f, "list<")?;
                write!(f, "{}", typ)?;
                write!(f, ">")
            }
            TypeHint::Boolean => write!(f, "boolean"),
            TypeHint::Option(None) => write!(f, "option"),
            TypeHint::Option(Some(inner)) => {
                write!(f, "option<")?;
                write!(f, "{}", inner.deref())?;
                write!(f, ">")
            }
            TypeHint::Enum(None) => write!(f, "enum"),
            TypeHint::Enum(Some(enums)) => {
                write!(f, "enum{{")?;
                for (i, enum_name) in enums.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", enum_name)?;
                }
                write!(f, "}}")
            }
            TypeHint::Char => write!(f, "char"),
            TypeHint::Result { ok, err } => {
                write!(f, "result<")?;
                if let Some(ok) = ok {
                    write!(f, "{}", ok)?;
                } else {
                    write!(f, "_")?;
                }
                write!(f, ", ")?;
                if let Some(err) = err {
                    write!(f, "{}", err)?;
                } else {
                    write!(f, "_")?;
                }
                write!(f, ">")
            }
            TypeHint::Resource => write!(f, "resource"),
            TypeHint::Variant(Some(variants)) => {
                write!(f, "variant{{")?;
                for (i, (name, kind)) in variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(
                        f,
                        "{}: {}",
                        name,
                        kind.clone().map_or("_".to_string(), |x| x.to_string())
                    )?;
                }
                write!(f, "}}")
            }
            TypeHint::Variant(None) => write!(f, "variant"),
            TypeHint::Unknown => write!(f, "unknown"),
            TypeHint::Range => write!(f, "range"),

            TypeHint::Ambiguous { possibilities } => {
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

impl GetTypeHint for AnalysedType {
    fn get_type_hint(&self) -> TypeHint {
        match self {
            AnalysedType::Record(fields) => {
                let fields = fields
                    .fields
                    .iter()
                    .map(|name_tpe| (name_tpe.name.clone(), name_tpe.typ.get_type_hint()))
                    .collect();
                TypeHint::Record(Some(fields))
            }
            AnalysedType::Tuple(elems) => {
                let elems = elems.items.iter().map(|tpe| tpe.get_type_hint()).collect();
                TypeHint::Tuple(Some(elems))
            }
            AnalysedType::Flags(flags) => {
                let flags = flags.names.clone();
                TypeHint::Flag(Some(flags))
            }
            AnalysedType::Str(_) => TypeHint::Str,
            AnalysedType::S8(_) => TypeHint::Number,
            AnalysedType::U8(_) => TypeHint::Number,
            AnalysedType::S16(_) => TypeHint::Number,
            AnalysedType::U16(_) => TypeHint::Number,
            AnalysedType::S32(_) => TypeHint::Number,
            AnalysedType::U32(_) => TypeHint::Number,
            AnalysedType::S64(_) => TypeHint::Number,
            AnalysedType::U64(_) => TypeHint::Number,
            AnalysedType::F32(_) => TypeHint::Number,
            AnalysedType::F64(_) => TypeHint::Number,
            AnalysedType::Chr(_) => TypeHint::Char,
            AnalysedType::List(tpe) => {
                let inner = tpe.inner.get_type_hint();
                TypeHint::List(Some(Box::new(inner)))
            }
            AnalysedType::Bool(_) => TypeHint::Boolean,
            AnalysedType::Option(tpe) => {
                let inner = tpe.inner.get_type_hint();
                TypeHint::Option(Some(Box::new(inner)))
            }
            AnalysedType::Enum(tpe) => {
                let variants = tpe.cases.clone();
                TypeHint::Enum(Some(variants))
            }
            AnalysedType::Result(tpe_result) => {
                let ok: &Option<Box<AnalysedType>> = &tpe_result.ok;
                let err: &Option<Box<AnalysedType>> = &tpe_result.err;
                let ok = ok.as_ref().map(|tpe| tpe.get_type_hint());
                let err = err.as_ref().map(|tpe| tpe.get_type_hint());
                TypeHint::Result {
                    ok: ok.map(Box::new),
                    err: err.map(Box::new),
                }
            }
            AnalysedType::Handle(_) => TypeHint::Resource,
            AnalysedType::Variant(variants) => {
                let variants = variants
                    .cases
                    .iter()
                    .map(|name_tpe| {
                        (
                            name_tpe.name.clone(),
                            name_tpe.typ.clone().map(|tpe| tpe.get_type_hint()),
                        )
                    })
                    .collect();
                TypeHint::Variant(Some(variants))
            }
        }
    }
}

impl GetTypeHint for InferredType {
    fn get_type_hint(&self) -> TypeHint {
        match self.internal_type() {
            TypeInternal::Bool => TypeHint::Boolean,
            TypeInternal::S8
            | TypeInternal::U8
            | TypeInternal::S16
            | TypeInternal::U16
            | TypeInternal::S32
            | TypeInternal::U32
            | TypeInternal::S64
            | TypeInternal::U64
            | TypeInternal::F32
            | TypeInternal::F64 => TypeHint::Number,
            TypeInternal::Chr => TypeHint::Char,
            TypeInternal::Str => TypeHint::Str,
            TypeInternal::List(inferred_type) => {
                TypeHint::List(Some(Box::new(inferred_type.get_type_hint())))
            }
            TypeInternal::Tuple(tuple) => {
                TypeHint::Tuple(Some(tuple.iter().map(GetTypeHint::get_type_hint).collect()))
            }
            TypeInternal::Record(record) => TypeHint::Record(Some(
                record
                    .iter()
                    .map(|(name, tpe)| (name.to_string(), tpe.get_type_hint()))
                    .collect(),
            )),
            TypeInternal::Flags(flags) => {
                TypeHint::Flag(Some(flags.iter().map(|x| x.to_string()).collect()))
            }
            TypeInternal::Enum(enums) => {
                TypeHint::Enum(Some(enums.iter().map(|s| s.to_string()).collect()))
            }
            TypeInternal::Option(inner) => TypeHint::Option(Some(Box::new(inner.get_type_hint()))),
            TypeInternal::Result { ok, error } => TypeHint::Result {
                ok: ok.as_ref().map(|tpe| Box::new(tpe.get_type_hint())),
                err: error.as_ref().map(|tpe| Box::new(tpe.get_type_hint())),
            },
            TypeInternal::Variant(variants) => TypeHint::Variant(Some(
                variants
                    .iter()
                    .map(|(name, tpe)| {
                        (
                            name.to_string(),
                            tpe.as_ref().map(GetTypeHint::get_type_hint),
                        )
                    })
                    .collect(),
            )),
            TypeInternal::Resource { .. } => TypeHint::Resource,
            TypeInternal::AllOf(possibilities) => get_type_kind(possibilities),
            TypeInternal::Unknown | TypeInternal::Sequence(_) | TypeInternal::Instance { .. } => {
                TypeHint::Unknown
            }
            TypeInternal::Range { .. } => TypeHint::Range,
        }
    }
}

fn get_type_kind(possibilities: &[InferredType]) -> TypeHint {
    if let Some(first) = possibilities.first() {
        let first = first.get_type_hint();
        if possibilities.iter().all(|p| p.get_type_hint() == first) {
            first
        } else {
            TypeHint::Ambiguous {
                possibilities: possibilities.iter().map(|p| p.get_type_hint()).collect(),
            }
        }
    } else {
        TypeHint::Unknown
    }
}
