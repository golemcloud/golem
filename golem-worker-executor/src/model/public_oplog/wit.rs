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
use crate::preview2::wasi::clocks::wall_clock::Datetime;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, BeginAtomicRegionParams, BeginRemoteTransactionParams,
    BeginRemoteWriteParams, CancelPendingInvocationParams, ChangePersistenceLevelParams,
    ChangeRetryPolicyParams, CommittedRemoteTransactionParams, CreateParams, CreateResourceParams,
    DeactivatePluginParams, DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams,
    ErrorParams, ExitedParams, ExportedFunctionCompletedParams, ExportedFunctionInvokedParams,
    ExportedFunctionParameters, FailedUpdateParams, FinishSpanParams, GrowMemoryParams,
    ImportedFunctionInvokedParams, InterruptedParams, JumpParams, LogParams,
    ManualUpdateParameters, NoOpParams, PendingUpdateParams, PendingWorkerInvocationParams,
    PluginInstallationDescription, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, PublicAttributeValue, PublicDurableFunctionType,
    PublicRetryConfig, PublicSpanData, PublicWorkerInvocation, RestartParams, RevertParams,
    RolledBackRemoteTransactionParams, SetSpanAttributeParams, StartSpanParams,
    StringAttributeValue, SuccessfulUpdateParams, SuspendParams, WriteRemoteBatchedParameters,
    WriteRemoteTransactionParameters,
};
use golem_common::model::oplog::{
    PublicOplogEntry, PublicUpdateDescription, SnapshotBasedUpdateParameters,
};
use golem_common::model::Timestamp;

impl From<PublicOplogEntry> for oplog::OplogEntry {
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
                component_revision: component_revision.0,
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
            PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParams {
                timestamp,
                function_name,
                request,
                response,
                durable_function_type: wrapped_function_type,
            }) => Self::ImportedFunctionInvoked(oplog::ImportedFunctionInvokedParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into(),
                response: response.into(),
                wrapped_function_type: wrapped_function_type.into(),
            }),
            PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParams {
                timestamp,
                function_name,
                request,
                idempotency_key,
                trace_id,
                trace_states,
                invocation_context,
            }) => Self::ExportedFunctionInvoked(oplog::ExportedFunctionInvokedParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into_iter().map(|v| v.into()).collect(),
                idempotency_key: idempotency_key.value,
                trace_id: trace_id.to_string(),
                trace_states,
                invocation_context: invocation_context
                    .into_iter()
                    .map(|inner| inner.into_iter().map(|span| span.into()).collect())
                    .collect(),
            }),
            PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParams {
                timestamp,
                response,
                consumed_fuel,
            }) => Self::ExportedFunctionCompleted(oplog::ExportedFunctionCompletedParameters {
                timestamp: timestamp.into(),
                response: response.map(golem_wasm::golem_rpc_0_2_x::types::ValueAndType::from),
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
            PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParams {
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
                target_revision: target_revision.0,
                update_description: description.into(),
            }),
            PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                timestamp,
                target_revision,
                new_component_size,
                new_active_plugins,
            }) => Self::SuccessfulUpdate(oplog::SuccessfulUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.0,
                new_component_size,
                new_active_plugins: new_active_plugins.into_iter().map(|pr| pr.into()).collect(),
            }),
            PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp,
                target_revision,
                details,
            }) => Self::FailedUpdate(oplog::FailedUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.0,
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
                start: dropped_region.start.into(),
                end: dropped_region.end.into(),
            }),
            PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
                timestamp,
                idempotency_key,
            }) => Self::CancelInvocation(oplog::CancelInvocationParameters {
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
                linked_context: linked_context.map(|id| id.to_string()),
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
        }
    }
}

impl From<Timestamp> for Datetime {
    fn from(value: Timestamp) -> Self {
        let ms = value.to_millis();
        Self {
            seconds: ms / 1000,
            nanoseconds: ((ms % 1000) * 1_000_000) as u32,
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
            PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters { payload }) => {
                Self::SnapshotBased(payload)
            }
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

impl From<PublicWorkerInvocation> for oplog::AgentInvocation {
    fn from(value: PublicWorkerInvocation) -> Self {
        match value {
            PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                idempotency_key,
                full_function_name,
                function_input,
                trace_id,
                trace_states,
                invocation_context,
            }) => Self::ExportedFunction(oplog::ExportedFunctionInvocationParameters {
                function_name: full_function_name,
                input: function_input.map(|input| input.into_iter().map(|v| v.into()).collect()),
                idempotency_key: idempotency_key.value,
                trace_id: trace_id.to_string(),
                trace_states,
                invocation_context: invocation_context
                    .into_iter()
                    .map(|inner| inner.into_iter().map(|span| span.into()).collect())
                    .collect(),
            }),
            PublicWorkerInvocation::ManualUpdate(ManualUpdateParameters { target_revision }) => {
                Self::ManualUpdate(target_revision.0)
            }
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
