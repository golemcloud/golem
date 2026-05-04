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

use super::{
    AgentError, AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentResourceId, FallibleResultParameters, JsonSnapshotData,
    LoadSnapshotParameters, LogLevel, ManualUpdateParameters, MultipartPartData,
    MultipartSnapshotData, MultipartSnapshotPart, OplogCursor, PluginInstallationDescription,
    ProcessOplogEntriesParameters, ProcessOplogEntriesResultParameters, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttribute, PublicAttributeValue, PublicDurableFunctionType,
    PublicExternalSpanData, PublicLocalSpanData, PublicOplogEntry, PublicOplogEntryWithIndex,
    PublicRetryPolicyState, PublicSnapshotData, PublicSpanData, PublicUpdateDescription,
    RawSnapshotData, SaveSnapshotResultParameters, SnapshotBasedUpdateParameters,
    StringAttributeValue, WriteRemoteBatchedParameters, WriteRemoteTransactionParameters,
};
use crate::base_model::OplogIndex;
use crate::model::AgentInvocationResult;
use crate::model::Empty;
use crate::model::agent::DataValue;
use crate::model::agent::UntypedDataValue;
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::payload::OplogPayload;
use crate::model::oplog::payload::host_functions::{
    HostFunctionName, host_request_from_value_and_type, host_response_from_value_and_type,
};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, BeginRemoteWriteParams,
    CancelPendingInvocationParams, ChangePersistenceLevelParams, CommittedRemoteTransactionParams,
    CreateParams, CreateResourceParams, DeactivatePluginParams, DropResourceParams,
    EndAtomicRegionParams, EndRemoteWriteParams, ErrorParams, ExitedParams, FailedUpdateParams,
    FilesystemStorageUsageUpdateParams, FinishSpanParams, GrowMemoryParams, HostCallParams,
    InterruptedParams, JumpParams, LogParams, NoOpParams, OplogProcessorCheckpointParams,
    PendingAgentInvocationParams, PendingUpdateParams, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, RemoveRetryPolicyParams, RestartParams, RevertParams,
    RolledBackRemoteTransactionParams, SetRetryPolicyParams, SetSpanAttributeParams,
    SnapshotParams, StartSpanParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    AgentTerminatedByQuotaError, DurableFunctionType, OplogEntry, PersistenceLevel,
};
use crate::model::quota::ResourceName;
use crate::model::regions::OplogRegion;
use crate::model::worker::TypedAgentConfigEntry;
use golem_api_grpc::proto::golem::worker::oplog_entry::Entry;
use golem_api_grpc::proto::golem::worker::{
    AttributeValue, ExternalParentSpan, InvocationSpan, LocalInvocationSpan, invocation_span,
    oplog_entry, wrapped_function_type,
};
use golem_wasm::wasmtime::ResourceTypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU64;

impl From<PersistenceLevel> for golem_api_grpc::proto::golem::worker::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => {
                golem_api_grpc::proto::golem::worker::PersistenceLevel::Smart
            }
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::PersistenceLevel> for PersistenceLevel {
    fn from(value: golem_api_grpc::proto::golem::worker::PersistenceLevel) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            golem_api_grpc::proto::golem::worker::PersistenceLevel::Smart => {
                PersistenceLevel::Smart
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentError> for AgentError {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentError,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::agent_error::Error;
        match value.error.ok_or("no error field")? {
            Error::StackOverflow(_) => Ok(Self::StackOverflow),
            Error::OutOfMemory(_) => Ok(Self::OutOfMemory),
            Error::InvalidRequest(inner) => Ok(Self::InvalidRequest(inner.details)),
            Error::UnknownError(inner) => Ok(Self::Unknown(inner.details)),
            Error::ExceededMemoryLimit(_) => Ok(Self::ExceededMemoryLimit),
            Error::InternalError(inner) => Ok(Self::InternalError(inner.details)),
            Error::DeterministicTrap(inner) => Ok(Self::DeterministicTrap(inner.details)),
            Error::TransientError(inner) => Ok(Self::TransientError(inner.details)),
            Error::PermanentError(inner) => Ok(Self::PermanentError(inner.details)),
            Error::ExceededTableLimit(_) => Ok(Self::ExceededTableLimit),
            Error::ExceededHttpCallLimit(_) => Ok(Self::ExceededHttpCallLimit),
            Error::ExceededRpcCallLimit(_) => Ok(Self::ExceededRpcCallLimit),
            Error::NodeOutOfFilesystemStorage(_) => Ok(Self::NodeOutOfFilesystemStorage),
            Error::AgentExceededFilesystemStorageLimit(_) => {
                Ok(Self::AgentExceededFilesystemStorageLimit)
            }
            Error::AgentTerminatedByQuota(inner) => {
                Ok(Self::AgentTerminatedByQuota(AgentTerminatedByQuotaError {
                    environment_id: inner
                        .environment_id
                        .ok_or("no environment_id field")?
                        .try_into()?,
                    resource_name: ResourceName(inner.resource_name),
                }))
            }
        }
    }
}

impl From<AgentError> for golem_api_grpc::proto::golem::worker::AgentError {
    fn from(value: AgentError) -> Self {
        use golem_api_grpc::proto::golem::worker as grpc_worker;
        use golem_api_grpc::proto::golem::worker::agent_error::Error;
        let error = match value {
            AgentError::StackOverflow => Error::StackOverflow(grpc_worker::StackOverflow {}),
            AgentError::OutOfMemory => Error::OutOfMemory(grpc_worker::OutOfMemory {}),
            AgentError::InvalidRequest(details) => {
                Error::InvalidRequest(grpc_worker::InvalidRequest { details })
            }
            AgentError::Unknown(details) => {
                Error::UnknownError(grpc_worker::UnknownError { details })
            }
            AgentError::ExceededMemoryLimit => {
                Error::ExceededMemoryLimit(grpc_worker::ExceededMemoryLimit {})
            }
            AgentError::InternalError(details) => {
                Error::InternalError(grpc_worker::InternalError { details })
            }
            AgentError::DeterministicTrap(details) => {
                Error::DeterministicTrap(grpc_worker::DeterministicTrap { details })
            }
            AgentError::TransientError(details) => {
                Error::TransientError(grpc_worker::TransientError { details })
            }
            AgentError::PermanentError(details) => {
                Error::PermanentError(grpc_worker::PermanentError { details })
            }
            AgentError::ExceededTableLimit => {
                Error::ExceededTableLimit(grpc_worker::ExceededTableLimit {})
            }
            AgentError::ExceededHttpCallLimit => {
                Error::ExceededHttpCallLimit(grpc_worker::ExceededHttpCallLimit {})
            }
            AgentError::ExceededRpcCallLimit => {
                Error::ExceededRpcCallLimit(grpc_worker::ExceededRpcCallLimit {})
            }
            AgentError::NodeOutOfFilesystemStorage => {
                Error::NodeOutOfFilesystemStorage(grpc_worker::NodeOutOfFilesystemStorage {})
            }
            AgentError::AgentExceededFilesystemStorageLimit => {
                Error::AgentExceededFilesystemStorageLimit(
                    grpc_worker::AgentExceededFilesystemStorageLimit {},
                )
            }
            AgentError::AgentTerminatedByQuota(details) => {
                Error::AgentTerminatedByQuota(grpc_worker::AgentTerminatedByQuota {
                    environment_id: Some(details.environment_id.into()),
                    resource_name: details.resource_name.0,
                })
            }
        };
        Self { error: Some(error) }
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogCursor> for OplogCursor {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_revision: value.current_component_revision,
        }
    }
}

impl From<OplogCursor> for golem_api_grpc::proto::golem::worker::OplogCursor {
    fn from(value: OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_revision: value.current_component_revision,
        }
    }
}

impl From<PluginInstallationDescription>
    for golem_api_grpc::proto::golem::worker::PluginInstallationDescription
{
    fn from(plugin_installation_description: PluginInstallationDescription) -> Self {
        golem_api_grpc::proto::golem::worker::PluginInstallationDescription {
            environment_plugin_grant_id: Some(
                plugin_installation_description
                    .environment_plugin_grant_id
                    .into(),
            ),
            plugin_priority: plugin_installation_description.plugin_priority.0,
            plugin_name: plugin_installation_description.plugin_name,
            plugin_version: plugin_installation_description.plugin_version,
            parameters: HashMap::from_iter(plugin_installation_description.parameters),
            registered: false,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PluginInstallationDescription>
    for PluginInstallationDescription
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PluginInstallationDescription,
    ) -> Result<Self, Self::Error> {
        Ok(PluginInstallationDescription {
            environment_plugin_grant_id: value
                .environment_plugin_grant_id
                .ok_or("Missing environment_plugin_grant_id field")?
                .try_into()?,
            plugin_priority: PluginPriority(value.plugin_priority),
            plugin_name: value.plugin_name,
            plugin_version: value.plugin_version,
            parameters: BTreeMap::from_iter(value.parameters),
        })
    }
}

impl From<PublicAttributeValue> for AttributeValue {
    fn from(value: PublicAttributeValue) -> Self {
        match value {
            PublicAttributeValue::String(StringAttributeValue { value }) => {
                golem_api_grpc::proto::golem::worker::AttributeValue {
                    value: Some(
                        golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                            value,
                        ),
                    ),
                }
            }
        }
    }
}

impl TryFrom<AttributeValue> for PublicAttributeValue {
    type Error = String;

    fn try_from(value: AttributeValue) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                value,
            )) => Ok(PublicAttributeValue::String(StringAttributeValue { value })),
            _ => Err("Invalid attribute value".to_string()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OplogEntry> for PublicOplogEntry {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::worker::OplogEntry) -> Result<Self, String> {
        match value.entry.ok_or("Oplog entry is empty")? {
            oplog_entry::Entry::Create(create) => Ok(PublicOplogEntry::Create(CreateParams {
                timestamp: create.timestamp.ok_or("Missing timestamp field")?.into(),
                agent_id: create
                    .agent_id
                    .ok_or("Missing agent_id field")?
                    .try_into()?,
                component_revision: create.component_revision.try_into()?,
                env: create.env.into_iter().collect(),
                local_agent_config: create
                    .config
                    .into_iter()
                    .map(TypedAgentConfigEntry::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
                environment_id: create
                    .environment_id
                    .ok_or("Missing environment_id field")?
                    .try_into()?,
                created_by: create
                    .created_by
                    .ok_or("Missing created_by field")?
                    .try_into()?,
                parent: match create.parent {
                    Some(parent) => Some(parent.try_into()?),
                    None => None,
                },
                component_size: create.component_size,
                initial_total_linear_memory_size: create.initial_total_linear_memory_size,
                initial_active_plugins: BTreeSet::from_iter(
                    create
                        .initial_active_plugins
                        .into_iter()
                        .map(|pr| pr.try_into())
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                original_phantom_id: create.original_phantom_id.map(|id| id.into()),
            })),
            oplog_entry::Entry::HostCall(host_call) => {
                Ok(PublicOplogEntry::HostCall(HostCallParams {
                    timestamp: host_call.timestamp.ok_or("Missing timestamp field")?.into(),
                    function_name: host_call.function_name,
                    request: host_call
                        .request
                        .ok_or("Missing request field")?
                        .try_into()?,
                    response: host_call
                        .response
                        .ok_or("Missing response field")?
                        .try_into()?,
                    durable_function_type: host_call
                        .wrapped_function_type
                        .ok_or("Missing wrapped_function_type field")?
                        .try_into()?,
                }))
            }
            oplog_entry::Entry::AgentInvocationStarted(agent_invocation_started) => Ok(
                PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                    timestamp: agent_invocation_started
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    invocation: agent_invocation_started
                        .invocation
                        .ok_or("Missing invocation field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::AgentInvocationFinished(agent_invocation_finished) => Ok(
                PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                    timestamp: agent_invocation_finished
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    result: agent_invocation_finished
                        .result
                        .ok_or("Missing result field")?
                        .try_into()?,
                    consumed_fuel: agent_invocation_finished.consumed_fuel,
                    component_revision: agent_invocation_finished.component_revision.try_into()?,
                }),
            ),
            oplog_entry::Entry::Suspend(suspend) => Ok(PublicOplogEntry::Suspend(SuspendParams {
                timestamp: suspend.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Error(error) => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp: error.timestamp.ok_or("Missing timestamp field")?.into(),
                error: error.error,
                retry_from: OplogIndex::from_u64(error.retry_from),
                inside_atomic_region: error.inside_atomic_region,
                retry_policy_state: error
                    .retry_policy_state
                    .map(TryInto::try_into)
                    .transpose()?,
            })),
            oplog_entry::Entry::NoOp(no_op) => Ok(PublicOplogEntry::NoOp(NoOpParams {
                timestamp: no_op.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Jump(jump) => Ok(PublicOplogEntry::Jump(JumpParams {
                timestamp: jump.timestamp.ok_or("Missing timestamp field")?.into(),
                jump: OplogRegion {
                    start: OplogIndex::from_u64(jump.start),
                    end: OplogIndex::from_u64(jump.end),
                },
            })),
            oplog_entry::Entry::Interrupted(interrupted) => {
                Ok(PublicOplogEntry::Interrupted(InterruptedParams {
                    timestamp: interrupted
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }))
            }
            oplog_entry::Entry::Exited(exited) => Ok(PublicOplogEntry::Exited(ExitedParams {
                timestamp: exited.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::BeginAtomicRegion(begin_atomic_region) => Ok(
                PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams {
                    timestamp: begin_atomic_region
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }),
            ),
            oplog_entry::Entry::EndAtomicRegion(end_atomic_region) => {
                Ok(PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                    timestamp: end_atomic_region
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    begin_index: OplogIndex::from_u64(end_atomic_region.begin_index),
                }))
            }
            oplog_entry::Entry::BeginRemoteWrite(begin_remote_write) => {
                Ok(PublicOplogEntry::BeginRemoteWrite(BeginRemoteWriteParams {
                    timestamp: begin_remote_write
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }))
            }
            oplog_entry::Entry::EndRemoteWrite(end_remote_write) => {
                Ok(PublicOplogEntry::EndRemoteWrite(EndRemoteWriteParams {
                    timestamp: end_remote_write
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    begin_index: OplogIndex::from_u64(end_remote_write.begin_index),
                }))
            }
            oplog_entry::Entry::PendingAgentInvocation(pending_worker_invocation) => Ok(
                PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                    timestamp: pending_worker_invocation
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    invocation: pending_worker_invocation
                        .invocation
                        .ok_or("Missing invocation field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::PendingUpdate(pending_update) => {
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                    timestamp: pending_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_revision: pending_update.target_revision.try_into()?,
                    description: pending_update
                        .update_description
                        .ok_or("Missing update_description field")?
                        .try_into()?,
                }))
            }
            oplog_entry::Entry::SuccessfulUpdate(successful_update) => {
                Ok(PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                    timestamp: successful_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_revision: successful_update.target_revision.try_into()?,
                    new_component_size: successful_update.new_component_size,
                    new_active_plugins: BTreeSet::from_iter(
                        successful_update
                            .new_active_plugins
                            .into_iter()
                            .map(|pr| pr.try_into())
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                }))
            }
            oplog_entry::Entry::FailedUpdate(failed_update) => {
                Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                    timestamp: failed_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_revision: failed_update.target_revision.try_into()?,
                    details: failed_update.details,
                }))
            }
            oplog_entry::Entry::GrowMemory(grow_memory) => {
                Ok(PublicOplogEntry::GrowMemory(GrowMemoryParams {
                    timestamp: grow_memory
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    delta: grow_memory.delta,
                }))
            }
            oplog_entry::Entry::FilesystemStorageUsageUpdate(filesystem_storage_usage_update) => {
                Ok(PublicOplogEntry::FilesystemStorageUsageUpdate(
                    FilesystemStorageUsageUpdateParams {
                        timestamp: filesystem_storage_usage_update
                            .timestamp
                            .ok_or("Missing timestamp field")?
                            .into(),
                        delta: filesystem_storage_usage_update.delta,
                    },
                ))
            }
            oplog_entry::Entry::CreateResource(create_resource) => {
                Ok(PublicOplogEntry::CreateResource(CreateResourceParams {
                    timestamp: create_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: AgentResourceId(create_resource.resource_id),
                    name: create_resource.name,
                    owner: create_resource.owner,
                }))
            }
            oplog_entry::Entry::DropResource(drop_resource) => {
                Ok(PublicOplogEntry::DropResource(DropResourceParams {
                    timestamp: drop_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: AgentResourceId(drop_resource.resource_id),
                    name: drop_resource.name,
                    owner: drop_resource.owner,
                }))
            }
            oplog_entry::Entry::Log(log) => Ok(PublicOplogEntry::Log(LogParams {
                level: log.level().into(),
                timestamp: log.timestamp.ok_or("Missing timestamp field")?.into(),
                context: log.context,
                message: log.message,
            })),
            oplog_entry::Entry::Restart(restart) => Ok(PublicOplogEntry::Restart(RestartParams {
                timestamp: restart.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::ActivatePlugin(activate) => {
                Ok(PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                    timestamp: activate.timestamp.ok_or("Missing timestamp field")?.into(),
                    plugin: activate.plugin.ok_or("Missing plugin field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::DeactivatePlugin(deactivate) => {
                Ok(PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
                    timestamp: deactivate
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    plugin: deactivate
                        .plugin
                        .ok_or("Missing plugin field")?
                        .try_into()?,
                }))
            }
            oplog_entry::Entry::Revert(revert) => Ok(PublicOplogEntry::Revert(RevertParams {
                timestamp: revert.timestamp.ok_or("Missing timestamp field")?.into(),
                dropped_region: OplogRegion {
                    start: OplogIndex::from_u64(revert.start),
                    end: OplogIndex::from_u64(revert.end),
                },
            })),
            oplog_entry::Entry::CancelInvocation(cancel) => Ok(
                PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
                    timestamp: cancel.timestamp.ok_or("Missing timestamp field")?.into(),
                    idempotency_key: cancel
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                }),
            ),
            Entry::StartSpan(start) => Ok(PublicOplogEntry::StartSpan(StartSpanParams {
                timestamp: start.timestamp.ok_or("Missing timestamp field")?.into(),
                span_id: SpanId(
                    NonZeroU64::new(start.span_id).ok_or("Span ID cannot be zero".to_string())?,
                ),
                parent_id: start
                    .parent_id
                    .map(|id| {
                        NonZeroU64::new(id)
                            .ok_or("Span ID cannot be zero".to_string())
                            .map(SpanId)
                    })
                    .transpose()?,
                linked_context: start
                    .linked_context
                    .map(|id| {
                        NonZeroU64::new(id)
                            .ok_or("Span ID cannot be zero".to_string())
                            .map(SpanId)
                    })
                    .transpose()?,
                attributes: start
                    .attributes
                    .into_iter()
                    .map(|(key, value)| value.try_into().map(|v| PublicAttribute { key, value: v }))
                    .collect::<Result<Vec<PublicAttribute>, String>>()?,
            })),
            Entry::FinishSpan(finish) => Ok(PublicOplogEntry::FinishSpan(FinishSpanParams {
                timestamp: finish.timestamp.ok_or("Missing timestamp field")?.into(),
                span_id: SpanId(
                    NonZeroU64::new(finish.span_id).ok_or("Span ID cannot be zero".to_string())?,
                ),
            })),
            Entry::SetSpanAttribute(set) => {
                Ok(PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
                    timestamp: set.timestamp.ok_or("Missing timestamp field")?.into(),
                    span_id: SpanId(
                        NonZeroU64::new(set.span_id).ok_or("Span ID cannot be zero".to_string())?,
                    ),
                    key: set.key,
                    value: set
                        .value
                        .ok_or("Missing attribute value".to_string())?
                        .try_into()?,
                }))
            }
            Entry::ChangePersistenceLevel(change) => Ok(PublicOplogEntry::ChangePersistenceLevel(
                ChangePersistenceLevelParams {
                    timestamp: change.timestamp.ok_or("Missing timestamp field")?.into(),
                    persistence_level: change.persistence_level().into(),
                },
            )),
            oplog_entry::Entry::BeginRemoteTransaction(value) => Ok(
                PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    transaction_id: value.transaction_id.into(),
                }),
            ),
            oplog_entry::Entry::PreCommitRemoteTransaction(value) => Ok(
                PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    begin_index: OplogIndex::from_u64(value.begin_index),
                }),
            ),
            oplog_entry::Entry::PreRollbackRemoteTransaction(value) => {
                Ok(PublicOplogEntry::PreRollbackRemoteTransaction(
                    PreRollbackRemoteTransactionParams {
                        timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                        begin_index: OplogIndex::from_u64(value.begin_index),
                    },
                ))
            }
            oplog_entry::Entry::CommittedRemoteTransaction(value) => Ok(
                PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    begin_index: OplogIndex::from_u64(value.begin_index),
                }),
            ),

            oplog_entry::Entry::RolledBackRemoteTransaction(value) => Ok(
                PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    begin_index: OplogIndex::from_u64(value.begin_index),
                }),
            ),

            oplog_entry::Entry::Snapshot(snapshot) => {
                let data = match snapshot.data.ok_or("Missing data field")? {
                    golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Raw(
                        raw,
                    ) => PublicSnapshotData::Raw(RawSnapshotData {
                        data: raw.data,
                        mime_type: raw.mime_type,
                    }),
                    golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Json(
                        json,
                    ) => PublicSnapshotData::Json(JsonSnapshotData {
                        data: serde_json::from_str(&json.data).map_err(|e| e.to_string())?,
                    }),
                    golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Multipart(
                        multipart,
                    ) => PublicSnapshotData::Multipart(MultipartSnapshotData {
                        mime_type: multipart.mime_type,
                        parts: multipart.parts.into_iter().map(|p| {
                            let data = match p.data.and_then(|d| d.data) {
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(json)) => {
                                    MultipartPartData::Json(JsonSnapshotData {
                                        data: serde_json::from_str(&json.data).unwrap_or_default(),
                                    })
                                }
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(raw)) => {
                                    MultipartPartData::Raw(RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    })
                                }
                                None => MultipartPartData::Raw(RawSnapshotData {
                                    data: vec![],
                                    mime_type: String::new(),
                                }),
                            };
                            MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data,
                            }
                        }).collect(),
                    }),
                };
                Ok(PublicOplogEntry::Snapshot(SnapshotParams {
                    timestamp: snapshot.timestamp.ok_or("Missing timestamp field")?.into(),
                    data,
                }))
            }
            oplog_entry::Entry::OplogProcessorCheckpoint(value) => Ok(
                PublicOplogEntry::OplogProcessorCheckpoint(OplogProcessorCheckpointParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    plugin: value.plugin.ok_or("Missing plugin field")?.try_into()?,
                    target_agent_id: value
                        .target_agent_id
                        .ok_or("Missing target_agent_id field")?
                        .try_into()?,
                    confirmed_up_to: OplogIndex::from_u64(value.confirmed_up_to),
                    sending_up_to: OplogIndex::from_u64(value.sending_up_to),
                    last_batch_start: OplogIndex::from_u64(value.last_batch_start),
                }),
            ),
            oplog_entry::Entry::SetRetryPolicy(params) => {
                let named_policy = params.named_policy.ok_or("Missing named_policy field")?;
                let internal: crate::model::retry_policy::NamedRetryPolicy =
                    named_policy.try_into()?;
                Ok(PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    policy: internal.into(),
                }))
            }
            oplog_entry::Entry::RemoveRetryPolicy(params) => Ok(
                PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    name: params.policy_name,
                }),
            ),
        }
    }
}

impl TryFrom<PublicOplogEntry> for golem_api_grpc::proto::golem::worker::OplogEntry {
    type Error = String;

    fn try_from(value: PublicOplogEntry) -> Result<Self, String> {
        Ok(match value {
            PublicOplogEntry::Create(create) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Create(
                    golem_api_grpc::proto::golem::worker::CreateParameters {
                        timestamp: Some(create.timestamp.into()),
                        agent_id: Some(create.agent_id.into()),
                        component_revision: create.component_revision.into(),
                        env: create.env.into_iter().collect(),
                        config: create
                            .local_agent_config
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        created_by: Some(create.created_by.into()),
                        environment_id: Some(create.environment_id.into()),
                        parent: create.parent.map(Into::into),
                        component_size: create.component_size,
                        initial_total_linear_memory_size: create.initial_total_linear_memory_size,
                        initial_active_plugins: create
                            .initial_active_plugins
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        original_phantom_id: create.original_phantom_id.map(|id| id.into()),
                    },
                )),
            },
            PublicOplogEntry::HostCall(host_call) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::HostCall(
                        golem_api_grpc::proto::golem::worker::HostCallParameters {
                            timestamp: Some(host_call.timestamp.into()),
                            function_name: host_call.function_name.clone(),
                            request: Some(host_call.request.into()),
                            response: Some(host_call.response.into()),
                            wrapped_function_type: Some(host_call.durable_function_type.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::AgentInvocationStarted(agent_invocation_started) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::AgentInvocationStarted(
                        golem_api_grpc::proto::golem::worker::AgentInvocationStartedParameters {
                            timestamp: Some(agent_invocation_started.timestamp.into()),
                            invocation: Some(agent_invocation_started.invocation.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::AgentInvocationFinished(agent_invocation_finished) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::AgentInvocationFinished(
                        golem_api_grpc::proto::golem::worker::AgentInvocationFinishedParameters {
                            timestamp: Some(agent_invocation_finished.timestamp.into()),
                            result: Some(agent_invocation_finished.result.try_into()?),
                            consumed_fuel: agent_invocation_finished.consumed_fuel,
                            component_revision: agent_invocation_finished.component_revision.get(),
                        },
                    )),
                }
            }
            PublicOplogEntry::Suspend(suspend) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Suspend(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(suspend.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::Error(error) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Error(
                    golem_api_grpc::proto::golem::worker::ErrorParameters {
                        timestamp: Some(error.timestamp.into()),
                        error: error.error,
                        retry_from: error.retry_from.0,
                        inside_atomic_region: error.inside_atomic_region,
                        retry_policy_state: error.retry_policy_state.map(Into::into),
                    },
                )),
            },
            PublicOplogEntry::NoOp(no_op) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::NoOp(
                    golem_api_grpc::proto::golem::worker::TimestampParameter {
                        timestamp: Some(no_op.timestamp.into()),
                    },
                )),
            },
            PublicOplogEntry::Jump(jump) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Jump(
                    golem_api_grpc::proto::golem::worker::JumpParameters {
                        timestamp: Some(jump.timestamp.into()),
                        start: jump.jump.start.into(),
                        end: jump.jump.end.into(),
                    },
                )),
            },
            PublicOplogEntry::Interrupted(interrupted) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Interrupted(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(interrupted.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::Exited(exited) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Exited(
                    golem_api_grpc::proto::golem::worker::TimestampParameter {
                        timestamp: Some(exited.timestamp.into()),
                    },
                )),
            },
            PublicOplogEntry::BeginAtomicRegion(begin_atomic_region) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::BeginAtomicRegion(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(begin_atomic_region.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::EndAtomicRegion(end_atomic_region) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::EndAtomicRegion(
                        golem_api_grpc::proto::golem::worker::EndAtomicRegionParameters {
                            timestamp: Some(end_atomic_region.timestamp.into()),
                            begin_index: end_atomic_region.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::BeginRemoteWrite(begin_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::BeginRemoteWrite(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(begin_remote_write.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::EndRemoteWrite(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::EndRemoteWrite(
                        golem_api_grpc::proto::golem::worker::EndRemoteWriteParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::PendingAgentInvocation(pending_worker_invocation) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PendingAgentInvocation(
                        golem_api_grpc::proto::golem::worker::PendingAgentInvocationParameters {
                            timestamp: Some(pending_worker_invocation.timestamp.into()),
                            invocation: Some(pending_worker_invocation.invocation.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::PendingUpdate(pending_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PendingUpdate(
                        golem_api_grpc::proto::golem::worker::PendingUpdateParameters {
                            timestamp: Some(pending_update.timestamp.into()),
                            target_revision: pending_update.target_revision.into(),
                            update_description: Some(pending_update.description.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::SuccessfulUpdate(successful_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::SuccessfulUpdate(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdateParameters {
                            timestamp: Some(successful_update.timestamp.into()),
                            target_revision: successful_update.target_revision.into(),
                            new_component_size: successful_update.new_component_size,
                            new_active_plugins: successful_update
                                .new_active_plugins
                                .into_iter()
                                .map(Into::into)
                                .collect(),
                        },
                    )),
                }
            }
            PublicOplogEntry::FailedUpdate(failed_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::FailedUpdate(
                        golem_api_grpc::proto::golem::worker::FailedUpdateParameters {
                            timestamp: Some(failed_update.timestamp.into()),
                            target_revision: failed_update.target_revision.into(),
                            details: failed_update.details,
                        },
                    )),
                }
            }
            PublicOplogEntry::GrowMemory(grow_memory) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::GrowMemory(
                        golem_api_grpc::proto::golem::worker::GrowMemoryParameters {
                            timestamp: Some(grow_memory.timestamp.into()),
                            delta: grow_memory.delta,
                        },
                    )),
                }
            }
            PublicOplogEntry::FilesystemStorageUsageUpdate(filesystem_storage_usage_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::FilesystemStorageUsageUpdate(
                        golem_api_grpc::proto::golem::worker::FilesystemStorageUsageUpdateParameters {
                            timestamp: Some(filesystem_storage_usage_update.timestamp.into()),
                            delta: filesystem_storage_usage_update.delta,
                        },
                    )),
                }
            }
            PublicOplogEntry::CreateResource(create_resource) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CreateResource(
                        golem_api_grpc::proto::golem::worker::CreateResourceParameters {
                            timestamp: Some(create_resource.timestamp.into()),
                            resource_id: create_resource.id.0,
                            name: create_resource.name,
                            owner: create_resource.owner,
                        },
                    )),
                }
            }
            PublicOplogEntry::DropResource(drop_resource) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::DropResource(
                        golem_api_grpc::proto::golem::worker::DropResourceParameters {
                            timestamp: Some(drop_resource.timestamp.into()),
                            resource_id: drop_resource.id.0,
                            name: drop_resource.name,
                            owner: drop_resource.owner,
                        },
                    )),
                }
            }
            PublicOplogEntry::Log(log) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Log(
                    golem_api_grpc::proto::golem::worker::LogParameters {
                        timestamp: Some(log.timestamp.into()),
                        level: Into::<golem_api_grpc::proto::golem::worker::OplogLogLevel>::into(
                            log.level,
                        ) as i32,
                        context: log.context,
                        message: log.message,
                    },
                )),
            },
            PublicOplogEntry::Restart(restart) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Restart(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(restart.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::ActivatePlugin(activate) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ActivatePlugin(
                        golem_api_grpc::proto::golem::worker::ActivatePluginParameters {
                            timestamp: Some(activate.timestamp.into()),
                            plugin: Some(activate.plugin.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::DeactivatePlugin(deactivate) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::DeactivatePlugin(
                        golem_api_grpc::proto::golem::worker::DeactivatePluginParameters {
                            timestamp: Some(deactivate.timestamp.into()),
                            plugin: Some(deactivate.plugin.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::Revert(revert) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Revert(
                    golem_api_grpc::proto::golem::worker::RevertParameters {
                        timestamp: Some(revert.timestamp.into()),
                        start: revert.dropped_region.start.0,
                        end: revert.dropped_region.end.0,
                    },
                )),
            },
            PublicOplogEntry::CancelPendingInvocation(cancel) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CancelInvocation(
                        golem_api_grpc::proto::golem::worker::CancelInvocationParameters {
                            timestamp: Some(cancel.timestamp.into()),
                            idempotency_key: Some(cancel.idempotency_key.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::StartSpan(start) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StartSpan(
                        golem_api_grpc::proto::golem::worker::StartSpanParameters {
                            timestamp: Some(start.timestamp.into()),
                            span_id: start.span_id.0.get(),
                            parent_id: start.parent_id.map(|id| id.0.get()),
                            linked_context: start.linked_context.map(|id| id.0.get()),
                            attributes: start
                                .attributes
                                .into_iter()
                                .map(|attr| (attr.key, attr.value.into()))
                                .collect(),
                        },
                    )),
                }
            }
            PublicOplogEntry::FinishSpan(finish) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::FinishSpan(
                        golem_api_grpc::proto::golem::worker::FinishSpanParameters {
                            timestamp: Some(finish.timestamp.into()),
                            span_id: finish.span_id.0.get(),
                        },
                    )),
                }
            }
            PublicOplogEntry::SetSpanAttribute(set) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::SetSpanAttribute(
                        golem_api_grpc::proto::golem::worker::SetSpanAttributeParameters {
                            timestamp: Some(set.timestamp.into()),
                            span_id: set.span_id.0.get(),
                            key: set.key,
                            value: Some(set.value.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::ChangePersistenceLevel(change) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ChangePersistenceLevel(
                        golem_api_grpc::proto::golem::worker::ChangePersistenceLevelParameters {
                            timestamp: Some(change.timestamp.into()),
                            persistence_level: Into::<
                                golem_api_grpc::proto::golem::worker::PersistenceLevel,
                            >::into(
                                change.persistence_level
                            ) as i32,
                        },
                    )),
                }
            }
            PublicOplogEntry::BeginRemoteTransaction(begin_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::BeginRemoteTransaction(
                        golem_api_grpc::proto::golem::worker::BeginRemoteTransactionParameters {
                            timestamp: Some(begin_remote_write.timestamp.into()),
                            transaction_id: begin_remote_write.transaction_id.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::PreCommitRemoteTransaction(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PreCommitRemoteTransaction(
                        golem_api_grpc::proto::golem::worker::RemoteTransactionParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::PreRollbackRemoteTransaction(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PreRollbackRemoteTransaction(
                        golem_api_grpc::proto::golem::worker::RemoteTransactionParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::CommittedRemoteTransaction(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CommittedRemoteTransaction(
                        golem_api_grpc::proto::golem::worker::RemoteTransactionParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::RolledBackRemoteTransaction(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::RolledBackRemoteTransaction(
                        golem_api_grpc::proto::golem::worker::RemoteTransactionParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::Snapshot(snapshot) => {
                let data = match snapshot.data {
                    PublicSnapshotData::Raw(raw) => {
                        golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Raw(
                            golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                data: raw.data,
                                mime_type: raw.mime_type,
                            },
                        )
                    }
                    PublicSnapshotData::Json(json) => {
                        golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Json(
                            golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                data: serde_json::to_string(&json.data)
                                    .map_err(|e| e.to_string())?,
                            },
                        )
                    }
                    PublicSnapshotData::Multipart(multipart) => {
                        let parts = multipart.parts.into_iter().map(|p| {
                            let data = match p.data {
                                MultipartPartData::Json(json) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(
                                        golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                            data: serde_json::to_string(&json.data).unwrap_or_default(),
                                        },
                                    )
                                }
                                MultipartPartData::Raw(raw) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(
                                        golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                            data: raw.data,
                                            mime_type: raw.mime_type,
                                        },
                                    )
                                }
                            };
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data: Some(golem_api_grpc::proto::golem::worker::MultipartPartData {
                                    data: Some(data),
                                }),
                            }
                        }).collect();
                        golem_api_grpc::proto::golem::worker::snapshot_data_parameters::Data::Multipart(
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotData {
                                mime_type: multipart.mime_type,
                                parts,
                            },
                        )
                    }
                };
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Snapshot(
                        golem_api_grpc::proto::golem::worker::SnapshotDataParameters {
                            timestamp: Some(snapshot.timestamp.into()),
                            data: Some(data),
                        },
                    )),
                }
            }
            PublicOplogEntry::OplogProcessorCheckpoint(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::OplogProcessorCheckpoint(
                        golem_api_grpc::proto::golem::worker::OplogProcessorCheckpointParameters {
                            timestamp: Some(params.timestamp.into()),
                            plugin: Some(params.plugin.into()),
                            target_agent_id: Some(params.target_agent_id.into()),
                            confirmed_up_to: params.confirmed_up_to.into(),
                            sending_up_to: params.sending_up_to.into(),
                            last_batch_start: params.last_batch_start.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::SetRetryPolicy(params) => {
                let internal: crate::model::retry_policy::NamedRetryPolicy = params.policy.into();
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::SetRetryPolicy(
                        golem_api_grpc::proto::golem::worker::SetRetryPolicyParameters {
                            timestamp: Some(params.timestamp.into()),
                            named_policy: Some(internal.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::RemoveRetryPolicy(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::RemoveRetryPolicy(
                        golem_api_grpc::proto::golem::worker::RemoveRetryPolicyParameters {
                            timestamp: Some(params.timestamp.into()),
                            policy_name: params.name,
                        },
                    )),
                }
            }
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WrappedFunctionType>
    for PublicDurableFunctionType
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WrappedFunctionType,
    ) -> Result<Self, Self::Error> {
        match value.r#type() {
            wrapped_function_type::Type::ReadLocal => {
                Ok(PublicDurableFunctionType::ReadLocal(Empty {}))
            }
            wrapped_function_type::Type::WriteLocal => {
                Ok(PublicDurableFunctionType::WriteLocal(Empty {}))
            }
            wrapped_function_type::Type::ReadRemote => {
                Ok(PublicDurableFunctionType::ReadRemote(Empty {}))
            }
            wrapped_function_type::Type::WriteRemote => {
                Ok(PublicDurableFunctionType::WriteRemote(Empty {}))
            }
            wrapped_function_type::Type::WriteRemoteBatched => Ok(
                PublicDurableFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                    index: value.oplog_index.map(OplogIndex::from_u64),
                }),
            ),
            wrapped_function_type::Type::WriteRemoteTransaction => {
                Ok(PublicDurableFunctionType::WriteRemoteTransaction(
                    WriteRemoteTransactionParameters {
                        index: value.oplog_index.map(OplogIndex::from_u64),
                    },
                ))
            }
        }
    }
}

impl From<PublicDurableFunctionType> for golem_api_grpc::proto::golem::worker::WrappedFunctionType {
    fn from(value: PublicDurableFunctionType) -> Self {
        match value {
            PublicDurableFunctionType::ReadLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadLocal as i32,
                    oplog_index: None,
                }
            }
            PublicDurableFunctionType::WriteLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteLocal as i32,
                    oplog_index: None,
                }
            }
            PublicDurableFunctionType::ReadRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadRemote as i32,
                    oplog_index: None,
                }
            }
            PublicDurableFunctionType::WriteRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemote as i32,
                    oplog_index: None,
                }
            }
            PublicDurableFunctionType::WriteRemoteBatched(parameters) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemoteBatched as i32,
                    oplog_index: parameters.index.map(|index| index.into()),
                }
            }
            PublicDurableFunctionType::WriteRemoteTransaction(parameters) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemoteTransaction as i32,
                    oplog_index: parameters.index.map(|index| index.into()),
                }
            }
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogLogLevel> for LogLevel {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogLogLevel) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace => LogLevel::Trace,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug => LogLevel::Debug,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo => LogLevel::Info,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn => LogLevel::Warn,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError => LogLevel::Error,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical => {
                LogLevel::Critical
            }
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr => LogLevel::Stderr,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout => LogLevel::Stdout,
        }
    }
}

impl From<LogLevel> for golem_api_grpc::proto::golem::worker::OplogLogLevel {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace,
            LogLevel::Debug => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug,
            LogLevel::Info => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo,
            LogLevel::Warn => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn,
            LogLevel::Error => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError,
            LogLevel::Critical => {
                golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical
            }
            LogLevel::Stderr => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr,
            LogLevel::Stdout => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicAgentInvocation>
    for PublicAgentInvocation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicAgentInvocation,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation::Invocation;
        match value.invocation.ok_or("Missing invocation field")? {
            Invocation::AgentInitialization(init) => {
                let typed = init
                    .constructor_parameters
                    .ok_or("Missing constructor_parameters field")?;
                let schema = typed.schema.ok_or("Missing schema field")?.try_into()?;
                let untyped = typed.value.ok_or("Missing value field")?.try_into()?;
                let constructor_parameters = DataValue::try_from_untyped(untyped, schema)?;
                let invocation_context = encode_public_span_data(init.invocation_context)?;
                Ok(PublicAgentInvocation::AgentInitialization(
                    AgentInitializationParameters {
                        idempotency_key: init
                            .idempotency_key
                            .ok_or("Missing idempotency_key field")?
                            .into(),
                        constructor_parameters,
                        trace_id: TraceId::from_string(init.trace_id)?,
                        trace_states: init.trace_states,
                        invocation_context,
                    },
                ))
            }
            Invocation::AgentMethod(method) => {
                let typed = method
                    .function_input
                    .ok_or("Missing function_input field")?;
                let schema = typed.schema.ok_or("Missing schema field")?.try_into()?;
                let untyped = typed.value.ok_or("Missing value field")?.try_into()?;
                let function_input = DataValue::try_from_untyped(untyped, schema)?;
                let invocation_context = encode_public_span_data(method.invocation_context)?;
                Ok(PublicAgentInvocation::AgentMethodInvocation(
                    AgentMethodInvocationParameters {
                        idempotency_key: method
                            .idempotency_key
                            .ok_or("Missing idempotency_key field")?
                            .into(),
                        method_name: method.method_name,
                        function_input,
                        trace_id: TraceId::from_string(method.trace_id)?,
                        trace_states: method.trace_states,
                        invocation_context,
                    },
                ))
            }
            Invocation::SaveSnapshot(_) => Ok(PublicAgentInvocation::SaveSnapshot(Empty {})),
            Invocation::LoadSnapshot(load) => {
                let snapshot = load.snapshot.ok_or("Missing snapshot field")?;
                let data = match snapshot.data.ok_or("Missing data field")? {
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(raw) => {
                        PublicSnapshotData::Raw(RawSnapshotData {
                            data: raw.data,
                            mime_type: raw.mime_type,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(json) => {
                        PublicSnapshotData::Json(JsonSnapshotData {
                            data: serde_json::from_str(&json.data).map_err(|e| e.to_string())?,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                        multipart,
                    ) => PublicSnapshotData::Multipart(MultipartSnapshotData {
                        mime_type: multipart.mime_type,
                        parts: multipart.parts.into_iter().map(|p| {
                            let data = match p.data.and_then(|d| d.data) {
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(json)) => {
                                    MultipartPartData::Json(JsonSnapshotData {
                                        data: serde_json::from_str(&json.data).unwrap_or_default(),
                                    })
                                }
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(raw)) => {
                                    MultipartPartData::Raw(RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    })
                                }
                                None => MultipartPartData::Raw(RawSnapshotData {
                                    data: vec![],
                                    mime_type: String::new(),
                                }),
                            };
                            MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data,
                            }
                        }).collect(),
                    }),
                };
                Ok(PublicAgentInvocation::LoadSnapshot(
                    LoadSnapshotParameters { snapshot: data },
                ))
            }
            Invocation::ProcessOplogEntries(process) => Ok(
                PublicAgentInvocation::ProcessOplogEntries(ProcessOplogEntriesParameters {
                    idempotency_key: process
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                }),
            ),
            Invocation::ManualUpdate(manual) => Ok(PublicAgentInvocation::ManualUpdate(
                ManualUpdateParameters {
                    target_revision: manual.target_revision.try_into()?,
                },
            )),
        }
    }
}

impl TryFrom<PublicAgentInvocation>
    for golem_api_grpc::proto::golem::worker::PublicAgentInvocation
{
    type Error = String;

    fn try_from(value: PublicAgentInvocation) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation::Invocation;
        let invocation = match value {
            PublicAgentInvocation::AgentInitialization(init) => {
                let typed_data_value: super::TypedDataValue = init.constructor_parameters.into();
                let invocation_context = decode_public_span_data(&init.invocation_context, 0);
                Invocation::AgentInitialization(
                    golem_api_grpc::proto::golem::worker::PublicAgentInitializationInvocation {
                        idempotency_key: Some(init.idempotency_key.into()),
                        constructor_parameters: Some(
                            golem_api_grpc::proto::golem::component::TypedDataValue {
                                value: Some(typed_data_value.value.into()),
                                schema: Some(typed_data_value.schema.into()),
                            },
                        ),
                        trace_id: init.trace_id.to_string(),
                        trace_states: init.trace_states,
                        invocation_context,
                    },
                )
            }
            PublicAgentInvocation::AgentMethodInvocation(method) => {
                let typed_data_value: super::TypedDataValue = method.function_input.into();
                let invocation_context = decode_public_span_data(&method.invocation_context, 0);
                Invocation::AgentMethod(
                    golem_api_grpc::proto::golem::worker::PublicAgentMethodInvocation {
                        idempotency_key: Some(method.idempotency_key.into()),
                        method_name: method.method_name,
                        function_input: Some(
                            golem_api_grpc::proto::golem::component::TypedDataValue {
                                value: Some(typed_data_value.value.into()),
                                schema: Some(typed_data_value.schema.into()),
                            },
                        ),
                        trace_id: method.trace_id.to_string(),
                        trace_states: method.trace_states,
                        invocation_context,
                    },
                )
            }
            PublicAgentInvocation::SaveSnapshot(_) => {
                Invocation::SaveSnapshot(golem_api_grpc::proto::golem::common::Empty {})
            }
            PublicAgentInvocation::LoadSnapshot(load) => {
                let snapshot_data = match load.snapshot {
                    PublicSnapshotData::Raw(raw) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(
                                    golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Json(json) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(
                                    golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                        data: serde_json::to_string(&json.data)
                                            .map_err(|e| e.to_string())?,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Multipart(multipart) => {
                        let parts = multipart.parts.into_iter().map(|p| {
                            let data = match p.data {
                                MultipartPartData::Json(json) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(
                                        golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                            data: serde_json::to_string(&json.data).unwrap_or_default(),
                                        },
                                    )
                                }
                                MultipartPartData::Raw(raw) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(
                                        golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                            data: raw.data,
                                            mime_type: raw.mime_type,
                                        },
                                    )
                                }
                            };
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data: Some(golem_api_grpc::proto::golem::worker::MultipartPartData {
                                    data: Some(data),
                                }),
                            }
                        }).collect();
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                                    golem_api_grpc::proto::golem::worker::MultipartSnapshotData {
                                        mime_type: multipart.mime_type,
                                        parts,
                                    },
                                ),
                            ),
                        }
                    }
                };
                Invocation::LoadSnapshot(
                    golem_api_grpc::proto::golem::worker::LoadSnapshotInvocationParameters {
                        snapshot: Some(snapshot_data),
                    },
                )
            }
            PublicAgentInvocation::ProcessOplogEntries(process) => Invocation::ProcessOplogEntries(
                golem_api_grpc::proto::golem::worker::ProcessOplogEntriesInvocationParameters {
                    idempotency_key: Some(process.idempotency_key.into()),
                },
            ),
            PublicAgentInvocation::ManualUpdate(manual) => Invocation::ManualUpdate(
                golem_api_grpc::proto::golem::worker::ManualUpdateInvocationParameters {
                    target_revision: manual.target_revision.into(),
                },
            ),
        };
        Ok(
            golem_api_grpc::proto::golem::worker::PublicAgentInvocation {
                invocation: Some(invocation),
            },
        )
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult>
    for PublicAgentInvocationResult
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation_result::Result as ProtoResult;
        match value.result.ok_or("Missing result field")? {
            ProtoResult::AgentInitializationOutput(typed) => {
                let schema = typed.schema.ok_or("Missing schema field")?.try_into()?;
                let untyped = typed.value.ok_or("Missing value field")?.try_into()?;
                let output = DataValue::try_from_untyped(untyped, schema)?;
                Ok(PublicAgentInvocationResult::AgentInitialization(
                    AgentInvocationOutputParameters { output },
                ))
            }
            ProtoResult::AgentMethodOutput(typed) => {
                let schema = typed.schema.ok_or("Missing schema field")?.try_into()?;
                let untyped = typed.value.ok_or("Missing value field")?.try_into()?;
                let output = DataValue::try_from_untyped(untyped, schema)?;
                Ok(PublicAgentInvocationResult::AgentMethod(
                    AgentInvocationOutputParameters { output },
                ))
            }
            ProtoResult::ManualUpdate(_) => Ok(PublicAgentInvocationResult::ManualUpdate(Empty {})),
            ProtoResult::LoadSnapshot(opt_err) => Ok(PublicAgentInvocationResult::LoadSnapshot(
                FallibleResultParameters {
                    error: opt_err.error,
                },
            )),
            ProtoResult::SaveSnapshot(snapshot) => {
                let data = match snapshot.data.ok_or("Missing data field")? {
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(raw) => {
                        PublicSnapshotData::Raw(RawSnapshotData {
                            data: raw.data,
                            mime_type: raw.mime_type,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(json) => {
                        PublicSnapshotData::Json(JsonSnapshotData {
                            data: serde_json::from_str(&json.data).map_err(|e| e.to_string())?,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                        multipart,
                    ) => PublicSnapshotData::Multipart(MultipartSnapshotData {
                        mime_type: multipart.mime_type,
                        parts: multipart.parts.into_iter().map(|p| {
                            let data = match p.data.and_then(|d| d.data) {
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(json)) => {
                                    MultipartPartData::Json(JsonSnapshotData {
                                        data: serde_json::from_str(&json.data).unwrap_or_default(),
                                    })
                                }
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(raw)) => {
                                    MultipartPartData::Raw(RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    })
                                }
                                None => MultipartPartData::Raw(RawSnapshotData {
                                    data: vec![],
                                    mime_type: String::new(),
                                }),
                            };
                            MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data,
                            }
                        }).collect(),
                    }),
                };
                Ok(PublicAgentInvocationResult::SaveSnapshot(
                    SaveSnapshotResultParameters { snapshot: data },
                ))
            }
            ProtoResult::ProcessOplogEntries(result) => {
                Ok(PublicAgentInvocationResult::ProcessOplogEntries(
                    ProcessOplogEntriesResultParameters {
                        error: result.error,
                    },
                ))
            }
        }
    }
}

impl TryFrom<PublicAgentInvocationResult>
    for golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult
{
    type Error = String;

    fn try_from(value: PublicAgentInvocationResult) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation_result::Result as ProtoResult;
        let result = match value {
            PublicAgentInvocationResult::AgentInitialization(output) => {
                let typed: super::TypedDataValue = output.output.into();
                ProtoResult::AgentInitializationOutput(
                    golem_api_grpc::proto::golem::component::TypedDataValue {
                        value: Some(typed.value.into()),
                        schema: Some(typed.schema.into()),
                    },
                )
            }
            PublicAgentInvocationResult::AgentMethod(output) => {
                let typed: super::TypedDataValue = output.output.into();
                ProtoResult::AgentMethodOutput(
                    golem_api_grpc::proto::golem::component::TypedDataValue {
                        value: Some(typed.value.into()),
                        schema: Some(typed.schema.into()),
                    },
                )
            }
            PublicAgentInvocationResult::ManualUpdate(_) => {
                ProtoResult::ManualUpdate(golem_api_grpc::proto::golem::common::Empty {})
            }
            PublicAgentInvocationResult::LoadSnapshot(fallible) => {
                ProtoResult::LoadSnapshot(golem_api_grpc::proto::golem::worker::OptionalError {
                    error: fallible.error,
                })
            }
            PublicAgentInvocationResult::SaveSnapshot(save) => {
                let snapshot_data = match save.snapshot {
                    PublicSnapshotData::Raw(raw) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(
                                    golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Json(json) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(
                                    golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                        data: serde_json::to_string(&json.data)
                                            .map_err(|e| e.to_string())?,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Multipart(multipart) => {
                        let parts = multipart.parts.into_iter().map(|p| {
                            let data = match p.data {
                                MultipartPartData::Json(json) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(
                                        golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                            data: serde_json::to_string(&json.data).unwrap_or_default(),
                                        },
                                    )
                                }
                                MultipartPartData::Raw(raw) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(
                                        golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                            data: raw.data,
                                            mime_type: raw.mime_type,
                                        },
                                    )
                                }
                            };
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data: Some(golem_api_grpc::proto::golem::worker::MultipartPartData {
                                    data: Some(data),
                                }),
                            }
                        }).collect();
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                                    golem_api_grpc::proto::golem::worker::MultipartSnapshotData {
                                        mime_type: multipart.mime_type,
                                        parts,
                                    },
                                ),
                            ),
                        }
                    }
                };
                ProtoResult::SaveSnapshot(snapshot_data)
            }
            PublicAgentInvocationResult::ProcessOplogEntries(result) => {
                ProtoResult::ProcessOplogEntries(
                    golem_api_grpc::proto::golem::worker::ProcessOplogEntriesResult {
                        error: result.error,
                    },
                )
            }
        };
        Ok(
            golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult {
                result: Some(result),
            },
        )
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateDescription> for PublicUpdateDescription {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateDescription,
    ) -> Result<Self, Self::Error> {
        match value.description.ok_or("Missing description field")? {
            golem_api_grpc::proto::golem::worker::update_description::Description::AutoUpdate(_) => {
                Ok(PublicUpdateDescription::Automatic(Empty {}))
            }
            golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                snapshot_based,
            ) => Ok(PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload: snapshot_based.payload,
                mime_type: snapshot_based.mime_type,
            })),
        }
    }
}

impl From<PublicUpdateDescription> for golem_api_grpc::proto::golem::worker::UpdateDescription {
    fn from(value: PublicUpdateDescription) -> Self {
        match value {
            PublicUpdateDescription::Automatic(_) => golem_api_grpc::proto::golem::worker::UpdateDescription {
                description: Some(
                    golem_api_grpc::proto::golem::worker::update_description::Description::AutoUpdate(
                        golem_api_grpc::proto::golem::common::Empty {},
                    ),
                ),
            },
            PublicUpdateDescription::SnapshotBased(snapshot_based) => {
                golem_api_grpc::proto::golem::worker::UpdateDescription {
                    description: Some(
                        golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                            golem_api_grpc::proto::golem::worker::SnapshotBasedUpdateParameters {
                                payload: snapshot_based.payload,
                                mime_type: snapshot_based.mime_type,
                            }
                        ),
                    ),
                }
            }
        }
    }
}

fn encode_public_span_data(spans: Vec<InvocationSpan>) -> Result<Vec<Vec<PublicSpanData>>, String> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for span in spans.into_iter().rev() {
        match span.span {
            Some(invocation_span::Span::Local(span)) => {
                let linked_context = if !span.linked_context.is_empty() {
                    let id = result.len() as u64;
                    let encoded_linked_context = encode_public_span_data(span.linked_context)?;
                    result.extend(encoded_linked_context);
                    Some(id)
                } else {
                    None
                };
                let span_data = PublicSpanData::LocalSpan(PublicLocalSpanData {
                    span_id: SpanId(NonZeroU64::new(span.span_id).unwrap()),
                    start: span.start.ok_or("Missing start field")?.into(),
                    parent_id: current
                        .first()
                        .map(|span: &PublicSpanData| span.span_id().clone()),
                    linked_context,
                    attributes: span
                        .attributes
                        .into_iter()
                        .map(|(k, v)| v.try_into().map(|v| PublicAttribute { key: k, value: v }))
                        .collect::<Result<Vec<_>, _>>()?,
                    inherited: span.inherited,
                });
                current.insert(0, span_data);
            }
            Some(invocation_span::Span::ExternalParent(span)) => {
                let span_data = PublicSpanData::ExternalSpan(PublicExternalSpanData {
                    span_id: SpanId(NonZeroU64::new(span.span_id).unwrap()),
                });
                current.insert(0, span_data);
            }
            None => return Err("Missing span field".to_string()),
        }
    }

    for stack in &mut result {
        for span in stack {
            if let PublicSpanData::LocalSpan(local_span) = span
                && let Some(linked_id) = &mut local_span.linked_context
            {
                *linked_id += 1;
            }
        }
    }
    result.insert(0, current);

    Ok(result)
}

fn decode_public_span_data(
    invocation_context: &Vec<Vec<PublicSpanData>>,
    idx: usize,
) -> Vec<InvocationSpan> {
    if idx >= invocation_context.len() {
        Vec::new()
    } else {
        let stack = &invocation_context[idx];
        let mut result = Vec::new();
        for span_data in stack {
            let span = InvocationSpan {
                span: Some(match span_data {
                    PublicSpanData::LocalSpan(local_span_data) => {
                        invocation_span::Span::Local(LocalInvocationSpan {
                            span_id: local_span_data.span_id.0.get(),
                            start: Some(local_span_data.start.into()),
                            linked_context: local_span_data
                                .linked_context
                                .map(|id| decode_public_span_data(invocation_context, id as usize))
                                .unwrap_or_default(),
                            attributes: local_span_data
                                .attributes
                                .iter()
                                .map(|attr| (attr.key.clone(), attr.value.clone().into()))
                                .collect(),
                            inherited: local_span_data.inherited,
                        })
                    }
                    PublicSpanData::ExternalSpan(external_span_data) => {
                        invocation_span::Span::ExternalParent(ExternalParentSpan {
                            span_id: external_span_data.span_id.0.get(),
                        })
                    }
                }),
            };
            result.push(span);
        }

        result
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OplogEntryWithIndex>
    for PublicOplogEntryWithIndex
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::OplogEntryWithIndex,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: OplogIndex::from_u64(value.oplog_index),
            entry: value.entry.ok_or("Missing field: entry")?.try_into()?,
        })
    }
}

impl TryFrom<PublicOplogEntryWithIndex>
    for golem_api_grpc::proto::golem::worker::OplogEntryWithIndex
{
    type Error = String;

    fn try_from(value: PublicOplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: value.oplog_index.into(),
            entry: Some(value.entry.try_into()?),
        })
    }
}

impl From<PublicRetryPolicyState> for golem_api_grpc::proto::golem::worker::RetryPolicyState {
    fn from(value: PublicRetryPolicyState) -> Self {
        use golem_api_grpc::proto::golem::worker::retry_policy_state::State;
        use golem_api_grpc::proto::golem::worker::{
            RetryPolicyStateAndThen, RetryPolicyStateCountBox, RetryPolicyStatePair,
            RetryPolicyStateWrapper,
        };

        let state = match value {
            PublicRetryPolicyState::Counter(c) => State::Counter(c.count),
            PublicRetryPolicyState::Terminal(_) => {
                State::Terminal(golem_api_grpc::proto::golem::common::Empty {})
            }
            PublicRetryPolicyState::Wrapper(w) => {
                State::Wrapper(Box::new(RetryPolicyStateWrapper {
                    inner: Some(Box::new((*w.inner).into())),
                }))
            }
            PublicRetryPolicyState::CountBox(cb) => {
                State::CountBox(Box::new(RetryPolicyStateCountBox {
                    attempts: cb.attempts,
                    inner: Some(Box::new((*cb.inner).into())),
                }))
            }
            PublicRetryPolicyState::AndThen(at) => {
                State::AndThen(Box::new(RetryPolicyStateAndThen {
                    left: Some(Box::new((*at.left).into())),
                    right: Some(Box::new((*at.right).into())),
                    on_right: at.on_right,
                }))
            }
            PublicRetryPolicyState::Pair(p) => State::Pair(Box::new(RetryPolicyStatePair {
                first: Some(Box::new((*p.first).into())),
                second: Some(Box::new((*p.second).into())),
            })),
        };
        golem_api_grpc::proto::golem::worker::RetryPolicyState { state: Some(state) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::RetryPolicyState> for PublicRetryPolicyState {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::RetryPolicyState,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::retry_policy_state::State;

        let state = value.state.ok_or("Missing retry policy state")?;
        match state {
            State::Counter(n) => Ok(PublicRetryPolicyState::Counter(
                super::PublicRetryPolicyStateCounter { count: n },
            )),
            State::Terminal(_) => Ok(PublicRetryPolicyState::Terminal(
                crate::base_model::Empty {},
            )),
            State::Wrapper(w) => {
                let inner = w.inner.ok_or("Missing inner in Wrapper")?;
                Ok(PublicRetryPolicyState::Wrapper(
                    super::PublicRetryPolicyStateWrapper {
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::CountBox(cb) => {
                let inner = cb.inner.ok_or("Missing inner in CountBox")?;
                Ok(PublicRetryPolicyState::CountBox(
                    super::PublicRetryPolicyStateCountBox {
                        attempts: cb.attempts,
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::TimeBox(tb) => {
                let inner = tb.inner.ok_or("Missing inner in TimeBox")?;
                Ok(PublicRetryPolicyState::Wrapper(
                    super::PublicRetryPolicyStateWrapper {
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::AndThen(at) => {
                let left = at.left.ok_or("Missing left in AndThen")?;
                let right = at.right.ok_or("Missing right in AndThen")?;
                Ok(PublicRetryPolicyState::AndThen(
                    super::PublicRetryPolicyStateAndThen {
                        left: Box::new((*left).try_into()?),
                        right: Box::new((*right).try_into()?),
                        on_right: at.on_right,
                    },
                ))
            }
            State::Pair(p) => {
                let first = p.first.ok_or("Missing first in Pair")?;
                let second = p.second.ok_or("Missing second in Pair")?;
                Ok(PublicRetryPolicyState::Pair(
                    super::PublicRetryPolicyStatePair {
                        first: Box::new((*first).try_into()?),
                        second: Box::new((*second).try_into()?),
                    },
                ))
            }
        }
    }
}

/// Convert a `PublicOplogEntry` to a domain `OplogEntry`.
///
/// Some entry types (e.g. `AgentInvocationStarted`) cannot be losslessly
/// round-tripped because the public representation discards internal detail.
/// For those cases this conversion returns an error.
impl TryFrom<PublicOplogEntry> for OplogEntry {
    type Error = String;

    fn try_from(value: PublicOplogEntry) -> Result<Self, String> {
        match value {
            PublicOplogEntry::Create(create) => Ok(OplogEntry::Create {
                timestamp: create.timestamp,
                agent_id: create.agent_id,
                component_revision: create.component_revision,
                env: create.env.into_iter().collect(),
                environment_id: create.environment_id,
                created_by: create.created_by,
                local_agent_config: create.local_agent_config.into_iter().map(Into::into).collect(),
                parent: create.parent,
                component_size: create.component_size,
                initial_total_linear_memory_size: create.initial_total_linear_memory_size,
                initial_active_plugins: create
                    .initial_active_plugins
                    .into_iter()
                    .map(|p| p.environment_plugin_grant_id)
                    .collect(),
                original_phantom_id: create.original_phantom_id,
            }),
            PublicOplogEntry::HostCall(host_call) => {
                let durable_function_type = match host_call.durable_function_type {
                    PublicDurableFunctionType::ReadLocal(_) => DurableFunctionType::ReadLocal,
                    PublicDurableFunctionType::WriteLocal(_) => DurableFunctionType::WriteLocal,
                    PublicDurableFunctionType::ReadRemote(_) => DurableFunctionType::ReadRemote,
                    PublicDurableFunctionType::WriteRemote(_) => DurableFunctionType::WriteRemote,
                    PublicDurableFunctionType::WriteRemoteBatched(params) => {
                        DurableFunctionType::WriteRemoteBatched(params.index)
                    }
                    PublicDurableFunctionType::WriteRemoteTransaction(params) => {
                        DurableFunctionType::WriteRemoteTransaction(params.index)
                    }
                };

                let request = OplogPayload::Inline(Box::new(host_request_from_value_and_type(
                    &host_call.function_name,
                    host_call.request,
                )?));
                let response = OplogPayload::Inline(Box::new(host_response_from_value_and_type(
                    &host_call.function_name,
                    host_call.response,
                )?));

                Ok(OplogEntry::HostCall {
                    timestamp: host_call.timestamp,
                    function_name: HostFunctionName::from(host_call.function_name.as_str()),
                    request,
                    response,
                    durable_function_type,
                })
            }
            PublicOplogEntry::AgentInvocationStarted(_) => {
                Err("Converting AgentInvocationStarted from public to raw oplog entry is not yet supported".to_string())
            }
            PublicOplogEntry::AgentInvocationFinished(finished) => {
                let raw_result = public_agent_invocation_result_to_raw(finished.result)?;
                Ok(OplogEntry::AgentInvocationFinished {
                    timestamp: finished.timestamp,
                    result: OplogPayload::Inline(Box::new(raw_result)),
                    consumed_fuel: finished.consumed_fuel,
                    component_revision: finished.component_revision,
                })
            }
            PublicOplogEntry::Suspend(p) => Ok(OplogEntry::Suspend {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Error(error) => Ok(OplogEntry::Error {
                timestamp: error.timestamp,
                error: AgentError::Unknown(error.error),
                retry_from: error.retry_from,
                inside_atomic_region: error.inside_atomic_region,
                retry_policy_state: error.retry_policy_state.map(Into::into),
            }),
            PublicOplogEntry::NoOp(p) => Ok(OplogEntry::NoOp {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Jump(jump) => Ok(OplogEntry::Jump {
                timestamp: jump.timestamp,
                jump: jump.jump,
            }),
            PublicOplogEntry::Interrupted(p) => Ok(OplogEntry::Interrupted {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Exited(p) => Ok(OplogEntry::Exited {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::BeginAtomicRegion(_) => {
                Err("Cannot convert BeginAtomicRegion from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::EndAtomicRegion(_) => {
                Err("Cannot convert EndAtomicRegion from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::BeginRemoteWrite(_) => {
                Err("Cannot convert BeginRemoteWrite from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::EndRemoteWrite(_) => {
                Err("Cannot convert EndRemoteWrite from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::PendingAgentInvocation(_) => {
                Err("Cannot convert PendingAgentInvocation from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::PendingUpdate(_) => {
                Err("Cannot convert PendingUpdate from public to raw oplog entry".to_string())
            }
            PublicOplogEntry::SuccessfulUpdate(p) => {
                let new_active_plugins = p
                    .new_active_plugins
                    .iter()
                    .map(|plugin| plugin.environment_plugin_grant_id)
                    .collect();
                Ok(OplogEntry::SuccessfulUpdate {
                    timestamp: p.timestamp,
                    target_revision: p.target_revision,
                    new_component_size: p.new_component_size,
                    new_active_plugins,
                })
            }
            PublicOplogEntry::FailedUpdate(p) => Ok(OplogEntry::FailedUpdate {
                timestamp: p.timestamp,
                target_revision: p.target_revision,
                details: p.details,
            }),
            PublicOplogEntry::GrowMemory(p) => Ok(OplogEntry::GrowMemory {
                timestamp: p.timestamp,
                delta: p.delta,
            }),
            PublicOplogEntry::FilesystemStorageUsageUpdate(p) => {
                Ok(OplogEntry::FilesystemStorageUsageUpdate {
                    timestamp: p.timestamp,
                    delta: p.delta,
                })
            }
            PublicOplogEntry::CreateResource(p) => Ok(OplogEntry::CreateResource {
                timestamp: p.timestamp,
                id: p.id,
                resource_type_id: ResourceTypeId {
                    owner: p.owner,
                    name: p.name,
                },
            }),
            PublicOplogEntry::DropResource(p) => Ok(OplogEntry::DropResource {
                timestamp: p.timestamp,
                id: p.id,
                resource_type_id: ResourceTypeId {
                    owner: p.owner,
                    name: p.name,
                },
            }),
            PublicOplogEntry::Log(p) => Ok(OplogEntry::Log {
                timestamp: p.timestamp,
                level: p.level,
                context: p.context,
                message: p.message,
            }),
            PublicOplogEntry::Restart(p) => Ok(OplogEntry::Restart {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::ActivatePlugin(p) => Ok(OplogEntry::ActivatePlugin {
                timestamp: p.timestamp,
                plugin_grant_id: p.plugin.environment_plugin_grant_id,
            }),
            PublicOplogEntry::DeactivatePlugin(p) => Ok(OplogEntry::DeactivatePlugin {
                timestamp: p.timestamp,
                plugin_grant_id: p.plugin.environment_plugin_grant_id,
            }),
            PublicOplogEntry::Revert(p) => Ok(OplogEntry::Revert {
                timestamp: p.timestamp,
                dropped_region: p.dropped_region,
            }),
            PublicOplogEntry::CancelPendingInvocation(p) => {
                Ok(OplogEntry::CancelPendingInvocation {
                    timestamp: p.timestamp,
                    idempotency_key: p.idempotency_key,
                })
            }
            PublicOplogEntry::StartSpan(p) => Ok(OplogEntry::StartSpan {
                timestamp: p.timestamp,
                span_id: p.span_id,
                parent: p.parent_id,
                linked_context_id: p.linked_context,
                attributes: p
                    .attributes
                    .into_iter()
                    .map(|attr| (attr.key, attr.value.into()))
                    .collect::<HashMap<_, _>>()
                    .into(),
            }),
            PublicOplogEntry::FinishSpan(p) => Ok(OplogEntry::FinishSpan {
                timestamp: p.timestamp,
                span_id: p.span_id,
            }),
            PublicOplogEntry::SetSpanAttribute(p) => Ok(OplogEntry::SetSpanAttribute {
                timestamp: p.timestamp,
                span_id: p.span_id,
                key: p.key,
                value: p.value.into(),
            }),
            PublicOplogEntry::ChangePersistenceLevel(p) => {
                Ok(OplogEntry::ChangePersistenceLevel {
                    timestamp: p.timestamp,
                    persistence_level: p.persistence_level,
                })
            }
            PublicOplogEntry::BeginRemoteTransaction(_) => {
                Err("Cannot convert BeginRemoteTransaction from public to raw oplog entry"
                    .to_string())
            }
            PublicOplogEntry::PreCommitRemoteTransaction(p) => {
                Ok(OplogEntry::PreCommitRemoteTransaction {
                    timestamp: p.timestamp,
                    begin_index: p.begin_index,
                })
            }
            PublicOplogEntry::PreRollbackRemoteTransaction(p) => {
                Ok(OplogEntry::PreRollbackRemoteTransaction {
                    timestamp: p.timestamp,
                    begin_index: p.begin_index,
                })
            }
            PublicOplogEntry::CommittedRemoteTransaction(p) => {
                Ok(OplogEntry::CommittedRemoteTransaction {
                    timestamp: p.timestamp,
                    begin_index: p.begin_index,
                })
            }
            PublicOplogEntry::RolledBackRemoteTransaction(p) => {
                Ok(OplogEntry::RolledBackRemoteTransaction {
                    timestamp: p.timestamp,
                    begin_index: p.begin_index,
                })
            }
            PublicOplogEntry::Snapshot(p) => {
                let (data, mime_type) = match p.data {
                    PublicSnapshotData::Raw(raw) => (raw.data, raw.mime_type),
                    PublicSnapshotData::Json(json) => (
                        serde_json::to_vec(&json.data).map_err(|e| e.to_string())?,
                        "application/json".to_string(),
                    ),
                    PublicSnapshotData::Multipart(multipart) => {
                        use crate::base_model::oplog::multipart::extract_boundary;
                        use super::MultipartPartData;

                        let boundary = extract_boundary(&multipart.mime_type)
                            .unwrap_or("boundary")
                            .to_string();
                        let mut output = Vec::new();
                        for part in &multipart.parts {
                            output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                            output.extend_from_slice(
                                format!("Content-Type: {}\r\n", part.content_type).as_bytes(),
                            );
                            output.extend_from_slice(
                                format!(
                                    "Content-Disposition: attachment; name=\"{}\"\r\n",
                                    part.name
                                )
                                .as_bytes(),
                            );
                            output.extend_from_slice(b"\r\n");
                            match &part.data {
                                MultipartPartData::Json(json) => {
                                    output.extend_from_slice(
                                        serde_json::to_vec(&json.data)
                                            .unwrap_or_default()
                                            .as_slice(),
                                    );
                                }
                                MultipartPartData::Raw(raw) => {
                                    output.extend_from_slice(&raw.data);
                                }
                            }
                            output.extend_from_slice(b"\r\n");
                        }
                        output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
                        (output, multipart.mime_type)
                    }
                };
                Ok(OplogEntry::Snapshot {
                    timestamp: p.timestamp,
                    data: OplogPayload::Inline(Box::new(data)),
                    mime_type,
                })
            }
            PublicOplogEntry::OplogProcessorCheckpoint(p) => {
                Ok(OplogEntry::OplogProcessorCheckpoint {
                    timestamp: p.timestamp,
                    plugin_grant_id: p.plugin.environment_plugin_grant_id,
                    target_agent_id: p.target_agent_id,
                    confirmed_up_to: p.confirmed_up_to,
                    sending_up_to: p.sending_up_to,
                    last_batch_start: p.last_batch_start,
                })
            }
            PublicOplogEntry::SetRetryPolicy(p) => Ok(OplogEntry::SetRetryPolicy {
                timestamp: p.timestamp,
                policy: p.policy.into(),
            }),
            PublicOplogEntry::RemoveRetryPolicy(p) => Ok(OplogEntry::RemoveRetryPolicy {
                timestamp: p.timestamp,
                name: p.name,
            }),
        }
    }
}

fn public_agent_invocation_result_to_raw(
    result: PublicAgentInvocationResult,
) -> Result<AgentInvocationResult, String> {
    match result {
        PublicAgentInvocationResult::AgentInitialization(_) => {
            Ok(AgentInvocationResult::AgentInitialization)
        }
        PublicAgentInvocationResult::AgentMethod(_) => Ok(AgentInvocationResult::AgentMethod {
            output: UntypedDataValue::Tuple(vec![]),
        }),
        PublicAgentInvocationResult::ManualUpdate(_) => Ok(AgentInvocationResult::ManualUpdate),
        PublicAgentInvocationResult::LoadSnapshot(params) => {
            Ok(AgentInvocationResult::LoadSnapshot {
                error: params.error,
            })
        }
        PublicAgentInvocationResult::SaveSnapshot(params) => {
            let snapshot = match params.snapshot {
                PublicSnapshotData::Raw(raw) => raw,
                PublicSnapshotData::Json(json) => RawSnapshotData {
                    data: serde_json::to_vec(&json.data).map_err(|e| e.to_string())?,
                    mime_type: "application/json".to_string(),
                },
                PublicSnapshotData::Multipart(multipart) => {
                    use super::MultipartPartData;
                    use crate::base_model::oplog::multipart::extract_boundary;

                    let boundary = extract_boundary(&multipart.mime_type)
                        .unwrap_or("boundary")
                        .to_string();
                    let mut output = Vec::new();
                    for part in &multipart.parts {
                        output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                        output.extend_from_slice(
                            format!("Content-Type: {}\r\n", part.content_type).as_bytes(),
                        );
                        output.extend_from_slice(
                            format!(
                                "Content-Disposition: attachment; name=\"{}\"\r\n",
                                part.name
                            )
                            .as_bytes(),
                        );
                        output.extend_from_slice(b"\r\n");
                        match &part.data {
                            MultipartPartData::Json(json) => {
                                output.extend_from_slice(
                                    serde_json::to_vec(&json.data)
                                        .unwrap_or_default()
                                        .as_slice(),
                                );
                            }
                            MultipartPartData::Raw(raw) => {
                                output.extend_from_slice(&raw.data);
                            }
                        }
                        output.extend_from_slice(b"\r\n");
                    }
                    output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
                    RawSnapshotData {
                        data: output,
                        mime_type: multipart.mime_type,
                    }
                }
            };
            Ok(AgentInvocationResult::SaveSnapshot { snapshot })
        }
        PublicAgentInvocationResult::ProcessOplogEntries(params) => {
            Ok(AgentInvocationResult::ProcessOplogEntries {
                error: params.error,
            })
        }
    }
}

/// Chained conversion: proto → `PublicOplogEntry` → domain `OplogEntry`.
impl TryFrom<golem_api_grpc::proto::golem::worker::OplogEntry> for OplogEntry {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::worker::OplogEntry) -> Result<Self, String> {
        let public: PublicOplogEntry = value.try_into()?;
        public.try_into()
    }
}

// =============================================================================
// Raw OplogEntry ↔ proto RawOplogEntry conversions
// =============================================================================

fn oplog_payload_to_proto<T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq>(
    payload: OplogPayload<T>,
) -> Result<golem_api_grpc::proto::golem::worker::RawOplogPayload, String> {
    use golem_api_grpc::proto::golem::worker::raw_oplog_payload::Payload;
    use golem_api_grpc::proto::golem::worker::{RawExternalPayload, RawOplogPayload};
    match payload {
        OplogPayload::Inline(data) => {
            let bytes = crate::serialization::serialize(&data)?;
            Ok(RawOplogPayload {
                payload: Some(Payload::InlineData(bytes)),
            })
        }
        OplogPayload::SerializedInline { bytes, .. } => Ok(RawOplogPayload {
            payload: Some(Payload::InlineData(bytes)),
        }),
        OplogPayload::External {
            payload_id,
            md5_hash,
            ..
        } => Ok(RawOplogPayload {
            payload: Some(Payload::External(RawExternalPayload {
                payload_id: Some(payload_id.0.into()),
                md5_hash,
            })),
        }),
    }
}

fn oplog_payload_from_proto<T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq>(
    proto: golem_api_grpc::proto::golem::worker::RawOplogPayload,
) -> Result<OplogPayload<T>, String> {
    use golem_api_grpc::proto::golem::worker::raw_oplog_payload::Payload;
    match proto.payload.ok_or("Missing payload in RawOplogPayload")? {
        Payload::InlineData(bytes) => Ok(OplogPayload::SerializedInline {
            bytes,
            cached: None,
        }),
        Payload::External(ext) => {
            let payload_id = crate::model::oplog::raw_types::PayloadId(uuid::Uuid::from(
                ext.payload_id.ok_or("Missing payload_id")?,
            ));
            Ok(OplogPayload::External {
                payload_id,
                md5_hash: ext.md5_hash,
                cached: None,
            })
        }
    }
}

fn span_data_to_proto(
    span: crate::model::oplog::raw_types::SpanData,
) -> golem_api_grpc::proto::golem::worker::RawSpanData {
    use crate::model::oplog::raw_types::SpanData;
    use golem_api_grpc::proto::golem::worker::{
        RawExternalSpan, RawLocalSpan, RawSpanData as ProtoSpanData, raw_span_data,
    };
    match span {
        SpanData::LocalSpan {
            span_id,
            start,
            parent_id,
            linked_context,
            attributes,
            inherited,
        } => ProtoSpanData {
            span: Some(raw_span_data::Span::LocalSpan(RawLocalSpan {
                span_id: span_id.to_string(),
                start: Some(start.into()),
                parent_id: parent_id.map(|id| id.to_string()),
                linked_context: linked_context
                    .unwrap_or_default()
                    .into_iter()
                    .map(span_data_to_proto)
                    .collect(),
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            match v {
                                crate::model::invocation_context::AttributeValue::String(s) => s,
                            },
                        )
                    })
                    .collect(),
                inherited,
            })),
        },
        SpanData::ExternalSpan { span_id } => ProtoSpanData {
            span: Some(raw_span_data::Span::ExternalSpan(RawExternalSpan {
                span_id: span_id.to_string(),
            })),
        },
    }
}

fn span_data_from_proto(
    proto: golem_api_grpc::proto::golem::worker::RawSpanData,
) -> Result<crate::model::oplog::raw_types::SpanData, String> {
    use crate::model::oplog::raw_types::SpanData;
    use golem_api_grpc::proto::golem::worker::raw_span_data::Span;
    match proto.span.ok_or("Missing span data")? {
        Span::LocalSpan(local) => {
            let span_id = SpanId::from_string(local.span_id)?;
            let start: crate::model::Timestamp = local
                .start
                .ok_or("Missing start timestamp in LocalSpan")?
                .into();
            let parent_id = local.parent_id.map(SpanId::from_string).transpose()?;
            let linked_context = if local.linked_context.is_empty() {
                None
            } else {
                Some(
                    local
                        .linked_context
                        .into_iter()
                        .map(span_data_from_proto)
                        .collect::<Result<Vec<_>, _>>()?,
                )
            };
            let attributes = local
                .attributes
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        crate::model::invocation_context::AttributeValue::String(v),
                    )
                })
                .collect();
            Ok(SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited: local.inherited,
            })
        }
        Span::ExternalSpan(ext) => Ok(SpanData::ExternalSpan {
            span_id: SpanId::from_string(ext.span_id)?,
        }),
    }
}

fn durable_function_type_to_proto(
    dft: DurableFunctionType,
) -> golem_api_grpc::proto::golem::worker::WrappedFunctionType {
    use golem_api_grpc::proto::golem::worker::wrapped_function_type;
    match dft {
        DurableFunctionType::ReadLocal => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::ReadLocal as i32,
                oplog_index: None,
            }
        }
        DurableFunctionType::WriteLocal => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteLocal as i32,
                oplog_index: None,
            }
        }
        DurableFunctionType::ReadRemote => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::ReadRemote as i32,
                oplog_index: None,
            }
        }
        DurableFunctionType::WriteRemote => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteRemote as i32,
                oplog_index: None,
            }
        }
        DurableFunctionType::WriteRemoteBatched(idx) => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteRemoteBatched as i32,
                oplog_index: idx.map(|i| i.into()),
            }
        }
        DurableFunctionType::WriteRemoteTransaction(idx) => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteRemoteTransaction as i32,
                oplog_index: idx.map(|i| i.into()),
            }
        }
    }
}

fn durable_function_type_from_proto(
    wft: golem_api_grpc::proto::golem::worker::WrappedFunctionType,
) -> Result<DurableFunctionType, String> {
    use golem_api_grpc::proto::golem::worker::wrapped_function_type;
    match wft.r#type() {
        wrapped_function_type::Type::ReadLocal => Ok(DurableFunctionType::ReadLocal),
        wrapped_function_type::Type::WriteLocal => Ok(DurableFunctionType::WriteLocal),
        wrapped_function_type::Type::ReadRemote => Ok(DurableFunctionType::ReadRemote),
        wrapped_function_type::Type::WriteRemote => Ok(DurableFunctionType::WriteRemote),
        wrapped_function_type::Type::WriteRemoteBatched => Ok(
            DurableFunctionType::WriteRemoteBatched(wft.oplog_index.map(OplogIndex::from_u64)),
        ),
        wrapped_function_type::Type::WriteRemoteTransaction => Ok(
            DurableFunctionType::WriteRemoteTransaction(wft.oplog_index.map(OplogIndex::from_u64)),
        ),
    }
}

fn update_description_to_proto(
    desc: crate::model::oplog::raw_types::UpdateDescription,
) -> Result<golem_api_grpc::proto::golem::worker::RawUpdateDescription, String> {
    use crate::model::oplog::raw_types::UpdateDescription;
    use golem_api_grpc::proto::golem::worker::raw_update_description::Description;
    use golem_api_grpc::proto::golem::worker::{RawSnapshotBasedUpdate, RawUpdateDescription};
    match desc {
        UpdateDescription::Automatic { target_revision } => Ok(RawUpdateDescription {
            description: Some(Description::AutomaticTargetRevision(target_revision.into())),
        }),
        UpdateDescription::SnapshotBased {
            target_revision,
            payload,
            mime_type,
        } => Ok(RawUpdateDescription {
            description: Some(Description::SnapshotBased(RawSnapshotBasedUpdate {
                target_revision: target_revision.into(),
                payload: Some(oplog_payload_to_proto(payload)?),
                mime_type,
            })),
        }),
    }
}

fn update_description_from_proto(
    proto: golem_api_grpc::proto::golem::worker::RawUpdateDescription,
) -> Result<crate::model::oplog::raw_types::UpdateDescription, String> {
    use crate::model::oplog::raw_types::UpdateDescription;
    use golem_api_grpc::proto::golem::worker::raw_update_description::Description;
    match proto
        .description
        .ok_or("Missing description in RawUpdateDescription")?
    {
        Description::AutomaticTargetRevision(rev) => Ok(UpdateDescription::Automatic {
            target_revision: rev.try_into().map_err(|e: String| e)?,
        }),
        Description::SnapshotBased(snap) => Ok(UpdateDescription::SnapshotBased {
            target_revision: snap.target_revision.try_into().map_err(|e: String| e)?,
            payload: oplog_payload_from_proto(
                snap.payload
                    .ok_or("Missing payload in SnapshotBasedUpdate")?,
            )?,
            mime_type: snap.mime_type,
        }),
    }
}

impl TryFrom<OplogEntry> for golem_api_grpc::proto::golem::worker::RawOplogEntry {
    type Error = String;

    fn try_from(
        value: OplogEntry,
    ) -> Result<golem_api_grpc::proto::golem::worker::RawOplogEntry, String> {
        use golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry;
        use golem_api_grpc::proto::golem::worker::{
            RawActivatePluginParameters, RawAgentInvocationFinishedParameters,
            RawAgentInvocationStartedParameters, RawBeginRemoteTransactionParameters,
            RawCancelPendingInvocationParameters, RawChangePersistenceLevelParameters,
            RawCreateParameters, RawCreateResourceParameters, RawDeactivatePluginParameters,
            RawDropResourceParameters, RawEndAtomicRegionParameters, RawEndRemoteWriteParameters,
            RawEnvVar, RawErrorParameters, RawFailedUpdateParameters,
            RawFilesystemStorageUsageUpdateParameters, RawFinishSpanParameters,
            RawGrowMemoryParameters, RawHostCallParameters, RawJumpParameters, RawLogParameters,
            RawOplogProcessorCheckpointParameters, RawOplogRegion,
            RawPendingAgentInvocationParameters, RawPendingUpdateParameters,
            RawRemoteTransactionParameters, RawRemoveRetryPolicyParameters, RawResourceTypeId,
            RawRevertParameters, RawSetRetryPolicyParameters, RawSetSpanAttributeParameters,
            RawSnapshotParameters, RawStartSpanParameters, RawSuccessfulUpdateParameters,
            RawTimestampOnly,
        };

        let timestamp = value.timestamp();
        let proto_ts: prost_types::Timestamp = timestamp.into();

        let entry = match value {
            OplogEntry::Create {
                agent_id,
                component_revision,
                env,
                environment_id,
                created_by,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                local_agent_config,
                original_phantom_id,
                ..
            } => Entry::Create(RawCreateParameters {
                agent_id: Some(agent_id.into()),
                component_revision: component_revision.into(),
                env: env
                    .into_iter()
                    .map(|(k, v)| RawEnvVar { key: k, value: v })
                    .collect(),
                environment_id: Some(environment_id.into()),
                created_by: Some(created_by.into()),
                parent: parent.map(Into::into),
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins: initial_active_plugins
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                local_agent_config: local_agent_config
                    .into_iter()
                    .map(|e| crate::serialization::serialize(&e))
                    .collect::<Result<Vec<_>, _>>()?,
                original_phantom_id: original_phantom_id.map(Into::into),
            }),
            OplogEntry::HostCall {
                function_name,
                request,
                response,
                durable_function_type,
                ..
            } => Entry::HostCall(RawHostCallParameters {
                function_name: function_name.to_string(),
                request: Some(oplog_payload_to_proto(request)?),
                response: Some(oplog_payload_to_proto(response)?),
                durable_function_type: Some(durable_function_type_to_proto(durable_function_type)),
            }),
            OplogEntry::AgentInvocationStarted {
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                ..
            } => Entry::AgentInvocationStarted(RawAgentInvocationStartedParameters {
                idempotency_key: Some(idempotency_key.into()),
                payload: Some(oplog_payload_to_proto(payload)?),
                trace_id: trace_id.to_string(),
                trace_states,
                invocation_context: invocation_context
                    .into_iter()
                    .map(span_data_to_proto)
                    .collect(),
            }),
            OplogEntry::AgentInvocationFinished {
                result,
                consumed_fuel,
                component_revision,
                ..
            } => Entry::AgentInvocationFinished(RawAgentInvocationFinishedParameters {
                result: Some(oplog_payload_to_proto(result)?),
                consumed_fuel,
                component_revision: component_revision.into(),
            }),
            OplogEntry::Suspend { .. } => Entry::Suspend(RawTimestampOnly {}),
            OplogEntry::Error {
                error,
                retry_from,
                inside_atomic_region,
                retry_policy_state,
                ..
            } => Entry::Error(RawErrorParameters {
                error: Some(error.into()),
                retry_from: retry_from.into(),
                inside_atomic_region,
                retry_policy_state: retry_policy_state
                    .map(|s| crate::serialization::serialize(&s))
                    .transpose()?,
            }),
            OplogEntry::NoOp { .. } => Entry::NoOp(RawTimestampOnly {}),
            OplogEntry::Jump { jump, .. } => Entry::Jump(RawJumpParameters {
                jump: Some(RawOplogRegion {
                    start: jump.start.into(),
                    end: jump.end.into(),
                }),
            }),
            OplogEntry::Interrupted { .. } => Entry::Interrupted(RawTimestampOnly {}),
            OplogEntry::Exited { .. } => Entry::Exited(RawTimestampOnly {}),
            OplogEntry::BeginAtomicRegion { .. } => Entry::BeginAtomicRegion(RawTimestampOnly {}),
            OplogEntry::EndAtomicRegion { begin_index, .. } => {
                Entry::EndAtomicRegion(RawEndAtomicRegionParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::BeginRemoteWrite { .. } => Entry::BeginRemoteWrite(RawTimestampOnly {}),
            OplogEntry::EndRemoteWrite { begin_index, .. } => {
                Entry::EndRemoteWrite(RawEndRemoteWriteParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::PendingAgentInvocation {
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                ..
            } => Entry::PendingAgentInvocation(RawPendingAgentInvocationParameters {
                idempotency_key: Some(idempotency_key.into()),
                payload: Some(oplog_payload_to_proto(payload)?),
                trace_id: trace_id.to_string(),
                trace_states,
                invocation_context: invocation_context
                    .into_iter()
                    .map(span_data_to_proto)
                    .collect(),
            }),
            OplogEntry::PendingUpdate { description, .. } => {
                Entry::PendingUpdate(RawPendingUpdateParameters {
                    description: Some(update_description_to_proto(description)?),
                })
            }
            OplogEntry::SuccessfulUpdate {
                target_revision,
                new_component_size,
                new_active_plugins,
                ..
            } => Entry::SuccessfulUpdate(RawSuccessfulUpdateParameters {
                target_revision: target_revision.into(),
                new_component_size,
                new_active_plugins: new_active_plugins.into_iter().map(Into::into).collect(),
            }),
            OplogEntry::FailedUpdate {
                target_revision,
                details,
                ..
            } => Entry::FailedUpdate(RawFailedUpdateParameters {
                target_revision: target_revision.into(),
                details,
            }),
            OplogEntry::GrowMemory { delta, .. } => {
                Entry::GrowMemory(RawGrowMemoryParameters { delta })
            }
            OplogEntry::FilesystemStorageUsageUpdate { delta, .. } => {
                Entry::FilesystemStorageUsageUpdate(RawFilesystemStorageUsageUpdateParameters {
                    delta,
                })
            }
            OplogEntry::CreateResource {
                id,
                resource_type_id,
                ..
            } => Entry::CreateResource(RawCreateResourceParameters {
                id: id.0,
                resource_type_id: Some(RawResourceTypeId {
                    name: resource_type_id.name,
                    owner: resource_type_id.owner,
                }),
            }),
            OplogEntry::DropResource {
                id,
                resource_type_id,
                ..
            } => Entry::DropResource(RawDropResourceParameters {
                id: id.0,
                resource_type_id: Some(RawResourceTypeId {
                    name: resource_type_id.name,
                    owner: resource_type_id.owner,
                }),
            }),
            OplogEntry::Log {
                level,
                context,
                message,
                ..
            } => Entry::Log(RawLogParameters {
                level: Into::<golem_api_grpc::proto::golem::worker::OplogLogLevel>::into(level)
                    as i32,
                context,
                message,
            }),
            OplogEntry::Restart { .. } => Entry::Restart(RawTimestampOnly {}),
            OplogEntry::ActivatePlugin {
                plugin_grant_id, ..
            } => Entry::ActivatePlugin(RawActivatePluginParameters {
                plugin_grant_id: Some(plugin_grant_id.into()),
            }),
            OplogEntry::DeactivatePlugin {
                plugin_grant_id, ..
            } => Entry::DeactivatePlugin(RawDeactivatePluginParameters {
                plugin_grant_id: Some(plugin_grant_id.into()),
            }),
            OplogEntry::Revert { dropped_region, .. } => Entry::Revert(RawRevertParameters {
                dropped_region: Some(RawOplogRegion {
                    start: dropped_region.start.into(),
                    end: dropped_region.end.into(),
                }),
            }),
            OplogEntry::CancelPendingInvocation {
                idempotency_key, ..
            } => Entry::CancelPendingInvocation(RawCancelPendingInvocationParameters {
                idempotency_key: Some(idempotency_key.into()),
            }),
            OplogEntry::StartSpan {
                span_id,
                parent,
                linked_context_id,
                attributes,
                ..
            } => Entry::StartSpan(RawStartSpanParameters {
                span_id: span_id.to_string(),
                parent: parent.map(|id| id.to_string()),
                linked_context_id: linked_context_id.map(|id: SpanId| id.to_string()),
                attributes: attributes
                    .0
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            match v {
                                crate::model::invocation_context::AttributeValue::String(s) => s,
                            },
                        )
                    })
                    .collect(),
            }),
            OplogEntry::FinishSpan { span_id, .. } => Entry::FinishSpan(RawFinishSpanParameters {
                span_id: span_id.to_string(),
            }),
            OplogEntry::SetSpanAttribute {
                span_id,
                key,
                value,
                ..
            } => Entry::SetSpanAttribute(RawSetSpanAttributeParameters {
                span_id: span_id.to_string(),
                key,
                value: match value {
                    crate::model::invocation_context::AttributeValue::String(s) => s,
                },
            }),
            OplogEntry::ChangePersistenceLevel {
                persistence_level, ..
            } => Entry::ChangePersistenceLevel(RawChangePersistenceLevelParameters {
                persistence_level:
                    Into::<golem_api_grpc::proto::golem::worker::PersistenceLevel>::into(
                        persistence_level,
                    ) as i32,
            }),
            OplogEntry::BeginRemoteTransaction {
                transaction_id,
                original_begin_index,
                ..
            } => Entry::BeginRemoteTransaction(RawBeginRemoteTransactionParameters {
                transaction_id: String::from(transaction_id),
                original_begin_index: original_begin_index.map(|i: OplogIndex| i.into()),
            }),
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. } => {
                Entry::PreCommitRemoteTransaction(RawRemoteTransactionParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::PreRollbackRemoteTransaction { begin_index, .. } => {
                Entry::PreRollbackRemoteTransaction(RawRemoteTransactionParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::CommittedRemoteTransaction { begin_index, .. } => {
                Entry::CommittedRemoteTransaction(RawRemoteTransactionParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => {
                Entry::RolledBackRemoteTransaction(RawRemoteTransactionParameters {
                    begin_index: begin_index.into(),
                })
            }
            OplogEntry::Snapshot {
                data, mime_type, ..
            } => Entry::Snapshot(RawSnapshotParameters {
                data: Some(oplog_payload_to_proto(data)?),
                mime_type,
            }),
            OplogEntry::OplogProcessorCheckpoint {
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
                ..
            } => Entry::OplogProcessorCheckpoint(RawOplogProcessorCheckpointParameters {
                plugin_grant_id: Some(plugin_grant_id.into()),
                target_agent_id: Some(target_agent_id.into()),
                confirmed_up_to: confirmed_up_to.into(),
                sending_up_to: sending_up_to.into(),
                last_batch_start: last_batch_start.into(),
            }),
            OplogEntry::SetRetryPolicy { policy, .. } => {
                Entry::SetRetryPolicy(RawSetRetryPolicyParameters {
                    policy: Some(policy.into()),
                })
            }
            OplogEntry::RemoveRetryPolicy { name, .. } => {
                Entry::RemoveRetryPolicy(RawRemoveRetryPolicyParameters { name })
            }
        };

        Ok(golem_api_grpc::proto::golem::worker::RawOplogEntry {
            timestamp: Some(proto_ts),
            entry: Some(entry),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::RawOplogEntry> for OplogEntry {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::RawOplogEntry,
    ) -> Result<Self, String> {
        use golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry;

        let timestamp: crate::model::Timestamp = value
            .timestamp
            .ok_or("Missing timestamp in RawOplogEntry")?
            .into();

        match value.entry.ok_or("Missing entry in RawOplogEntry")? {
            Entry::Create(p) => {
                let agent_id = p.agent_id.ok_or("Missing agent_id")?.try_into()?;
                let component_revision: crate::model::component::ComponentRevision =
                    p.component_revision.try_into().map_err(|e: String| e)?;
                let env: Vec<(String, String)> =
                    p.env.into_iter().map(|e| (e.key, e.value)).collect();
                let environment_id = p
                    .environment_id
                    .ok_or("Missing environment_id")?
                    .try_into()?;
                let created_by = p.created_by.ok_or("Missing created_by")?.try_into()?;
                let parent = p.parent.map(|a| a.try_into()).transpose()?;
                let initial_active_plugins = p
                    .initial_active_plugins
                    .into_iter()
                    .map(|id| id.try_into())
                    .collect::<Result<std::collections::HashSet<_>, _>>()?;
                let local_agent_config = p
                    .local_agent_config
                    .into_iter()
                    .map(|bytes| crate::serialization::deserialize(&bytes))
                    .collect::<Result<Vec<_>, _>>()?;
                let original_phantom_id: Option<uuid::Uuid> = p.original_phantom_id.map(|u| {
                    let proto_uuid: golem_api_grpc::proto::golem::common::Uuid = u;
                    uuid::Uuid::from(proto_uuid)
                });
                Ok(OplogEntry::Create {
                    timestamp,
                    agent_id,
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    parent,
                    component_size: p.component_size,
                    initial_total_linear_memory_size: p.initial_total_linear_memory_size,
                    initial_active_plugins,
                    local_agent_config,
                    original_phantom_id,
                })
            }
            Entry::HostCall(p) => {
                let function_name =
                    crate::model::oplog::payload::host_functions::HostFunctionName::from(
                        p.function_name.as_str(),
                    );
                let request =
                    oplog_payload_from_proto(p.request.ok_or("Missing request payload")?)?;
                let response =
                    oplog_payload_from_proto(p.response.ok_or("Missing response payload")?)?;
                let durable_function_type = durable_function_type_from_proto(
                    p.durable_function_type
                        .ok_or("Missing durable_function_type")?,
                )?;
                Ok(OplogEntry::HostCall {
                    timestamp,
                    function_name,
                    request,
                    response,
                    durable_function_type,
                })
            }
            Entry::AgentInvocationStarted(p) => {
                let idempotency_key = p.idempotency_key.ok_or("Missing idempotency_key")?.into();
                let payload = oplog_payload_from_proto(p.payload.ok_or("Missing payload")?)?;
                let trace_id = TraceId::from_string(p.trace_id)?;
                let invocation_context = p
                    .invocation_context
                    .into_iter()
                    .map(span_data_from_proto)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OplogEntry::AgentInvocationStarted {
                    timestamp,
                    idempotency_key,
                    payload,
                    trace_id,
                    trace_states: p.trace_states,
                    invocation_context,
                })
            }
            Entry::AgentInvocationFinished(p) => {
                let result = oplog_payload_from_proto(p.result.ok_or("Missing result payload")?)?;
                let component_revision: crate::model::component::ComponentRevision =
                    p.component_revision.try_into().map_err(|e: String| e)?;
                Ok(OplogEntry::AgentInvocationFinished {
                    timestamp,
                    result,
                    consumed_fuel: p.consumed_fuel,
                    component_revision,
                })
            }
            Entry::Suspend(_) => Ok(OplogEntry::Suspend { timestamp }),
            Entry::Error(p) => {
                let error: crate::model::oplog::AgentError =
                    p.error.ok_or("Missing error")?.try_into()?;
                let retry_from = OplogIndex::from_u64(p.retry_from);
                let retry_policy_state = p
                    .retry_policy_state
                    .map(|bytes| crate::serialization::deserialize(&bytes))
                    .transpose()?;
                Ok(OplogEntry::Error {
                    timestamp,
                    error,
                    retry_from,
                    inside_atomic_region: p.inside_atomic_region,
                    retry_policy_state,
                })
            }
            Entry::NoOp(_) => Ok(OplogEntry::NoOp { timestamp }),
            Entry::Jump(p) => {
                let jump = p.jump.ok_or("Missing jump region")?;
                Ok(OplogEntry::Jump {
                    timestamp,
                    jump: crate::model::regions::OplogRegion {
                        start: OplogIndex::from_u64(jump.start),
                        end: OplogIndex::from_u64(jump.end),
                    },
                })
            }
            Entry::Interrupted(_) => Ok(OplogEntry::Interrupted { timestamp }),
            Entry::Exited(_) => Ok(OplogEntry::Exited { timestamp }),
            Entry::BeginAtomicRegion(_) => Ok(OplogEntry::BeginAtomicRegion { timestamp }),
            Entry::EndAtomicRegion(p) => Ok(OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index: OplogIndex::from_u64(p.begin_index),
            }),
            Entry::BeginRemoteWrite(_) => Ok(OplogEntry::BeginRemoteWrite { timestamp }),
            Entry::EndRemoteWrite(p) => Ok(OplogEntry::EndRemoteWrite {
                timestamp,
                begin_index: OplogIndex::from_u64(p.begin_index),
            }),
            Entry::PendingAgentInvocation(p) => {
                let idempotency_key = p.idempotency_key.ok_or("Missing idempotency_key")?.into();
                let payload = oplog_payload_from_proto(p.payload.ok_or("Missing payload")?)?;
                let trace_id = TraceId::from_string(p.trace_id)?;
                let invocation_context = p
                    .invocation_context
                    .into_iter()
                    .map(span_data_from_proto)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OplogEntry::PendingAgentInvocation {
                    timestamp,
                    idempotency_key,
                    payload,
                    trace_id,
                    trace_states: p.trace_states,
                    invocation_context,
                })
            }
            Entry::PendingUpdate(p) => {
                let description = update_description_from_proto(
                    p.description.ok_or("Missing update description")?,
                )?;
                Ok(OplogEntry::PendingUpdate {
                    timestamp,
                    description,
                })
            }
            Entry::SuccessfulUpdate(p) => {
                let target_revision: crate::model::component::ComponentRevision =
                    p.target_revision.try_into().map_err(|e: String| e)?;
                let new_active_plugins = p
                    .new_active_plugins
                    .into_iter()
                    .map(|id| id.try_into())
                    .collect::<Result<std::collections::HashSet<_>, _>>()?;
                Ok(OplogEntry::SuccessfulUpdate {
                    timestamp,
                    target_revision,
                    new_component_size: p.new_component_size,
                    new_active_plugins,
                })
            }
            Entry::FailedUpdate(p) => {
                let target_revision: crate::model::component::ComponentRevision =
                    p.target_revision.try_into().map_err(|e: String| e)?;
                Ok(OplogEntry::FailedUpdate {
                    timestamp,
                    target_revision,
                    details: p.details,
                })
            }
            Entry::GrowMemory(p) => Ok(OplogEntry::GrowMemory {
                timestamp,
                delta: p.delta,
            }),
            Entry::FilesystemStorageUsageUpdate(p) => {
                Ok(OplogEntry::FilesystemStorageUsageUpdate {
                    timestamp,
                    delta: p.delta,
                })
            }
            Entry::CreateResource(p) => {
                let rt = p.resource_type_id.ok_or("Missing resource_type_id")?;
                Ok(OplogEntry::CreateResource {
                    timestamp,
                    id: AgentResourceId(p.id),
                    resource_type_id: ResourceTypeId {
                        name: rt.name,
                        owner: rt.owner,
                    },
                })
            }
            Entry::DropResource(p) => {
                let rt = p.resource_type_id.ok_or("Missing resource_type_id")?;
                Ok(OplogEntry::DropResource {
                    timestamp,
                    id: AgentResourceId(p.id),
                    resource_type_id: ResourceTypeId {
                        name: rt.name,
                        owner: rt.owner,
                    },
                })
            }
            Entry::Log(p) => {
                let level: LogLevel =
                    golem_api_grpc::proto::golem::worker::OplogLogLevel::try_from(p.level)
                        .map_err(|_| format!("Invalid log level: {}", p.level))?
                        .into();
                Ok(OplogEntry::Log {
                    timestamp,
                    level,
                    context: p.context,
                    message: p.message,
                })
            }
            Entry::Restart(_) => Ok(OplogEntry::Restart { timestamp }),
            Entry::ActivatePlugin(p) => {
                let plugin_grant_id = p
                    .plugin_grant_id
                    .ok_or("Missing plugin_grant_id")?
                    .try_into()?;
                Ok(OplogEntry::ActivatePlugin {
                    timestamp,
                    plugin_grant_id,
                })
            }
            Entry::DeactivatePlugin(p) => {
                let plugin_grant_id = p
                    .plugin_grant_id
                    .ok_or("Missing plugin_grant_id")?
                    .try_into()?;
                Ok(OplogEntry::DeactivatePlugin {
                    timestamp,
                    plugin_grant_id,
                })
            }
            Entry::Revert(p) => {
                let region = p.dropped_region.ok_or("Missing dropped_region")?;
                Ok(OplogEntry::Revert {
                    timestamp,
                    dropped_region: crate::model::regions::OplogRegion {
                        start: OplogIndex::from_u64(region.start),
                        end: OplogIndex::from_u64(region.end),
                    },
                })
            }
            Entry::CancelPendingInvocation(p) => {
                let idempotency_key = p.idempotency_key.ok_or("Missing idempotency_key")?.into();
                Ok(OplogEntry::CancelPendingInvocation {
                    timestamp,
                    idempotency_key,
                })
            }
            Entry::StartSpan(p) => {
                let span_id = SpanId::from_string(p.span_id)?;
                let parent = p.parent.map(SpanId::from_string).transpose()?;
                let linked_context_id = p.linked_context_id.map(SpanId::from_string).transpose()?;
                let attributes: HashMap<String, crate::model::invocation_context::AttributeValue> =
                    p.attributes
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k,
                                crate::model::invocation_context::AttributeValue::String(v),
                            )
                        })
                        .collect();
                Ok(OplogEntry::StartSpan {
                    timestamp,
                    span_id,
                    parent,
                    linked_context_id,
                    attributes: crate::model::oplog::raw_types::AttributeMap(attributes),
                })
            }
            Entry::FinishSpan(p) => {
                let span_id = SpanId::from_string(p.span_id)?;
                Ok(OplogEntry::FinishSpan { timestamp, span_id })
            }
            Entry::SetSpanAttribute(p) => {
                let span_id = SpanId::from_string(p.span_id)?;
                Ok(OplogEntry::SetSpanAttribute {
                    timestamp,
                    span_id,
                    key: p.key,
                    value: crate::model::invocation_context::AttributeValue::String(p.value),
                })
            }
            Entry::ChangePersistenceLevel(p) => {
                let persistence_level: PersistenceLevel =
                    golem_api_grpc::proto::golem::worker::PersistenceLevel::try_from(
                        p.persistence_level,
                    )
                    .map_err(|_| format!("Invalid persistence level: {}", p.persistence_level))?
                    .into();
                Ok(OplogEntry::ChangePersistenceLevel {
                    timestamp,
                    persistence_level,
                })
            }
            Entry::BeginRemoteTransaction(p) => {
                let transaction_id = crate::model::TransactionId::from(p.transaction_id);
                let original_begin_index = p.original_begin_index.map(OplogIndex::from_u64);
                Ok(OplogEntry::BeginRemoteTransaction {
                    timestamp,
                    transaction_id,
                    original_begin_index,
                })
            }
            Entry::PreCommitRemoteTransaction(p) => Ok(OplogEntry::PreCommitRemoteTransaction {
                timestamp,
                begin_index: OplogIndex::from_u64(p.begin_index),
            }),
            Entry::PreRollbackRemoteTransaction(p) => {
                Ok(OplogEntry::PreRollbackRemoteTransaction {
                    timestamp,
                    begin_index: OplogIndex::from_u64(p.begin_index),
                })
            }
            Entry::CommittedRemoteTransaction(p) => Ok(OplogEntry::CommittedRemoteTransaction {
                timestamp,
                begin_index: OplogIndex::from_u64(p.begin_index),
            }),
            Entry::RolledBackRemoteTransaction(p) => Ok(OplogEntry::RolledBackRemoteTransaction {
                timestamp,
                begin_index: OplogIndex::from_u64(p.begin_index),
            }),
            Entry::Snapshot(p) => {
                let data = oplog_payload_from_proto(p.data.ok_or("Missing snapshot data")?)?;
                Ok(OplogEntry::Snapshot {
                    timestamp,
                    data,
                    mime_type: p.mime_type,
                })
            }
            Entry::OplogProcessorCheckpoint(p) => {
                let plugin_grant_id = p
                    .plugin_grant_id
                    .ok_or("Missing plugin_grant_id")?
                    .try_into()?;
                let target_agent_id = p
                    .target_agent_id
                    .ok_or("Missing target_agent_id")?
                    .try_into()?;
                Ok(OplogEntry::OplogProcessorCheckpoint {
                    timestamp,
                    plugin_grant_id,
                    target_agent_id,
                    confirmed_up_to: OplogIndex::from_u64(p.confirmed_up_to),
                    sending_up_to: OplogIndex::from_u64(p.sending_up_to),
                    last_batch_start: OplogIndex::from_u64(p.last_batch_start),
                })
            }
            Entry::SetRetryPolicy(p) => {
                let policy: crate::model::retry_policy::NamedRetryPolicy =
                    p.policy.ok_or("Missing policy")?.try_into()?;
                Ok(OplogEntry::SetRetryPolicy { timestamp, policy })
            }
            Entry::RemoveRetryPolicy(p) => Ok(OplogEntry::RemoveRetryPolicy {
                timestamp,
                name: p.name,
            }),
        }
    }
}
