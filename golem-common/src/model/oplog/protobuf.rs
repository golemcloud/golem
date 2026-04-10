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
use crate::model::Empty;
use crate::model::agent::DataValue;
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
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
use crate::model::oplog::{AgentTerminatedByQuotaError, PersistenceLevel};
use crate::model::quota::ResourceName;
use crate::model::regions::OplogRegion;
use crate::model::worker::ParsedWorkerAgentConfigEntry;
use golem_api_grpc::proto::golem::worker::oplog_entry::Entry;
use golem_api_grpc::proto::golem::worker::{
    AttributeValue, ExternalParentSpan, InvocationSpan, LocalInvocationSpan, invocation_span,
    oplog_entry, wrapped_function_type,
};
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
                config_vars: create.config_vars.into_iter().collect(),
                local_agent_config: create
                    .agent_config
                    .into_iter()
                    .map(ParsedWorkerAgentConfigEntry::try_from)
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
                        config_vars: create.config_vars.into_iter().collect(),
                        agent_config: create
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
