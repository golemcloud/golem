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

use crate::preview2::golem_api_1_x::oplog;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, BeginRemoteWriteParams,
    CancelPendingInvocationParams, ChangePersistenceLevelParams, ChangeRetryPolicyParams,
    CommittedRemoteTransactionParams, CreateParams, CreateResourceParams, DeactivatePluginParams,
    DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams, ErrorParams, ExitedParams,
    FailedUpdateParams, FinishSpanParams, GrowMemoryParams, HostCallParams, InterruptedParams,
    JumpParams, LogParams, ManualUpdateParameters, NoOpParams, PendingAgentInvocationParams,
    PendingUpdateParams, PluginInstallationDescription, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicAttributeValue, PublicDurableFunctionType, PublicRetryConfig, PublicSpanData,
    RestartParams, RevertParams, RolledBackRemoteTransactionParams, SetSpanAttributeParams,
    SnapshotParams, StartSpanParams, StringAttributeValue, SuccessfulUpdateParams, SuspendParams,
    WriteRemoteBatchedParameters, WriteRemoteTransactionParameters,
};
use golem_common::model::oplog::{
    AgentInvocationOutputParameters, FallibleResultParameters, JsonSnapshotData, PublicOplogEntry,
    PublicSnapshotData, PublicUpdateDescription, RawSnapshotData, SaveSnapshotResultParameters,
    SnapshotBasedUpdateParameters,
};
use golem_common::model::{Empty, Timestamp};
use std::time::Duration;

impl From<PublicOplogEntry> for oplog::PublicOplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create(CreateParams {
                timestamp,
                worker_id,
                component_revision,
                env,
                created_by,
                environment_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                wasi_config_vars,
                original_phantom_id: _,
            }) => Self::Create(oplog::CreateParameters {
                timestamp: timestamp.into(),
                agent_id: worker_id.into(),
                component_revision: component_revision.into(),
                args: vec![],
                env: env.into_iter().collect(),
                created_by: created_by.into(),
                environment_id: environment_id.into(),
                parent: parent.map(|id| id.into()),
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins: initial_active_plugins
                    .into_iter()
                    .map(|pr| pr.into())
                    .collect(),
                config_vars: wasi_config_vars
                    .0
                    .into_iter()
                    .map(|entry| (entry.key, entry.value))
                    .collect(),
            }),
            PublicOplogEntry::HostCall(HostCallParams {
                timestamp,
                function_name,
                request,
                response,
                durable_function_type: wrapped_function_type,
            }) => Self::HostCall(oplog::HostCallParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into(),
                response: response.into(),
                wrapped_function_type: wrapped_function_type.into(),
            }),
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                timestamp,
                invocation,
            }) => Self::AgentInvocationStarted(oplog::AgentInvocationStartedParameters {
                timestamp: timestamp.into(),
                invocation: invocation.into(),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp,
                result,
                consumed_fuel,
            }) => Self::AgentInvocationFinished(oplog::AgentInvocationFinishedParameters {
                timestamp: timestamp.into(),
                invocation_result: result.into(),
                consumed_fuel,
            }),
            PublicOplogEntry::Suspend(SuspendParams { timestamp }) => {
                Self::Suspend(timestamp.into())
            }
            PublicOplogEntry::Error(ErrorParams {
                timestamp,
                error,
                retry_from,
            }) => Self::Error(oplog::ErrorParameters {
                timestamp: timestamp.into(),
                error: error.to_string(),
                retry_from: retry_from.into(),
            }),
            PublicOplogEntry::NoOp(NoOpParams { timestamp }) => Self::NoOp(timestamp.into()),
            PublicOplogEntry::Jump(JumpParams { timestamp, jump }) => {
                Self::Jump(oplog::JumpParameters {
                    timestamp: timestamp.into(),
                    jump: oplog::OplogRegion {
                        start: jump.start.into(),
                        end: jump.end.into(),
                    },
                })
            }
            PublicOplogEntry::Interrupted(InterruptedParams { timestamp }) => {
                Self::Interrupted(timestamp.into())
            }
            PublicOplogEntry::Exited(ExitedParams { timestamp }) => Self::Exited(timestamp.into()),
            PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParams {
                timestamp,
                new_policy,
            }) => Self::ChangeRetryPolicy(oplog::ChangeRetryPolicyParameters {
                timestamp: timestamp.into(),
                new_policy: new_policy.into(),
            }),
            PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams { timestamp }) => {
                Self::BeginAtomicRegion(timestamp.into())
            }
            PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                timestamp,
                begin_index,
            }) => Self::EndAtomicRegion(oplog::EndAtomicRegionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::BeginRemoteWrite(BeginRemoteWriteParams { timestamp }) => {
                Self::BeginRemoteWrite(timestamp.into())
            }
            PublicOplogEntry::EndRemoteWrite(EndRemoteWriteParams {
                timestamp,
                begin_index,
            }) => Self::EndRemoteWrite(oplog::EndRemoteWriteParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp,
                invocation,
            }) => Self::PendingAgentInvocation(oplog::PendingAgentInvocationParameters {
                timestamp: timestamp.into(),
                invocation: invocation.into(),
            }),
            PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                timestamp,
                target_revision,
                description,
            }) => Self::PendingUpdate(oplog::PendingUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.into(),
                update_description: description.into(),
            }),
            PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                timestamp,
                target_revision,
                new_component_size,
                new_active_plugins,
            }) => Self::SuccessfulUpdate(oplog::SuccessfulUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.into(),
                new_component_size,
                new_active_plugins: new_active_plugins.into_iter().map(|pr| pr.into()).collect(),
            }),
            PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp,
                target_revision,
                details,
            }) => Self::FailedUpdate(oplog::FailedUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.into(),
                details,
            }),
            PublicOplogEntry::GrowMemory(GrowMemoryParams { timestamp, delta }) => {
                Self::GrowMemory(oplog::GrowMemoryParameters {
                    timestamp: timestamp.into(),
                    delta,
                })
            }
            PublicOplogEntry::CreateResource(CreateResourceParams {
                timestamp,
                id,
                name,
                owner,
            }) => Self::CreateResource(oplog::CreateResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
                name,
                owner,
            }),
            PublicOplogEntry::DropResource(DropResourceParams {
                timestamp,
                id,
                name,
                owner,
            }) => Self::DropResource(oplog::DropResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
                name,
                owner,
            }),
            PublicOplogEntry::Log(LogParams {
                timestamp,
                level,
                context,
                message,
            }) => Self::Log(oplog::LogParameters {
                timestamp: timestamp.into(),
                level: level.into(),
                context,
                message,
            }),
            PublicOplogEntry::Restart(RestartParams { timestamp }) => {
                Self::Restart(timestamp.into())
            }
            PublicOplogEntry::ActivatePlugin(ActivatePluginParams { timestamp, plugin }) => {
                Self::ActivatePlugin(oplog::ActivatePluginParameters {
                    timestamp: timestamp.into(),
                    plugin: plugin.into(),
                })
            }
            PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams { timestamp, plugin }) => {
                Self::DeactivatePlugin(oplog::DeactivatePluginParameters {
                    timestamp: timestamp.into(),
                    plugin: plugin.into(),
                })
            }
            PublicOplogEntry::Revert(RevertParams {
                timestamp,
                dropped_region,
            }) => Self::Revert(oplog::RevertParameters {
                timestamp: timestamp.into(),
                dropped_region: oplog::OplogRegion {
                    start: dropped_region.start.into(),
                    end: dropped_region.end.into(),
                },
            }),
            PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
                timestamp,
                idempotency_key,
            }) => Self::CancelPendingInvocation(oplog::CancelPendingInvocationParameters {
                timestamp: timestamp.into(),
                idempotency_key: idempotency_key.to_string(),
            }),
            PublicOplogEntry::StartSpan(StartSpanParams {
                timestamp,
                span_id,
                parent_id,
                linked_context,
                attributes,
            }) => Self::StartSpan(oplog::StartSpanParameters {
                timestamp: timestamp.into(),
                span_id: span_id.to_string(),
                parent: parent_id.map(|id| id.to_string()),
                linked_context_id: linked_context.map(|id| id.to_string()),
                attributes: attributes
                    .into_iter()
                    .map(|attr| oplog::Attribute {
                        key: attr.key,
                        value: attr.value.into(),
                    })
                    .collect(),
            }),
            PublicOplogEntry::FinishSpan(FinishSpanParams { timestamp, span_id }) => {
                Self::FinishSpan(oplog::FinishSpanParameters {
                    timestamp: timestamp.into(),
                    span_id: span_id.to_string(),
                })
            }
            PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
                timestamp,
                span_id,
                key,
                value,
            }) => Self::SetSpanAttribute(oplog::SetSpanAttributeParameters {
                timestamp: timestamp.into(),
                span_id: span_id.to_string(),
                key,
                value: value.into(),
            }),
            PublicOplogEntry::ChangePersistenceLevel(ChangePersistenceLevelParams {
                timestamp,
                persistence_level,
            }) => Self::ChangePersistenceLevel(oplog::ChangePersistenceLevelParameters {
                timestamp: timestamp.into(),
                persistence_level: persistence_level.into(),
            }),
            PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
                timestamp,
                transaction_id,
            }) => Self::BeginRemoteTransaction(oplog::BeginRemoteTransactionParameters {
                timestamp: timestamp.into(),
                transaction_id: transaction_id.into(),
            }),
            PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
                timestamp,
                begin_index,
            }) => Self::PreCommitRemoteTransaction(oplog::RemoteTransactionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::PreRollbackRemoteTransaction(
                PreRollbackRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            ) => Self::PreRollbackRemoteTransaction(oplog::RemoteTransactionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
                timestamp,
                begin_index,
            }) => Self::CommittedRemoteTransaction(oplog::RemoteTransactionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
                timestamp,
                begin_index,
            }) => Self::RolledBackRemoteTransaction(oplog::RemoteTransactionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::Snapshot(SnapshotParams { timestamp, data }) => {
                let (snapshot_bytes, mime_type) = match data {
                    PublicSnapshotData::Raw(RawSnapshotData { data, mime_type }) => {
                        (data, mime_type)
                    }
                    PublicSnapshotData::Json(JsonSnapshotData { data }) => (
                        serde_json::to_vec(&data).unwrap_or_default(),
                        "application/json".to_string(),
                    ),
                };
                Self::Snapshot(oplog::SnapshotParameters {
                    timestamp: timestamp.into(),
                    data: snapshot_bytes,
                    mime_type,
                })
            }
        }
    }
}

impl From<PublicDurableFunctionType> for oplog::WrappedFunctionType {
    fn from(value: PublicDurableFunctionType) -> Self {
        match value {
            PublicDurableFunctionType::WriteLocal(_) => Self::WriteLocal,
            PublicDurableFunctionType::ReadLocal(_) => Self::ReadLocal,
            PublicDurableFunctionType::WriteRemote(_) => Self::WriteRemote,
            PublicDurableFunctionType::ReadRemote(_) => Self::ReadRemote,
            PublicDurableFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                index: idx,
            }) => Self::WriteRemoteBatched(idx.map(|idx| idx.into())),
            PublicDurableFunctionType::WriteRemoteTransaction(
                WriteRemoteTransactionParameters { index: idx },
            ) => Self::WriteRemoteTransaction(idx.map(|idx| idx.into())),
        }
    }
}

impl From<PublicUpdateDescription> for oplog::UpdateDescription {
    fn from(value: PublicUpdateDescription) -> Self {
        match value {
            PublicUpdateDescription::Automatic(_) => Self::AutoUpdate,
            PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload,
                mime_type,
            }) => Self::SnapshotBased(crate::preview2::golem_api_1_x::host::Snapshot {
                data: payload,
                mime_type,
            }),
        }
    }
}

impl From<golem_common::model::oplog::LogLevel> for oplog::LogLevel {
    fn from(value: golem_common::model::oplog::LogLevel) -> Self {
        match value {
            golem_common::model::oplog::LogLevel::Trace => Self::Trace,
            golem_common::model::oplog::LogLevel::Debug => Self::Debug,
            golem_common::model::oplog::LogLevel::Info => Self::Info,
            golem_common::model::oplog::LogLevel::Warn => Self::Warn,
            golem_common::model::oplog::LogLevel::Error => Self::Error,
            golem_common::model::oplog::LogLevel::Critical => Self::Critical,
            golem_common::model::oplog::LogLevel::Stdout => Self::Stdout,
            golem_common::model::oplog::LogLevel::Stderr => Self::Stderr,
        }
    }
}

impl From<PublicAgentInvocation> for oplog::AgentInvocation {
    fn from(value: PublicAgentInvocation) -> Self {
        match value {
            PublicAgentInvocation::AgentInitialization(params) => {
                let schema = params.constructor_parameters.extract_schema();
                Self::AgentInitialization(oplog::AgentInitializationParameters {
                    idempotency_key: params.idempotency_key.value,
                    constructor_parameters: oplog::TypedDataValue {
                        value: params.constructor_parameters.into(),
                        schema: schema.into(),
                    },
                    trace_id: params.trace_id.to_string(),
                    trace_states: params.trace_states,
                    invocation_context: params
                        .invocation_context
                        .into_iter()
                        .map(|inner| inner.into_iter().map(|span| span.into()).collect())
                        .collect(),
                })
            }
            PublicAgentInvocation::AgentMethodInvocation(params) => {
                let schema = params.function_input.extract_schema();
                Self::AgentMethodInvocation(oplog::AgentMethodInvocationParameters {
                    idempotency_key: params.idempotency_key.value,
                    method_name: params.method_name,
                    function_input: oplog::TypedDataValue {
                        value: params.function_input.into(),
                        schema: schema.into(),
                    },
                    trace_id: params.trace_id.to_string(),
                    trace_states: params.trace_states,
                    invocation_context: params
                        .invocation_context
                        .into_iter()
                        .map(|inner| inner.into_iter().map(|span| span.into()).collect())
                        .collect(),
                })
            }
            PublicAgentInvocation::SaveSnapshot(_) => Self::SaveSnapshot,
            PublicAgentInvocation::LoadSnapshot(params) => {
                let (data, mime_type) = match params.snapshot {
                    PublicSnapshotData::Raw(RawSnapshotData { data, mime_type }) => {
                        (data, mime_type)
                    }
                    PublicSnapshotData::Json(JsonSnapshotData { data }) => (
                        serde_json::to_vec(&data).unwrap_or_default(),
                        "application/json".to_string(),
                    ),
                };
                Self::LoadSnapshot(oplog::LoadSnapshotParameters {
                    snapshot: crate::preview2::golem_api_1_x::host::Snapshot { data, mime_type },
                })
            }
            PublicAgentInvocation::ProcessOplogEntries(params) => {
                Self::ProcessOplogEntries(oplog::ProcessOplogEntriesParameters {
                    idempotency_key: params.idempotency_key.value,
                })
            }
            PublicAgentInvocation::ManualUpdate(ManualUpdateParameters { target_revision }) => {
                Self::ManualUpdate(oplog::ManualUpdateParameters {
                    target_revision: target_revision.into(),
                })
            }
        }
    }
}

impl From<PublicAgentInvocationResult> for oplog::AgentInvocationResult {
    fn from(value: PublicAgentInvocationResult) -> Self {
        match value {
            PublicAgentInvocationResult::AgentInitialization(AgentInvocationOutputParameters {
                output,
            }) => {
                let schema = output.extract_schema();
                Self::AgentInitialization(oplog::AgentInvocationOutputParameters {
                    output: oplog::TypedDataValue {
                        value: output.into(),
                        schema: schema.into(),
                    },
                })
            }
            PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                output,
            }) => {
                let schema = output.extract_schema();
                Self::AgentMethod(oplog::AgentInvocationOutputParameters {
                    output: oplog::TypedDataValue {
                        value: output.into(),
                        schema: schema.into(),
                    },
                })
            }
            PublicAgentInvocationResult::ManualUpdate(Empty {}) => Self::ManualUpdate,
            PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters { error }) => {
                Self::LoadSnapshot(oplog::FallibleResultParameters { error })
            }
            PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                snapshot,
            }) => {
                let (snapshot_bytes, mime_type) = match snapshot {
                    PublicSnapshotData::Raw(RawSnapshotData { data, mime_type }) => {
                        (data, mime_type)
                    }
                    PublicSnapshotData::Json(JsonSnapshotData { data }) => (
                        serde_json::to_vec(&data).unwrap_or_default(),
                        "application/json".to_string(),
                    ),
                };
                Self::SaveSnapshot(oplog::SaveSnapshotResultParameters {
                    snapshot: crate::preview2::golem_api_1_x::host::Snapshot {
                        data: snapshot_bytes,
                        mime_type,
                    },
                })
            }
            PublicAgentInvocationResult::ProcessOplogEntries(FallibleResultParameters {
                error,
            }) => Self::ProcessOplogEntries(oplog::FallibleResultParameters { error }),
        }
    }
}

impl From<PublicRetryConfig> for oplog::RetryPolicy {
    fn from(value: PublicRetryConfig) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay.as_nanos() as u64,
            max_delay: value.max_delay.as_nanos() as u64,
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        }
    }
}

impl From<PluginInstallationDescription> for oplog::PluginInstallationDescription {
    fn from(value: PluginInstallationDescription) -> Self {
        Self {
            name: value.plugin_name,
            version: value.plugin_version,
            parameters: value.parameters.into_iter().collect(),
        }
    }
}

impl From<PublicSpanData> for oplog::SpanData {
    fn from(value: PublicSpanData) -> Self {
        match value {
            PublicSpanData::LocalSpan(local_span) => Self::LocalSpan(oplog::LocalSpanData {
                span_id: local_span.span_id.to_string(),
                start: local_span.start.into(),
                parent: local_span.parent_id.map(|id| id.to_string()),
                linked_context: local_span.linked_context,
                attributes: local_span
                    .attributes
                    .into_iter()
                    .map(|attr| oplog::Attribute {
                        key: attr.key,
                        value: attr.value.into(),
                    })
                    .collect(),
                inherited: local_span.inherited,
            }),
            PublicSpanData::ExternalSpan(external_span) => {
                Self::ExternalSpan(oplog::ExternalSpanData {
                    span_id: external_span.span_id.to_string(),
                })
            }
        }
    }
}

impl From<PublicAttributeValue> for oplog::AttributeValue {
    fn from(value: PublicAttributeValue) -> Self {
        match value {
            PublicAttributeValue::String(StringAttributeValue { value }) => Self::String(value),
        }
    }
}

impl From<EnvironmentId> for oplog::EnvironmentId {
    fn from(value: EnvironmentId) -> Self {
        Self {
            uuid: value.0.into(),
        }
    }
}

impl From<Timestamp> for oplog::Timestamp {
    fn from(value: Timestamp) -> Self {
        oplog::Timestamp {
            timestamp: value.into(),
        }
    }
}

fn timestamp_from_datetime(
    dt: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
) -> Timestamp {
    Timestamp::from(dt.seconds * 1000 + (dt.nanoseconds / 1_000_000) as u64)
}

fn oplog_payload_from_wit<T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq>(
    payload: oplog::OplogPayload,
) -> golem_common::model::oplog::payload::OplogPayload<T> {
    match payload {
        oplog::OplogPayload::Inline(bytes) => {
            golem_common::model::oplog::payload::OplogPayload::SerializedInline(bytes)
        }
        oplog::OplogPayload::External(ext) => {
            golem_common::model::oplog::payload::OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId(
                    uuid::Uuid::from_u64_pair(ext.payload_id.high_bits, ext.payload_id.low_bits),
                ),
                md5_hash: ext.md5_hash,
            }
        }
    }
}

impl From<oplog::WorkerError> for golem_common::model::oplog::WorkerError {
    fn from(err: oplog::WorkerError) -> Self {
        match err {
            oplog::WorkerError::Unknown(msg) => Self::Unknown(msg),
            oplog::WorkerError::InvalidRequest(msg) => Self::InvalidRequest(msg),
            oplog::WorkerError::StackOverflow => Self::StackOverflow,
            oplog::WorkerError::OutOfMemory => Self::OutOfMemory,
            oplog::WorkerError::ExceededMemoryLimit => Self::ExceededMemoryLimit,
            oplog::WorkerError::AgentError(msg) => Self::AgentError(msg),
        }
    }
}

// Note: From<oplog::WrappedFunctionType> for DurableFunctionType is provided by golem_common's derive macros

impl From<oplog::LogLevel> for golem_common::model::oplog::LogLevel {
    fn from(level: oplog::LogLevel) -> Self {
        match level {
            oplog::LogLevel::Trace => Self::Trace,
            oplog::LogLevel::Debug => Self::Debug,
            oplog::LogLevel::Info => Self::Info,
            oplog::LogLevel::Warn => Self::Warn,
            oplog::LogLevel::Error => Self::Error,
            oplog::LogLevel::Critical => Self::Critical,
            oplog::LogLevel::Stdout => Self::Stdout,
            oplog::LogLevel::Stderr => Self::Stderr,
        }
    }
}

impl TryFrom<oplog::RawUpdateDescription> for golem_common::model::oplog::UpdateDescription {
    type Error = String;

    fn try_from(desc: oplog::RawUpdateDescription) -> Result<Self, String> {
        match desc {
            oplog::RawUpdateDescription::Automatic(target_revision) => {
                Ok(Self::Automatic {
                    target_revision:
                        golem_common::model::component::ComponentRevision::try_from(
                            target_revision,
                        )
                        .map_err(|e| e.to_string())?,
                })
            }
            oplog::RawUpdateDescription::SnapshotBased(sbu) => {
                Ok(Self::SnapshotBased {
                    target_revision:
                        golem_common::model::component::ComponentRevision::try_from(
                            sbu.target_revision,
                        )
                        .map_err(|e| e.to_string())?,
                    payload: oplog_payload_from_wit(sbu.payload),
                    mime_type: sbu.mime_type,
                })
            }
        }
    }
}

impl TryFrom<oplog::SpanData> for golem_common::model::oplog::SpanData {
    type Error = String;

    fn try_from(span: oplog::SpanData) -> Result<Self, String> {
        match span {
            oplog::SpanData::LocalSpan(local) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(&local.span_id)?;
                let start = timestamp_from_datetime(local.start);
                let parent_id = local
                    .parent
                    .map(|p| golem_common::model::invocation_context::SpanId::from_string(&p))
                    .transpose()?;
                let attributes = local
                    .attributes
                    .into_iter()
                    .map(|attr| (attr.key, attr.value.into()))
                    .collect();
                Ok(Self::LocalSpan {
                    span_id,
                    start,
                    parent_id,
                    linked_context: None,
                    attributes,
                    inherited: local.inherited,
                })
            }
            oplog::SpanData::ExternalSpan(ext) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(&ext.span_id)?;
                Ok(Self::ExternalSpan { span_id })
            }
        }
    }
}

// Note: From<oplog::AttributeValue> for AttributeValue is provided in invocation_context_api.rs
// Note: From<oplog::PersistenceLevel> for PersistenceLevel is provided in model/mod.rs

impl TryFrom<oplog::OplogEntry> for golem_common::model::oplog::OplogEntry {
    type Error = String;

    fn try_from(value: oplog::OplogEntry) -> Result<Self, String> {
        match value {
            oplog::OplogEntry::Create(params) => Ok(Self::Create {
                timestamp: timestamp_from_datetime(params.timestamp),
                worker_id: golem_common::model::WorkerId::from(params.worker_id),
                component_revision:
                    golem_common::model::component::ComponentRevision::try_from(
                        params.component_revision,
                    )
                    .map_err(|e| e.to_string())?,
                env: params.env,
                environment_id: EnvironmentId::from(uuid::Uuid::from_u64_pair(
                    params.environment_id.uuid.high_bits,
                    params.environment_id.uuid.low_bits,
                )),
                created_by: golem_common::model::account::AccountId::from(
                    uuid::Uuid::from_u64_pair(
                        params.created_by.uuid.high_bits,
                        params.created_by.uuid.low_bits,
                    ),
                ),
                parent: params.parent.map(golem_common::model::WorkerId::from),
                component_size: params.component_size,
                initial_total_linear_memory_size: params.initial_total_linear_memory_size,
                initial_active_plugins: params
                    .initial_active_plugins
                    .into_iter()
                    .map(golem_common::model::component::PluginPriority)
                    .collect(),
                wasi_config_vars: params.wasi_config_vars.into_iter().collect(),
                original_phantom_id: params.original_phantom_id.map(|uuid| {
                    uuid::Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits)
                }),
            }),
            oplog::OplogEntry::HostCall(params) => Ok(Self::HostCall {
                timestamp: timestamp_from_datetime(params.timestamp),
                function_name:
                    golem_common::model::oplog::payload::host_functions::HostFunctionName::from(
                        params.function_name.as_str(),
                    ),
                request: oplog_payload_from_wit(params.request),
                response: oplog_payload_from_wit(params.response),
                durable_function_type: params.durable_function_type.into(),
            }),
            oplog::OplogEntry::AgentInvocationStarted(params) => {
                let trace_id = golem_common::model::invocation_context::TraceId::from_string(
                    &params.trace_id,
                )?;
                let invocation_context = params
                    .invocation_context
                    .into_iter()
                    .map(golem_common::model::oplog::SpanData::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::AgentInvocationStarted {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    idempotency_key: golem_common::model::IdempotencyKey::new(
                        params.idempotency_key,
                    ),
                    payload: oplog_payload_from_wit(params.payload),
                    trace_id,
                    trace_states: params.trace_states,
                    invocation_context,
                })
            }
            oplog::OplogEntry::AgentInvocationFinished(params) => {
                Ok(Self::AgentInvocationFinished {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    result: oplog_payload_from_wit(params.result),
                    consumed_fuel: params.consumed_fuel,
                })
            }
            oplog::OplogEntry::Suspend(ts) => Ok(Self::Suspend {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::Error(params) => Ok(Self::Error {
                timestamp: timestamp_from_datetime(params.timestamp),
                error: params.error.into(),
                retry_from: golem_common::model::OplogIndex::from_u64(params.retry_from),
            }),
            oplog::OplogEntry::NoOp(ts) => Ok(Self::NoOp {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::Jump(params) => Ok(Self::Jump {
                timestamp: timestamp_from_datetime(params.timestamp),
                jump: golem_common::model::regions::OplogRegion {
                    start: golem_common::model::OplogIndex::from_u64(params.jump.start),
                    end: golem_common::model::OplogIndex::from_u64(params.jump.end),
                },
            }),
            oplog::OplogEntry::Interrupted(ts) => Ok(Self::Interrupted {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::Exited(ts) => Ok(Self::Exited {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::ChangeRetryPolicy(params) => Ok(Self::ChangeRetryPolicy {
                timestamp: timestamp_from_datetime(params.timestamp),
                new_policy: golem_common::model::RetryConfig {
                    max_attempts: params.new_policy.max_attempts,
                    min_delay: Duration::from_nanos(params.new_policy.min_delay),
                    max_delay: Duration::from_nanos(params.new_policy.max_delay),
                    multiplier: params.new_policy.multiplier,
                    max_jitter_factor: params.new_policy.max_jitter_factor,
                },
            }),
            oplog::OplogEntry::BeginAtomicRegion(ts) => Ok(Self::BeginAtomicRegion {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::EndAtomicRegion(params) => Ok(Self::EndAtomicRegion {
                timestamp: timestamp_from_datetime(params.timestamp),
                begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
            }),
            oplog::OplogEntry::BeginRemoteWrite(ts) => Ok(Self::BeginRemoteWrite {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::EndRemoteWrite(params) => Ok(Self::EndRemoteWrite {
                timestamp: timestamp_from_datetime(params.timestamp),
                begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
            }),
            oplog::OplogEntry::PendingAgentInvocation(params) => {
                let trace_id = golem_common::model::invocation_context::TraceId::from_string(
                    &params.trace_id,
                )?;
                let invocation_context = params
                    .invocation_context
                    .into_iter()
                    .map(golem_common::model::oplog::SpanData::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::PendingAgentInvocation {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    idempotency_key: golem_common::model::IdempotencyKey::new(
                        params.idempotency_key,
                    ),
                    payload: oplog_payload_from_wit(params.payload),
                    trace_id,
                    trace_states: params.trace_states,
                    invocation_context,
                })
            }
            oplog::OplogEntry::PendingUpdate(params) => Ok(Self::PendingUpdate {
                timestamp: timestamp_from_datetime(params.timestamp),
                description: params.description.try_into()?,
            }),
            oplog::OplogEntry::SuccessfulUpdate(params) => Ok(Self::SuccessfulUpdate {
                timestamp: timestamp_from_datetime(params.timestamp),
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    params.target_revision,
                )
                .map_err(|e| e.to_string())?,
                new_component_size: params.new_component_size,
                new_active_plugins: params
                    .new_active_plugins
                    .into_iter()
                    .map(golem_common::model::component::PluginPriority)
                    .collect(),
            }),
            oplog::OplogEntry::FailedUpdate(params) => Ok(Self::FailedUpdate {
                timestamp: timestamp_from_datetime(params.timestamp),
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    params.target_revision,
                )
                .map_err(|e| e.to_string())?,
                details: params.details,
            }),
            oplog::OplogEntry::GrowMemory(params) => Ok(Self::GrowMemory {
                timestamp: timestamp_from_datetime(params.timestamp),
                delta: params.delta,
            }),
            oplog::OplogEntry::CreateResource(params) => Ok(Self::CreateResource {
                timestamp: timestamp_from_datetime(params.timestamp),
                id: golem_common::model::oplog::WorkerResourceId(params.id),
                resource_type_id: golem_wasm::wasmtime::ResourceTypeId {
                    name: params.resource_type_id.name,
                    owner: params.resource_type_id.owner,
                },
            }),
            oplog::OplogEntry::DropResource(params) => Ok(Self::DropResource {
                timestamp: timestamp_from_datetime(params.timestamp),
                id: golem_common::model::oplog::WorkerResourceId(params.id),
                resource_type_id: golem_wasm::wasmtime::ResourceTypeId {
                    name: params.resource_type_id.name,
                    owner: params.resource_type_id.owner,
                },
            }),
            oplog::OplogEntry::Log(params) => Ok(Self::Log {
                timestamp: timestamp_from_datetime(params.timestamp),
                level: params.level.into(),
                context: params.context,
                message: params.message,
            }),
            oplog::OplogEntry::Restart(ts) => Ok(Self::Restart {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::ActivatePlugin(params) => Ok(Self::ActivatePlugin {
                timestamp: timestamp_from_datetime(params.timestamp),
                plugin_priority: golem_common::model::component::PluginPriority(
                    params.plugin_priority,
                ),
            }),
            oplog::OplogEntry::DeactivatePlugin(params) => Ok(Self::DeactivatePlugin {
                timestamp: timestamp_from_datetime(params.timestamp),
                plugin_priority: golem_common::model::component::PluginPriority(
                    params.plugin_priority,
                ),
            }),
            oplog::OplogEntry::Revert(params) => Ok(Self::Revert {
                timestamp: timestamp_from_datetime(params.timestamp),
                dropped_region: golem_common::model::regions::OplogRegion {
                    start: golem_common::model::OplogIndex::from_u64(
                        params.dropped_region.start,
                    ),
                    end: golem_common::model::OplogIndex::from_u64(params.dropped_region.end),
                },
            }),
            oplog::OplogEntry::CancelPendingInvocation(params) => {
                Ok(Self::CancelPendingInvocation {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    idempotency_key: golem_common::model::IdempotencyKey::new(
                        params.idempotency_key,
                    ),
                })
            }
            oplog::OplogEntry::StartSpan(params) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(
                        &params.span_id,
                    )?;
                let parent = params
                    .parent
                    .map(|p| {
                        golem_common::model::invocation_context::SpanId::from_string(&p)
                    })
                    .transpose()?;
                let linked_context_id = params
                    .linked_context_id
                    .map(|p| {
                        golem_common::model::invocation_context::SpanId::from_string(&p)
                    })
                    .transpose()?;
                let attributes: std::collections::HashMap<
                    String,
                    golem_common::model::invocation_context::AttributeValue,
                > = params
                    .attributes
                    .into_iter()
                    .map(|attr| (attr.key, attr.value.into()))
                    .collect();
                Ok(Self::StartSpan {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    span_id,
                    parent,
                    linked_context_id,
                    attributes: golem_common::model::oplog::AttributeMap(attributes),
                })
            }
            oplog::OplogEntry::FinishSpan(params) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(
                        &params.span_id,
                    )?;
                Ok(Self::FinishSpan {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    span_id,
                })
            }
            oplog::OplogEntry::SetSpanAttribute(params) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(
                        &params.span_id,
                    )?;
                let value = params.value.into();
                Ok(Self::SetSpanAttribute {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    span_id,
                    key: params.key,
                    value,
                })
            }
            oplog::OplogEntry::ChangePersistenceLevel(params) => {
                Ok(Self::ChangePersistenceLevel {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    persistence_level: params.persistence_level.into(),
                })
            }
            oplog::OplogEntry::BeginRemoteTransaction(params) => {
                Ok(Self::BeginRemoteTransaction {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    transaction_id: golem_common::model::TransactionId::from(
                        params.transaction_id,
                    ),
                    original_begin_index: params
                        .original_begin_index
                        .map(golem_common::model::OplogIndex::from_u64),
                })
            }
            oplog::OplogEntry::PreCommitRemoteTransaction(params) => {
                Ok(Self::PreCommitRemoteTransaction {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
                })
            }
            oplog::OplogEntry::PreRollbackRemoteTransaction(params) => {
                Ok(Self::PreRollbackRemoteTransaction {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
                })
            }
            oplog::OplogEntry::CommittedRemoteTransaction(params) => {
                Ok(Self::CommittedRemoteTransaction {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
                })
            }
            oplog::OplogEntry::RolledBackRemoteTransaction(params) => {
                Ok(Self::RolledBackRemoteTransaction {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
                })
            }
            oplog::OplogEntry::Snapshot(params) => Ok(Self::Snapshot {
                timestamp: timestamp_from_datetime(params.timestamp),
                data: oplog_payload_from_wit(params.data),
                mime_type: params.mime_type,
            }),
        }
    }
}
