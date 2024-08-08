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

use crate::analysis::{
    AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeBool, TypeChr, TypeF32, TypeF64,
    TypeS16, TypeS32, TypeS64, TypeS8, TypeStr, TypeU16, TypeU32, TypeU64, TypeU8,
};
use std::ops::Deref;

include!(concat!(env!("OUT_DIR"), "/wasm.ast.rs"));

impl TryFrom<&Type> for AnalysedType {
    type Error = String;

    fn try_from(typ: &Type) -> Result<Self, Self::Error> {
        match &typ.r#type {
            Some(r#type::Type::Primitive(primitive)) => {
                match PrimitiveType::try_from(primitive.primitive) {
                    Ok(PrimitiveType::Bool) => Ok(AnalysedType::Bool(TypeBool)),
                    Ok(PrimitiveType::S8) => Ok(AnalysedType::S8(TypeS8)),
                    Ok(PrimitiveType::U8) => Ok(AnalysedType::U8(TypeU8)),
                    Ok(PrimitiveType::S16) => Ok(AnalysedType::S16(TypeS16)),
                    Ok(PrimitiveType::U16) => Ok(AnalysedType::U16(TypeU16)),
                    Ok(PrimitiveType::S32) => Ok(AnalysedType::S32(TypeS32)),
                    Ok(PrimitiveType::U32) => Ok(AnalysedType::U32(TypeU32)),
                    Ok(PrimitiveType::S64) => Ok(AnalysedType::S64(TypeS64)),
                    Ok(PrimitiveType::U64) => Ok(AnalysedType::U64(TypeU64)),
                    Ok(PrimitiveType::F32) => Ok(AnalysedType::F32(TypeF32)),
                    Ok(PrimitiveType::F64) => Ok(AnalysedType::F64(TypeF64)),
                    Ok(PrimitiveType::Chr) => Ok(AnalysedType::Chr(TypeChr)),
                    Ok(PrimitiveType::Str) => Ok(AnalysedType::Str(TypeStr)),
                    Err(_) => Err("Unknown primitive type".to_string()),
                }
            }
            Some(r#type::Type::List(inner)) => {
                let elem_type = inner
                    .elem
                    .as_ref()
                    .ok_or_else(|| "List element type is None".to_string())?;
                let analysed_type = AnalysedType::try_from(elem_type.as_ref())?;
                Ok(AnalysedType::List(crate::analysis::TypeList {
                    inner: Box::new(analysed_type),
                }))
            }
            Some(r#type::Type::Tuple(inner)) => {
                let elems = inner
                    .elems
                    .iter()
                    .map(AnalysedType::try_from)
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(AnalysedType::Tuple(crate::analysis::TypeTuple {
                    items: elems,
                }))
            }
            Some(r#type::Type::Record(inner)) => {
                let fields = inner
                    .fields
                    .iter()
                    .map(|field| {
                        let field_type = field
                            .typ
                            .as_ref()
                            .ok_or_else(|| format!("Record field {} type is None", field.name))?;
                        let analysed_type = AnalysedType::try_from(field_type)?;
                        Ok(crate::analysis::NameTypePair {
                            name: field.name.clone(),
                            typ: analysed_type,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(AnalysedType::Record(crate::analysis::TypeRecord { fields }))
            }
            Some(r#type::Type::Flags(inner)) => {
                Ok(AnalysedType::Flags(crate::analysis::TypeFlags {
                    names: inner.names.clone(),
                }))
            }
            Some(r#type::Type::Enum(inner)) => Ok(AnalysedType::Enum(crate::analysis::TypeEnum {
                cases: inner.names.clone(),
            })),
            Some(r#type::Type::Option(inner)) => {
                let elem_type = inner
                    .elem
                    .as_ref()
                    .ok_or_else(|| "Option element type is None".to_string())?;
                let analysed_type = AnalysedType::try_from(elem_type.as_ref())?;
                Ok(AnalysedType::Option(crate::analysis::TypeOption {
                    inner: Box::new(analysed_type),
                }))
            }
            Some(r#type::Type::Result(inner)) => {
                let ok_type = inner
                    .ok
                    .as_ref()
                    .map(|tpe| AnalysedType::try_from(tpe.as_ref()))
                    .transpose()?;
                let err_type = inner
                    .err
                    .as_ref()
                    .map(|tpe| AnalysedType::try_from(tpe.as_ref()))
                    .transpose()?;
                Ok(AnalysedType::Result(crate::analysis::TypeResult {
                    ok: ok_type.map(Box::new),
                    err: err_type.map(Box::new),
                }))
            }
            Some(r#type::Type::Variant(inner)) => {
                let cases = inner
                    .cases
                    .iter()
                    .map(|case| {
                        let case_type =
                            case.typ.as_ref().map(AnalysedType::try_from).transpose()?;
                        Ok(crate::analysis::NameOptionTypePair {
                            name: case.name.clone(),
                            typ: case_type,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(AnalysedType::Variant(crate::analysis::TypeVariant {
                    cases,
                }))
            }
            Some(r#type::Type::Handle(inner)) => {
                let resource_mode = match inner.mode {
                    0 => Ok(AnalysedResourceMode::Owned),
                    1 => Ok(AnalysedResourceMode::Borrowed),
                    _ => Err("Invalid resource mode".to_string()),
                }?;
                Ok(AnalysedType::Handle(crate::analysis::TypeHandle {
                    resource_id: AnalysedResourceId(inner.resource_id),
                    mode: resource_mode,
                }))
            }
            None => Err("Type is None".to_string()),
        }
    }
}

impl From<&AnalysedType> for Type {
    fn from(value: &AnalysedType) -> Type {
        let r#type = match value {
            AnalysedType::Bool(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::Bool as i32,
            })),
            AnalysedType::S8(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S8 as i32,
            })),
            AnalysedType::U8(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U8 as i32,
            })),
            AnalysedType::S16(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S16 as i32,
            })),
            AnalysedType::U16(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U16 as i32,
            })),
            AnalysedType::S32(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S32 as i32,
            })),
            AnalysedType::U32(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U32 as i32,
            })),
            AnalysedType::S64(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S64 as i32,
            })),
            AnalysedType::U64(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U64 as i32,
            })),
            AnalysedType::F32(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::F32 as i32,
            })),
            AnalysedType::F64(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::F64 as i32,
            })),
            AnalysedType::Chr(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::Chr as i32,
            })),
            AnalysedType::Str(_) => Some(r#type::Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::Str as i32,
            })),
            AnalysedType::List(crate::analysis::TypeList { inner }) => {
                Some(r#type::Type::List(Box::new(TypeList {
                    elem: Some(Box::new(Type::from(inner.deref()))),
                })))
            }
            AnalysedType::Tuple(crate::analysis::TypeTuple { items }) => {
                Some(r#type::Type::Tuple(TypeTuple {
                    elems: items
                        .iter()
                        .map(|analysed_type| analysed_type.into())
                        .collect(),
                }))
            }
            AnalysedType::Record(crate::analysis::TypeRecord { fields }) => {
                Some(r#type::Type::Record(TypeRecord {
                    fields: fields
                        .iter()
                        .map(|pair| NameTypePair {
                            name: pair.name.clone(),
                            typ: Some((&pair.typ).into()),
                        })
                        .collect(),
                }))
            }
            AnalysedType::Flags(crate::analysis::TypeFlags { names }) => {
                Some(r#type::Type::Flags(TypeFlags {
                    names: names.clone(),
                }))
            }
            AnalysedType::Enum(crate::analysis::TypeEnum { cases }) => {
                Some(r#type::Type::Enum(TypeEnum {
                    names: cases.clone(),
                }))
            }
            AnalysedType::Option(crate::analysis::TypeOption { inner }) => {
                Some(r#type::Type::Option(Box::new(TypeOption {
                    elem: Some(Box::new(Type::from(inner.deref()))),
                })))
            }
            AnalysedType::Result(crate::analysis::TypeResult { ok, err }) => {
                Some(r#type::Type::Result(Box::new(TypeResult {
                    ok: ok
                        .clone()
                        .map(|ok_type| Box::new(Type::from(ok_type.as_ref()))),
                    err: err
                        .clone()
                        .map(|err_type| Box::new(Type::from(err_type.as_ref()))),
                })))
            }
            AnalysedType::Variant(crate::analysis::TypeVariant { cases }) => {
                Some(r#type::Type::Variant(TypeVariant {
                    cases: cases
                        .iter()
                        .map(|pair| NameOptionTypePair {
                            name: pair.name.clone(),
                            typ: pair.typ.as_ref().map(|analysed_type| analysed_type.into()),
                        })
                        .collect(),
                }))
            }
            AnalysedType::Handle(crate::analysis::TypeHandle { resource_id, mode }) => {
                Some(r#type::Type::Handle(TypeHandle {
                    resource_id: resource_id.0,
                    mode: match mode {
                        AnalysedResourceMode::Owned => 0,
                        AnalysedResourceMode::Borrowed => 1,
                    },
                }))
            }
        };

        Type { r#type }
    }
}
