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

use crate::model::public_oplog::{PublicOplogEntry, PublicUpdateDescription};
use crate::preview2::golem::api1_1_0_rc1::oplog::*;
use crate::preview2::wasi::clocks::wall_clock::Datetime;
use golem_common::model::oplog::WrappedFunctionType;
use golem_common::model::Timestamp;

impl From<PublicOplogEntry> for crate::preview2::golem::api1_1_0_rc1::oplog::OplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
            } => Self::Create(CreateParameters {
                timestamp: timestamp.into(),
                worker_id: worker_id.into(),
                component_version,
                args,
                env,
                account_id: AccountId {
                    value: account_id.value,
                },
                parent: parent.map(|id| id.into()),
                component_size,
                initial_total_linear_memory_size,
            }),
            PublicOplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                response,
                wrapped_function_type,
            } => Self::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into(),
                response: response.into(),
                wrapped_function_type: wrapped_function_type.into(),
            }),
            PublicOplogEntry::ExportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                idempotency_key,
            } => Self::ExportedFunctionInvoked(ExportedFunctionInvokedParameters {
                timestamp: timestamp.into(),
                function_name,
                request: request.into_iter().map(|v| v.into()).collect(),
                idempotency_key: idempotency_key.value,
            }),
            PublicOplogEntry::ExportedFunctionCompleted {
                timestamp,
                response,
                consumed_fuel,
            } => Self::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
                timestamp: timestamp.into(),
                response: response.into(),
                consumed_fuel,
            }),
            PublicOplogEntry::Suspend { timestamp } => Self::Suspend(timestamp.into()),
            PublicOplogEntry::Error { timestamp, error } => Self::Error(ErrorParameters {
                timestamp: timestamp.into(),
                error: error.to_string(""),
            }),
            PublicOplogEntry::NoOp { timestamp } => Self::NoOp(timestamp.into()),
            PublicOplogEntry::Jump { timestamp, jump } => Self::Jump(JumpParameters {
                timestamp: timestamp.into(),
                start: jump.start.into(),
                end: jump.end.into(),
            }),
            PublicOplogEntry::Interrupted { timestamp } => Self::Interrupted(timestamp.into()),
            PublicOplogEntry::Exited { timestamp } => Self::Exited(timestamp.into()),
            PublicOplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            } => Self::ChangeRetryPolicy(ChangeRetryPolicyParameters {
                timestamp: timestamp.into(),
                retry_policy: (&new_policy).into(),
            }),
            PublicOplogEntry::BeginAtomicRegion { timestamp } => {
                Self::BeginAtomicRegion(timestamp.into())
            }
            PublicOplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            } => Self::EndAtomicRegion(EndAtomicRegionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::BeginRemoteWrite { timestamp } => {
                Self::BeginRemoteWrite(timestamp.into())
            }
            PublicOplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            } => Self::EndRemoteWrite(EndRemoteWriteParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            }),
            PublicOplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
            } => Self::PendingWorkerInvocation(PendingWorkerInvocationParameters {
                timestamp: timestamp.into(),
                invocation: invocation.into(),
            }),
            PublicOplogEntry::PendingUpdate {
                timestamp,
                target_version,
                description,
            } => Self::PendingUpdate(PendingUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                update_description: description.into(),
            }),
            PublicOplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
                new_component_size,
            } => Self::SuccessfulUpdate(SuccessfulUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                new_component_size,
            }),
            PublicOplogEntry::FailedUpdate {
                timestamp,
                target_version,
                details,
            } => Self::FailedUpdate(FailedUpdateParameters {
                timestamp: timestamp.into(),
                target_version,
                details,
            }),
            PublicOplogEntry::GrowMemory { timestamp, delta } => {
                Self::GrowMemory(GrowMemoryParameters {
                    timestamp: timestamp.into(),
                    delta,
                })
            }
            PublicOplogEntry::CreateResource { timestamp, id } => {
                Self::CreateResource(CreateResourceParameters {
                    timestamp: timestamp.into(),
                    resource_id: id.0,
                })
            }
            PublicOplogEntry::DropResource { timestamp, id } => {
                Self::DropResource(DropResourceParameters {
                    timestamp: timestamp.into(),
                    resource_id: id.0,
                })
            }
            PublicOplogEntry::DescribeResource {
                timestamp,
                id,
                resource_name,
                resource_params,
            } => Self::DescribeResource(DescribeResourceParameters {
                timestamp: timestamp.into(),
                resource_id: id.0,
                resource_name,
                resource_params: resource_params
                    .into_iter()
                    .map(|value| value.into())
                    .collect(),
            }),
            PublicOplogEntry::Log {
                timestamp,
                level,
                context,
                message,
            } => Self::Log(LogParameters {
                timestamp: timestamp.into(),
                level: level.into(),
                context,
                message,
            }),
            PublicOplogEntry::Restart { timestamp } => Self::Restart(timestamp.into()),
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

impl From<WrappedFunctionType>
    for crate::preview2::golem::api1_1_0_rc1::oplog::WrappedFunctionType
{
    fn from(value: WrappedFunctionType) -> Self {
        match value {
            WrappedFunctionType::WriteLocal => Self::WriteLocal,
            WrappedFunctionType::ReadLocal => Self::ReadLocal,
            WrappedFunctionType::WriteRemote => Self::WriteRemote,
            WrappedFunctionType::ReadRemote => Self::ReadRemote,
            WrappedFunctionType::WriteRemoteBatched(idx) => {
                Self::WriteRemoteBatched(idx.map(|idx| idx.into()))
            }
        }
    }
}

impl From<PublicUpdateDescription> for UpdateDescription {
    fn from(value: PublicUpdateDescription) -> Self {
        match value {
            PublicUpdateDescription::Automatic => Self::AutoUpdate,
            PublicUpdateDescription::SnapshotBased { payload } => Self::SnapshotBased(payload),
        }
    }
}

impl From<golem_common::model::oplog::LogLevel> for LogLevel {
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

impl From<golem_common::model::WorkerInvocation> for WorkerInvocation {
    fn from(value: golem_common::model::WorkerInvocation) -> Self {
        match value {
            golem_common::model::WorkerInvocation::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
            } => Self::ExportedFunction(ExportedFunctionInvocationParameters {
                function_name: full_function_name,
                input: function_input.into_iter().map(|v| v.into()).collect(),
                idempotency_key: idempotency_key.value,
            }),
            golem_common::model::WorkerInvocation::ManualUpdate { target_version } => {
                Self::ManualUpdate(target_version)
            }
        }
    }
}
