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

use golem_common::model::{
    ComponentId, ComponentVersion, ScanCursor, ShardId, Timestamp, WorkerFilter, WorkerStatus,
};
use golem_wasm_ast::analysis::{AnalysedResourceId, AnalysedResourceMode};
use poem_openapi::{Enum, NewType, Object, Union};
use rib::ParsedFunctionName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::HashMap, fmt::Display, fmt::Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkerCreationRequest {
    pub name: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerCreationResponse {
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, NewType)]
pub struct ComponentName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct VersionedComponentId {
    pub component_id: ComponentId,
    pub version: ComponentVersion,
}

impl VersionedComponentId {
    pub fn slug(&self) -> String {
        format!("{}#{}", self.component_id.0, self.version)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
    for VersionedComponentId
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::VersionedComponentId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing component_id")?
                .try_into()?,
            version: value.version,
        })
    }
}

impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
    fn from(value: VersionedComponentId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            version: value.version,
        }
    }
}

impl std::fmt::Display for VersionedComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.component_id, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UserComponentId {
    pub versioned_component_id: VersionedComponentId,
}

impl TryFrom<golem_api_grpc::proto::golem::component::UserComponentId> for UserComponentId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::UserComponentId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
        })
    }
}

impl From<UserComponentId> for golem_api_grpc::proto::golem::component::UserComponentId {
    fn from(value: UserComponentId) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
        }
    }
}

impl UserComponentId {
    pub fn slug(&self) -> String {
        format!("{}:user", self.versioned_component_id.slug())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProtectedComponentId {
    pub versioned_component_id: VersionedComponentId,
}

impl ProtectedComponentId {
    pub fn slug(&self) -> String {
        format!("{}:protected", self.versioned_component_id.slug())
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ProtectedComponentId>
    for ProtectedComponentId
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ProtectedComponentId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
        })
    }
}

impl From<ProtectedComponentId> for golem_api_grpc::proto::golem::component::ProtectedComponentId {
    fn from(value: ProtectedComponentId) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeResult {
    pub ok: Option<Box<Type>>,
    pub err: Option<Box<Type>>,
}

impl<'de> Deserialize<'de> for TypeResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (ok, err) = <(Option<Type>, Option<Type>)>::deserialize(deserializer)?;

        Ok(Self {
            ok: ok.map(Box::new),
            err: err.map(Box::new),
        })
    }
}

impl Serialize for TypeResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ok: Option<Type> = self.ok.clone().map(|t| *t);
        let err: Option<Type> = self.err.clone().map(|t| *t);
        let pair: (Option<Type>, Option<Type>) = (ok, err);
        <(Option<Type>, Option<Type>)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct NameTypePair {
    pub name: String,
    pub typ: Box<Type>,
}

impl<'de> Deserialize<'de> for NameTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, Type)>::deserialize(deserializer)?;

        Ok(Self {
            name,
            typ: Box::new(typ),
        })
    }
}

impl Serialize for NameTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let pair: (String, Type) = (self.name.clone(), *self.typ.clone());
        <(String, Type)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct NameOptionTypePair {
    pub name: String,
    pub typ: Option<Box<Type>>,
}

impl<'de> Deserialize<'de> for NameOptionTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, Option<Type>)>::deserialize(deserializer)?;

        Ok(Self {
            name,
            typ: typ.map(Box::new),
        })
    }
}

impl Serialize for NameOptionTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let typ: Option<Type> = self.typ.clone().map(|t| *t);
        let pair: (String, Option<Type>) = (self.name.clone(), typ);
        <(String, Option<Type>)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeVariant {
    pub cases: Vec<NameOptionTypePair>,
}

impl<'de> Deserialize<'de> for TypeVariant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<NameOptionTypePair>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeVariant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<NameOptionTypePair>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeOption {
    pub inner: Box<Type>,
}

impl<'de> Deserialize<'de> for TypeOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = Type::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeOption {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Type::serialize(&self.inner, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeEnum {
    pub cases: Vec<String>,
}

impl<'de> Deserialize<'de> for TypeEnum {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeEnum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<String>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeFlags {
    pub cases: Vec<String>,
}

impl<'de> Deserialize<'de> for TypeFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<String>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeRecord {
    pub cases: Vec<NameTypePair>,
}

impl<'de> Deserialize<'de> for TypeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<NameTypePair>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<NameTypePair>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeTuple {
    pub items: Vec<Type>,
}

impl<'de> Deserialize<'de> for TypeTuple {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let items = <Vec<Type>>::deserialize(deserializer)?;
        Ok(Self { items })
    }
}

impl Serialize for TypeTuple {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<Type>>::serialize(&self.items, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeList {
    pub inner: Box<Type>,
}

impl<'de> Deserialize<'de> for TypeList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = Type::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Type::serialize(&self.inner, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeStr;

impl<'de> Deserialize<'de> for TypeStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeStr),
            serde_json::Value::Null => Ok(TypeStr),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeStr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeChr;

impl<'de> Deserialize<'de> for TypeChr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeChr),
            serde_json::Value::Null => Ok(TypeChr),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeChr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeF64;

impl<'de> Deserialize<'de> for TypeF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeF64),
            serde_json::Value::Null => Ok(TypeF64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeF64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeF32;

impl<'de> Deserialize<'de> for TypeF32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeF32),
            serde_json::Value::Null => Ok(TypeF32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeF32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU64;

impl<'de> Deserialize<'de> for TypeU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU64),
            serde_json::Value::Null => Ok(TypeU64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS64;

impl<'de> Deserialize<'de> for TypeS64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS64),
            serde_json::Value::Null => Ok(TypeS64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU32;

impl<'de> Deserialize<'de> for TypeU32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU32),
            serde_json::Value::Null => Ok(TypeU32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS32;

impl<'de> Deserialize<'de> for TypeS32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS32),
            serde_json::Value::Null => Ok(TypeS32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU16;

impl<'de> Deserialize<'de> for TypeU16 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU16),
            serde_json::Value::Null => Ok(TypeU16),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU16 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS16;

impl<'de> Deserialize<'de> for TypeS16 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS16),
            serde_json::Value::Null => Ok(TypeS16),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS16 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU8;

impl<'de> Deserialize<'de> for TypeU8 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU8),
            serde_json::Value::Null => Ok(TypeU8),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU8 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS8;

impl<'de> Deserialize<'de> for TypeS8 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS8),
            serde_json::Value::Null => Ok(TypeS8),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS8 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeBool;

impl<'de> Deserialize<'de> for TypeBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeBool),
            serde_json::Value::Null => Ok(TypeBool),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeBool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Enum)]
pub enum ResourceMode {
    Borrowed,
    Owned,
}

impl From<AnalysedResourceMode> for ResourceMode {
    fn from(value: AnalysedResourceMode) -> Self {
        match value {
            AnalysedResourceMode::Borrowed => ResourceMode::Borrowed,
            AnalysedResourceMode::Owned => ResourceMode::Owned,
        }
    }
}

impl From<ResourceMode> for AnalysedResourceMode {
    fn from(value: ResourceMode) -> Self {
        match value {
            ResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
            ResourceMode::Owned => AnalysedResourceMode::Owned,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct TypeHandle {
    resource_id: u64,
    mode: ResourceMode,
}

impl TryFrom<golem_wasm_rpc::protobuf::TypeHandle> for TypeHandle {
    type Error = String;

    fn try_from(value: golem_wasm_rpc::protobuf::TypeHandle) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: value.resource_id,
            mode: match golem_wasm_rpc::protobuf::ResourceMode::try_from(value.mode) {
                Ok(golem_wasm_rpc::protobuf::ResourceMode::Borrowed) => ResourceMode::Borrowed,
                Ok(golem_wasm_rpc::protobuf::ResourceMode::Owned) => ResourceMode::Owned,
                Err(_) => Err("Invalid mode".to_string())?,
            },
        })
    }
}

impl From<TypeHandle> for golem_wasm_rpc::protobuf::TypeHandle {
    fn from(value: TypeHandle) -> Self {
        Self {
            resource_id: value.resource_id,
            mode: match value.mode {
                ResourceMode::Borrowed => golem_wasm_rpc::protobuf::ResourceMode::Borrowed as i32,
                ResourceMode::Owned => golem_wasm_rpc::protobuf::ResourceMode::Owned as i32,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum Type {
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

impl TryFrom<golem_wasm_rpc::protobuf::Type> for Type {
    type Error = String;

    fn try_from(value: golem_wasm_rpc::protobuf::Type) -> Result<Self, Self::Error> {
        match value.r#type {
            None => Err("Missing type".to_string()),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Variant(variant)) => {
                Ok(Self::Variant(TypeVariant {
                    cases: variant
                        .cases
                        .into_iter()
                        .map(|case| match case.typ {
                            None => Ok(NameOptionTypePair {
                                name: case.name,
                                typ: None,
                            }),
                            Some(typ) => typ.try_into().map(|t| NameOptionTypePair {
                                name: case.name,
                                typ: Some(Box::new(t)),
                            }),
                        })
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Result(result)) => {
                let ok = match result.ok {
                    None => None,
                    Some(ok) => Some(Box::new((*ok).try_into()?)),
                };
                let err = match result.err {
                    None => None,
                    Some(err) => Some(Box::new((*err).try_into()?)),
                };

                Ok(Self::Result(TypeResult { ok, err }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Option(option)) => {
                Ok(Self::Option(TypeOption {
                    inner: Box::new((*option.elem.ok_or("Missing elem")?).try_into()?),
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Enum(r#enum)) => {
                Ok(Self::Enum(TypeEnum {
                    cases: r#enum.names,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Flags(flags)) => {
                Ok(Self::Flags(TypeFlags { cases: flags.names }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Record(record)) => {
                Ok(Self::Record(TypeRecord {
                    cases: record
                        .fields
                        .into_iter()
                        .map(|field| {
                            Ok::<NameTypePair, String>(NameTypePair {
                                name: field.name,
                                typ: Box::new(field.typ.ok_or("Missing typ")?.try_into()?),
                            })
                        })
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Tuple(tuple)) => {
                Ok(Self::Tuple(TypeTuple {
                    items: tuple
                        .elems
                        .into_iter()
                        .map(|item| item.try_into())
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::List(list)) => Ok(Self::List(TypeList {
                inner: Box::new((*list.elem.ok_or("Missing elem")?).try_into()?),
            })),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 12 },
            )) => Ok(Self::Str(TypeStr)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 11 },
            )) => Ok(Self::Chr(TypeChr)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 10 },
            )) => Ok(Self::F64(TypeF64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 9 },
            )) => Ok(Self::F32(TypeF32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 8 },
            )) => Ok(Self::U64(TypeU64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 7 },
            )) => Ok(Self::S64(TypeS64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 6 },
            )) => Ok(Self::U32(TypeU32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 5 },
            )) => Ok(Self::S32(TypeS32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 4 },
            )) => Ok(Self::U16(TypeU16)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 3 },
            )) => Ok(Self::S16(TypeS16)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 2 },
            )) => Ok(Self::U8(TypeU8)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 1 },
            )) => Ok(Self::S8(TypeS8)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 0 },
            )) => Ok(Self::Bool(TypeBool)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive },
            )) => Err(format!("Invalid primitive: {}", primitive)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Handle(handle)) => {
                Ok(Self::Handle(handle.try_into()?))
            }
        }
    }
}

impl From<Type> for golem_wasm_rpc::protobuf::Type {
    fn from(value: Type) -> Self {
        match value {
            Type::Variant(variant) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Variant(
                    golem_wasm_rpc::protobuf::TypeVariant {
                        cases: variant
                            .cases
                            .into_iter()
                            .map(|case| golem_wasm_rpc::protobuf::NameOptionTypePair {
                                name: case.name,
                                typ: case.typ.map(|typ| (*typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Result(result) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Result(Box::new(
                    golem_wasm_rpc::protobuf::TypeResult {
                        ok: result.ok.map(|ok| Box::new((*ok).into())),
                        err: result.err.map(|err| Box::new((*err).into())),
                    },
                ))),
            },
            Type::Option(option) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Option(Box::new(
                    golem_wasm_rpc::protobuf::TypeOption {
                        elem: Some(Box::new((*option.inner).into())),
                    },
                ))),
            },
            Type::Enum(r#enum) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Enum(
                    golem_wasm_rpc::protobuf::TypeEnum {
                        names: r#enum.cases,
                    },
                )),
            },
            Type::Flags(flags) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Flags(
                    golem_wasm_rpc::protobuf::TypeFlags { names: flags.cases },
                )),
            },
            Type::Record(record) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Record(
                    golem_wasm_rpc::protobuf::TypeRecord {
                        fields: record
                            .cases
                            .into_iter()
                            .map(|case| golem_wasm_rpc::protobuf::NameTypePair {
                                name: case.name,
                                typ: Some((*case.typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Tuple(tuple) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Tuple(
                    golem_wasm_rpc::protobuf::TypeTuple {
                        elems: tuple.items.into_iter().map(|item| item.into()).collect(),
                    },
                )),
            },
            Type::List(list) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::List(Box::new(
                    golem_wasm_rpc::protobuf::TypeList {
                        elem: Some(Box::new((*list.inner).into())),
                    },
                ))),
            },
            Type::Str(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 12 },
                )),
            },
            Type::Chr(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 11 },
                )),
            },
            Type::F64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 10 },
                )),
            },
            Type::F32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 9 },
                )),
            },
            Type::U64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 8 },
                )),
            },
            Type::S64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 7 },
                )),
            },
            Type::U32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 6 },
                )),
            },
            Type::S32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 5 },
                )),
            },
            Type::U16(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 4 },
                )),
            },
            Type::S16(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 3 },
                )),
            },
            Type::U8(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 2 },
                )),
            },
            Type::S8(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 1 },
                )),
            },
            Type::Bool(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 0 },
                )),
            },
            Type::Handle(handle) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Handle(
                    handle.into(),
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct FunctionParameter {
    pub name: String,
    //  TODO: Fix this in DB. Temp fix for now.
    #[serde(rename = "tpe")]
    pub typ: Type,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionParameter> for FunctionParameter {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionParameter,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            typ: value.tpe.ok_or("Missing tpe")?.try_into()?,
        })
    }
}

impl From<FunctionParameter> for golem_api_grpc::proto::golem::component::FunctionParameter {
    fn from(value: FunctionParameter) -> Self {
        Self {
            name: value.name,
            tpe: Some(value.typ.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct FunctionResult {
    pub name: Option<String>,
    // TODO: Fix this in DB. Temp fix for now.
    #[serde(rename = "tpe")]
    pub typ: Type,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionResult> for FunctionResult {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionResult,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            typ: value.tpe.ok_or("Missing tpe")?.try_into()?,
        })
    }
}

impl From<FunctionResult> for golem_api_grpc::proto::golem::component::FunctionResult {
    fn from(value: FunctionResult) -> Self {
        Self {
            name: value.name,
            tpe: Some(value.typ.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct ExportInstance {
    pub name: String,
    pub functions: Vec<ExportFunction>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::ExportInstance> for ExportInstance {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ExportInstance,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            functions: value
                .functions
                .into_iter()
                .map(|function| function.try_into())
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ExportInstance> for golem_api_grpc::proto::golem::component::ExportInstance {
    fn from(value: ExportInstance) -> Self {
        Self {
            name: value.name,
            functions: value
                .functions
                .into_iter()
                .map(|function| function.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct ExportFunction {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub results: Vec<FunctionResult>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::ExportFunction> for ExportFunction {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ExportFunction,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            parameters: value
                .parameters
                .into_iter()
                .map(|parameter| parameter.try_into())
                .collect::<Result<_, _>>()?,
            results: value
                .results
                .into_iter()
                .map(|result| result.try_into())
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ExportFunction> for golem_api_grpc::proto::golem::component::ExportFunction {
    fn from(value: ExportFunction) -> Self {
        Self {
            name: value.name,
            parameters: value
                .parameters
                .into_iter()
                .map(|parameter| parameter.into())
                .collect(),
            results: value
                .results
                .into_iter()
                .map(|result| result.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum Export {
    Instance(ExportInstance),
    Function(ExportFunction),
}

impl Export {
    pub fn function_names(&self) -> Vec<String> {
        match self {
            Export::Instance(instance) => instance
                .functions
                .iter()
                .map(|function| format!("{}.{{{}}}", instance.name, function.name))
                .collect(),
            Export::Function(function) => vec![function.name.clone()],
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Export> for Export {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Export,
    ) -> Result<Self, Self::Error> {
        match value.export {
            None => Err("Missing export".to_string()),
            Some(golem_api_grpc::proto::golem::component::export::Export::Instance(instance)) => {
                Ok(Self::Instance(instance.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::component::export::Export::Function(function)) => {
                Ok(Self::Function(function.try_into()?))
            }
        }
    }
}

impl From<Export> for golem_api_grpc::proto::golem::component::Export {
    fn from(value: Export) -> Self {
        match value {
            Export::Instance(instance) => Self {
                export: Some(
                    golem_api_grpc::proto::golem::component::export::Export::Instance(
                        instance.into(),
                    ),
                ),
            },
            Export::Function(function) => Self {
                export: Some(
                    golem_api_grpc::proto::golem::component::export::Export::Function(
                        function.into(),
                    ),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}

impl From<golem_api_grpc::proto::golem::component::VersionedName> for VersionedName {
    fn from(value: golem_api_grpc::proto::golem::component::VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<VersionedName> for golem_api_grpc::proto::golem::component::VersionedName {
    fn from(value: VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ProducerField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

impl From<golem_api_grpc::proto::golem::component::ProducerField> for ProducerField {
    fn from(value: golem_api_grpc::proto::golem::component::ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

impl From<ProducerField> for golem_api_grpc::proto::golem::component::ProducerField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct Producers {
    pub fields: Vec<ProducerField>,
}

impl From<golem_api_grpc::proto::golem::component::Producers> for Producers {
    fn from(value: golem_api_grpc::proto::golem::component::Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

impl From<Producers> for golem_api_grpc::proto::golem::component::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

impl From<golem_wasm_ast::metadata::Producers> for Producers {
    fn from(value: golem_wasm_ast::metadata::Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<Producers> for golem_wasm_ast::metadata::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<golem_wasm_ast::metadata::ProducersField> for ProducerField {
    fn from(value: golem_wasm_ast::metadata::ProducersField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }
}

impl From<ProducerField> for golem_wasm_ast::metadata::ProducersField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| golem_wasm_ast::metadata::VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedExport> for Export {
    fn from(value: golem_wasm_ast::analysis::AnalysedExport) -> Self {
        match value {
            golem_wasm_ast::analysis::AnalysedExport::Function(analysed_function) => {
                Export::Function(analysed_function.into())
            }
            golem_wasm_ast::analysis::AnalysedExport::Instance(analysed_instance) => {
                Export::Instance(analysed_instance.into())
            }
        }
    }
}

impl From<Export> for golem_wasm_ast::analysis::AnalysedExport {
    fn from(value: Export) -> Self {
        match value {
            Export::Function(export_function) => {
                golem_wasm_ast::analysis::AnalysedExport::Function(export_function.into())
            }
            Export::Instance(export_instance) => {
                golem_wasm_ast::analysis::AnalysedExport::Instance(export_instance.into())
            }
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunction> for ExportFunction {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunction) -> Self {
        Self {
            name: value.name,
            parameters: value.params.into_iter().map(|p| p.into()).collect(),
            results: value.results.into_iter().map(|r| r.into()).collect(),
        }
    }
}

impl From<ExportFunction> for golem_wasm_ast::analysis::AnalysedFunction {
    fn from(value: ExportFunction) -> Self {
        Self {
            name: value.name,
            params: value.parameters.into_iter().map(|p| p.into()).collect(),
            results: value.results.into_iter().map(|r| r.into()).collect(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedInstance> for ExportInstance {
    fn from(value: golem_wasm_ast::analysis::AnalysedInstance) -> Self {
        Self {
            name: value.name,
            functions: value.funcs.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl From<ExportInstance> for golem_wasm_ast::analysis::AnalysedInstance {
    fn from(value: ExportInstance) -> Self {
        Self {
            name: value.name,
            funcs: value.functions.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunctionParameter> for FunctionParameter {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunctionParameter) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<FunctionParameter> for golem_wasm_ast::analysis::AnalysedFunctionParameter {
    fn from(value: FunctionParameter) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunctionResult> for FunctionResult {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunctionResult) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<FunctionResult> for golem_wasm_ast::analysis::AnalysedFunctionResult {
    fn from(value: FunctionResult) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedType> for Type {
    fn from(value: golem_wasm_ast::analysis::AnalysedType) -> Self {
        match value {
            golem_wasm_ast::analysis::AnalysedType::Bool => Type::Bool(TypeBool),
            golem_wasm_ast::analysis::AnalysedType::S8 => Type::S8(TypeS8),
            golem_wasm_ast::analysis::AnalysedType::U8 => Type::U8(TypeU8),
            golem_wasm_ast::analysis::AnalysedType::S16 => Type::S16(TypeS16),
            golem_wasm_ast::analysis::AnalysedType::U16 => Type::U16(TypeU16),
            golem_wasm_ast::analysis::AnalysedType::S32 => Type::S32(TypeS32),
            golem_wasm_ast::analysis::AnalysedType::U32 => Type::U32(TypeU32),
            golem_wasm_ast::analysis::AnalysedType::S64 => Type::S64(TypeS64),
            golem_wasm_ast::analysis::AnalysedType::U64 => Type::U64(TypeU64),
            golem_wasm_ast::analysis::AnalysedType::F32 => Type::F32(TypeF32),
            golem_wasm_ast::analysis::AnalysedType::F64 => Type::F64(TypeF64),
            golem_wasm_ast::analysis::AnalysedType::Chr => Type::Chr(TypeChr),
            golem_wasm_ast::analysis::AnalysedType::Str => Type::Str(TypeStr),
            golem_wasm_ast::analysis::AnalysedType::List(inner) => Type::List(TypeList {
                inner: Box::new((*inner).into()),
            }),
            golem_wasm_ast::analysis::AnalysedType::Tuple(items) => Type::Tuple(TypeTuple {
                items: items.into_iter().map(|t| t.into()).collect(),
            }),
            golem_wasm_ast::analysis::AnalysedType::Record(cases) => Type::Record(TypeRecord {
                cases: cases
                    .into_iter()
                    .map(|(name, typ)| NameTypePair {
                        name,
                        typ: Box::new(typ.into()),
                    })
                    .collect(),
            }),
            golem_wasm_ast::analysis::AnalysedType::Flags(cases) => {
                Type::Flags(TypeFlags { cases })
            }
            golem_wasm_ast::analysis::AnalysedType::Enum(cases) => Type::Enum(TypeEnum { cases }),
            golem_wasm_ast::analysis::AnalysedType::Option(inner) => Type::Option(TypeOption {
                inner: Box::new((*inner).into()),
            }),
            golem_wasm_ast::analysis::AnalysedType::Result { ok, error } => {
                Type::Result(TypeResult {
                    ok: ok.map(|t| Box::new((*t).into())),
                    err: error.map(|t| Box::new((*t).into())),
                })
            }
            golem_wasm_ast::analysis::AnalysedType::Variant(variants) => {
                Type::Variant(TypeVariant {
                    cases: variants
                        .into_iter()
                        .map(|(name, typ)| NameOptionTypePair {
                            name,
                            typ: typ.map(|t| Box::new(t.into())),
                        })
                        .collect(),
                })
            }
            golem_wasm_ast::analysis::AnalysedType::Resource { id, resource_mode } => {
                Type::Handle(TypeHandle {
                    resource_id: id.value,
                    mode: resource_mode.into(),
                })
            }
        }
    }
}

impl From<Type> for golem_wasm_ast::analysis::AnalysedType {
    fn from(value: Type) -> Self {
        match value {
            Type::Bool(_) => golem_wasm_ast::analysis::AnalysedType::Bool,
            Type::S8(_) => golem_wasm_ast::analysis::AnalysedType::S8,
            Type::U8(_) => golem_wasm_ast::analysis::AnalysedType::U8,
            Type::S16(_) => golem_wasm_ast::analysis::AnalysedType::S16,
            Type::U16(_) => golem_wasm_ast::analysis::AnalysedType::U16,
            Type::S32(_) => golem_wasm_ast::analysis::AnalysedType::S32,
            Type::U32(_) => golem_wasm_ast::analysis::AnalysedType::U32,
            Type::S64(_) => golem_wasm_ast::analysis::AnalysedType::S64,
            Type::U64(_) => golem_wasm_ast::analysis::AnalysedType::U64,
            Type::F32(_) => golem_wasm_ast::analysis::AnalysedType::F32,
            Type::F64(_) => golem_wasm_ast::analysis::AnalysedType::F64,
            Type::Chr(_) => golem_wasm_ast::analysis::AnalysedType::Chr,
            Type::Str(_) => golem_wasm_ast::analysis::AnalysedType::Str,
            Type::List(inner) => {
                let elem_type: golem_wasm_ast::analysis::AnalysedType = (*inner.inner).into();
                golem_wasm_ast::analysis::AnalysedType::List(Box::new(elem_type))
            }
            Type::Tuple(inner) => golem_wasm_ast::analysis::AnalysedType::Tuple(
                inner.items.into_iter().map(|t| t.into()).collect(),
            ),
            Type::Record(inner) => golem_wasm_ast::analysis::AnalysedType::Record(
                inner
                    .cases
                    .into_iter()
                    .map(|case| (case.name, (*case.typ).into()))
                    .collect(),
            ),
            Type::Flags(inner) => golem_wasm_ast::analysis::AnalysedType::Flags(inner.cases),
            Type::Enum(inner) => golem_wasm_ast::analysis::AnalysedType::Enum(inner.cases),
            Type::Option(inner) => {
                golem_wasm_ast::analysis::AnalysedType::Option(Box::new((*inner.inner).into()))
            }
            Type::Result(inner) => golem_wasm_ast::analysis::AnalysedType::Result {
                ok: inner.ok.map(|t| Box::new((*t).into())),
                error: inner.err.map(|t| Box::new((*t).into())),
            },
            Type::Variant(variants) => golem_wasm_ast::analysis::AnalysedType::Variant(
                variants
                    .cases
                    .into_iter()
                    .map(|case| (case.name, case.typ.map(|t| (*t).into())))
                    .collect(),
            ),
            Type::Handle(handle) => golem_wasm_ast::analysis::AnalysedType::Resource {
                id: AnalysedResourceId {
                    value: handle.resource_id,
                },
                resource_mode: handle.mode.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct LinearMemory {
    /// Initial size of the linear memory in bytes
    pub initial: u64,
    /// Optional maximal size of the linear memory in bytes
    pub maximum: Option<u64>,
}

impl LinearMemory {
    const PAGE_SIZE: u64 = 65536;
}

impl From<golem_wasm_ast::core::Mem> for LinearMemory {
    fn from(value: golem_wasm_ast::core::Mem) -> Self {
        Self {
            initial: value.mem_type.limits.min * LinearMemory::PAGE_SIZE,
            maximum: value
                .mem_type
                .limits
                .max
                .map(|m| m * LinearMemory::PAGE_SIZE),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::LinearMemory> for LinearMemory {
    fn from(value: golem_api_grpc::proto::golem::component::LinearMemory) -> Self {
        Self {
            initial: value.initial,
            maximum: value.maximum,
        }
    }
}

impl From<LinearMemory> for golem_api_grpc::proto::golem::component::LinearMemory {
    fn from(value: LinearMemory) -> Self {
        Self {
            initial: value.initial,
            maximum: value.maximum,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct ComponentMetadata {
    pub exports: Vec<Export>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,
}

impl ComponentMetadata {
    pub fn instances(&self) -> Vec<ExportInstance> {
        let mut instances = vec![];
        for export in self.exports.clone() {
            if let Export::Instance(instance) = export {
                instances.push(instance.clone())
            }
        }
        instances
    }

    pub fn functions(&self) -> Vec<ExportFunction> {
        let mut functions = vec![];
        for export in self.exports.clone() {
            if let Export::Function(function) = export {
                functions.push(function.clone())
            }
        }
        functions
    }

    pub fn function_by_name(&self, name: &str) -> Result<Option<ExportFunction>, String> {
        let parsed = ParsedFunctionName::parse(name)?;

        match &parsed.site().interface_name() {
            None => Ok(self.functions().iter().find(|f| f.name == *name).cloned()),
            Some(interface_name) => {
                let exported_function = self
                    .instances()
                    .iter()
                    .find(|instance| instance.name == *interface_name)
                    .and_then(|instance| {
                        instance
                            .functions
                            .iter()
                            .find(|f| f.name == parsed.function().function_name())
                            .cloned()
                    });
                if exported_function.is_none() {
                    match parsed.method_as_static() {
                        Some(parsed_static) => Ok(self
                            .instances()
                            .iter()
                            .find(|instance| instance.name == *interface_name)
                            .and_then(|instance| {
                                instance
                                    .functions
                                    .iter()
                                    .find(|f| f.name == parsed_static.function().function_name())
                                    .cloned()
                            })),
                        None => Ok(None),
                    }
                } else {
                    Ok(exported_function)
                }
            }
        }
    }

    /// Gets the sum of all the initial memory sizes of the component
    pub fn total_initial_memory(&self) -> u64 {
        self.memories.iter().map(|m| m.initial).sum()
    }

    /// Gets the sum of the maximum memory sizes, if all are bounded
    pub fn total_maximum_memory(&self) -> Option<u64> {
        self.memories.iter().map(|m| m.maximum).sum()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata> for ComponentMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ComponentMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            exports: value
                .exports
                .into_iter()
                .map(|export| export.try_into())
                .collect::<Result<_, _>>()?,
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
            memories: value
                .memories
                .into_iter()
                .map(|memory| memory.into())
                .collect(),
        })
    }
}

impl From<ComponentMetadata> for golem_api_grpc::proto::golem::component::ComponentMetadata {
    fn from(value: ComponentMetadata) -> Self {
        Self {
            exports: value
                .exports
                .into_iter()
                .map(|export| export.into())
                .collect(),
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
            memories: value
                .memories
                .into_iter()
                .map(|memory| memory.into())
                .collect(),
        }
    }
}

// NOTE: different from golem_common::model::WorkerId because of field name annotations
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerId {
    pub component_id: ComponentId,
    pub worker_name: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, NewType)]
pub struct Id(String);

impl TryFrom<String> for Id {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = valid_id(value.as_str())?;
        Ok(Self(value))
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.clone())
    }
}

impl WorkerId {
    pub fn new(component_id: ComponentId, worker_name: String) -> Result<Self, &'static str> {
        Ok(Self {
            component_id,
            worker_name: worker_name.try_into()?,
        })
    }
}

fn valid_id(identifier: &str) -> Result<&str, &'static str> {
    let length = identifier.len();
    if !(1..=100).contains(&length) {
        Err("Identifier must be between 1 and 100 characters")
    } else if identifier.contains(' ') {
        Err("Identifier must not contain spaces")
    } else if !identifier
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        Err("Identifier must contain only alphanumeric characters, underscores, and dashes")
    } else if identifier.starts_with('-') {
        Err("Identifier must not start with a dash")
    } else {
        Ok(identifier)
    }
}

impl From<golem_common::model::WorkerId> for WorkerId {
    fn from(value: golem_common::model::WorkerId) -> Self {
        Self {
            component_id: value.component_id,
            worker_name: Id(value.worker_name),
        }
    }
}

impl From<WorkerId> for golem_common::model::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            component_id: value.component_id,
            worker_name: value.worker_name.0,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerId> for WorkerId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerId,
    ) -> Result<Self, Self::Error> {
        let worker_name: Id = value.name.try_into().map_err(String::from)?;

        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing component_id")?
                .try_into()?,
            worker_name,
        })
    }
}

impl From<WorkerId> for golem_api_grpc::proto::golem::worker::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            name: value.worker_name.0,
        }
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.component_id, self.worker_name.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CompleteParameters {
    pub oplog_idx: u64,
    pub data: Vec<u8>,
}

impl From<CompleteParameters> for golem_api_grpc::proto::golem::worker::CompleteParameters {
    fn from(value: CompleteParameters) -> Self {
        Self {
            oplog_idx: value.oplog_idx,
            data: value.data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PromiseId {
    pub worker_id: WorkerId,
    pub oplog_idx: u64,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            oplog_idx: value.oplog_idx,
        })
    }
}

impl From<PromiseId> for golem_api_grpc::proto::golem::worker::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl Display for PromiseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.worker_id, self.oplog_idx)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Invalid request: {details}")]
pub struct GolemErrorInvalidRequest {
    pub details: String,
}

impl From<golem_api_grpc::proto::golem::worker::InvalidRequest> for GolemErrorInvalidRequest {
    fn from(value: golem_api_grpc::proto::golem::worker::InvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorInvalidRequest> for golem_api_grpc::proto::golem::worker::InvalidRequest {
    fn from(value: GolemErrorInvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker already exists: {worker_id}")]
pub struct GolemErrorWorkerAlreadyExists {
    pub worker_id: WorkerId,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerAlreadyExists>
    for GolemErrorWorkerAlreadyExists
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerAlreadyExists,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerAlreadyExists>
    for golem_api_grpc::proto::golem::worker::WorkerAlreadyExists
{
    fn from(value: GolemErrorWorkerAlreadyExists) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker not found: {worker_id}")]
pub struct GolemErrorWorkerNotFound {
    pub worker_id: WorkerId,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerNotFound> for GolemErrorWorkerNotFound {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerNotFound,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerNotFound> for golem_api_grpc::proto::golem::worker::WorkerNotFound {
    fn from(value: GolemErrorWorkerNotFound) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker creation failed {worker_id}: {details}")]
pub struct GolemErrorWorkerCreationFailed {
    pub worker_id: WorkerId,
    pub details: String,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerCreationFailed>
    for GolemErrorWorkerCreationFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerCreationFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            details: value.details,
        })
    }
}

impl From<GolemErrorWorkerCreationFailed>
    for golem_api_grpc::proto::golem::worker::WorkerCreationFailed
{
    fn from(value: GolemErrorWorkerCreationFailed) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to resume worker: {worker_id}")]
pub struct GolemErrorFailedToResumeWorker {
    pub worker_id: WorkerId,
    pub reason: Box<GolemError>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::FailedToResumeWorker>
    for GolemErrorFailedToResumeWorker
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::FailedToResumeWorker,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            reason: Box::new((*value.reason.ok_or("Missing field: reason")?).try_into()?),
        })
    }
}

impl From<GolemErrorFailedToResumeWorker>
    for golem_api_grpc::proto::golem::worker::FailedToResumeWorker
{
    fn from(value: GolemErrorFailedToResumeWorker) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            reason: Some(Box::new((*value.reason).into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to download component {component_id}: {reason}")]
pub struct GolemErrorComponentDownloadFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::ComponentDownloadFailed>
    for GolemErrorComponentDownloadFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::ComponentDownloadFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: VersionedComponentId {
                component_id: value
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                version: value.component_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorComponentDownloadFailed>
    for golem_api_grpc::proto::golem::worker::ComponentDownloadFailed
{
    fn from(value: GolemErrorComponentDownloadFailed) -> Self {
        let component_version = value.component_id.version;
        let component_id = golem_api_grpc::proto::golem::component::ComponentId {
            value: Some(value.component_id.component_id.0.into()),
        };
        Self {
            component_id: Some(component_id),
            component_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to parse component {component_id}: {reason}")]
pub struct GolemErrorComponentParseFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::ComponentParseFailed>
    for GolemErrorComponentParseFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::ComponentParseFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: VersionedComponentId {
                component_id: value
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                version: value.component_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorComponentParseFailed>
    for golem_api_grpc::proto::golem::worker::ComponentParseFailed
{
    fn from(value: GolemErrorComponentParseFailed) -> Self {
        let component_version = value.component_id.version;
        let component_id = golem_api_grpc::proto::golem::component::ComponentId {
            value: Some(value.component_id.component_id.0.into()),
        };
        Self {
            component_id: Some(component_id),
            component_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to get latest version of component {component_id}: {reason}")]
pub struct GolemErrorGetLatestVersionOfComponentFailed {
    pub component_id: ComponentId,
    pub reason: String,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::GetLatestVersionOfComponentFailed>
    for GolemErrorGetLatestVersionOfComponentFailed
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::GetLatestVersionOfComponentFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing field: component_id")?
                .try_into()?,
            reason: value.reason,
        })
    }
}

impl From<GolemErrorGetLatestVersionOfComponentFailed>
    for golem_api_grpc::proto::golem::worker::GetLatestVersionOfComponentFailed
{
    fn from(value: GolemErrorGetLatestVersionOfComponentFailed) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Failed to find promise: {promise_id}")]
pub struct GolemErrorPromiseNotFound {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseNotFound> for GolemErrorPromiseNotFound {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseNotFound,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseNotFound> for golem_api_grpc::proto::golem::worker::PromiseNotFound {
    fn from(value: GolemErrorPromiseNotFound) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Promise dropped: {promise_id}")]
pub struct GolemErrorPromiseDropped {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseDropped> for GolemErrorPromiseDropped {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseDropped,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseDropped> for golem_api_grpc::proto::golem::worker::PromiseDropped {
    fn from(value: GolemErrorPromiseDropped) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Promise already completed: {promise_id}")]
pub struct GolemErrorPromiseAlreadyCompleted {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseAlreadyCompleted>
    for GolemErrorPromiseAlreadyCompleted
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseAlreadyCompleted,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseAlreadyCompleted>
    for golem_api_grpc::proto::golem::worker::PromiseAlreadyCompleted
{
    fn from(value: GolemErrorPromiseAlreadyCompleted) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Worker Interrupted: {}", if *.recover_immediately { "recovering immediately" } else { "not recovering immediately" })]
pub struct GolemErrorInterrupted {
    pub recover_immediately: bool,
}
impl From<golem_api_grpc::proto::golem::worker::Interrupted> for GolemErrorInterrupted {
    fn from(value: golem_api_grpc::proto::golem::worker::Interrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

impl From<GolemErrorInterrupted> for golem_api_grpc::proto::golem::worker::Interrupted {
    fn from(value: GolemErrorInterrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Parameter type mismatch")]
pub struct GolemErrorParamTypeMismatch {}

impl From<golem_api_grpc::proto::golem::worker::ParamTypeMismatch> for GolemErrorParamTypeMismatch {
    fn from(_value: golem_api_grpc::proto::golem::worker::ParamTypeMismatch) -> Self {
        Self {}
    }
}

impl From<GolemErrorParamTypeMismatch> for golem_api_grpc::proto::golem::worker::ParamTypeMismatch {
    fn from(_value: GolemErrorParamTypeMismatch) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("No value in message")]
pub struct GolemErrorNoValueInMessage {}

impl From<golem_api_grpc::proto::golem::worker::NoValueInMessage> for GolemErrorNoValueInMessage {
    fn from(_value: golem_api_grpc::proto::golem::worker::NoValueInMessage) -> Self {
        Self {}
    }
}

impl From<GolemErrorNoValueInMessage> for golem_api_grpc::proto::golem::worker::NoValueInMessage {
    fn from(_value: GolemErrorNoValueInMessage) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Value mismatch: {details}")]
pub struct GolemErrorValueMismatch {
    pub details: String,
}

impl From<golem_api_grpc::proto::golem::worker::ValueMismatch> for GolemErrorValueMismatch {
    fn from(value: golem_api_grpc::proto::golem::worker::ValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorValueMismatch> for golem_api_grpc::proto::golem::worker::ValueMismatch {
    fn from(value: GolemErrorValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Unexpected oplog entry: expected {expected}, got {got}")]
pub struct GolemErrorUnexpectedOplogEntry {
    pub expected: String,
    pub got: String,
}

impl From<golem_api_grpc::proto::golem::worker::UnexpectedOplogEntry>
    for GolemErrorUnexpectedOplogEntry
{
    fn from(value: golem_api_grpc::proto::golem::worker::UnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

impl From<GolemErrorUnexpectedOplogEntry>
    for golem_api_grpc::proto::golem::worker::UnexpectedOplogEntry
{
    fn from(value: GolemErrorUnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Runtime error: {details}")]
pub struct GolemErrorRuntimeError {
    pub details: String,
}

impl From<golem_api_grpc::proto::golem::worker::RuntimeError> for GolemErrorRuntimeError {
    fn from(value: golem_api_grpc::proto::golem::worker::RuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorRuntimeError> for golem_api_grpc::proto::golem::worker::RuntimeError {
    fn from(value: GolemErrorRuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[error("Invalid shard id: {shard_id}, valid shard ids: {shard_ids:?}")]
pub struct GolemErrorInvalidShardId {
    pub shard_id: ShardId,
    pub shard_ids: std::collections::HashSet<ShardId>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::InvalidShardId> for GolemErrorInvalidShardId {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::worker::InvalidShardId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            shard_id: value.shard_id.ok_or("Missing field: shard_id")?.into(),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        })
    }
}

impl From<GolemErrorInvalidShardId> for golem_api_grpc::proto::golem::worker::InvalidShardId {
    fn from(value: GolemErrorInvalidShardId) -> Self {
        Self {
            shard_id: Some(value.shard_id.into()),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Previous invocation failed: {details}")]
pub struct GolemErrorPreviousInvocationFailed {
    pub details: String,
}

impl From<golem_api_grpc::proto::golem::worker::PreviousInvocationFailed>
    for GolemErrorPreviousInvocationFailed
{
    fn from(value: golem_api_grpc::proto::golem::worker::PreviousInvocationFailed) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorPreviousInvocationFailed>
    for golem_api_grpc::proto::golem::worker::PreviousInvocationFailed
{
    fn from(value: GolemErrorPreviousInvocationFailed) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Previous invocation exited")]
pub struct GolemErrorPreviousInvocationExited {}

impl From<golem_api_grpc::proto::golem::worker::PreviousInvocationExited>
    for GolemErrorPreviousInvocationExited
{
    fn from(_value: golem_api_grpc::proto::golem::worker::PreviousInvocationExited) -> Self {
        Self {}
    }
}

impl From<GolemErrorPreviousInvocationExited>
    for golem_api_grpc::proto::golem::worker::PreviousInvocationExited
{
    fn from(_value: GolemErrorPreviousInvocationExited) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Unknown error: {details}")]
pub struct GolemErrorUnknown {
    pub details: String,
}

impl From<golem_api_grpc::proto::golem::worker::UnknownError> for GolemErrorUnknown {
    fn from(value: golem_api_grpc::proto::golem::worker::UnknownError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorUnknown> for golem_api_grpc::proto::golem::worker::UnknownError {
    fn from(value: GolemErrorUnknown) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, thiserror::Error)]
#[error("Invalid account")]
pub struct GolemErrorInvalidAccount {}

impl From<golem_api_grpc::proto::golem::worker::InvalidAccount> for GolemErrorInvalidAccount {
    fn from(_value: golem_api_grpc::proto::golem::worker::InvalidAccount) -> Self {
        Self {}
    }
}

impl From<GolemErrorInvalidAccount> for golem_api_grpc::proto::golem::worker::InvalidAccount {
    fn from(_value: GolemErrorInvalidAccount) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct InvokeParameters {
    pub params: serde_json::value::Value,
}

impl InvokeParameters {
    pub fn as_json_string(&self) -> String {
        serde_json::to_string(&self.params).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct DeleteWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InvokeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InterruptResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ResumeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct UpdateWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Enum)]
pub enum WorkerUpdateMode {
    Automatic,
    Manual,
}

impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for WorkerUpdateMode {
    fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                WorkerUpdateMode::Automatic
            }
            golem_api_grpc::proto::golem::worker::UpdateMode::Manual => WorkerUpdateMode::Manual,
        }
    }
}

impl From<WorkerUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn from(value: WorkerUpdateMode) -> Self {
        match value {
            WorkerUpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            WorkerUpdateMode::Manual => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UpdateWorkerRequest {
    pub mode: WorkerUpdateMode,
    pub target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkersMetadataRequest {
    pub filter: Option<WorkerFilter>,
    pub cursor: Option<ScanCursor>,
    pub count: Option<u64>,
    pub precise: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadata>,
    pub cursor: Option<ScanCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub component_version: ComponentVersion,
    pub retry_count: u64,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerMetadata> for WorkerMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            args: value.args,
            env: value.env,
            status: value.status.try_into()?,
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value
                .updates
                .into_iter()
                .map(|update| update.try_into())
                .collect::<Result<Vec<UpdateRecord>, String>>()?,
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
        })
    }
}

impl From<WorkerMetadata> for golem_api_grpc::proto::golem::worker::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            account_id: Some(golem_api_grpc::proto::golem::common::AccountId {
                name: "-1".to_string(),
            }),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.iter().cloned().map(|u| u.into()).collect(),
            created_at: Some(value.created_at.into()),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union)]
#[serde(rename_all = "camelCase")]
#[oai(discriminator_name = "type", one_of = true, rename_all = "camelCase")]
pub enum UpdateRecord {
    PendingUpdate(PendingUpdate),
    SuccessfulUpdate(SuccessfulUpdate),
    FailedUpdate(FailedUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PendingUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SuccessfulUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FailedUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
    details: Option<String>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateRecord> for UpdateRecord {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateRecord,
    ) -> Result<Self, Self::Error> {
        match value.update.ok_or("Missing update field")? {
            golem_api_grpc::proto::golem::worker::update_record::Update::Failed(failed) => {
                Ok(Self::FailedUpdate(FailedUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                    details: { failed.details },
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                Ok(Self::PendingUpdate(PendingUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
        }
    }
}

impl From<UpdateRecord> for golem_api_grpc::proto::golem::worker::UpdateRecord {
    fn from(value: UpdateRecord) -> Self {
        match value {
            UpdateRecord::FailedUpdate(FailedUpdate {
                timestamp,
                target_version,
                details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                        golem_api_grpc::proto::golem::worker::FailedUpdate { details },
                    ),
                ),
            },
            UpdateRecord::PendingUpdate(PendingUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Pending(
                        golem_api_grpc::proto::golem::worker::PendingUpdate {},
                    ),
                ),
            },
            UpdateRecord::SuccessfulUpdate(SuccessfulUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Successful(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdate {},
                    ),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
pub struct InvokeResult {
    pub result: serde_json::value::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union, thiserror::Error)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum GolemError {
    #[error(transparent)]
    InvalidRequest(GolemErrorInvalidRequest),
    #[error(transparent)]
    WorkerAlreadyExists(GolemErrorWorkerAlreadyExists),
    #[error(transparent)]
    WorkerNotFound(GolemErrorWorkerNotFound),
    #[error(transparent)]
    WorkerCreationFailed(GolemErrorWorkerCreationFailed),
    #[error(transparent)]
    FailedToResumeWorker(GolemErrorFailedToResumeWorker),
    #[error(transparent)]
    ComponentDownloadFailed(GolemErrorComponentDownloadFailed),
    #[error(transparent)]
    ComponentParseFailed(GolemErrorComponentParseFailed),
    #[error(transparent)]
    GetLatestVersionOfComponentFailed(GolemErrorGetLatestVersionOfComponentFailed),
    #[error(transparent)]
    PromiseNotFound(GolemErrorPromiseNotFound),
    #[error(transparent)]
    PromiseDropped(GolemErrorPromiseDropped),
    #[error(transparent)]
    PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted),
    #[error(transparent)]
    Interrupted(GolemErrorInterrupted),
    #[error(transparent)]
    ParamTypeMismatch(GolemErrorParamTypeMismatch),
    #[error(transparent)]
    NoValueInMessage(GolemErrorNoValueInMessage),
    #[error(transparent)]
    ValueMismatch(GolemErrorValueMismatch),
    #[error(transparent)]
    UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry),
    #[error(transparent)]
    RuntimeError(GolemErrorRuntimeError),
    #[error(transparent)]
    InvalidShardId(GolemErrorInvalidShardId),
    #[error(transparent)]
    PreviousInvocationFailed(GolemErrorPreviousInvocationFailed),
    #[error(transparent)]
    PreviousInvocationExited(GolemErrorPreviousInvocationExited),
    #[error(transparent)]
    Unknown(GolemErrorUnknown),
    #[error(transparent)]
    InvalidAccount(GolemErrorInvalidAccount),
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerExecutionError> for GolemError {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerExecutionError,
    ) -> Result<Self, Self::Error> {
        match value.error {
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidRequest(err)) => {
                Ok(GolemError::InvalidRequest(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerAlreadyExists(err)) => {
                Ok(GolemError::WorkerAlreadyExists(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerNotFound(err)) => {
                Ok(GolemError::WorkerNotFound(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerCreationFailed(err)) => {
                Ok(GolemError::WorkerCreationFailed(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::FailedToResumeWorker(err)) => {
                Ok(GolemError::FailedToResumeWorker((*err).try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ComponentDownloadFailed(err)) => {
                Ok(GolemError::ComponentDownloadFailed(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ComponentParseFailed(err)) => {
                Ok(GolemError::ComponentParseFailed(err.try_into()?))
            }
            Some(
                golem_api_grpc::proto::golem::worker::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                    err,
                ),
            ) => Ok(GolemError::GetLatestVersionOfComponentFailed(
                err.try_into()?,
            )),
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseNotFound(err)) => {
                Ok(GolemError::PromiseNotFound(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseDropped(err)) => {
                Ok(GolemError::PromiseDropped(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseAlreadyCompleted(err)) => {
                Ok(GolemError::PromiseAlreadyCompleted(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::Interrupted(err)) => {
                Ok(GolemError::Interrupted(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ParamTypeMismatch(err)) => {
                Ok(GolemError::ParamTypeMismatch(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::NoValueInMessage(err)) => {
                Ok(GolemError::NoValueInMessage(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ValueMismatch(err)) => {
                Ok(GolemError::ValueMismatch(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::UnexpectedOplogEntry(err)) => {
                Ok(GolemError::UnexpectedOplogEntry(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::RuntimeError(err)) => {
                Ok(GolemError::RuntimeError(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidShardId(err)) => {
                Ok(GolemError::InvalidShardId(err.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PreviousInvocationFailed(err)) => {
                Ok(GolemError::PreviousInvocationFailed(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PreviousInvocationExited(err)) => {
                Ok(GolemError::PreviousInvocationExited(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::Unknown(err)) => {
                Ok(GolemError::Unknown(err.into()))
            }
            Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidAccount(err)) => {
                Ok(GolemError::InvalidAccount(err.into()))
            }
            None => Err("Missing field: error".to_string()),
        }
    }
}

impl From<GolemError> for golem_api_grpc::proto::golem::worker::WorkerExecutionError {
    fn from(error: GolemError) -> Self {
        match error {
            GolemError::InvalidRequest(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidRequest(err.into())),
                }
            }
            GolemError::WorkerAlreadyExists(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerAlreadyExists(err.into())),
                }
            }
            GolemError::WorkerNotFound(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerNotFound(err.into())),
                }
            }
            GolemError::WorkerCreationFailed(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::WorkerCreationFailed(err.into())),
                }
            }
            GolemError::FailedToResumeWorker(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::FailedToResumeWorker(Box::new(err.into()))),
                }
            }
            GolemError::ComponentDownloadFailed(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ComponentDownloadFailed(err.into())),
                }
            }
            GolemError::ComponentParseFailed(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ComponentParseFailed(err.into())),
                }
            }
            GolemError::GetLatestVersionOfComponentFailed(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::GetLatestVersionOfComponentFailed(err.into())),
                }
            }
            GolemError::PromiseNotFound(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseNotFound(err.into())),
                }
            }
            GolemError::PromiseDropped(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseDropped(err.into())),
                }
            }
            GolemError::PromiseAlreadyCompleted(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PromiseAlreadyCompleted(err.into())),
                }
            }
            GolemError::Interrupted(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::Interrupted(err.into())),
                }
            }
            GolemError::ParamTypeMismatch(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ParamTypeMismatch(err.into())),
                }
            }
            GolemError::NoValueInMessage(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::NoValueInMessage(err.into())),
                }
            }
            GolemError::ValueMismatch(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::ValueMismatch(err.into())),
                }
            }
            GolemError::UnexpectedOplogEntry(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::UnexpectedOplogEntry(err.into())),
                }
            }
            GolemError::RuntimeError(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::RuntimeError(err.into())),
                }
            }
            GolemError::InvalidShardId(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidShardId(err.into())),
                }
            }
            GolemError::PreviousInvocationFailed(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PreviousInvocationFailed(err.into())),
                }
            }
            GolemError::PreviousInvocationExited(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::PreviousInvocationExited(err.into())),
                }
            }
            GolemError::Unknown(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::Unknown(err.into())),
                }
            }
            GolemError::InvalidAccount(err) => {
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(golem_api_grpc::proto::golem::worker::worker_execution_error::Error::InvalidAccount(err.into())),
                }
            }
        }
    }
}

#[derive(Object)]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorBody {
    pub golem_error: GolemError,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerExecutionError> for GolemErrorBody {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerExecutionError,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            golem_error: value.try_into()?,
        })
    }
}

#[derive(Object, Serialize)]
pub struct ErrorsBody {
    pub errors: Vec<String>,
}

#[derive(Object, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

impl From<golem_api_grpc::proto::golem::common::ErrorBody> for ErrorBody {
    fn from(value: golem_api_grpc::proto::golem::common::ErrorBody) -> Self {
        Self { error: value.error }
    }
}

impl From<golem_api_grpc::proto::golem::common::ErrorsBody> for ErrorsBody {
    fn from(value: golem_api_grpc::proto::golem::common::ErrorsBody) -> Self {
        Self {
            errors: value.errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub user_component_id: UserComponentId,
    pub protected_component_id: ProtectedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
}

impl Component {
    pub fn function_names(&self) -> Vec<String> {
        self.metadata
            .exports
            .iter()
            .flat_map(|x| x.function_names())
            .collect::<Vec<_>>()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
            user_component_id: value
                .user_component_id
                .ok_or("Missing user_component_id")?
                .try_into()?,
            protected_component_id: value
                .protected_component_id
                .ok_or("Missing protected_component_id")?
                .try_into()?,
            component_name: ComponentName(value.component_name),
            component_size: value.component_size,
            metadata: value.metadata.ok_or("Missing metadata")?.try_into()?,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            user_component_id: Some(value.user_component_id.into()),
            protected_component_id: Some(value.protected_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
        }
    }
}

impl Component {
    pub fn next_version(self) -> Self {
        let new_version = VersionedComponentId {
            component_id: self.versioned_component_id.component_id,
            version: self.versioned_component_id.version + 1,
        };
        Self {
            versioned_component_id: new_version.clone(),
            user_component_id: UserComponentId {
                versioned_component_id: new_version.clone(),
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: new_version,
            },
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub available_fuel: i64,
    pub max_memory_per_worker: i64,
}

impl From<ResourceLimits> for golem_api_grpc::proto::golem::common::ResourceLimits {
    fn from(value: ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::ResourceLimits> for ResourceLimits {
    fn from(value: golem_api_grpc::proto::golem::common::ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}
