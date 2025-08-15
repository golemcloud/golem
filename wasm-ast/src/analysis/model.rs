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
    bool, chr, f32, f64, s16, s32, s64, s8, str, u16, u32, u64, u8,
};
use crate::analysis::AnalysisResult;
use crate::component::{ComponentExternalKind, PrimitiveValueType};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(tag = "type"))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Union))]
#[cfg_attr(
    feature = "poem_openapi",
    oai(discriminator_name = "type", one_of = true)
)]
pub enum AnalysedExport {
    Function(AnalysedFunction),
    Instance(AnalysedInstance),
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct AnalysedFunction {
    pub name: String,
    pub parameters: Vec<AnalysedFunctionParameter>,
    pub result: Option<AnalysedFunctionResult>,
}

impl AnalysedFunction {
    pub fn is_constructor(&self) -> bool {
        self.name.starts_with("[constructor]")
            && self.result.is_some()
            && matches!(
                &self.result.as_ref().unwrap().typ,
                AnalysedType::Handle(TypeHandle {
                    mode: AnalysedResourceMode::Owned,
                    ..
                })
            )
    }

    pub fn is_method(&self) -> bool {
        self.name.starts_with("[method]")
            && !self.parameters.is_empty()
            && matches!(
                &self.parameters[0].typ,
                AnalysedType::Handle(TypeHandle {
                    mode: AnalysedResourceMode::Borrowed,
                    ..
                })
            )
    }

    pub fn is_static_method(&self) -> bool {
        self.name.starts_with("[static]")
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct AnalysedInstance {
    pub name: String,
    pub functions: Vec<AnalysedFunction>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub ok: Option<Box<AnalysedType>>,
    pub err: Option<Box<AnalysedType>>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct NameTypePair {
    pub name: String,
    pub typ: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct NameOptionTypePair {
    pub name: String,
    pub typ: Option<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeVariant {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub cases: Vec<NameOptionTypePair>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeOption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub inner: Box<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeEnum {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub cases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeFlags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub fields: Vec<NameTypePair>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeTuple {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub items: Vec<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeList {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub inner: Box<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeStr;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeChr;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeF64;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeF32;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeU64;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeS64;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeU32;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeS32;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeU16;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeS16;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeU8;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeS8;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeBool;

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeHandle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub resource_id: AnalysedResourceId,
    pub mode: AnalysedResourceMode,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(tag = "type"))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Union))]
#[cfg_attr(
    feature = "poem_openapi",
    oai(discriminator_name = "type", one_of = true)
)]
pub enum AnalysedType {
    Variant(TypeVariant),
    Result(TypeResult),
    Option(TypeOption),
    Enum(TypeEnum),
    Flags(TypeFlags),
    Record(TypeRecord),
    Tuple(TypeTuple),
    List(TypeList),
    Str(TypeStr),
    Chr(TypeChr),
    F64(TypeF64),
    F32(TypeF32),
    U64(TypeU64),
    S64(TypeS64),
    U32(TypeU32),
    S32(TypeS32),
    U16(TypeU16),
    S16(TypeS16),
    U8(TypeU8),
    S8(TypeS8),
    Bool(TypeBool),
    Handle(TypeHandle),
}

impl AnalysedType {
    pub fn name(&self) -> Option<&str> {
        match self {
            AnalysedType::Variant(typ) => typ.name.as_deref(),
            AnalysedType::Result(typ) => typ.name.as_deref(),
            AnalysedType::Option(typ) => typ.name.as_deref(),
            AnalysedType::Enum(typ) => typ.name.as_deref(),
            AnalysedType::Flags(typ) => typ.name.as_deref(),
            AnalysedType::Record(typ) => typ.name.as_deref(),
            AnalysedType::Tuple(typ) => typ.name.as_deref(),
            AnalysedType::List(typ) => typ.name.as_deref(),
            AnalysedType::Handle(typ) => typ.name.as_deref(),
            _ => None,
        }
    }

    pub fn with_optional_name(self, name: Option<String>) -> Self {
        match self {
            AnalysedType::Variant(mut typ) => {
                typ.name = name;
                AnalysedType::Variant(typ)
            }
            AnalysedType::Result(mut typ) => {
                typ.name = name;
                AnalysedType::Result(typ)
            }
            AnalysedType::Option(mut typ) => {
                typ.name = name;
                AnalysedType::Option(typ)
            }
            AnalysedType::Enum(mut typ) => {
                typ.name = name;
                AnalysedType::Enum(typ)
            }
            AnalysedType::Flags(mut typ) => {
                typ.name = name;
                AnalysedType::Flags(typ)
            }
            AnalysedType::Record(mut typ) => {
                typ.name = name;
                AnalysedType::Record(typ)
            }
            AnalysedType::Tuple(mut typ) => {
                typ.name = name;
                AnalysedType::Tuple(typ)
            }
            AnalysedType::List(mut typ) => {
                typ.name = name;
                AnalysedType::List(typ)
            }
            AnalysedType::Handle(mut typ) => {
                typ.name = name;
                AnalysedType::Handle(typ)
            }
            _ => self,
        }
    }

    pub fn named(self, name: impl AsRef<str>) -> Self {
        self.with_optional_name(Some(name.as_ref().to_string()))
    }

    pub fn owner(&self) -> Option<&str> {
        match self {
            AnalysedType::Variant(typ) => typ.owner.as_deref(),
            AnalysedType::Result(typ) => typ.owner.as_deref(),
            AnalysedType::Option(typ) => typ.owner.as_deref(),
            AnalysedType::Enum(typ) => typ.owner.as_deref(),
            AnalysedType::Flags(typ) => typ.owner.as_deref(),
            AnalysedType::Record(typ) => typ.owner.as_deref(),
            AnalysedType::Tuple(typ) => typ.owner.as_deref(),
            AnalysedType::List(typ) => typ.owner.as_deref(),
            AnalysedType::Handle(typ) => typ.owner.as_deref(),
            _ => None,
        }
    }

    pub fn with_optional_owner(self, owner: Option<String>) -> Self {
        match self {
            AnalysedType::Variant(mut typ) => {
                typ.owner = owner;
                AnalysedType::Variant(typ)
            }
            AnalysedType::Result(mut typ) => {
                typ.owner = owner;
                AnalysedType::Result(typ)
            }
            AnalysedType::Option(mut typ) => {
                typ.owner = owner;
                AnalysedType::Option(typ)
            }
            AnalysedType::Enum(mut typ) => {
                typ.owner = owner;
                AnalysedType::Enum(typ)
            }
            AnalysedType::Flags(mut typ) => {
                typ.owner = owner;
                AnalysedType::Flags(typ)
            }
            AnalysedType::Record(mut typ) => {
                typ.owner = owner;
                AnalysedType::Record(typ)
            }
            AnalysedType::Tuple(mut typ) => {
                typ.owner = owner;
                AnalysedType::Tuple(typ)
            }
            AnalysedType::List(mut typ) => {
                typ.owner = owner;
                AnalysedType::List(typ)
            }
            AnalysedType::Handle(mut typ) => {
                typ.owner = owner;
                AnalysedType::Handle(typ)
            }
            _ => self,
        }
    }

    pub fn owned(self, owner: impl AsRef<str>) -> Self {
        self.with_optional_owner(Some(owner.as_ref().to_string()))
    }

    pub fn contains_handle(&self) -> bool {
        match self {
            AnalysedType::Handle(_) => true,
            AnalysedType::Variant(typ) => typ
                .cases
                .iter()
                .any(|case| case.typ.as_ref().is_some_and(|t| t.contains_handle())),
            AnalysedType::Result(typ) => {
                typ.ok.as_ref().is_some_and(|t| t.contains_handle())
                    || typ.err.as_ref().is_some_and(|t| t.contains_handle())
            }
            AnalysedType::Option(typ) => typ.inner.contains_handle(),
            AnalysedType::Record(typ) => typ.fields.iter().any(|f| f.typ.contains_handle()),
            AnalysedType::Tuple(typ) => typ.items.iter().any(|t| t.contains_handle()),
            AnalysedType::List(typ) => typ.inner.contains_handle(),
            _ => false,
        }
    }
}

pub mod analysed_type {
    use crate::analysis::*;

    pub fn field(name: &str, typ: AnalysedType) -> NameTypePair {
        NameTypePair {
            name: name.to_string(),
            typ,
        }
    }

    pub fn case(name: &str, typ: AnalysedType) -> NameOptionTypePair {
        NameOptionTypePair {
            name: name.to_string(),
            typ: Some(typ),
        }
    }

    pub fn opt_case(name: &str, typ: Option<AnalysedType>) -> NameOptionTypePair {
        NameOptionTypePair {
            name: name.to_string(),
            typ,
        }
    }

    pub fn unit_case(name: &str) -> NameOptionTypePair {
        NameOptionTypePair {
            name: name.to_string(),
            typ: None,
        }
    }

    pub fn bool() -> AnalysedType {
        AnalysedType::Bool(TypeBool)
    }

    pub fn s8() -> AnalysedType {
        AnalysedType::S8(TypeS8)
    }

    pub fn s16() -> AnalysedType {
        AnalysedType::S16(TypeS16)
    }

    pub fn s32() -> AnalysedType {
        AnalysedType::S32(TypeS32)
    }

    pub fn s64() -> AnalysedType {
        AnalysedType::S64(TypeS64)
    }

    pub fn u8() -> AnalysedType {
        AnalysedType::U8(TypeU8)
    }

    pub fn u16() -> AnalysedType {
        AnalysedType::U16(TypeU16)
    }

    pub fn u32() -> AnalysedType {
        AnalysedType::U32(TypeU32)
    }

    pub fn u64() -> AnalysedType {
        AnalysedType::U64(TypeU64)
    }

    pub fn f32() -> AnalysedType {
        AnalysedType::F32(TypeF32)
    }

    pub fn f64() -> AnalysedType {
        AnalysedType::F64(TypeF64)
    }

    pub fn chr() -> AnalysedType {
        AnalysedType::Chr(TypeChr)
    }

    pub fn str() -> AnalysedType {
        AnalysedType::Str(TypeStr)
    }

    pub fn list(inner: AnalysedType) -> AnalysedType {
        AnalysedType::List(TypeList {
            name: None,
            owner: None,
            inner: Box::new(inner),
        })
    }

    pub fn option(inner: AnalysedType) -> AnalysedType {
        AnalysedType::Option(TypeOption {
            name: None,
            owner: None,
            inner: Box::new(inner),
        })
    }

    pub fn flags(names: &[&str]) -> AnalysedType {
        AnalysedType::Flags(TypeFlags {
            name: None,
            owner: None,
            names: names.iter().map(|n| n.to_string()).collect(),
        })
    }

    pub fn r#enum(cases: &[&str]) -> AnalysedType {
        AnalysedType::Enum(TypeEnum {
            name: None,
            owner: None,
            cases: cases.iter().map(|n| n.to_string()).collect(),
        })
    }

    pub fn tuple(items: Vec<AnalysedType>) -> AnalysedType {
        AnalysedType::Tuple(TypeTuple {
            name: None,
            owner: None,
            items,
        })
    }

    pub fn result(ok: AnalysedType, err: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            name: None,
            owner: None,
            ok: Some(Box::new(ok)),
            err: Some(Box::new(err)),
        })
    }

    pub fn result_ok(ok: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            name: None,
            owner: None,
            ok: Some(Box::new(ok)),
            err: None,
        })
    }

    pub fn result_err(err: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            name: None,
            owner: None,
            ok: None,
            err: Some(Box::new(err)),
        })
    }

    pub fn record(fields: Vec<NameTypePair>) -> AnalysedType {
        AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
            fields,
        })
    }

    pub fn variant(cases: Vec<NameOptionTypePair>) -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            name: None,
            owner: None,
            cases,
        })
    }

    pub fn handle(resource_id: AnalysedResourceId, mode: AnalysedResourceMode) -> AnalysedType {
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id,
            mode,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Enum))]
pub enum AnalysedResourceMode {
    Owned,
    Borrowed,
}

#[derive(Debug, Copy, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::NewType))]
pub struct AnalysedResourceId(pub u64);

impl From<&PrimitiveValueType> for AnalysedType {
    fn from(value: &PrimitiveValueType) -> Self {
        match value {
            PrimitiveValueType::Bool => bool(),
            PrimitiveValueType::S8 => s8(),
            PrimitiveValueType::U8 => u8(),
            PrimitiveValueType::S16 => s16(),
            PrimitiveValueType::U16 => u16(),
            PrimitiveValueType::S32 => s32(),
            PrimitiveValueType::U32 => u32(),
            PrimitiveValueType::S64 => s64(),
            PrimitiveValueType::U64 => u64(),
            PrimitiveValueType::F32 => f32(),
            PrimitiveValueType::F64 => f64(),
            PrimitiveValueType::Chr => chr(),
            PrimitiveValueType::Str => str(),
            PrimitiveValueType::ErrorContext => panic!("ErrorContext is not supported yet"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct AnalysedFunctionParameter {
    pub name: String,
    pub typ: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct AnalysedFunctionResult {
    pub typ: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct UnsupportedExportWarning {
    pub kind: ComponentExternalKind,
    pub name: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct InterfaceCouldNotBeAnalyzedWarning {
    pub name: String,
    pub failure: AnalysisFailure,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(tag = "type"))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Union))]
#[cfg_attr(
    feature = "poem_openapi",
    oai(discriminator_name = "type", one_of = true)
)]
pub enum AnalysisWarning {
    UnsupportedExport(UnsupportedExportWarning),
    InterfaceCouldNotBeAnalyzed(InterfaceCouldNotBeAnalyzedWarning),
}

impl Display for AnalysisWarning {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisWarning::UnsupportedExport(warning) => {
                write!(f, "Unsupported export: {:?} {}", warning.kind, warning.name)
            }
            AnalysisWarning::InterfaceCouldNotBeAnalyzed(warning) => {
                write!(
                    f,
                    "Interface could not be analyzed: {} {}",
                    warning.name, warning.failure.reason
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct AnalysisFailure {
    pub reason: String,
}

impl AnalysisFailure {
    pub fn failed(message: impl Into<String>) -> AnalysisFailure {
        AnalysisFailure {
            reason: message.into(),
        }
    }

    pub fn fail_on_missing<T>(value: Option<T>, description: impl AsRef<str>) -> AnalysisResult<T> {
        match value {
            Some(value) => Ok(value),
            None => Err(AnalysisFailure::failed(format!(
                "Missing {}",
                description.as_ref()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::analysed_type::{bool, list, str};
    use crate::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance,
    };
    use poem_openapi::types::ToJSON;
    use test_r::test;

    #[cfg(feature = "poem_openapi")]
    #[cfg(feature = "json")]
    #[test]
    fn analysed_export_poem_and_serde_are_compatible() {
        let export1 = AnalysedExport::Instance(AnalysedInstance {
            name: "inst1".to_string(),
            functions: vec![AnalysedFunction {
                name: "func1".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "param1".to_string(),
                    typ: bool(),
                }],
                result: Some(AnalysedFunctionResult { typ: list(str()) }),
            }],
        });
        let poem_serialized = export1.to_json_string();
        let serde_deserialized: AnalysedExport = serde_json::from_str(&poem_serialized).unwrap();

        assert_eq!(export1, serde_deserialized);
    }
}
