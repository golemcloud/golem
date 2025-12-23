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

use crate::base_model::OplogIndex;
use crate::model::component::{ComponentRevision, PluginPriority};
use crate::model::invocation_context::{AttributeValue, SpanId, TraceId};
use crate::model::oplog::public_oplog_entry::{Deserialize, Serialize};
use crate::model::oplog::DurableFunctionType;
use crate::model::{Empty, IdempotencyKey, RetryConfig, Timestamp};
use desert_rust::BinaryCodec;
use golem_wasm::ValueAndType;
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::time::Duration;

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
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

impl From<RetryConfig> for PublicRetryConfig {
    fn from(retry_config: RetryConfig) -> Self {
        PublicRetryConfig {
            max_attempts: retry_config.max_attempts,
            min_delay: retry_config.min_delay,
            max_delay: retry_config.max_delay,
            multiplier: retry_config.multiplier,
            max_jitter_factor: retry_config.max_jitter_factor,
        }
    }
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct ExportedFunctionParameters {
    pub idempotency_key: IdempotencyKey,
    pub full_function_name: String,
    pub function_input: Option<Vec<ValueAndType>>,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
pub struct ManualUpdateParameters {
    pub target_revision: ComponentRevision,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, poem_openapi::Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicWorkerInvocation {
    ExportedFunction(ExportedFunctionParameters),
    ManualUpdate(ManualUpdateParameters),
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
pub struct SnapshotBasedUpdateParameters {
    pub payload: Vec<u8>,
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicUpdateDescription {
    #[unit_case]
    Automatic(Empty),
    SnapshotBased(SnapshotBasedUpdateParameters),
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
pub struct WriteRemoteBatchedParameters {
    pub index: Option<OplogIndex>,
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteTransactionParameters {
    pub index: Option<OplogIndex>,
}

#[derive(
    Clone, Debug, Serialize, PartialEq, Deserialize, IntoValue, FromValue, poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
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

impl From<DurableFunctionType> for PublicDurableFunctionType {
    fn from(function_type: DurableFunctionType) -> Self {
        match function_type {
            DurableFunctionType::ReadLocal => PublicDurableFunctionType::ReadLocal(Empty {}),
            DurableFunctionType::WriteLocal => PublicDurableFunctionType::WriteLocal(Empty {}),
            DurableFunctionType::ReadRemote => PublicDurableFunctionType::ReadRemote(Empty {}),
            DurableFunctionType::WriteRemote => PublicDurableFunctionType::WriteRemote(Empty {}),
            DurableFunctionType::WriteRemoteBatched(index) => {
                PublicDurableFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                    index,
                })
            }
            DurableFunctionType::WriteRemoteTransaction(index) => {
                PublicDurableFunctionType::WriteRemoteTransaction(
                    WriteRemoteTransactionParameters { index },
                )
            }
        }
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[wit_transparent]
#[desert(transparent)]
pub struct StringAttributeValue {
    pub value: String,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum PublicAttributeValue {
    String(StringAttributeValue),
}

impl From<AttributeValue> for PublicAttributeValue {
    fn from(value: AttributeValue) -> Self {
        match value {
            AttributeValue::String(value) => {
                PublicAttributeValue::String(StringAttributeValue { value })
            }
        }
    }
}

#[derive(
    Clone,
    Debug,
    Serialize,
    PartialEq,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
pub struct PublicLocalSpanData {
    pub span_id: SpanId,
    pub start: Timestamp,
    pub parent_id: Option<SpanId>,
    pub linked_context: Option<u64>,
    pub attributes: Vec<PublicAttribute>,
    pub inherited: bool,
}

#[derive(
    Clone,
    Debug,
    Serialize,
    PartialEq,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
pub struct PublicAttribute {
    pub key: String,
    pub value: PublicAttributeValue,
}

#[derive(
    Clone,
    Debug,
    Serialize,
    PartialEq,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
pub struct PublicExternalSpanData {
    pub span_id: SpanId,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
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

#[derive(
    Clone,
    Debug,
    Serialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Deserialize,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationDescription {
    pub plugin_priority: PluginPriority,
    pub plugin_name: String,
    pub plugin_version: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct OplogCursor {
    pub next_oplog_index: u64,
    pub current_component_version: u64,
}

impl poem_openapi::types::ParseFromParameter for OplogCursor {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 2 {
            return Err("Invalid oplog cursor".into());
        }
        let next_oplog_index = parts[0]
            .parse()
            .map_err(|_| "Invalid index in the oplog cursor")?;
        let current_component_version = parts[1]
            .parse()
            .map_err(|_| "Invalid component version in the oplog cursor")?;
        Ok(OplogCursor {
            next_oplog_index,
            current_component_version,
        })
    }
}

impl Display for OplogCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            self.next_oplog_index, self.current_component_version
        )
    }
}
