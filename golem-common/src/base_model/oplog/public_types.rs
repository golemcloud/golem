// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::base_model::component::{ComponentRevision, PluginPriority};
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::invocation_context::{SpanId, TraceId};
use crate::base_model::oplog::PublicOplogEntry;
use crate::base_model::oplog::public_oplog_entry::{Deserialize, Serialize};
use crate::base_model::{Empty, IdempotencyKey, OplogIndex, Timestamp};
use crate::declare_structs;
use crate::model::agent::{DataSchema, DataValue, UntypedDataValue};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue))]
pub struct TypedDataValue {
    pub value: UntypedDataValue,
    pub schema: DataSchema,
}

#[cfg(feature = "full")]
impl From<DataValue> for TypedDataValue {
    fn from(value: DataValue) -> Self {
        let schema = value.extract_schema();
        Self {
            value: value.into(),
            schema,
        }
    }
}

#[cfg(feature = "full")]
impl TryFrom<TypedDataValue> for DataValue {
    type Error = String;

    fn try_from(td: TypedDataValue) -> Result<Self, Self::Error> {
        DataValue::try_from_untyped(td.value, td.schema)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OplogCursor {
    pub next_oplog_index: u64,
    pub current_component_revision: u64,
}

impl Display for OplogCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            self.next_oplog_index, self.current_component_revision
        )
    }
}

declare_structs! {
    pub struct PublicOplogEntryWithIndex {
        pub oplog_index: OplogIndex,
        pub entry: PublicOplogEntry,
    }
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Deserialize, IntoValue, FromValue,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationDescription {
    pub environment_plugin_grant_id: EnvironmentPluginGrantId,
    pub plugin_priority: PluginPriority,
    pub plugin_name: String,
    pub plugin_version: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
pub struct WriteRemoteBatchedParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteTransactionParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicDurableFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    #[unit_case]
    ReadLocal(Empty),
    /// The side-effect writes to the worker's local state (for example local file system)
    #[unit_case]
    WriteLocal(Empty),
    /// The side-effect reads from external state (for example a key-value store)
    #[unit_case]
    ReadRemote(Empty),
    /// The side-effect manipulates external state (for example an RPC call)
    #[unit_case]
    WriteRemote(Empty),
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
    WriteRemoteBatched(WriteRemoteBatchedParameters),
    WriteRemoteTransaction(WriteRemoteTransactionParameters),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct StringAttributeValue {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PublicAttributeValue {
    String(StringAttributeValue),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicLocalSpanData {
    pub span_id: SpanId,
    pub start: Timestamp,
    pub parent_id: Option<SpanId>,
    pub linked_context: Option<u64>,
    pub attributes: Vec<PublicAttribute>,
    pub inherited: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicAttribute {
    pub key: String,
    pub value: PublicAttributeValue,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicExternalSpanData {
    pub span_id: SpanId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PublicSpanData {
    LocalSpan(PublicLocalSpanData),
    ExternalSpan(PublicExternalSpanData),
}

impl PublicSpanData {
    pub fn span_id(&self) -> &SpanId {
        match self {
            PublicSpanData::LocalSpan(data) => &data.span_id,
            PublicSpanData::ExternalSpan(data) => &data.span_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentInitializationParameters {
    pub idempotency_key: IdempotencyKey,
    #[cfg_attr(feature = "full", wit_field(try_convert = TypedDataValue))]
    pub constructor_parameters: DataValue,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethodInvocationParameters {
    pub idempotency_key: IdempotencyKey,
    pub method_name: String,
    #[cfg_attr(feature = "full", wit_field(try_convert = TypedDataValue))]
    pub function_input: DataValue,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LoadSnapshotParameters {
    pub snapshot: PublicSnapshotData,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProcessOplogEntriesParameters {
    pub idempotency_key: IdempotencyKey,
    // TODO
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ManualUpdateParameters {
    pub target_revision: ComponentRevision,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicAgentInvocation {
    AgentInitialization(AgentInitializationParameters),
    AgentMethodInvocation(AgentMethodInvocationParameters),
    #[cfg_attr(feature = "full", unit_case)]
    SaveSnapshot(Empty),
    LoadSnapshot(LoadSnapshotParameters),
    ProcessOplogEntries(ProcessOplogEntriesParameters),
    ManualUpdate(ManualUpdateParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationOutputParameters {
    #[cfg_attr(feature = "full", wit_field(try_convert = TypedDataValue))]
    pub output: DataValue,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct FallibleResultParameters {
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SaveSnapshotResultParameters {
    pub snapshot: PublicSnapshotData,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProcessOplogEntriesResultParameters {
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union, IntoValue, FromValue))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicAgentInvocationResult {
    AgentInitialization(AgentInvocationOutputParameters),
    AgentMethod(AgentInvocationOutputParameters),
    #[cfg_attr(feature = "full", unit_case)]
    ManualUpdate(Empty),
    LoadSnapshot(FallibleResultParameters),
    SaveSnapshot(SaveSnapshotResultParameters),
    ProcessOplogEntries(ProcessOplogEntriesResultParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
pub struct SnapshotBasedUpdateParameters {
    pub payload: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicUpdateDescription {
    #[unit_case]
    Automatic(Empty),
    SnapshotBased(SnapshotBasedUpdateParameters),
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialOrd,
    Ord,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    IntoValue,
    FromValue,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentResourceId(pub u64);

impl AgentResourceId {
    pub const INITIAL: AgentResourceId = AgentResourceId(0);

    pub fn next(&self) -> AgentResourceId {
        AgentResourceId(self.0 + 1)
    }
}

impl Display for AgentResourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Worker log levels including the special stdout and stderr channels
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[wit(name = "log-level", owner = "golem:api@1.5.0/oplog")]
#[repr(u8)]
pub enum LogLevel {
    #[cfg_attr(feature = "full", desert(transparent))]
    Stdout,
    #[cfg_attr(feature = "full", desert(transparent))]
    Stderr,
    #[cfg_attr(feature = "full", desert(transparent))]
    Trace,
    #[cfg_attr(feature = "full", desert(transparent))]
    Debug,
    #[cfg_attr(feature = "full", desert(transparent))]
    Info,
    #[cfg_attr(feature = "full", desert(transparent))]
    Warn,
    #[cfg_attr(feature = "full", desert(transparent))]
    Error,
    #[cfg_attr(feature = "full", desert(transparent))]
    Critical,
}

#[derive(
    Copy, Clone, Debug, PartialOrd, PartialEq, Serialize, Deserialize, IntoValue, FromValue,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[wit(name = "persistence-level", owner = "golem:api@1.5.0/host", as_variant)]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RawSnapshotData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct JsonSnapshotData {
    pub data: serde_json::Value,
}

#[cfg(feature = "full")]
impl golem_wasm::IntoValue for JsonSnapshotData {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Record(vec![golem_wasm::Value::String(self.data.to_string())])
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::record(vec![
            golem_wasm::analysis::analysed_type::field(
                "data",
                golem_wasm::analysis::analysed_type::str(),
            ),
        ])
    }
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for JsonSnapshotData {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Record(mut fields) if fields.len() == 1 => {
                let data_str = <String as golem_wasm::FromValue>::from_value(fields.remove(0))?;
                let data: serde_json::Value = serde_json::from_str(&data_str)
                    .map_err(|e| format!("Failed to parse JSON: {e}"))?;
                Ok(JsonSnapshotData { data })
            }
            _ => Err(format!(
                "Expected Record with 1 field for JsonSnapshotData, got {:?}",
                value
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum MultipartPartData {
    Json(JsonSnapshotData),
    Raw(RawSnapshotData),
}

#[cfg(feature = "full")]
impl golem_wasm::IntoValue for MultipartPartData {
    fn into_value(self) -> golem_wasm::Value {
        match self {
            MultipartPartData::Json(json) => golem_wasm::Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(json.into_value())),
            },
            MultipartPartData::Raw(raw) => golem_wasm::Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(raw.into_value())),
            },
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::variant(vec![
            golem_wasm::analysis::analysed_type::case("Json", JsonSnapshotData::get_type()),
            golem_wasm::analysis::analysed_type::case("Raw", RawSnapshotData::get_type()),
        ])
    }
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for MultipartPartData {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Variant {
                case_idx: 0,
                case_value,
            } => {
                let inner = case_value.ok_or("Missing case value for Json")?;
                Ok(MultipartPartData::Json(
                    <JsonSnapshotData as golem_wasm::FromValue>::from_value(*inner)?,
                ))
            }
            golem_wasm::Value::Variant {
                case_idx: 1,
                case_value,
            } => {
                let inner = case_value.ok_or("Missing case value for Raw")?;
                Ok(MultipartPartData::Raw(
                    <RawSnapshotData as golem_wasm::FromValue>::from_value(*inner)?,
                ))
            }
            _ => Err(format!(
                "Expected Variant for MultipartPartData, got {:?}",
                value
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct MultipartSnapshotPart {
    pub name: String,
    pub content_type: String,
    pub data: MultipartPartData,
}

#[cfg(feature = "full")]
impl golem_wasm::IntoValue for MultipartSnapshotPart {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Record(vec![
            golem_wasm::Value::String(self.name),
            golem_wasm::Value::String(self.content_type),
            self.data.into_value(),
        ])
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::record(vec![
            golem_wasm::analysis::analysed_type::field(
                "name",
                golem_wasm::analysis::analysed_type::str(),
            ),
            golem_wasm::analysis::analysed_type::field(
                "content-type",
                golem_wasm::analysis::analysed_type::str(),
            ),
            golem_wasm::analysis::analysed_type::field("data", MultipartPartData::get_type()),
        ])
    }
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for MultipartSnapshotPart {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Record(mut fields) if fields.len() == 3 => {
                let data = MultipartPartData::from_value(fields.remove(2))?;
                let content_type = <String as golem_wasm::FromValue>::from_value(fields.remove(1))?;
                let name = <String as golem_wasm::FromValue>::from_value(fields.remove(0))?;
                Ok(MultipartSnapshotPart {
                    name,
                    content_type,
                    data,
                })
            }
            _ => Err(format!(
                "Expected Record with 3 fields for MultipartSnapshotPart, got {:?}",
                value
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct MultipartSnapshotData {
    pub mime_type: String,
    pub parts: Vec<MultipartSnapshotPart>,
}

#[cfg(feature = "full")]
impl golem_wasm::IntoValue for MultipartSnapshotData {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Record(vec![
            golem_wasm::Value::String(self.mime_type),
            golem_wasm::Value::List(self.parts.into_iter().map(|p| p.into_value()).collect()),
        ])
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::record(vec![
            golem_wasm::analysis::analysed_type::field(
                "mime-type",
                golem_wasm::analysis::analysed_type::str(),
            ),
            golem_wasm::analysis::analysed_type::field(
                "parts",
                golem_wasm::analysis::analysed_type::list(MultipartSnapshotPart::get_type()),
            ),
        ])
    }
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for MultipartSnapshotData {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Record(mut fields) if fields.len() == 2 => {
                let parts = match fields.remove(1) {
                    golem_wasm::Value::List(items) => items
                        .into_iter()
                        .map(MultipartSnapshotPart::from_value)
                        .collect::<Result<Vec<_>, String>>()?,
                    other => return Err(format!("Expected List for parts, got {:?}", other)),
                };
                let mime_type = <String as golem_wasm::FromValue>::from_value(fields.remove(0))?;
                Ok(MultipartSnapshotData { mime_type, parts })
            }
            _ => Err(format!(
                "Expected Record with 2 fields for MultipartSnapshotData, got {:?}",
                value
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicSnapshotData {
    Raw(RawSnapshotData),
    Json(JsonSnapshotData),
    Multipart(MultipartSnapshotData),
}

#[cfg(feature = "full")]
impl golem_wasm::IntoValue for PublicSnapshotData {
    fn into_value(self) -> golem_wasm::Value {
        match self {
            PublicSnapshotData::Raw(raw) => golem_wasm::Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(raw.into_value())),
            },
            PublicSnapshotData::Json(json) => golem_wasm::Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(json.into_value())),
            },
            PublicSnapshotData::Multipart(multipart) => golem_wasm::Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(multipart.into_value())),
            },
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::variant(vec![
            golem_wasm::analysis::analysed_type::case("Raw", RawSnapshotData::get_type()),
            golem_wasm::analysis::analysed_type::case("Json", JsonSnapshotData::get_type()),
            golem_wasm::analysis::analysed_type::case(
                "Multipart",
                MultipartSnapshotData::get_type(),
            ),
        ])
    }
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for PublicSnapshotData {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Variant {
                case_idx: 0,
                case_value,
            } => {
                let inner = case_value.ok_or("Missing case value for Raw")?;
                Ok(PublicSnapshotData::Raw(
                    <RawSnapshotData as golem_wasm::FromValue>::from_value(*inner)?,
                ))
            }
            golem_wasm::Value::Variant {
                case_idx: 1,
                case_value,
            } => {
                let inner = case_value.ok_or("Missing case value for Json")?;
                Ok(PublicSnapshotData::Json(
                    <JsonSnapshotData as golem_wasm::FromValue>::from_value(*inner)?,
                ))
            }
            golem_wasm::Value::Variant {
                case_idx: 2,
                case_value,
            } => {
                let inner = case_value.ok_or("Missing case value for Multipart")?;
                Ok(PublicSnapshotData::Multipart(
                    <MultipartSnapshotData as golem_wasm::FromValue>::from_value(*inner)?,
                ))
            }
            _ => Err(format!(
                "Expected Variant for PublicSnapshotData, got {:?}",
                value
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateCounter {
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateWrapper {
    pub inner: Box<PublicRetryPolicyState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateCountBox {
    pub attempts: u32,
    pub inner: Box<PublicRetryPolicyState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateTimeBox {
    pub inner: Box<PublicRetryPolicyState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateAndThen {
    pub left: Box<PublicRetryPolicyState>,
    pub right: Box<PublicRetryPolicyState>,
    pub on_right: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object, golem_wasm_derive::IntoValue))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStatePair {
    pub first: Box<PublicRetryPolicyState>,
    pub second: Box<PublicRetryPolicyState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union, golem_wasm_derive::IntoValue))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicRetryPolicyState {
    Counter(PublicRetryPolicyStateCounter),
    Terminal(Empty),
    Wrapper(PublicRetryPolicyStateWrapper),
    CountBox(PublicRetryPolicyStateCountBox),
    TimeBox(PublicRetryPolicyStateTimeBox),
    AndThen(PublicRetryPolicyStateAndThen),
    Pair(PublicRetryPolicyStatePair),
}

#[cfg(feature = "full")]
impl From<crate::model::retry_policy::RetryPolicyState> for PublicRetryPolicyState {
    fn from(value: crate::model::retry_policy::RetryPolicyState) -> Self {
        use crate::model::retry_policy::RetryPolicyState;
        match value {
            RetryPolicyState::Counter(n) => {
                PublicRetryPolicyState::Counter(PublicRetryPolicyStateCounter { count: n })
            }
            RetryPolicyState::Terminal => PublicRetryPolicyState::Terminal(Empty {}),
            RetryPolicyState::Wrapper(inner) => {
                PublicRetryPolicyState::Wrapper(PublicRetryPolicyStateWrapper {
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicyState::CountBox { attempts, inner } => {
                PublicRetryPolicyState::CountBox(PublicRetryPolicyStateCountBox {
                    attempts,
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicyState::TimeBox(inner) => {
                PublicRetryPolicyState::TimeBox(PublicRetryPolicyStateTimeBox {
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicyState::AndThen {
                left,
                right,
                on_right,
            } => PublicRetryPolicyState::AndThen(PublicRetryPolicyStateAndThen {
                left: Box::new((*left).into()),
                right: Box::new((*right).into()),
                on_right,
            }),
            RetryPolicyState::Pair(first, second) => {
                PublicRetryPolicyState::Pair(PublicRetryPolicyStatePair {
                    first: Box::new((*first).into()),
                    second: Box::new((*second).into()),
                })
            }
        }
    }
}

#[cfg(feature = "full")]
impl From<PublicRetryPolicyState> for crate::model::retry_policy::RetryPolicyState {
    fn from(value: PublicRetryPolicyState) -> Self {
        use crate::model::retry_policy::RetryPolicyState;
        match value {
            PublicRetryPolicyState::Counter(c) => RetryPolicyState::Counter(c.count),
            PublicRetryPolicyState::Terminal(_) => RetryPolicyState::Terminal,
            PublicRetryPolicyState::Wrapper(w) => {
                RetryPolicyState::Wrapper(Box::new((*w.inner).into()))
            }
            PublicRetryPolicyState::CountBox(cb) => RetryPolicyState::CountBox {
                attempts: cb.attempts,
                inner: Box::new((*cb.inner).into()),
            },
            PublicRetryPolicyState::TimeBox(tb) => {
                RetryPolicyState::TimeBox(Box::new((*tb.inner).into()))
            }
            PublicRetryPolicyState::AndThen(at) => RetryPolicyState::AndThen {
                left: Box::new((*at.left).into()),
                right: Box::new((*at.right).into()),
                on_right: at.on_right,
            },
            PublicRetryPolicyState::Pair(p) => RetryPolicyState::Pair(
                Box::new((*p.first).into()),
                Box::new((*p.second).into()),
            ),
        }
    }
}
