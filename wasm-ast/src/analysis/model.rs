// Copyright 2024-2025 Golem Cloud
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

use crate::analysis::analysed_type::{
    bool, chr, f32, f64, s16, s32, s64, s8, str, u16, u32, u64, u8,
};
use crate::analysis::AnalysisResult;
use crate::component::{ComponentExternalKind, PrimitiveValueType};

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
            inner: Box::new(inner),
        })
    }

    pub fn option(inner: AnalysedType) -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(inner),
        })
    }

    pub fn flags(names: &[&str]) -> AnalysedType {
        AnalysedType::Flags(TypeFlags {
            names: names.iter().map(|n| n.to_string()).collect(),
        })
    }

    pub fn r#enum(cases: &[&str]) -> AnalysedType {
        AnalysedType::Enum(TypeEnum {
            cases: cases.iter().map(|n| n.to_string()).collect(),
        })
    }

    pub fn tuple(items: Vec<AnalysedType>) -> AnalysedType {
        AnalysedType::Tuple(TypeTuple { items })
    }

    pub fn result(ok: AnalysedType, err: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(ok)),
            err: Some(Box::new(err)),
        })
    }

    pub fn result_ok(ok: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(ok)),
            err: None,
        })
    }

    pub fn result_err(err: AnalysedType) -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: None,
            err: Some(Box::new(err)),
        })
    }

    pub fn record(fields: Vec<NameTypePair>) -> AnalysedType {
        AnalysedType::Record(TypeRecord { fields })
    }

    pub fn variant(cases: Vec<NameOptionTypePair>) -> AnalysedType {
        AnalysedType::Variant(TypeVariant { cases })
    }

    pub fn handle(resource_id: AnalysedResourceId, mode: AnalysedResourceMode) -> AnalysedType {
        AnalysedType::Handle(TypeHandle { resource_id, mode })
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

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
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

#[cfg(test)]
mod tests {

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
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: list(str()),
                }],
            }],
        });
        let poem_serialized = export1.to_json_string();
        let serde_deserialized: AnalysedExport = serde_json::from_str(&poem_serialized).unwrap();

        assert_eq!(export1, serde_deserialized);
    }
}
