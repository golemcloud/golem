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

use crate::model::public_oplog::{PublicOplogEntry, PublicUpdateDescription};
use crate::preview2::golem_api_1_x::oplog;
use crate::preview2::wasi::clocks::wall_clock::Datetime;
use golem_common::base_model::ProjectId;
use golem_common::model::public_oplog::{
    ActivatePluginParameters, CancelInvocationParameters, ChangePersistenceLevelParameters,
    ChangeRetryPolicyParameters, CreateAgentInstanceParameters, CreateParameters,
    DeactivatePluginParameters, DescribeResourceParameters, DropAgentInstanceParameters,
    EndRegionParameters, ErrorParameters, ExportedFunctionCompletedParameters,
    ExportedFunctionInvokedParameters, ExportedFunctionParameters, FailedUpdateParameters,
    FinishSpanParameters, GrowMemoryParameters, ImportedFunctionInvokedParameters, JumpParameters,
    LogParameters, ManualUpdateParameters, PendingUpdateParameters,
    PendingWorkerInvocationParameters, PluginInstallationDescription, PublicAttributeValue,
    PublicDurableFunctionType, PublicRetryConfig, PublicSpanData, PublicWorkerInvocation,
    ResourceParameters, RevertParameters, SetSpanAttributeParameters,
    SnapshotBasedUpdateParameters, StartSpanParameters, StringAttributeValue,
    SuccessfulUpdateParameters, TimestampParameter, WriteRemoteBatchedParameters,
};
use golem_common::model::Timestamp;
use golem_wasm_rpc::WitValue;

impl From<PublicOplogEntry> for oplog::OplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create(CreateParameters {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                created_by,
                project_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                wasi_config_vars: _,
            }) => Self::Create(oplog::CreateParameters {
                timestamp: timestamp.into(),
                worker_id: worker_id.into(),
                component_version,
                args,
                env: env.into_iter().collect(),
                created_by: oplog::AccountId {
                    value: created_by.value,
                },
                project_id: project_id.into(),
                parent: parent.map(|id| id.into()),
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins: initial_active_plugins
                    .into_iter()
                    .map(|pr| pr.into())
                    .collect(),
            }),
            PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
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
            PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParameters {
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
            PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
                timestamp,
                response,
                consumed_fuel,
            }) => Self::ExportedFunctionCompleted(oplog::ExportedFunctionCompletedParameters {
                timestamp: timestamp.into(),
                response: response.map(WitValue::from),
                consumed_fuel,
            }),
            PublicOplogEntry::Suspend(TimestampParameter { timestamp }) => {
                Self::Suspend(timestamp.into())
            }
            PublicOplogEntry::Error(ErrorParameters { timestamp, error }) => {
                Self::Error(oplog::ErrorParameters {
                    timestamp: timestamp.into(),
                    error: error.to_string(),
                })
            }
            PublicOplogEntry::NoOp(TimestampParameter { timestamp }) => {
                Self::NoOp(timestamp.into())
            }
            PublicOplogEntry::Jump(JumpParameters { timestamp, jump }) => {
                Self::Jump(oplog::JumpParameters {
                    timestamp: timestamp.into(),
                    start: jump.start.into(),
                    end: jump.end.into(),
                })
            }
            PublicOplogEntry::Interrupted(TimestampParameter { timestamp }) => {
                Self::Interrupted(timestamp.into())
            }
            PublicOplogEntry::Exited(TimestampParameter { timestamp }) => {
                Self::Exited(timestamp.into())
            }
            PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParameters {
                timestamp,
                new_policy,
            }) => Self::ChangeRetryPolicy(oplog::ChangeRetryPolicyParameters {
                timestamp: timestamp.into(),
                retry_policy: new_policy.into(),
            }),
            PublicOplogEntry::BeginAtomicRegion(TimestampParameter { timestamp }) => {
                Self::BeginAtomicRegion(timestamp.into())
            }
            PublicOplogEntry::EndAtomicRegion(EndRegionParameters {
                timestamp,
                begin_index,
            }) => Self::EndAtomicRegion(oplog::EndAtomicRegionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::BeginRemoteWrite(TimestampParameter { timestamp }) => {
                Self::BeginRemoteWrite(timestamp.into())
            }
            PublicOplogEntry::EndRemoteWrite(EndRegionParameters {
                timestamp,
                begin_index,
            }) => Self::EndRemoteWrite(oplog::EndRemoteWriteParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParameters {
                timestamp,
                invocation,
            }) => Self::PendingWorkerInvocation(oplog::PendingWorkerInvocationParameters {
                timestamp: timestamp.into(),
                invocation: invocation.into(),
            }),
            PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
                timestamp,
                target_version,
                description,
            }) => Self::PendingUpdate(oplog::PendingUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                update_description: description.into(),
            }),
            PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParameters {
                timestamp,
                target_version,
                new_component_size,
                new_active_plugins,
            }) => Self::SuccessfulUpdate(oplog::SuccessfulUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                new_component_size,
                new_active_plugins: new_active_plugins.into_iter().map(|pr| pr.into()).collect(),
            }),
            PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
                timestamp,
                target_version,
                details,
            }) => Self::FailedUpdate(oplog::FailedUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                details,
            }),
            PublicOplogEntry::GrowMemory(GrowMemoryParameters { timestamp, delta }) => {
                Self::GrowMemory(oplog::GrowMemoryParameters {
                    timestamp: timestamp.into(),
                    delta,
                })
            }
            PublicOplogEntry::CreateResource(ResourceParameters {
                timestamp,
                id,
                name: _,  // TODO
                owner: _, // TODO
            }) => Self::CreateResource(oplog::CreateResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
            }),
            PublicOplogEntry::DropResource(ResourceParameters {
                timestamp,
                id,
                name: _,  // TODO
                owner: _, // TODO
            }) => Self::DropResource(oplog::DropResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
            }),
            PublicOplogEntry::DescribeResource(DescribeResourceParameters {
                timestamp,
                id,
                resource_name,
                resource_params,
                resource_owner: _, // TODO
            }) => Self::DescribeResource(oplog::DescribeResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
                resource_name,
                resource_params: resource_params
                    .into_iter()
                    .map(|value| value.into())
                    .collect(),
            }),
            PublicOplogEntry::Log(LogParameters {
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
            PublicOplogEntry::Restart(TimestampParameter { timestamp }) => {
                Self::Restart(timestamp.into())
            }
            PublicOplogEntry::ActivatePlugin(ActivatePluginParameters { timestamp, plugin }) => {
                Self::ActivatePlugin(oplog::ActivatePluginParameters {
                    timestamp: timestamp.into(),
                    plugin: plugin.into(),
                })
            }
            PublicOplogEntry::DeactivatePlugin(DeactivatePluginParameters {
                timestamp,
                plugin,
            }) => Self::DeactivatePlugin(oplog::DeactivatePluginParameters {
                timestamp: timestamp.into(),
                plugin: plugin.into(),
            }),
            PublicOplogEntry::Revert(RevertParameters {
                timestamp,
                dropped_region,
            }) => Self::Revert(oplog::RevertParameters {
                timestamp: timestamp.into(),
                start: dropped_region.start.into(),
                end: dropped_region.end.into(),
            }),
            PublicOplogEntry::CancelInvocation(CancelInvocationParameters {
                timestamp,
                idempotency_key,
            }) => Self::CancelInvocation(oplog::CancelInvocationParameters {
                timestamp: timestamp.into(),
                idempotency_key: idempotency_key.to_string(),
            }),
            PublicOplogEntry::StartSpan(StartSpanParameters {
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
            PublicOplogEntry::FinishSpan(FinishSpanParameters { timestamp, span_id }) => {
                Self::FinishSpan(oplog::FinishSpanParameters {
                    timestamp: timestamp.into(),
                    span_id: span_id.to_string(),
                })
            }
            PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParameters {
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
            PublicOplogEntry::ChangePersistenceLevel(ChangePersistenceLevelParameters {
                timestamp,
                persistence_level,
            }) => Self::ChangePersistenceLevel(oplog::ChangePersistenceLevelParameters {
                timestamp: timestamp.into(),
                persistence_level: persistence_level.into(),
            }),
            PublicOplogEntry::CreateAgentInstance(CreateAgentInstanceParameters {
                timestamp,
                key,
                parameters,
            }) => {
                // TODO: add this to WIT - until then we temporarily represent with a log entry
                Self::Log(oplog::LogParameters {
                    timestamp: timestamp.into(),
                    level: golem_common::model::oplog::LogLevel::Info.into(),
                    context: "CreateAgentInstance".to_string(),
                    message: format!("Key: {key:?}, Parameters: {parameters:?}"),
                })
            }
            PublicOplogEntry::DropAgentInstance(DropAgentInstanceParameters { timestamp, key }) => {
                // TODO: add this to WIT - until then we temporarily represent with a log entry
                Self::Log(oplog::LogParameters {
                    timestamp: timestamp.into(),
                    level: golem_common::model::oplog::LogLevel::Info.into(),
                    context: "CreateAgentInstance".to_string(),
                    message: format!("Key: {key:?}"),
                })
            }
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

impl From<PublicWorkerInvocation> for oplog::WorkerInvocation {
    fn from(value: PublicWorkerInvocation) -> Self {
        match value {
            PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                idempotency_key,
                full_function_name,
                function_input,
                ..
            }) => Self::ExportedFunction(oplog::ExportedFunctionInvocationParameters {
                function_name: full_function_name,
                input: function_input.map(|input| input.into_iter().map(|v| v.into()).collect()),
                idempotency_key: idempotency_key.value,
            }),
            PublicWorkerInvocation::ManualUpdate(ManualUpdateParameters { target_version }) => {
                Self::ManualUpdate(target_version)
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
            installation_id: value.installation_id.0.into(),
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

impl From<ProjectId> for oplog::ProjectId {
    fn from(value: ProjectId) -> Self {
        Self {
            uuid: value.0.into(),
        }
    }
}
