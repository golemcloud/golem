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

use super::{
    ExportedFunctionParameters, LogLevel, ManualUpdateParameters, OplogCursor,
    PluginInstallationDescription, PublicAttribute, PublicAttributeValue,
    PublicDurableFunctionType, PublicExternalSpanData, PublicLocalSpanData, PublicOplogEntry,
    PublicOplogEntryWithIndex, PublicRetryConfig, PublicSpanData, PublicUpdateDescription,
    PublicWorkerInvocation, SnapshotBasedUpdateParameters, StringAttributeValue, WorkerError,
    WorkerResourceId, WriteRemoteBatchedParameters, WriteRemoteTransactionParameters,
};
use crate::base_model::OplogIndex;
use crate::model::component::{ComponentRevision, PluginPriority};
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, BeginAtomicRegionParams, BeginRemoteTransactionParams,
    BeginRemoteWriteParams, CancelPendingInvocationParams, ChangePersistenceLevelParams,
    ChangeRetryPolicyParams, CommittedRemoteTransactionParams, CreateParams, CreateResourceParams,
    DeactivatePluginParams, DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams,
    ErrorParams, ExitedParams, ExportedFunctionCompletedParams, ExportedFunctionInvokedParams,
    FailedUpdateParams, FinishSpanParams, GrowMemoryParams, ImportedFunctionInvokedParams,
    InterruptedParams, JumpParams, LogParams, NoOpParams, PendingUpdateParams,
    PendingWorkerInvocationParams, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, RestartParams, RevertParams,
    RolledBackRemoteTransactionParams, SetSpanAttributeParams, StartSpanParams,
    SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::PersistenceLevel;
use crate::model::regions::OplogRegion;
use crate::model::Empty;
use golem_api_grpc::proto::golem::worker::oplog_entry::Entry;
use golem_api_grpc::proto::golem::worker::{
    invocation_span, oplog_entry, worker_invocation, wrapped_function_type, AttributeValue,
    ExternalParentSpan, InvocationSpan, LocalInvocationSpan,
};
use golem_wasm::ValueAndType;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU64;
use std::time::Duration;

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

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerError> for WorkerError {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerError,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::worker_error::Error;
        match value.error.ok_or("no error field")? {
            Error::StackOverflow(_) => Ok(Self::StackOverflow),
            Error::OutOfMemory(_) => Ok(Self::OutOfMemory),
            Error::InvalidRequest(inner) => Ok(Self::InvalidRequest(inner.details)),
            Error::UnknownError(inner) => Ok(Self::Unknown(inner.details)),
            Error::ExceededMemoryLimit(_) => Ok(Self::ExceededMemoryLimit),
        }
    }
}

impl From<WorkerError> for golem_api_grpc::proto::golem::worker::WorkerError {
    fn from(value: WorkerError) -> Self {
        use golem_api_grpc::proto::golem::worker as grpc_worker;
        use golem_api_grpc::proto::golem::worker::worker_error::Error;
        let error = match value {
            WorkerError::StackOverflow => Error::StackOverflow(grpc_worker::StackOverflow {}),
            WorkerError::OutOfMemory => Error::OutOfMemory(grpc_worker::OutOfMemory {}),
            WorkerError::InvalidRequest(details) => {
                Error::InvalidRequest(grpc_worker::InvalidRequest { details })
            }
            WorkerError::Unknown(details) => {
                Error::UnknownError(grpc_worker::UnknownError { details })
            }
            WorkerError::ExceededMemoryLimit => {
                Error::ExceededMemoryLimit(grpc_worker::ExceededMemoryLimit {})
            }
        };
        Self { error: Some(error) }
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogCursor> for OplogCursor {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_version: value.current_component_version,
        }
    }
}

impl From<OplogCursor> for golem_api_grpc::proto::golem::worker::OplogCursor {
    fn from(value: OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_version: value.current_component_version,
        }
    }
}

impl From<PluginInstallationDescription>
    for golem_api_grpc::proto::golem::worker::PluginInstallationDescription
{
    fn from(plugin_installation_description: PluginInstallationDescription) -> Self {
        golem_api_grpc::proto::golem::worker::PluginInstallationDescription {
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
                worker_id: create
                    .worker_id
                    .ok_or("Missing worker_id field")?
                    .try_into()?,
                component_revision: ComponentRevision(create.component_version),
                env: create.env.into_iter().collect(),
                environment_id: create
                    .environment_id
                    .ok_or("Missing environment_id field")?
                    .try_into()?,
                created_by: create
                    .created_by
                    .ok_or("Missing created_by field")?
                    .try_into()?,
                wasi_config_vars: create
                    .wasi_config_vars
                    .ok_or("Missing wasi_config_vars field")?
                    .into(),
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
            oplog_entry::Entry::ImportedFunctionInvoked(imported_function_invoked) => Ok(
                PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParams {
                    timestamp: imported_function_invoked
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    function_name: imported_function_invoked.function_name,
                    request: imported_function_invoked
                        .request
                        .ok_or("Missing request field")?
                        .try_into()?,
                    response: imported_function_invoked
                        .response
                        .ok_or("Missing response field")?
                        .try_into()?,
                    durable_function_type: imported_function_invoked
                        .wrapped_function_type
                        .ok_or("Missing wrapped_function_type field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::ExportedFunctionInvoked(exported_function_invoked) => Ok(
                PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParams {
                    timestamp: exported_function_invoked
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    function_name: exported_function_invoked.function_name,
                    request: exported_function_invoked
                        .request
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<ValueAndType>, String>>()?,
                    idempotency_key: exported_function_invoked
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                    trace_id: TraceId::from_string(&exported_function_invoked.trace_id)?,
                    trace_states: exported_function_invoked.trace_states,
                    invocation_context: encode_public_span_data(
                        exported_function_invoked.invocation_context,
                    )?,
                }),
            ),
            oplog_entry::Entry::ExportedFunctionCompleted(exported_function_completed) => Ok(
                PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParams {
                    timestamp: exported_function_completed
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    response: exported_function_completed
                        .response
                        .map(|tav| tav.try_into())
                        .transpose()?,
                    consumed_fuel: exported_function_completed.consumed_fuel,
                }),
            ),
            oplog_entry::Entry::Suspend(suspend) => Ok(PublicOplogEntry::Suspend(SuspendParams {
                timestamp: suspend.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Error(error) => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp: error.timestamp.ok_or("Missing timestamp field")?.into(),
                error: error.error,
                retry_from: OplogIndex::from_u64(error.retry_from),
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
            oplog_entry::Entry::ChangeRetryPolicy(change_retry_policy) => Ok(
                PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParams {
                    timestamp: change_retry_policy
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    new_policy: change_retry_policy
                        .retry_policy
                        .ok_or("Missing retry_policy field")?
                        .try_into()?,
                }),
            ),
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
            oplog_entry::Entry::PendingWorkerInvocation(pending_worker_invocation) => Ok(
                PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParams {
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
                    target_revision: ComponentRevision(pending_update.target_version),
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
                    target_revision: ComponentRevision(successful_update.target_version),
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
                    target_revision: ComponentRevision(failed_update.target_version),
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
            oplog_entry::Entry::CreateResource(create_resource) => {
                Ok(PublicOplogEntry::CreateResource(CreateResourceParams {
                    timestamp: create_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: WorkerResourceId(create_resource.resource_id),
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
                    id: WorkerResourceId(drop_resource.resource_id),
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
                        worker_id: Some(create.worker_id.into()),
                        component_version: create.component_revision.0,
                        env: create.env.into_iter().collect(),
                        created_by: Some(create.created_by.into()),
                        environment_id: Some(create.environment_id.into()),
                        wasi_config_vars: Some(create.wasi_config_vars.into()),
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
            PublicOplogEntry::ImportedFunctionInvoked(imported_function_invoked) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ImportedFunctionInvoked(
                        golem_api_grpc::proto::golem::worker::ImportedFunctionInvokedParameters {
                            timestamp: Some(imported_function_invoked.timestamp.into()),
                            function_name: imported_function_invoked.function_name.clone(),
                            request: Some(imported_function_invoked.request.into()),
                            response: Some(imported_function_invoked.response.into()),
                            wrapped_function_type: Some(
                                imported_function_invoked.durable_function_type.into(),
                            ),
                        },
                    )),
                }
            }
            PublicOplogEntry::ExportedFunctionInvoked(exported_function_invoked) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ExportedFunctionInvoked(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionInvokedParameters {
                            timestamp: Some(exported_function_invoked.timestamp.into()),
                            function_name: exported_function_invoked.function_name.clone(),
                            request: exported_function_invoked
                                .request
                                .into_iter()
                                .map(|value| value.into())
                                .collect(),
                            idempotency_key: Some(exported_function_invoked.idempotency_key.into()),
                            trace_id: exported_function_invoked.trace_id.to_string(),
                            trace_states: exported_function_invoked.trace_states,
                            invocation_context: decode_public_span_data(
                                &exported_function_invoked.invocation_context,
                                0,
                            ),
                        },
                    )),
                }
            }
            PublicOplogEntry::ExportedFunctionCompleted(exported_function_completed) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ExportedFunctionCompleted(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionCompletedParameters {
                            timestamp: Some(exported_function_completed.timestamp.into()),
                            response: exported_function_completed
                                .response
                                .map(|value| value.into()),
                            consumed_fuel: exported_function_completed.consumed_fuel,
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
            PublicOplogEntry::ChangeRetryPolicy(change_retry_policy) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ChangeRetryPolicy(
                        golem_api_grpc::proto::golem::worker::ChangeRetryPolicyParameters {
                            timestamp: Some(change_retry_policy.timestamp.into()),
                            retry_policy: Some(change_retry_policy.new_policy.into()),
                        },
                    )),
                }
            }
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
            PublicOplogEntry::PendingWorkerInvocation(pending_worker_invocation) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PendingWorkerInvocation(
                        golem_api_grpc::proto::golem::worker::PendingWorkerInvocationParameters {
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
                            target_version: pending_update.target_revision.0,
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
                            target_version: successful_update.target_revision.0,
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
                            target_version: failed_update.target_revision.0,
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

impl TryFrom<golem_api_grpc::proto::golem::worker::RetryPolicy> for PublicRetryConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::RetryPolicy,
    ) -> Result<Self, Self::Error> {
        Ok(PublicRetryConfig {
            max_attempts: value.max_attempts,
            min_delay: Duration::from_millis(value.min_delay),
            max_delay: Duration::from_millis(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        })
    }
}

impl From<PublicRetryConfig> for golem_api_grpc::proto::golem::worker::RetryPolicy {
    fn from(value: PublicRetryConfig) -> Self {
        golem_api_grpc::proto::golem::worker::RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay.as_millis() as u64,
            max_delay: value.max_delay.as_millis() as u64,
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
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

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerInvocation> for PublicWorkerInvocation {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerInvocation,
    ) -> Result<Self, Self::Error> {
        match value.invocation.ok_or("Missing invocation field")? {
            worker_invocation::Invocation::ExportedFunction(exported_function) => Ok(
                PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                    idempotency_key: exported_function
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                    full_function_name: exported_function.function_name,
                    function_input: if exported_function.valid_input {
                        Some(
                            exported_function
                                .input
                                .into_iter()
                                .map(TryInto::try_into)
                                .collect::<Result<Vec<ValueAndType>, String>>()?,
                        )
                    } else {
                        None
                    },
                    trace_id: TraceId::from_string(&exported_function.trace_id)?,
                    trace_states: exported_function.trace_states,
                    invocation_context: encode_public_span_data(
                        exported_function.invocation_context,
                    )?,
                }),
            ),
            worker_invocation::Invocation::ManualUpdate(manual_update) => Ok(
                PublicWorkerInvocation::ManualUpdate(ManualUpdateParameters {
                    target_revision: ComponentRevision(manual_update),
                }),
            ),
        }
    }
}

impl TryFrom<PublicWorkerInvocation> for golem_api_grpc::proto::golem::worker::WorkerInvocation {
    type Error = String;

    fn try_from(value: PublicWorkerInvocation) -> Result<Self, Self::Error> {
        Ok(match value {
            PublicWorkerInvocation::ExportedFunction(exported_function) => {
                golem_api_grpc::proto::golem::worker::WorkerInvocation {
                    invocation: Some(worker_invocation::Invocation::ExportedFunction(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionInvocationParameters {
                            idempotency_key: Some(exported_function.idempotency_key.into()),
                            function_name: exported_function.full_function_name,
                            valid_input: exported_function.function_input.is_some(),
                            input: exported_function
                                .function_input
                                .unwrap_or_default()
                                .into_iter()
                                .map(|input| input.into()).collect(),
                            trace_id: exported_function.trace_id.to_string(),
                            trace_states: exported_function.trace_states,
                            invocation_context: decode_public_span_data(&exported_function.invocation_context, 0),
                        },
                    )),
                }
            }
            PublicWorkerInvocation::ManualUpdate(manual_update) => {
                golem_api_grpc::proto::golem::worker::WorkerInvocation {
                    invocation: Some(worker_invocation::Invocation::ManualUpdate(
                        manual_update.target_revision.0,
                    )),
                }
            }
        })
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
                                payload: snapshot_based.payload
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
            if let PublicSpanData::LocalSpan(ref mut local_span) = span {
                if let Some(linked_id) = &mut local_span.linked_context {
                    *linked_id += 1;
                }
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
