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

use crate::analysis::AnalysisResult;
use crate::component::{ComponentExternalKind, PrimitiveValueType};

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
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
    pub results: Vec<AnalysedFunctionResult>,
}

impl AnalysedFunction {
    pub fn is_constructor(&self) -> bool {
        self.name.starts_with("[constructor]")
            && self.results.len() == 1
            && matches!(
                &self.results[0].typ,
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
    pub cases: Vec<NameOptionTypePair>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeOption {
    pub inner: Box<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeEnum {
    pub cases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeFlags {
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeRecord {
    pub fields: Vec<NameTypePair>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeTuple {
    pub items: Vec<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Object))]
pub struct TypeList {
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

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Enum))]
pub enum AnalysedResourceMode {
    Owned,
    Borrowed,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::NewType))]
pub struct AnalysedResourceId(pub u64);

impl From<&PrimitiveValueType> for AnalysedType {
    fn from(value: &PrimitiveValueType) -> Self {
        match value {
            PrimitiveValueType::Bool => AnalysedType::Bool(TypeBool),
            PrimitiveValueType::S8 => AnalysedType::S8(TypeS8),
            PrimitiveValueType::U8 => AnalysedType::U8(TypeU8),
            PrimitiveValueType::S16 => AnalysedType::S16(TypeS16),
            PrimitiveValueType::U16 => AnalysedType::U16(TypeU16),
            PrimitiveValueType::S32 => AnalysedType::S32(TypeS32),
            PrimitiveValueType::U32 => AnalysedType::U32(TypeU32),
            PrimitiveValueType::S64 => AnalysedType::S64(TypeS64),
            PrimitiveValueType::U64 => AnalysedType::U64(TypeU64),
            PrimitiveValueType::F32 => AnalysedType::F32(TypeF32),
            PrimitiveValueType::F64 => AnalysedType::F64(TypeF64),
            PrimitiveValueType::Chr => AnalysedType::Chr(TypeChr),
            PrimitiveValueType::Str => AnalysedType::Str(TypeStr),
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
    pub name: Option<String>,
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
#[cfg_attr(feature = "json", serde(tag = "type"))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Union))]
#[cfg_attr(
    feature = "poem_openapi",
    oai(discriminator_name = "type", one_of = true)
)]
pub enum AnalysisWarning {
    UnsupportedExport(UnsupportedExportWarning),
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
