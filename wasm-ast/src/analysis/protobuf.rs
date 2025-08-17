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

use crate::analysis::analysed_type::{
    bool, chr, f32, f64, field, flags, handle, list, option, r#enum, record, s16, s32, s64, s8,
    str, tuple, u16, u32, u64, u8, variant,
};
use crate::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType};
use std::ops::Deref;

include!(concat!(env!("OUT_DIR"), "/wasm.ast.rs"));

impl TryFrom<&Type> for AnalysedType {
    type Error = String;

    fn try_from(typ: &Type) -> Result<Self, Self::Error> {
        match &typ.r#type {
            Some(r#type::Type::Primitive(primitive)) => {
                match PrimitiveType::try_from(primitive.primitive) {
                    Ok(PrimitiveType::Bool) => Ok(bool()),
                    Ok(PrimitiveType::S8) => Ok(s8()),
                    Ok(PrimitiveType::U8) => Ok(u8()),
                    Ok(PrimitiveType::S16) => Ok(s16()),
                    Ok(PrimitiveType::U16) => Ok(u16()),
                    Ok(PrimitiveType::S32) => Ok(s32()),
                    Ok(PrimitiveType::U32) => Ok(u32()),
                    Ok(PrimitiveType::S64) => Ok(s64()),
                    Ok(PrimitiveType::U64) => Ok(u64()),
                    Ok(PrimitiveType::F32) => Ok(f32()),
                    Ok(PrimitiveType::F64) => Ok(f64()),
                    Ok(PrimitiveType::Chr) => Ok(chr()),
                    Ok(PrimitiveType::Str) => Ok(str()),
                    Err(_) => Err("Unknown primitive type".to_string()),
                }
            }
            Some(r#type::Type::List(inner)) => {
                let elem_type = inner
                    .elem
                    .as_ref()
                    .ok_or_else(|| "List element type is None".to_string())?;
                let analysed_type = AnalysedType::try_from(elem_type.as_ref())?;
                Ok(list(analysed_type)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
            }
            Some(r#type::Type::Tuple(inner)) => {
                let elems = inner
                    .elems
                    .iter()
                    .map(AnalysedType::try_from)
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(tuple(elems)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
            }
            Some(r#type::Type::Record(inner)) => {
                let fields = inner
                    .fields
                    .iter()
                    .map(|proto_field| {
                        let field_type = proto_field.typ.as_ref().ok_or_else(|| {
                            format!("Record field {} type is None", proto_field.name)
                        })?;
                        let analysed_type = AnalysedType::try_from(field_type)?;
                        Ok(field(&proto_field.name, analysed_type))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(record(fields)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
            }
            Some(r#type::Type::Flags(inner)) => Ok(flags(
                &inner
                    .names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<&str>>(),
            )
            .with_optional_name(inner.name.clone())
            .with_optional_owner(inner.owner.clone())),
            Some(r#type::Type::Enum(inner)) => Ok(r#enum(
                &inner
                    .names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<&str>>(),
            )
            .with_optional_name(inner.name.clone())
            .with_optional_owner(inner.owner.clone())),
            Some(r#type::Type::Option(inner)) => {
                let elem_type = inner
                    .elem
                    .as_ref()
                    .ok_or_else(|| "Option element type is None".to_string())?;
                let analysed_type = AnalysedType::try_from(elem_type.as_ref())?;
                Ok(option(analysed_type)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
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
                    name: inner.name.clone(),
                    owner: inner.owner.clone(),
                })
                .with_optional_name(inner.name.clone())
                .with_optional_owner(inner.owner.clone()))
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
                Ok(variant(cases)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
            }
            Some(r#type::Type::Handle(inner)) => {
                let resource_mode = match inner.mode {
                    0 => Ok(AnalysedResourceMode::Owned),
                    1 => Ok(AnalysedResourceMode::Borrowed),
                    _ => Err("Invalid resource mode".to_string()),
                }?;
                Ok(handle(AnalysedResourceId(inner.resource_id), resource_mode)
                    .with_optional_name(inner.name.clone())
                    .with_optional_owner(inner.owner.clone()))
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
            AnalysedType::List(crate::analysis::TypeList { inner, name, owner }) => {
                Some(r#type::Type::List(Box::new(TypeList {
                    elem: Some(Box::new(Type::from(inner.deref()))),
                    name: name.clone(),
                    owner: owner.clone(),
                })))
            }
            AnalysedType::Tuple(crate::analysis::TypeTuple { items, name, owner }) => {
                Some(r#type::Type::Tuple(TypeTuple {
                    elems: items
                        .iter()
                        .map(|analysed_type| analysed_type.into())
                        .collect(),
                    name: name.clone(),
                    owner: owner.clone(),
                }))
            }
            AnalysedType::Record(crate::analysis::TypeRecord {
                fields,
                name,
                owner,
            }) => Some(r#type::Type::Record(TypeRecord {
                fields: fields
                    .iter()
                    .map(|pair| NameTypePair {
                        name: pair.name.clone(),
                        typ: Some((&pair.typ).into()),
                    })
                    .collect(),
                name: name.clone(),
                owner: owner.clone(),
            })),
            AnalysedType::Flags(crate::analysis::TypeFlags { names, name, owner }) => {
                Some(r#type::Type::Flags(TypeFlags {
                    names: names.clone(),
                    name: name.clone(),
                    owner: owner.clone(),
                }))
            }
            AnalysedType::Enum(crate::analysis::TypeEnum { cases, name, owner }) => {
                Some(r#type::Type::Enum(TypeEnum {
                    names: cases.clone(),
                    name: name.clone(),
                    owner: owner.clone(),
                }))
            }
            AnalysedType::Option(crate::analysis::TypeOption { inner, name, owner }) => {
                Some(r#type::Type::Option(Box::new(TypeOption {
                    elem: Some(Box::new(Type::from(inner.deref()))),
                    name: name.clone(),
                    owner: owner.clone(),
                })))
            }
            AnalysedType::Result(crate::analysis::TypeResult {
                ok,
                err,
                name,
                owner,
            }) => Some(r#type::Type::Result(Box::new(TypeResult {
                ok: ok
                    .clone()
                    .map(|ok_type| Box::new(Type::from(ok_type.as_ref()))),
                err: err
                    .clone()
                    .map(|err_type| Box::new(Type::from(err_type.as_ref()))),
                name: name.clone(),
                owner: owner.clone(),
            }))),
            AnalysedType::Variant(crate::analysis::TypeVariant { cases, name, owner }) => {
                Some(r#type::Type::Variant(TypeVariant {
                    cases: cases
                        .iter()
                        .map(|pair| NameOptionTypePair {
                            name: pair.name.clone(),
                            typ: pair.typ.as_ref().map(|analysed_type| analysed_type.into()),
                        })
                        .collect(),
                    name: name.clone(),
                    owner: owner.clone(),
                }))
            }
            AnalysedType::Handle(crate::analysis::TypeHandle {
                resource_id,
                mode,
                name,
                owner,
            }) => Some(r#type::Type::Handle(TypeHandle {
                resource_id: resource_id.0,
                mode: match mode {
                    AnalysedResourceMode::Owned => 0,
                    AnalysedResourceMode::Borrowed => 1,
                },
                name: name.clone(),
                owner: owner.clone(),
            })),
        };

        Type { r#type }
    }
}
