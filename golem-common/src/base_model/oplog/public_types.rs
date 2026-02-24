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

use crate::base_model::component::{ComponentRevision, PluginPriority};
use crate::base_model::invocation_context::{SpanId, TraceId};
use crate::base_model::oplog::public_oplog_entry::{Deserialize, Serialize};
use crate::base_model::oplog::PublicOplogEntry;
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
    ProcessOplogEntries(FallibleResultParameters),
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
pub struct WorkerResourceId(pub u64);

impl WorkerResourceId {
    pub const INITIAL: WorkerResourceId = WorkerResourceId(0);

    pub fn next(&self) -> WorkerResourceId {
        WorkerResourceId(self.0 + 1)
    }
}

impl Display for WorkerResourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Worker log levels including the special stdout and stderr channels
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
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

impl golem_wasm::IntoValue for LogLevel {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Enum(self as u32)
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        r#enum(&[
            "stdout", "stderr", "trace", "debug", "info", "warn", "error", "critical",
        ])
        .named("log-level")
        .owned("golem:api@1.5.0/oplog")
    }
}

impl golem_wasm::FromValue for LogLevel {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Enum(idx) => match idx {
                0 => Ok(LogLevel::Stdout),
                1 => Ok(LogLevel::Stderr),
                2 => Ok(LogLevel::Trace),
                3 => Ok(LogLevel::Debug),
                4 => Ok(LogLevel::Info),
                5 => Ok(LogLevel::Warn),
                6 => Ok(LogLevel::Error),
                7 => Ok(LogLevel::Critical),
                _ => Err(format!("Invalid enum index for LogLevel: {idx}")),
            },
            other => Err(format!("Expected Enum for LogLevel, got {other:?}")),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialOrd, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

impl golem_wasm::IntoValue for PersistenceLevel {
    fn into_value(self) -> golem_wasm::Value {
        match self {
            PersistenceLevel::PersistNothing => golem_wasm::Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            PersistenceLevel::PersistRemoteSideEffects => golem_wasm::Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            PersistenceLevel::Smart => golem_wasm::Value::Variant {
                case_idx: 2,
                case_value: None,
            },
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        variant(vec![
            unit_case("persist-nothing"),
            unit_case("persist-remote-side-effects"),
            unit_case("smart"),
        ])
        .named("persistence-level")
        .owned("golem:api@1.5.0/host")
    }
}

impl golem_wasm::FromValue for PersistenceLevel {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Variant {
                case_idx,
                case_value: _,
            } => match case_idx {
                0 => Ok(PersistenceLevel::PersistNothing),
                1 => Ok(PersistenceLevel::PersistRemoteSideEffects),
                2 => Ok(PersistenceLevel::Smart),
                _ => Err(format!(
                    "Invalid case_idx for PersistenceLevel: {case_idx}"
                )),
            },
            other => Err(format!(
                "Expected Variant for PersistenceLevel, got {other:?}"
            )),
        }
    }
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
pub enum PublicSnapshotData {
    Raw(RawSnapshotData),
    Json(JsonSnapshotData),
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
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::variant(vec![
            golem_wasm::analysis::analysed_type::case("Raw", RawSnapshotData::get_type()),
            golem_wasm::analysis::analysed_type::case("Json", JsonSnapshotData::get_type()),
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
            _ => Err(format!(
                "Expected Variant for PublicSnapshotData, got {:?}",
                value
            )),
        }
    }
}
