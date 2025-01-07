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

use crate::model::public_oplog::{PublicOplogEntry, PublicUpdateDescription};
use crate::preview2::golem::api1_1_0::oplog;
use crate::preview2::wasi::clocks::wall_clock::Datetime;
use golem_common::model::public_oplog::{
    ActivatePluginParameters, ChangeRetryPolicyParameters, CreateParameters,
    DeactivatePluginParameters, DescribeResourceParameters, EndRegionParameters, ErrorParameters,
    ExportedFunctionCompletedParameters, ExportedFunctionInvokedParameters,
    ExportedFunctionParameters, FailedUpdateParameters, GrowMemoryParameters,
    ImportedFunctionInvokedParameters, JumpParameters, LogParameters, ManualUpdateParameters,
    PendingUpdateParameters, PendingWorkerInvocationParameters, PluginInstallationDescription,
    PublicRetryConfig, PublicWorkerInvocation, PublicWrappedFunctionType, ResourceParameters,
    SnapshotBasedUpdateParameters, SuccessfulUpdateParameters, TimestampParameter,
    WriteRemoteBatchedParameters,
};
use golem_common::model::Timestamp;

impl From<PublicOplogEntry> for oplog::OplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create(CreateParameters {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
            }) => Self::Create(oplog::CreateParameters {
                timestamp: timestamp.into(),
                worker_id: worker_id.into(),
                component_version,
                args,
                env: env.into_iter().collect(),
                account_id: oplog::AccountId {
                    value: account_id.value,
                },
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
                wrapped_function_type,
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
            }) => Self::ExportedFunctionInvoked(oplog::ExportedFunctionInvokedParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into_iter().map(|v| v.into()).collect(),
                idempotency_key: idempotency_key.value,
            }),
            PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
                timestamp,
                response,
                consumed_fuel,
            }) => Self::ExportedFunctionCompleted(oplog::ExportedFunctionCompletedParameters {
                timestamp: timestamp.into(),
                response: response.into(),
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
            PublicOplogEntry::CreateResource(ResourceParameters { timestamp, id }) => {
                Self::CreateResource(oplog::CreateResourceParameters {
                    timestamp: timestamp.into(),
                    resource_id: id.0,
                })
            }
            PublicOplogEntry::DropResource(ResourceParameters { timestamp, id }) => {
                Self::DropResource(oplog::DropResourceParameters {
                    timestamp: timestamp.into(),
                    resource_id: id.0,
                })
            }
            PublicOplogEntry::DescribeResource(DescribeResourceParameters {
                timestamp,
                id,
                resource_name,
                resource_params,
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

impl From<PublicWrappedFunctionType> for oplog::WrappedFunctionType {
    fn from(value: PublicWrappedFunctionType) -> Self {
        match value {
            PublicWrappedFunctionType::WriteLocal(_) => Self::WriteLocal,
            PublicWrappedFunctionType::ReadLocal(_) => Self::ReadLocal,
            PublicWrappedFunctionType::WriteRemote(_) => Self::WriteRemote,
            PublicWrappedFunctionType::ReadRemote(_) => Self::ReadRemote,
            PublicWrappedFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
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
