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

use crate::{GetTypeHint, InferredType, InstanceType, TypeInternal};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::analysed_type::{bool, field, record, str, tuple};
use golem_wasm_ast::analysis::{
    AnalysedResourceId, AnalysedResourceMode, AnalysedType, NameOptionTypePair, NameTypePair,
    TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64, TypeFlags, TypeHandle, TypeList, TypeOption,
    TypeRecord, TypeResult, TypeS16, TypeS32, TypeS64, TypeS8, TypeStr, TypeTuple, TypeU16,
    TypeU32, TypeU64, TypeU8, TypeVariant,
};
use serde::{Deserialize, Serialize};

// An absence of analysed type is really `Unit`, however, we avoid
// Option<AnalysedType> in favor of `AnalysedTypeWithUnit` for clarity.
// and conversions such as what to print if its `unit` becomes more precise
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum AnalysedTypeWithUnit {
    Unit,
    Type(AnalysedType),
}

impl AnalysedTypeWithUnit {
    pub fn unit() -> Self {
        AnalysedTypeWithUnit::Unit
    }

    pub fn analysed_type(typ: AnalysedType) -> Self {
        AnalysedTypeWithUnit::Type(typ)
    }
}

impl TryFrom<AnalysedTypeWithUnit> for AnalysedType {
    type Error = String;

    fn try_from(value: AnalysedTypeWithUnit) -> Result<Self, Self::Error> {
        match value {
            AnalysedTypeWithUnit::Unit => Ok(tuple(vec![])),
            AnalysedTypeWithUnit::Type(typ) => Ok(typ),
        }
    }
}

impl TryFrom<&InferredType> for AnalysedType {
    type Error = String;

    fn try_from(value: &InferredType) -> Result<Self, Self::Error> {
        let with_unit = AnalysedTypeWithUnit::try_from(value)?;
        AnalysedType::try_from(with_unit)
    }
}

impl TryFrom<&InferredType> for AnalysedTypeWithUnit {
    type Error = String;

    fn try_from(inferred_type: &InferredType) -> Result<Self, Self::Error> {
        match inferred_type.internal_type() {
            TypeInternal::Instance { instance_type } => match instance_type.as_ref() {
                InstanceType::Resource {
                    analysed_resource_id,
                    analysed_resource_mode,
                    ..
                } => {
                    let analysed_resource_id = AnalysedResourceId(*analysed_resource_id);

                    let analysed_resource_mode = if *analysed_resource_mode == 0 {
                        AnalysedResourceMode::Owned
                    } else {
                        AnalysedResourceMode::Borrowed
                    };

                    Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::Handle(
                        TypeHandle {
                            resource_id: analysed_resource_id,
                            mode: analysed_resource_mode,
                            name: None,
                            owner: None,
                        },
                    )))
                }

                _ => Ok(AnalysedTypeWithUnit::analysed_type(str())),
            },
            TypeInternal::Range { from, to } => {
                let from: AnalysedType = AnalysedType::try_from(from)?;
                let to: Option<AnalysedType> =
                    to.as_ref().map(AnalysedType::try_from).transpose()?;
                let analysed_type = match (from, to) {
                    (from_type, Some(to_type)) => record(vec![
                        field("from", from_type),
                        field("to", to_type),
                        field("inclusive", bool()),
                    ]),

                    (from_type, None) => {
                        record(vec![field("from", from_type), field("inclusive", bool())])
                    }
                };
                Ok(AnalysedTypeWithUnit::analysed_type(analysed_type))
            }
            TypeInternal::Bool => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::Bool(
                TypeBool,
            ))),
            TypeInternal::S8 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::S8(
                TypeS8,
            ))),
            TypeInternal::U8 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::U8(
                TypeU8,
            ))),
            TypeInternal::S16 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::S16(
                TypeS16,
            ))),
            TypeInternal::U16 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::U16(
                TypeU16,
            ))),
            TypeInternal::S32 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::S32(
                TypeS32,
            ))),
            TypeInternal::U32 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::U32(
                TypeU32,
            ))),
            TypeInternal::S64 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::S64(
                TypeS64,
            ))),
            TypeInternal::U64 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::U64(
                TypeU64,
            ))),
            TypeInternal::F32 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::F32(
                TypeF32,
            ))),
            TypeInternal::F64 => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::F64(
                TypeF64,
            ))),
            TypeInternal::Chr => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::Chr(
                TypeChr,
            ))),
            TypeInternal::Str => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::Str(
                TypeStr,
            ))),
            TypeInternal::List(inferred_type) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::List(TypeList {
                    inner: Box::new(inferred_type.try_into()?),
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Tuple(tuple) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Tuple(TypeTuple {
                    items: tuple
                        .iter()
                        .map(|t| t.try_into())
                        .collect::<Result<Vec<AnalysedType>, String>>()?,
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Record(record) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Record(TypeRecord {
                    fields: record
                        .iter()
                        .map(|(name, typ)| {
                            Ok(NameTypePair {
                                name: name.to_string(),
                                typ: typ.try_into()?,
                            })
                        })
                        .collect::<Result<Vec<NameTypePair>, String>>()?,
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Flags(flags) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Flags(TypeFlags {
                    names: flags.clone(),
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Enum(enums) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Enum(TypeEnum {
                    cases: enums.clone(),
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Option(option) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Option(TypeOption {
                    inner: Box::new(option.try_into()?),
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Result { ok, error } => Ok(AnalysedTypeWithUnit::analysed_type(
                // In the case of result, there are instances users give just 1 value with zero function calls, we need to be flexible here
                AnalysedType::Result(TypeResult {
                    ok: ok.as_ref().and_then(|t| t.try_into().ok().map(Box::new)),
                    err: error.as_ref().and_then(|t| t.try_into().ok().map(Box::new)),
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Variant(variant) => Ok(AnalysedTypeWithUnit::analysed_type(
                AnalysedType::Variant(TypeVariant {
                    cases: variant
                        .iter()
                        .map(|(name, typ)| {
                            Ok(NameOptionTypePair {
                                name: name.clone(),
                                typ: typ.as_ref().map(|t| t.try_into()).transpose()?,
                            })
                        })
                        .collect::<Result<Vec<NameOptionTypePair>, String>>()?,
                    name: None,
                    owner: None,
                }),
            )),
            TypeInternal::Resource {
                resource_id,
                resource_mode,
                name: _,
                owner: _,
            } => Ok(AnalysedTypeWithUnit::analysed_type(AnalysedType::Handle(
                TypeHandle {
                    resource_id: AnalysedResourceId(*resource_id),
                    mode: if resource_mode == &0 {
                        AnalysedResourceMode::Owned
                    } else {
                        AnalysedResourceMode::Borrowed
                    },
                    name: None,
                    owner: None,
                },
            ))),

            TypeInternal::AllOf(types) => Err(format!(
                "ambiguous types {}",
                types
                    .iter()
                    .map(|x| x.get_type_hint().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            TypeInternal::Unknown => Err("failed to infer type".to_string()),
            // We don't expect to have a sequence type in the inferred type.as
            // This implies Rib will not support multiple types from worker-function results
            TypeInternal::Sequence(vec) => {
                if vec.is_empty() {
                    Ok(AnalysedTypeWithUnit::unit())
                } else if vec.len() == 1 {
                    let first = &vec[0];
                    Ok(first.try_into()?)
                } else {
                    Err("function with multiple return types not supported".to_string())
                }
            }
        }
    }
}
