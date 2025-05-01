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

#[cfg(feature = "protobuf")]
mod protobuf;

#[cfg(test)]
mod tests;

use crate::model::invocation_context::{AttributeValue, SpanId, TraceId};
use crate::model::lucene::{LeafQuery, Query};
use crate::model::oplog::{
    DurableFunctionType, LogLevel, OplogIndex, PersistenceLevel, WorkerResourceId,
};
use crate::model::plugin::PluginInstallation;
use crate::model::regions::OplogRegion;
use crate::model::RetryConfig;
use crate::model::{
    AccountId, ComponentVersion, Empty, IdempotencyKey, PluginInstallationId, Timestamp, WorkerId,
};
use golem_wasm_ast::analysis::analysed_type::{
    bool, case, f64, field, list, option, record, s64, str, tuple, u32, u64, u8, unit_case, variant,
};
use golem_wasm_ast::analysis::{analysed_type, AnalysedType, NameOptionTypePair};
use golem_wasm_rpc::{IntoValue, IntoValueAndType, Value, ValueAndType, WitValue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::time::Duration;

use super::plugin::{PluginDefinition, PluginOwner, PluginScope};

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotBasedUpdateParameters {
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicUpdateDescription {
    Automatic(Empty),
    SnapshotBased(SnapshotBasedUpdateParameters),
}

impl IntoValue for PublicUpdateDescription {
    fn into_value(self) -> Value {
        match self {
            PublicUpdateDescription::Automatic(_) => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            PublicUpdateDescription::SnapshotBased(params) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(params.payload.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            unit_case("auto-update"),
            case("snapshot-based", list(u8())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteBatchedParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicDurableFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    ReadLocal(Empty),
    /// The side-effect writes to the worker's local state (for example local file system)
    WriteLocal(Empty),
    /// The side-effect reads from external state (for example a key-value store)
    ReadRemote(Empty),
    /// The side-effect manipulates external state (for example an RPC call)
    WriteRemote(Empty),
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
    WriteRemoteBatched(WriteRemoteBatchedParameters),
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
        }
    }
}

impl IntoValue for PublicDurableFunctionType {
    fn into_value(self) -> Value {
        match self {
            PublicDurableFunctionType::ReadLocal(_) => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            PublicDurableFunctionType::WriteLocal(_) => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            PublicDurableFunctionType::ReadRemote(_) => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            PublicDurableFunctionType::WriteRemote(_) => Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            PublicDurableFunctionType::WriteRemoteBatched(params) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(params.index.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            unit_case("read-local"),
            unit_case("write-local"),
            unit_case("read-remote"),
            unit_case("write-remote"),
            case("write-remote-batched", option(u64())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DetailsParameter {
    pub details: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
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

impl IntoValue for PublicRetryConfig {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.max_attempts.into_value(),
            self.min_delay.into_value(),
            self.max_delay.into_value(),
            self.multiplier.into_value(),
            self.max_jitter_factor.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("max-attempts", u32()),
            field("min-delay", u64()),
            field("max-delay", u64()),
            field("multiplier", f64()),
            field("max-jitter-factor", option(f64())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ExportedFunctionParameters {
    pub idempotency_key: IdempotencyKey,
    pub full_function_name: String,
    pub function_input: Option<Vec<ValueAndType>>,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

impl IntoValue for ExportedFunctionParameters {
    fn into_value(self) -> Value {
        let wit_values: Option<Vec<WitValue>> = self
            .function_input
            .map(|inputs| inputs.into_iter().map(Into::into).collect());
        Value::Record(vec![
            self.idempotency_key.into_value(),
            self.full_function_name.into_value(),
            wit_values.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("idempotency-key", IdempotencyKey::get_type()),
            field("full-function-name", str()),
            field("function-input", option(list(WitValue::get_type()))),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ManualUpdateParameters {
    pub target_version: ComponentVersion,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicWorkerInvocation {
    ExportedFunction(ExportedFunctionParameters),
    ManualUpdate(ManualUpdateParameters),
}

impl IntoValue for PublicWorkerInvocation {
    fn into_value(self) -> Value {
        match self {
            PublicWorkerInvocation::ExportedFunction(params) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicWorkerInvocation::ManualUpdate(params) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(params.target_version.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("exported-function", ExportedFunctionParameters::get_type()),
            case("manual-update", ComponentVersion::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationDescription {
    pub installation_id: PluginInstallationId,
    pub plugin_name: String,
    pub plugin_version: String,
    pub registered: bool,
    pub parameters: BTreeMap<String, String>,
}

impl PluginInstallationDescription {
    pub fn from_definition_and_installation<Owner: PluginOwner, Scope: PluginScope>(
        definition: PluginDefinition<Owner, Scope>,
        installation: PluginInstallation,
    ) -> Self {
        Self {
            installation_id: installation.id,
            plugin_name: definition.name,
            plugin_version: definition.version,
            parameters: installation.parameters.into_iter().collect(),
            registered: !definition.deleted,
        }
    }
}

impl IntoValue for PluginInstallationDescription {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.installation_id.into_value(),
            self.plugin_name.into_value(),
            self.plugin_version.into_value(),
            Value::List(
                self.parameters
                    .into_iter()
                    .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                    .collect::<Vec<Value>>(),
            ),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("installation_id", PluginInstallationId::get_type()),
            field("name", str()),
            field("version", str()),
            field("parameters", list(tuple(vec![str(), str()]))),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CreateParameters {
    pub timestamp: Timestamp,
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub account_id: AccountId,
    pub parent: Option<WorkerId>,
    pub component_size: u64,
    pub initial_total_linear_memory_size: u64,
    pub initial_active_plugins: BTreeSet<PluginInstallationDescription>,
}

impl IntoValue for CreateParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.worker_id.into_value(),
            self.component_version.into_value(),
            self.args.into_value(),
            Value::List(
                self.env
                    .into_iter()
                    .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                    .collect::<Vec<Value>>(),
            ),
            self.account_id.into_value(),
            self.parent.into_value(),
            self.component_size.into_value(),
            self.initial_total_linear_memory_size.into_value(),
            self.initial_active_plugins
                .into_iter()
                .collect::<Vec<_>>()
                .into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("worker_id", WorkerId::get_type()),
            field("component_version", ComponentVersion::get_type()),
            field("args", list(str())),
            field("env", list(tuple(vec![str(), str()]))),
            field("account_id", AccountId::get_type()),
            field("parent", option(WorkerId::get_type())),
            field("component_size", u64()),
            field("initial_total_linear_memory_size", u64()),
            field(
                "initial_active_plugins",
                list(PluginInstallationDescription::get_type()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ImportedFunctionInvokedParameters {
    pub timestamp: Timestamp,
    pub function_name: String,
    pub request: ValueAndType,
    pub response: ValueAndType,
    pub wrapped_function_type: PublicDurableFunctionType, // TODO: rename in Golem 2.0
}

impl IntoValue for ImportedFunctionInvokedParameters {
    fn into_value(self) -> Value {
        let request_wit_value: WitValue = self.request.into();
        let response_wit_value: WitValue = self.response.into();
        Value::Record(vec![
            self.timestamp.into_value(),
            self.function_name.into_value(),
            request_wit_value.into_value(),
            response_wit_value.into_value(),
            self.wrapped_function_type.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("function_name", str()),
            field("request", WitValue::get_type()),
            field("response", WitValue::get_type()),
            field(
                "wrapped_function_type",
                PublicDurableFunctionType::get_type(),
            ),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct StringAttributeValue {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicAttributeValue {
    String(StringAttributeValue),
}

impl IntoValue for PublicAttributeValue {
    fn into_value(self) -> Value {
        match self {
            Self::String(value) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(Value::String(value.value))),
            },
        }
    }

    fn get_type() -> AnalysedType {
        analysed_type::variant(vec![analysed_type::case("string", analysed_type::str())])
    }
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

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicLocalSpanData {
    pub span_id: SpanId,
    pub start: Timestamp,
    pub parent_id: Option<SpanId>,
    pub linked_context: Option<u64>,
    pub attributes: HashMap<String, PublicAttributeValue>,
    pub inherited: bool,
}

impl IntoValue for PublicLocalSpanData {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.span_id.into_value(),
            self.start.into_value(),
            self.parent_id.into_value(),
            self.linked_context.into_value(),
            Value::List(
                self.attributes
                    .into_iter()
                    .map(|(k, v)| Value::Record(vec![k.into_value(), v.into_value()]))
                    .collect::<Vec<Value>>(),
            ),
            self.inherited.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("span-id", SpanId::get_type()),
            field("start", Timestamp::get_type()),
            field("parent-id", option(SpanId::get_type())),
            field("linked-context", option(u64())),
            field(
                "attributes",
                list(record(vec![
                    field("key", str()),
                    field("value", PublicAttributeValue::get_type()),
                ])),
            ),
            field("inherited", bool()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicExternalSpanData {
    pub span_id: SpanId,
}

impl IntoValue for PublicExternalSpanData {
    fn into_value(self) -> Value {
        Value::Record(vec![self.span_id.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![field("span-id", SpanId::get_type())])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
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

impl IntoValue for PublicSpanData {
    fn into_value(self) -> Value {
        match self {
            PublicSpanData::LocalSpan(data) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(data.into_value())),
            },
            PublicSpanData::ExternalSpan(data) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(data.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("local-span", PublicLocalSpanData::get_type()),
            case("external-span", PublicExternalSpanData::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ExportedFunctionInvokedParameters {
    pub timestamp: Timestamp,
    pub function_name: String,
    pub request: Vec<ValueAndType>,
    pub idempotency_key: IdempotencyKey,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

impl IntoValue for ExportedFunctionInvokedParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.function_name.into_value(),
            self.request
                .into_iter()
                .map(Into::into)
                .collect::<Vec<WitValue>>()
                .into_value(),
            self.idempotency_key.into_value(),
            self.trace_id.into_value(),
            self.trace_states.into_value(),
            self.invocation_context.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("function_name", str()),
            field("request", list(WitValue::get_type())),
            field("idempotency-key", IdempotencyKey::get_type()),
            field("trace-id", TraceId::get_type()),
            field("trace-states", list(str())),
            field("invocation-context", list(list(PublicSpanData::get_type()))),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ExportedFunctionCompletedParameters {
    pub timestamp: Timestamp,
    pub response: ValueAndType,
    pub consumed_fuel: i64,
}

impl IntoValue for ExportedFunctionCompletedParameters {
    fn into_value(self) -> Value {
        let wit_value: WitValue = self.response.into();
        Value::Record(vec![
            self.timestamp.into_value(),
            wit_value.into_value(),
            self.consumed_fuel.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("response", WitValue::get_type()),
            field("consumed-fuel", s64()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TimestampParameter {
    pub timestamp: Timestamp,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ErrorParameters {
    pub timestamp: Timestamp,
    pub error: String,
}

impl IntoValue for ErrorParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.error.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("error", str()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct JumpParameters {
    pub timestamp: Timestamp,
    pub jump: OplogRegion,
}

impl IntoValue for JumpParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.jump.start.into_value(),
            self.jump.end.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("start", OplogIndex::get_type()),
            field("end", OplogIndex::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ChangeRetryPolicyParameters {
    pub timestamp: Timestamp,
    pub new_policy: PublicRetryConfig,
}

impl IntoValue for ChangeRetryPolicyParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.new_policy.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("retry-policy", PublicRetryConfig::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct EndRegionParameters {
    pub timestamp: Timestamp,
    pub begin_index: OplogIndex,
}

impl IntoValue for EndRegionParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.begin_index.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("begin-index", OplogIndex::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct PendingWorkerInvocationParameters {
    pub timestamp: Timestamp,
    pub invocation: PublicWorkerInvocation,
}

impl IntoValue for PendingWorkerInvocationParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.invocation.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("invocation", PublicWorkerInvocation::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct PendingUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub description: PublicUpdateDescription,
}

impl IntoValue for PendingUpdateParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.target_version.into_value(),
            self.description.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("target-version", ComponentVersion::get_type()),
            field("update-description", PublicUpdateDescription::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct SuccessfulUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub new_component_size: u64,
    pub new_active_plugins: BTreeSet<PluginInstallationDescription>,
}

impl IntoValue for SuccessfulUpdateParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.target_version.into_value(),
            self.new_component_size.into_value(),
            self.new_active_plugins
                .into_iter()
                .collect::<Vec<_>>()
                .into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("target-version", ComponentVersion::get_type()),
            field("new-component-size", u64()),
            field(
                "new-active-plugins",
                list(PluginInstallationDescription::get_type()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct FailedUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub details: Option<String>,
}

impl IntoValue for FailedUpdateParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.target_version.into_value(),
            self.details.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("target-version", ComponentVersion::get_type()),
            field("details", option(str())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GrowMemoryParameters {
    pub timestamp: Timestamp,
    pub delta: u64,
}

impl IntoValue for GrowMemoryParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.delta.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("delta", u64()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ResourceParameters {
    pub timestamp: Timestamp,
    pub id: WorkerResourceId,
}

impl IntoValue for ResourceParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.id.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("id", WorkerResourceId::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DescribeResourceParameters {
    pub timestamp: Timestamp,
    pub id: WorkerResourceId,
    pub resource_name: String,
    pub resource_params: Vec<ValueAndType>,
}

impl IntoValue for DescribeResourceParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.id.into_value(),
            self.resource_name.into_value(),
            self.resource_params
                .into_iter()
                .map(Into::into)
                .collect::<Vec<WitValue>>()
                .into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("resource-id", WorkerResourceId::get_type()),
            field("resource-name", str()),
            field("resource-params", list(WitValue::get_type())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LogParameters {
    pub timestamp: Timestamp,
    pub level: LogLevel,
    pub context: String,
    pub message: String,
}

impl IntoValue for LogParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.level.into_value(),
            self.context.into_value(),
            self.message.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("level", LogLevel::get_type()),
            field("context", str()),
            field("message", str()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ActivatePluginParameters {
    pub timestamp: Timestamp,
    pub plugin: PluginInstallationDescription,
}

impl IntoValue for ActivatePluginParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.plugin.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("plugin", PluginInstallationDescription::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DeactivatePluginParameters {
    pub timestamp: Timestamp,
    pub plugin: PluginInstallationDescription,
}

impl IntoValue for DeactivatePluginParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.plugin.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("plugin", PluginInstallationDescription::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RevertParameters {
    pub timestamp: Timestamp,
    pub dropped_region: OplogRegion,
}

impl IntoValue for RevertParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.dropped_region.start.into_value(),
            self.dropped_region.end.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("start", OplogIndex::get_type()),
            field("end", OplogIndex::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CancelInvocationParameters {
    pub timestamp: Timestamp,
    pub idempotency_key: IdempotencyKey,
}

impl IntoValue for CancelInvocationParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.idempotency_key.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("idempotency-key", IdempotencyKey::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct StartSpanParameters {
    pub timestamp: Timestamp,
    pub span_id: SpanId,
    pub parent_id: Option<SpanId>,
    pub linked_context: Option<SpanId>,
    pub attributes: HashMap<String, PublicAttributeValue>,
}

impl IntoValue for StartSpanParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.span_id.into_value(),
            self.parent_id.into_value(),
            self.linked_context.into_value(),
            Value::List(
                self.attributes
                    .into_iter()
                    .map(|(k, v)| Value::Record(vec![k.into_value(), v.into_value()]))
                    .collect::<Vec<Value>>(),
            ),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("span-id", SpanId::get_type()),
            field("parent-id", option(SpanId::get_type())),
            field("linked-context", option(SpanId::get_type())),
            field(
                "attributes",
                list(record(vec![
                    field("key", str()),
                    field("value", PublicAttributeValue::get_type()),
                ])),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct FinishSpanParameters {
    pub timestamp: Timestamp,
    pub span_id: SpanId,
}

impl IntoValue for FinishSpanParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![self.timestamp.into_value(), self.span_id.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("span-id", SpanId::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SetSpanAttributeParameters {
    pub timestamp: Timestamp,
    pub span_id: SpanId,
    pub key: String,
    pub value: PublicAttributeValue,
}

impl IntoValue for SetSpanAttributeParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.span_id.into_value(),
            self.key.into_value(),
            self.value.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("span-id", SpanId::get_type()),
            field("key", str()),
            field("value", PublicAttributeValue::get_type()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ChangePersistenceLevelParameters {
    pub timestamp: Timestamp,
    pub persistence_level: PersistenceLevel,
}

impl IntoValue for ChangePersistenceLevelParameters {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.timestamp.into_value(),
            self.persistence_level.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("timestamp", Timestamp::get_type()),
            field("persistence-level", PersistenceLevel::get_type()),
        ])
    }
}

/// A mirror of the core `OplogEntry` type, without the undefined arbitrary payloads.
///
/// Instead, it encodes all payloads with wasm-rpc `Value` types. This makes this the base type
/// for exposing oplog entries through various APIs such as gRPC, REST and WIT.
///
/// The rest of the system will always use `OplogEntry` internally - the only point where the
/// oplog payloads are decoded and re-encoded as `Value` is in this module, and it should only be used
/// before exposing an oplog entry through a public API.
#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicOplogEntry {
    Create(CreateParameters),
    /// The worker invoked a host function
    ImportedFunctionInvoked(ImportedFunctionInvokedParameters),
    /// The worker has been invoked
    ExportedFunctionInvoked(ExportedFunctionInvokedParameters),
    /// The worker has completed an invocation
    ExportedFunctionCompleted(ExportedFunctionCompletedParameters),
    /// Worker suspended
    Suspend(TimestampParameter),
    /// Worker failed
    Error(ErrorParameters),
    /// Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
    /// more predictable.
    NoOp(TimestampParameter),
    /// The worker needs to recover up to the given target oplog index and continue running from
    /// the source oplog index from there
    /// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
    /// ignore all recorded operations in between.
    Jump(JumpParameters),
    /// Indicates that the worker has been interrupted at this point.
    /// Only used to recompute the worker's (cached) status, has no effect on execution.
    Interrupted(TimestampParameter),
    /// Indicates that the worker has been exited using WASI's exit function.
    Exited(TimestampParameter),
    /// Overrides the worker's retry policy
    ChangeRetryPolicy(ChangeRetryPolicyParameters),
    /// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
    /// recovery except if there is a corresponding `EndAtomicRegion` entry.
    BeginAtomicRegion(TimestampParameter),
    /// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
    /// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
    /// compaction.
    EndAtomicRegion(EndRegionParameters),
    /// Begins a remote write operation. Only used when idempotence mode is off. In this case each
    /// remote write must be surrounded by a `BeginRemoteWrite` and `EndRemoteWrite` log pair and
    /// unfinished remote writes cannot be recovered.
    BeginRemoteWrite(TimestampParameter),
    /// Marks the end of a remote write operation. Only used when idempotence mode is off.
    EndRemoteWrite(EndRegionParameters),
    /// An invocation request arrived while the worker was busy
    PendingWorkerInvocation(PendingWorkerInvocationParameters),
    /// An update request arrived and will be applied as soon the worker restarts
    PendingUpdate(PendingUpdateParameters),
    /// An update was successfully applied
    SuccessfulUpdate(SuccessfulUpdateParameters),
    /// An update failed to be applied
    FailedUpdate(FailedUpdateParameters),
    /// Increased total linear memory size
    GrowMemory(GrowMemoryParameters),
    /// Created a resource instance
    CreateResource(ResourceParameters),
    /// Dropped a resource instance
    DropResource(ResourceParameters),
    /// Adds additional information for a created resource instance
    DescribeResource(DescribeResourceParameters),
    /// The worker emitted a log message
    Log(LogParameters),
    /// Marks the point where the worker was restarted from clean initial state
    Restart(TimestampParameter),
    /// Activates a plugin
    ActivatePlugin(ActivatePluginParameters),
    /// Deactivates a plugin
    DeactivatePlugin(DeactivatePluginParameters),
    /// Revert a worker to a previous state
    Revert(RevertParameters),
    /// Cancel a pending invocation
    CancelInvocation(CancelInvocationParameters),
    /// Start a new span in the invocation context
    StartSpan(StartSpanParameters),
    /// Finish an open span in the invocation context
    FinishSpan(FinishSpanParameters),
    /// Set an attribute on an open span in the invocation context
    SetSpanAttribute(SetSpanAttributeParameters),
    /// Change the current persistence level
    ChangePersistenceLevel(ChangePersistenceLevelParameters),
}

impl PublicOplogEntry {
    pub fn matches(&self, query: &Query) -> bool {
        fn matches_impl(entry: &PublicOplogEntry, query: &Query, field_stack: &[String]) -> bool {
            match query {
                Query::Or { queries } => queries
                    .iter()
                    .any(|query| matches_impl(entry, query, field_stack)),
                Query::And { queries } => queries
                    .iter()
                    .all(|query| matches_impl(entry, query, field_stack)),
                Query::Not { query } => !matches_impl(entry, query, field_stack),
                Query::Regex { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Term { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Phrase { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Field { field, query } => {
                    let mut new_stack: Vec<String> = field_stack.to_vec();
                    let parts: Vec<String> = field.split(".").map(|s| s.to_string()).collect();
                    new_stack.extend(parts);
                    matches_impl(entry, query, &new_stack)
                }
            }
        }

        matches_impl(self, query, &[])
    }

    fn string_match(s: &str, path: &[String], query_path: &[String], query: &LeafQuery) -> bool {
        let lowercase_path = path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        let lowercase_query_path = query_path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        if lowercase_path == lowercase_query_path || query_path.is_empty() {
            query.matches(s)
        } else {
            false
        }
    }

    fn span_attribute_match(
        attributes: &HashMap<String, PublicAttributeValue>,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        for (key, value) in attributes {
            let mut new_path: Vec<String> = path_stack.to_vec();
            new_path.push(key.clone());

            let vnt = match value {
                PublicAttributeValue::String(StringAttributeValue { value }) => {
                    value.clone().into_value_and_type()
                }
            };

            if Self::match_value(&vnt, &new_path, query_path, query) {
                return true;
            }
        }
        false
    }

    fn matches_leaf_query(&self, query_path: &[String], query: &LeafQuery) -> bool {
        match self {
            PublicOplogEntry::Create(_params) => {
                Self::string_match("create", &[], query_path, query)
            }
            PublicOplogEntry::ImportedFunctionInvoked(params) => {
                Self::string_match("importedfunctioninvoked", &[], query_path, query)
                    || Self::string_match("imported-function-invoked", &[], query_path, query)
                    || Self::string_match("imported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || Self::match_value(&params.request, &[], query_path, query)
                    || Self::match_value(&params.response, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionInvoked(params) => {
                Self::string_match("exportedfunctioninvoked", &[], query_path, query)
                    || Self::string_match("exported-function-invoked", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || params
                        .request
                        .iter()
                        .any(|v| Self::match_value(v, &[], query_path, query))
                    || Self::string_match(&params.idempotency_key.value, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionCompleted(params) => {
                Self::string_match("exportedfunctioncompleted", &[], query_path, query)
                    || Self::string_match("exported-function-completed", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || Self::match_value(&params.response, &[], query_path, query)
                // TODO: should we store function name and idempotency key in ExportedFunctionCompleted?
            }
            PublicOplogEntry::Suspend(_params) => {
                Self::string_match("suspend", &[], query_path, query)
            }
            PublicOplogEntry::Error(params) => {
                Self::string_match("error", &[], query_path, query)
                    || Self::string_match(&params.error, &[], query_path, query)
            }
            PublicOplogEntry::NoOp(_params) => Self::string_match("noop", &[], query_path, query),
            PublicOplogEntry::Jump(_params) => Self::string_match("jump", &[], query_path, query),
            PublicOplogEntry::Interrupted(_params) => {
                Self::string_match("interrupted", &[], query_path, query)
            }
            PublicOplogEntry::Exited(_params) => {
                Self::string_match("exited", &[], query_path, query)
            }
            PublicOplogEntry::ChangeRetryPolicy(_params) => {
                Self::string_match("changeretrypolicy", &[], query_path, query)
                    || Self::string_match("change-retry-policy", &[], query_path, query)
            }
            PublicOplogEntry::BeginAtomicRegion(_params) => {
                Self::string_match("beginatomicregion", &[], query_path, query)
                    || Self::string_match("begin-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::EndAtomicRegion(_params) => {
                Self::string_match("endatomicregion", &[], query_path, query)
                    || Self::string_match("end-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::BeginRemoteWrite(_params) => {
                Self::string_match("beginremotewrite", &[], query_path, query)
                    || Self::string_match("begin-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::EndRemoteWrite(_params) => {
                Self::string_match("endremotewrite", &[], query_path, query)
                    || Self::string_match("end-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::PendingWorkerInvocation(params) => {
                Self::string_match("pendingworkerinvocation", &[], query_path, query)
                    || Self::string_match("pending-worker-invocation", &[], query_path, query)
                    || match &params.invocation {
                        PublicWorkerInvocation::ExportedFunction(params) => {
                            Self::string_match(&params.full_function_name, &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                                || params
                                    .function_input
                                    .as_ref()
                                    .map(|params| {
                                        params
                                            .iter()
                                            .any(|v| Self::match_value(v, &[], query_path, query))
                                    })
                                    .unwrap_or(false)
                        }
                        PublicWorkerInvocation::ManualUpdate(params) => Self::string_match(
                            &params.target_version.to_string(),
                            &[],
                            query_path,
                            query,
                        ),
                    }
            }
            PublicOplogEntry::PendingUpdate(params) => {
                Self::string_match("pendingupdate", &[], query_path, query)
                    || Self::string_match("pending-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::SuccessfulUpdate(params) => {
                Self::string_match("successfulupdate", &[], query_path, query)
                    || Self::string_match("successful-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::FailedUpdate(params) => {
                Self::string_match("failedupdate", &[], query_path, query)
                    || Self::string_match("failed-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
                    || params
                        .details
                        .as_ref()
                        .map(|details| Self::string_match(details, &[], query_path, query))
                        .unwrap_or(false)
            }
            PublicOplogEntry::GrowMemory(_params) => {
                Self::string_match("growmemory", &[], query_path, query)
                    || Self::string_match("grow-memory", &[], query_path, query)
            }
            PublicOplogEntry::CreateResource(_params) => {
                Self::string_match("createresource", &[], query_path, query)
                    || Self::string_match("create-resource", &[], query_path, query)
            }
            PublicOplogEntry::DropResource(_params) => {
                Self::string_match("dropresource", &[], query_path, query)
                    || Self::string_match("drop-resource", &[], query_path, query)
            }
            PublicOplogEntry::DescribeResource(params) => {
                Self::string_match("describeresource", &[], query_path, query)
                    || Self::string_match("describe-resource", &[], query_path, query)
                    || Self::string_match(&params.resource_name, &[], query_path, query)
                    || params
                        .resource_params
                        .iter()
                        .any(|v| Self::match_value(v, &[], query_path, query))
            }
            PublicOplogEntry::Log(params) => {
                Self::string_match("log", &[], query_path, query)
                    || Self::string_match(&params.context, &[], query_path, query)
                    || Self::string_match(&params.message, &[], query_path, query)
            }
            PublicOplogEntry::Restart(_params) => {
                Self::string_match("restart", &[], query_path, query)
            }
            PublicOplogEntry::ActivatePlugin(_params) => {
                Self::string_match("activateplugin", &[], query_path, query)
                    || Self::string_match("activate-plugin", &[], query_path, query)
            }
            PublicOplogEntry::DeactivatePlugin(_params) => {
                Self::string_match("deactivateplugin", &[], query_path, query)
                    || Self::string_match("deactivate-plugin", &[], query_path, query)
            }
            PublicOplogEntry::Revert(_params) => {
                Self::string_match("revert", &[], query_path, query)
            }
            PublicOplogEntry::CancelInvocation(params) => {
                Self::string_match("cancel", &[], query_path, query)
                    || Self::string_match("cancel-invocation", &[], query_path, query)
                    || Self::string_match(&params.idempotency_key.value, &[], query_path, query)
            }
            PublicOplogEntry::StartSpan(params) => {
                Self::string_match("startspan", &[], query_path, query)
                    || Self::string_match("start-span", &[], query_path, query)
                    || Self::string_match(&params.span_id.to_string(), &[], query_path, query)
                    || Self::string_match(
                        &params
                            .parent_id
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        &[],
                        query_path,
                        query,
                    )
                    || Self::string_match(
                        &params
                            .linked_context
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        &[],
                        query_path,
                        query,
                    )
                    || Self::span_attribute_match(&params.attributes, &[], query_path, query)
            }
            PublicOplogEntry::FinishSpan(params) => {
                Self::string_match("finishspan", &[], query_path, query)
                    || Self::string_match("finish-span", &[], query_path, query)
                    || Self::string_match(&params.span_id.to_string(), &[], query_path, query)
            }
            PublicOplogEntry::SetSpanAttribute(params) => {
                let mut attributes = HashMap::new();
                attributes.insert(params.key.clone(), params.value.clone());
                Self::string_match("setspanattribute", &[], query_path, query)
                    || Self::string_match("set-span-attribute", &[], query_path, query)
                    || Self::string_match(&params.key, &[], query_path, query)
                    || Self::span_attribute_match(&attributes, &[], query_path, query)
            }
            PublicOplogEntry::ChangePersistenceLevel(_params) => {
                Self::string_match("changepersistencelevel", &[], query_path, query)
                    || Self::string_match("change-persistence-level", &[], query_path, query)
                    || Self::string_match("persistence-level", &[], query_path, query)
            }
        }
    }

    fn match_value(
        value: &ValueAndType,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        match (&value.value, &value.typ) {
            (Value::Bool(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::Char(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::String(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::List(elems), AnalysedType::List(list)) => elems.iter().any(|v| {
                Self::match_value(
                    &ValueAndType::new(v.clone(), (*list.inner).clone()),
                    path_stack,
                    query_path,
                    query,
                )
            }),
            (Value::Tuple(elems), AnalysedType::Tuple(tuple)) => {
                if elems.len() != tuple.items.len() {
                    false
                } else {
                    elems
                        .iter()
                        .zip(tuple.items.iter())
                        .enumerate()
                        .any(|(idx, (v, t))| {
                            let mut new_path: Vec<String> = path_stack.to_vec();
                            new_path.push(idx.to_string());
                            Self::match_value(
                                &ValueAndType::new(v.clone(), t.clone()),
                                &new_path,
                                query_path,
                                query,
                            )
                        })
                }
            }
            (Value::Record(fields), AnalysedType::Record(record)) => {
                if fields.len() != record.fields.len() {
                    false
                } else {
                    fields.iter().zip(record.fields.iter()).any(|(v, t)| {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(t.name.clone());
                        Self::match_value(
                            &ValueAndType::new(v.clone(), t.typ.clone()),
                            &new_path,
                            path_stack,
                            query,
                        )
                    })
                }
            }
            (
                Value::Variant {
                    case_value,
                    case_idx,
                },
                AnalysedType::Variant(variant),
            ) => {
                let case = variant.cases.get(*case_idx as usize);
                match (case_value, case) {
                    (
                        Some(value),
                        Some(NameOptionTypePair {
                            typ: Some(typ),
                            name,
                        }),
                    ) => {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(name.clone());
                        Self::match_value(
                            &ValueAndType::new((**value).clone(), typ.clone()),
                            &new_path,
                            query_path,
                            query,
                        )
                    }
                    _ => false,
                }
            }
            (Value::Enum(value), AnalysedType::Enum(typ)) => {
                if let Some(case) = typ.cases.get(*value as usize) {
                    Self::string_match(case, path_stack, query_path, query)
                } else {
                    false
                }
            }
            (Value::Flags(bitmap), AnalysedType::Flags(flags)) => {
                let names = bitmap
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, set)| if *set { flags.names.get(idx) } else { None })
                    .collect::<Vec<_>>();
                names
                    .iter()
                    .any(|name| Self::string_match(name, path_stack, query_path, query))
            }
            (Value::Option(Some(value)), AnalysedType::Option(typ)) => Self::match_value(
                &ValueAndType::new((**value).clone(), (*typ.inner).clone()),
                path_stack,
                query_path,
                query,
            ),
            (Value::Result(value), AnalysedType::Result(typ)) => match value {
                Ok(Some(value)) if typ.ok.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("ok".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.ok.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                Err(Some(value)) if typ.err.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("err".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.err.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                _ => false,
            },
            (Value::Handle { .. }, _) => false,
            _ => false,
        }
    }
}

impl IntoValue for PublicOplogEntry {
    fn into_value(self) -> Value {
        match self {
            PublicOplogEntry::Create(params) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::ImportedFunctionInvoked(params) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::ExportedFunctionInvoked(params) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::ExportedFunctionCompleted(params) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::Suspend(params) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::Error(params) => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::NoOp(params) => Value::Variant {
                case_idx: 6,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::Jump(params) => Value::Variant {
                case_idx: 7,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::Interrupted(params) => Value::Variant {
                case_idx: 8,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::Exited(params) => Value::Variant {
                case_idx: 9,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::ChangeRetryPolicy(params) => Value::Variant {
                case_idx: 10,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::BeginAtomicRegion(params) => Value::Variant {
                case_idx: 11,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::EndAtomicRegion(params) => Value::Variant {
                case_idx: 12,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::BeginRemoteWrite(params) => Value::Variant {
                case_idx: 13,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::EndRemoteWrite(params) => Value::Variant {
                case_idx: 14,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::PendingWorkerInvocation(params) => Value::Variant {
                case_idx: 15,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::PendingUpdate(params) => Value::Variant {
                case_idx: 16,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::SuccessfulUpdate(params) => Value::Variant {
                case_idx: 17,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::FailedUpdate(params) => Value::Variant {
                case_idx: 18,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::GrowMemory(params) => Value::Variant {
                case_idx: 19,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::CreateResource(params) => Value::Variant {
                case_idx: 20,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::DropResource(params) => Value::Variant {
                case_idx: 21,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::DescribeResource(params) => Value::Variant {
                case_idx: 22,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::Log(params) => Value::Variant {
                case_idx: 23,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::Restart(params) => Value::Variant {
                case_idx: 24,
                case_value: Some(Box::new(params.timestamp.into_value())),
            },
            PublicOplogEntry::ActivatePlugin(params) => Value::Variant {
                case_idx: 25,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::DeactivatePlugin(params) => Value::Variant {
                case_idx: 26,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::Revert(params) => Value::Variant {
                case_idx: 27,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::CancelInvocation(params) => Value::Variant {
                case_idx: 28,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::StartSpan(params) => Value::Variant {
                case_idx: 29,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::FinishSpan(params) => Value::Variant {
                case_idx: 30,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::SetSpanAttribute(params) => Value::Variant {
                case_idx: 31,
                case_value: Some(Box::new(params.into_value())),
            },
            PublicOplogEntry::ChangePersistenceLevel(params) => Value::Variant {
                case_idx: 32,
                case_value: Some(Box::new(params.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("create", CreateParameters::get_type()),
            case(
                "imported-function-invoked",
                ImportedFunctionInvokedParameters::get_type(),
            ),
            case(
                "exported-function-invoked",
                ExportedFunctionInvokedParameters::get_type(),
            ),
            case(
                "exported-function-completed",
                ExportedFunctionCompletedParameters::get_type(),
            ),
            case("suspend", Timestamp::get_type()),
            case("error", ErrorParameters::get_type()),
            case("noop", Timestamp::get_type()),
            case("jump", JumpParameters::get_type()),
            case("interrupted", Timestamp::get_type()),
            case("exited", Timestamp::get_type()),
            case(
                "change-retry-policy",
                ChangeRetryPolicyParameters::get_type(),
            ),
            case("begin-atomic-region", Timestamp::get_type()),
            case("end-atomic-region", EndRegionParameters::get_type()),
            case("begin-remote-write", Timestamp::get_type()),
            case("end-remote-write", EndRegionParameters::get_type()),
            case(
                "pending-worker-invocation",
                PendingWorkerInvocationParameters::get_type(),
            ),
            case("pending-update", PendingUpdateParameters::get_type()),
            case("successful-update", SuccessfulUpdateParameters::get_type()),
            case("failed-update", FailedUpdateParameters::get_type()),
            case("grow-memory", GrowMemoryParameters::get_type()),
            case("create-resource", ResourceParameters::get_type()),
            case("drop-resource", ResourceParameters::get_type()),
            case("describe-resource", DescribeResourceParameters::get_type()),
            case("log", LogParameters::get_type()),
            case("restart", Timestamp::get_type()),
            case("activate-plugin", ActivatePluginParameters::get_type()),
            case("deactivate-plugin", DeactivatePluginParameters::get_type()),
            case("revert", RevertParameters::get_type()),
            case("cancel-invocation", CancelInvocationParameters::get_type()),
            case("start-span", StartSpanParameters::get_type()),
            case("finish-span", FinishSpanParameters::get_type()),
            case("set-span-attribute", SetSpanAttributeParameters::get_type()),
            case(
                "change-persistence-level",
                ChangePersistenceLevelParameters::get_type(),
            ),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OplogCursor {
    pub next_oplog_index: u64,
    pub current_component_version: u64,
}

#[cfg(feature = "poem")]
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
