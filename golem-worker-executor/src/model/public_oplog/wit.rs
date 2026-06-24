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

use crate::preview2::golem_api_1_x::oplog;
use golem_common::base_model::oplog::{CardInstallFailure, PublicQueuedCardEvent, QueuedCardEvent};
use golem_common::model::card::CardId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, CancelPendingInvocationParams,
    CancelledParams, CardEventQueuedParams, CardInstallFailedParams, CardInstalledParams,
    CardRevokedParams, ChangePersistenceLevelParams, CommittedRemoteTransactionParams,
    CreateParams, CreateResourceParams, DeactivatePluginParams, DropResourceParams,
    EndAtomicRegionParams, EndParams, ErrorParams, ExitedParams, FailedUpdateParams,
    FilesystemStorageUsageUpdateParams, FinishSpanParams, GrowMemoryParams, InterruptedParams,
    JumpParams, LogParams, ManualUpdateParameters, NoOpParams, OplogProcessorCheckpointParams,
    PendingAgentInvocationParams, PendingUpdateParams, PluginInstallationDescription,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttributeValue, PublicDurableFunctionType, PublicSpanData,
    RemoveRetryPolicyParams, RestartParams, RevertParams, RolledBackRemoteTransactionParams,
    SetRetryPolicyParams, SetSpanAttributeParams, SnapshotParams, StartParams, StartSpanParams,
    StringAttributeValue, SuccessfulUpdateParams, SuspendParams, WriteRemoteBatchedParameters,
    WriteRemoteTransactionParameters,
};
use golem_common::model::oplog::{
    AgentInvocationOutputParameters, AgentTerminatedByQuotaError, EphemeralCannotSuspendError,
    EphemeralFuelExhaustedError, EphemeralSleepTooLongError, FallibleResultParameters,
    JsonSnapshotData, MultipartPartData, MultipartSnapshotData, PublicOplogEntry,
    PublicSnapshotData, PublicUpdateDescription, RawSnapshotData, ReadOnlyViolationError,
    SaveSnapshotResultParameters, SnapshotBasedUpdateParameters,
};
use golem_common::model::quota::ResourceName;
use golem_common::model::{Empty, Timestamp};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_schema::schema::wit::{decode_value, encode_typed, encode_value, wire};

/// Encode a public-oplog [`TypedSchemaValue`] into the `golem:core@2.0.0` WIT
/// wire form used by the oplog-processor plugin interface. Public-oplog
/// rendering always produces well-formed typed values, so encoding cannot
/// fail in practice.
fn encode_public_typed_schema_value(value: TypedSchemaValue) -> wire::TypedSchemaValue {
    encode_typed(&value).expect("public oplog TypedSchemaValue must be encodable as core@2.0.0 WIT")
}

fn encode_untyped_schema_value(value: SchemaValue) -> Result<wire::SchemaValueTree, String> {
    Ok(encode_value(&value))
}

fn decode_untyped_schema_value(value: wire::SchemaValueTree) -> Result<SchemaValue, String> {
    decode_value(&value).map_err(|e| e.to_string())
}

fn card_id_to_wit(card_id: CardId) -> oplog::CardId {
    oplog::CardId {
        uuid: card_id.0.into(),
    }
}

fn card_id_from_wit(card_id: oplog::CardId) -> CardId {
    CardId(card_id.uuid.into())
}

fn queued_card_event_to_wit(value: PublicQueuedCardEvent) -> oplog::PublicQueuedCardEvent {
    match value {
        PublicQueuedCardEvent::Install(event) => {
            oplog::PublicQueuedCardEvent::Install(oplog::PublicQueuedCardEventCard {
                card_id: card_id_to_wit(event.card_id),
            })
        }
        PublicQueuedCardEvent::Revoke(event) => {
            oplog::PublicQueuedCardEvent::Revoke(oplog::PublicQueuedCardEventCard {
                card_id: card_id_to_wit(event.card_id),
            })
        }
    }
}

fn raw_queued_card_event_to_wit(value: QueuedCardEvent) -> oplog::QueuedCardEvent {
    match value {
        QueuedCardEvent::Install(event) => {
            oplog::QueuedCardEvent::Install(oplog::QueuedCardEventCard {
                card_id: card_id_to_wit(event.card_id),
                card: event
                    .card
                    .and_then(|card| serde_json::to_vec(&card).ok())
                    .map(Some)
                    .unwrap_or_default(),
            })
        }
        QueuedCardEvent::Revoke(event) => {
            oplog::QueuedCardEvent::Revoke(oplog::QueuedCardEventCard {
                card_id: card_id_to_wit(event.card_id),
                card: None,
            })
        }
    }
}

fn card_install_failure_to_wit(value: CardInstallFailure) -> oplog::CardInstallFailure {
    match value {
        CardInstallFailure::CardRevoked => oplog::CardInstallFailure::CardRevoked,
        CardInstallFailure::NotFound => oplog::CardInstallFailure::NotFound,
        CardInstallFailure::RecipientMismatch => oplog::CardInstallFailure::RecipientMismatch,
        CardInstallFailure::NotPermitted => oplog::CardInstallFailure::NotPermitted,
    }
}

fn card_install_failure_from_wit(value: oplog::CardInstallFailure) -> CardInstallFailure {
    match value {
        oplog::CardInstallFailure::CardRevoked => CardInstallFailure::CardRevoked,
        oplog::CardInstallFailure::NotFound => CardInstallFailure::NotFound,
        oplog::CardInstallFailure::RecipientMismatch => CardInstallFailure::RecipientMismatch,
        oplog::CardInstallFailure::NotPermitted => CardInstallFailure::NotPermitted,
    }
}

impl From<PublicOplogEntry> for oplog::PublicOplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create(CreateParams {
                timestamp,
                agent_id,
                agent_mode,
                component_revision,
                env,
                created_by,
                environment_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                local_agent_config,
                original_phantom_id,
                instance_id,
            }) => Self::Create(oplog::CreateParameters {
                timestamp: timestamp.into(),
                agent_id: agent_id.into(),
                agent_mode: match agent_mode {
                    golem_common::model::agent::AgentMode::Durable => oplog::AgentMode::Durable,
                    golem_common::model::agent::AgentMode::Ephemeral => oplog::AgentMode::Ephemeral,
                },
                component_revision: component_revision.into(),
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
                local_agent_config: local_agent_config
                    .into_iter()
                    .map(|lac| oplog::LocalAgentConfigEntry {
                        path: lac.path,
                        value: encode_public_typed_schema_value(lac.value),
                    })
                    .collect(),
                original_phantom_id: original_phantom_id.map(|id| id.into()),
                instance_id: instance_id.into(),
            }),
            PublicOplogEntry::Start(StartParams {
                timestamp,
                parent_start_index,
                function_name,
                request,
                durable_function_type: wrapped_function_type,
            }) => Self::Start(oplog::StartParameters {
                timestamp: timestamp.into(),
                parent_start_index: parent_start_index.map(|c| c.into()),
                function_name,
                request: request.map(encode_public_typed_schema_value),
                durable_function_type: wrapped_function_type.into(),
            }),
            PublicOplogEntry::End(EndParams {
                timestamp,
                start_index,
                response,
                forced_commit,
            }) => Self::End(oplog::EndParameters {
                timestamp: timestamp.into(),
                start_index: start_index.into(),
                response: response.map(encode_public_typed_schema_value),
                forced_commit,
            }),
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp,
                start_index,
                partial,
            }) => Self::Cancelled(oplog::CancelledParameters {
                timestamp: timestamp.into(),
                start_index: start_index.into(),
                partial: partial.map(encode_public_typed_schema_value),
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
                method_name,
                consumed_fuel,
                component_revision,
            }) => Self::AgentInvocationFinished(oplog::AgentInvocationFinishedParameters {
                timestamp: timestamp.into(),
                result: result.into(),
                method_name,
                consumed_fuel,
                component_revision: component_revision.get(),
            }),
            PublicOplogEntry::Suspend(SuspendParams { timestamp }) => {
                Self::Suspend(timestamp.into())
            }
            PublicOplogEntry::Error(ErrorParams {
                timestamp,
                error,
                retry_from,
                inside_atomic_region,
                retry_policy_state,
            }) => Self::Error(oplog::ErrorParameters {
                timestamp: timestamp.into(),
                error: error.to_string(),
                retry_from: retry_from.into(),
                inside_atomic_region,
                retry_policy_state: retry_policy_state.map(|s| {
                    let internal: golem_common::model::RetryPolicyState = s.into();
                    internal.into()
                }),
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
                description: description.into(),
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
            PublicOplogEntry::FilesystemStorageUsageUpdate(
                FilesystemStorageUsageUpdateParams { timestamp, delta },
            ) => {
                Self::FilesystemStorageUsageUpdate(oplog::FilesystemStorageUsageUpdateParameters {
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
                id: id.0,
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
                id: id.0,
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
                    PublicSnapshotData::Multipart(multipart) => multipart_to_raw(multipart),
                };
                Self::Snapshot(oplog::SnapshotParameters {
                    timestamp: timestamp.into(),
                    data: oplog::SnapshotData {
                        data: snapshot_bytes,
                        mime_type,
                    },
                })
            }
            PublicOplogEntry::OplogProcessorCheckpoint(OplogProcessorCheckpointParams {
                timestamp,
                plugin,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
            }) => Self::OplogProcessorCheckpoint(oplog::OplogProcessorCheckpointParameters {
                timestamp: timestamp.into(),
                plugin: plugin.into(),
                target_agent_id: target_agent_id.into(),
                confirmed_up_to: confirmed_up_to.into(),
                sending_up_to: sending_up_to.into(),
                last_batch_start: last_batch_start.into(),
            }),
            PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams { timestamp, policy }) => {
                let named: golem_common::model::retry_policy::NamedRetryPolicy = policy.into();
                let wit_named: golem_common::schema::agent::bindings::golem::api::retry::NamedRetryPolicy = named.into();
                Self::SetRetryPolicy(oplog::SetRetryPolicyParameters {
                    timestamp: timestamp.into(),
                    policy: wit_named,
                })
            }
            PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams { timestamp, name }) => {
                Self::RemoveRetryPolicy(oplog::RemoveRetryPolicyParameters {
                    timestamp: timestamp.into(),
                    name,
                })
            }
            PublicOplogEntry::CardRevoked(CardRevokedParams {
                timestamp,
                queued_event_index,
                card_id,
            }) => Self::CardRevoked(oplog::CardRevokedParameters {
                timestamp: timestamp.into(),
                queued_event_index: queued_event_index.into(),
                card_id: card_id_to_wit(card_id),
            }),
            PublicOplogEntry::CardEventQueued(CardEventQueuedParams { timestamp, event }) => {
                Self::CardEventQueued(oplog::CardEventQueuedParameters {
                    timestamp: timestamp.into(),
                    event: queued_card_event_to_wit(event),
                })
            }
            PublicOplogEntry::CardInstalled(CardInstalledParams {
                timestamp,
                queued_event_index,
                card_id,
            }) => Self::CardInstalled(oplog::CardInstalledParameters {
                timestamp: timestamp.into(),
                queued_event_index: queued_event_index.map(Into::into),
                card_id: card_id_to_wit(card_id),
            }),
            PublicOplogEntry::CardInstallFailed(CardInstallFailedParams {
                timestamp,
                queued_event_index,
                card_id,
                reason,
            }) => Self::CardInstallFailed(oplog::CardInstallFailedParameters {
                timestamp: timestamp.into(),
                queued_event_index: queued_event_index.into(),
                card_id: card_id_to_wit(card_id),
                reason: card_install_failure_to_wit(reason),
            }),
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
                payload,
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
                Self::AgentInitialization(oplog::AgentInitializationParameters {
                    idempotency_key: params.idempotency_key.value,
                    constructor_parameters: encode_public_typed_schema_value(
                        params.constructor_parameters,
                    ),
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
                Self::AgentMethodInvocation(oplog::AgentMethodInvocationParameters {
                    idempotency_key: params.idempotency_key.value,
                    method_name: params.method_name,
                    function_input: encode_public_typed_schema_value(params.function_input),
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
                    PublicSnapshotData::Multipart(multipart) => multipart_to_raw(multipart),
                };
                Self::LoadSnapshot(oplog::LoadSnapshotParameters {
                    snapshot: oplog::SnapshotData { data, mime_type },
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
            }) => Self::AgentInitialization(oplog::AgentInvocationOutputParameters {
                output: encode_public_typed_schema_value(output),
            }),
            PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                output,
            }) => Self::AgentMethod(oplog::AgentInvocationOutputParameters {
                output: encode_public_typed_schema_value(output),
            }),
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
                    PublicSnapshotData::Multipart(multipart) => multipart_to_raw(multipart),
                };
                Self::SaveSnapshot(oplog::SaveSnapshotResultParameters {
                    snapshot: oplog::SnapshotData {
                        data: snapshot_bytes,
                        mime_type,
                    },
                })
            }
            PublicAgentInvocationResult::ProcessOplogEntries(result) => {
                Self::ProcessOplogEntries(oplog::FallibleResultParameters {
                    error: result.error,
                })
            }
        }
    }
}

impl From<PluginInstallationDescription> for oplog::PluginInstallationDescription {
    fn from(value: PluginInstallationDescription) -> Self {
        Self {
            environment_plugin_grant_id: oplog::EnvironmentPluginGrantId {
                uuid: value.environment_plugin_grant_id.0.into(),
            },
            plugin_priority: value.plugin_priority.0,
            plugin_name: value.plugin_name,
            plugin_version: value.plugin_version,
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
            golem_common::model::oplog::payload::OplogPayload::SerializedInline {
                bytes,
                cached: None,
            }
        }
        oplog::OplogPayload::External(ext) => {
            golem_common::model::oplog::payload::OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId(uuid::Uuid::from_u64_pair(
                    ext.payload_id.high_bits,
                    ext.payload_id.low_bits,
                )),
                md5_hash: ext.md5_hash,
                cached: None,
            }
        }
    }
}

impl From<oplog::WorkerError> for golem_common::model::oplog::AgentError {
    fn from(err: oplog::WorkerError) -> Self {
        match err {
            oplog::WorkerError::Unknown(msg) => Self::Unknown(msg),
            oplog::WorkerError::InvalidRequest(msg) => Self::InvalidRequest(msg),
            oplog::WorkerError::StackOverflow => Self::StackOverflow,
            oplog::WorkerError::OutOfMemory => Self::OutOfMemory,
            oplog::WorkerError::ExceededMemoryLimit => Self::ExceededMemoryLimit,
            oplog::WorkerError::InternalError(msg) => Self::InternalError(msg),
            oplog::WorkerError::DeterministicTrap(msg) => Self::DeterministicTrap(msg),
            oplog::WorkerError::TransientError(msg) => Self::TransientError(msg),
            oplog::WorkerError::PermanentError(msg) => Self::PermanentError(msg),
            oplog::WorkerError::ExceededTableLimit => Self::ExceededTableLimit,
            oplog::WorkerError::ExceededHttpCallLimit => Self::ExceededHttpCallLimit,
            oplog::WorkerError::ExceededRpcCallLimit => Self::ExceededRpcCallLimit,
            oplog::WorkerError::NodeOutOfFilesystemStorage => Self::NodeOutOfFilesystemStorage,
            oplog::WorkerError::AgentExceededFilesystemStorageLimit => {
                Self::AgentExceededFilesystemStorageLimit
            }
            oplog::WorkerError::AgentTerminatedByQuota(inner) => {
                Self::AgentTerminatedByQuota(AgentTerminatedByQuotaError {
                    environment_id: EnvironmentId(inner.environment_id.uuid.into()),
                    resource_name: ResourceName(inner.resource_name),
                })
            }
            oplog::WorkerError::EphemeralSleepTooLong(inner) => {
                Self::EphemeralSleepTooLong(EphemeralSleepTooLongError {
                    requested_nanos: inner.requested_nanos,
                    max_nanos: inner.max_nanos,
                })
            }
            oplog::WorkerError::EphemeralFuelExhausted(inner) => {
                Self::EphemeralFuelExhausted(EphemeralFuelExhaustedError {
                    overdraft_limit: inner.overdraft_limit,
                })
            }
            oplog::WorkerError::EphemeralCannotSuspend(inner) => {
                Self::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                    reason: inner.reason,
                })
            }
            oplog::WorkerError::ReadOnlyViolation(inner) => {
                Self::ReadOnlyViolation(ReadOnlyViolationError {
                    method: inner.method,
                    host_function: inner.host_function,
                })
            }
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
            oplog::RawUpdateDescription::Automatic(target_revision) => Ok(Self::Automatic {
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    target_revision,
                )
                .map_err(|e| e.to_string())?,
            }),
            oplog::RawUpdateDescription::SnapshotBased(sbu) => Ok(Self::SnapshotBased {
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    sbu.target_revision,
                )
                .map_err(|e| e.to_string())?,
                payload: oplog_payload_from_wit(sbu.payload),
                mime_type: sbu.mime_type,
            }),
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
                agent_id: golem_common::model::AgentId::from(params.agent_id),
                agent_mode: match params.agent_mode {
                    oplog::AgentMode::Durable => golem_common::model::agent::AgentMode::Durable,
                    oplog::AgentMode::Ephemeral => golem_common::model::agent::AgentMode::Ephemeral,
                },
                component_revision: golem_common::model::component::ComponentRevision::try_from(
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
                parent: params.parent.map(golem_common::model::AgentId::from),
                component_size: params.component_size,
                initial_total_linear_memory_size: params.initial_total_linear_memory_size,
                initial_active_plugins: params
                    .initial_active_plugins
                    .into_iter()
                    .map(|v| golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(uuid::Uuid::from_u64_pair(v.uuid.high_bits, v.uuid.low_bits)))
                    .collect(),
                local_agent_config: params.local_agent_config.into_iter().map(|entry| {
                    Ok(golem_common::model::worker::UntypedAgentConfigEntry {
                        path: entry.path,
                        value: decode_untyped_schema_value(entry.value)?,
                    })
                }).collect::<Result<_, String>>()?,
                original_phantom_id: params
                    .original_phantom_id
                    .map(|uuid| uuid::Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits)),
                instance_id: uuid::Uuid::from_u64_pair(
                    params.instance_id.high_bits,
                    params.instance_id.low_bits,
                ),
            }),
            oplog::OplogEntry::Start(params) => Ok(Self::Start {
                timestamp: timestamp_from_datetime(params.timestamp),
                parent_start_index: params
                    .parent_start_index
                    .map(golem_common::base_model::OplogIndex::from_u64),
                function_name:
                    golem_common::model::oplog::payload::host_functions::HostFunctionName::from(
                        params.function_name.as_str(),
                    ),
                request: params.request.map(oplog_payload_from_wit),
                durable_function_type: params.durable_function_type.into(),
            }),
            oplog::OplogEntry::End(params) => Ok(Self::End {
                timestamp: timestamp_from_datetime(params.timestamp),
                start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
                response: params.response.map(oplog_payload_from_wit),
                forced_commit: params.forced_commit,
            }),
            oplog::OplogEntry::Cancelled(params) => Ok(Self::Cancelled {
                timestamp: timestamp_from_datetime(params.timestamp),
                start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
                partial: params.partial.map(oplog_payload_from_wit),
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
                    method_name: params.method_name,
                    consumed_fuel: params.consumed_fuel,
                    component_revision: params
                        .component_revision
                        .try_into()
                        .map_err(|e: String| e)?,
                })
            }
            oplog::OplogEntry::Suspend(ts) => Ok(Self::Suspend {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::Error(params) => Ok(Self::Error {
                timestamp: timestamp_from_datetime(params.timestamp),
                error: params.error.into(),
                retry_from: golem_common::model::OplogIndex::from_u64(params.retry_from),
                inside_atomic_region: params.inside_atomic_region,
                retry_policy_state: params.retry_policy_state.map(|s| {
                    let internal: golem_common::model::RetryPolicyState = s.into();
                    internal
                }),
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
            oplog::OplogEntry::BeginAtomicRegion(ts) => Ok(Self::BeginAtomicRegion {
                timestamp: timestamp_from_datetime(ts.timestamp),
            }),
            oplog::OplogEntry::EndAtomicRegion(params) => Ok(Self::EndAtomicRegion {
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
                    .map(|v| golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(uuid::Uuid::from_u64_pair(v.uuid.high_bits, v.uuid.low_bits)))
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
            oplog::OplogEntry::FilesystemStorageUsageUpdate(params) => Ok(Self::FilesystemStorageUsageUpdate {
                timestamp: timestamp_from_datetime(params.timestamp),
                delta: params.delta,
            }),
            oplog::OplogEntry::CreateResource(params) => Ok(Self::CreateResource {
                timestamp: timestamp_from_datetime(params.timestamp),
                id: golem_common::model::oplog::AgentResourceId(params.id),
                resource_type_id: golem_common::resource_runtime::ResourceTypeId {
                    name: params.resource_type_id.name,
                    owner: params.resource_type_id.owner,
                },
            }),
            oplog::OplogEntry::DropResource(params) => Ok(Self::DropResource {
                timestamp: timestamp_from_datetime(params.timestamp),
                id: golem_common::model::oplog::AgentResourceId(params.id),
                resource_type_id: golem_common::resource_runtime::ResourceTypeId {
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
                plugin_grant_id: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(
                    uuid::Uuid::from_u64_pair(params.plugin_grant_id.uuid.high_bits, params.plugin_grant_id.uuid.low_bits),
                ),
            }),
            oplog::OplogEntry::DeactivatePlugin(params) => Ok(Self::DeactivatePlugin {
                timestamp: timestamp_from_datetime(params.timestamp),
                plugin_grant_id: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(
                    uuid::Uuid::from_u64_pair(params.plugin_grant_id.uuid.high_bits, params.plugin_grant_id.uuid.low_bits),
                ),
            }),
            oplog::OplogEntry::Revert(params) => Ok(Self::Revert {
                timestamp: timestamp_from_datetime(params.timestamp),
                dropped_region: golem_common::model::regions::OplogRegion {
                    start: golem_common::model::OplogIndex::from_u64(params.dropped_region.start),
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
                    golem_common::model::invocation_context::SpanId::from_string(&params.span_id)?;
                let parent = params
                    .parent
                    .map(|p| golem_common::model::invocation_context::SpanId::from_string(&p))
                    .transpose()?;
                let linked_context_id = params
                    .linked_context_id
                    .map(|p| golem_common::model::invocation_context::SpanId::from_string(&p))
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
                    golem_common::model::invocation_context::SpanId::from_string(&params.span_id)?;
                Ok(Self::FinishSpan {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    span_id,
                })
            }
            oplog::OplogEntry::SetSpanAttribute(params) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(&params.span_id)?;
                let value = params.value.into();
                Ok(Self::SetSpanAttribute {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    span_id,
                    key: params.key,
                    value,
                })
            }
            oplog::OplogEntry::ChangePersistenceLevel(params) => Ok(Self::ChangePersistenceLevel {
                timestamp: timestamp_from_datetime(params.timestamp),
                persistence_level: params.persistence_level.into(),
            }),
            oplog::OplogEntry::BeginRemoteTransaction(params) => Ok(Self::BeginRemoteTransaction {
                timestamp: timestamp_from_datetime(params.timestamp),
                transaction_id: golem_common::model::TransactionId::from(params.transaction_id),
                original_begin_index: params
                    .original_begin_index
                    .map(golem_common::model::OplogIndex::from_u64),
            }),
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
                active_cards: Vec::new(),
            }),
            oplog::OplogEntry::OplogProcessorCheckpoint(params) => {
                Ok(Self::OplogProcessorCheckpoint {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    plugin_grant_id: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(
                        uuid::Uuid::from_u64_pair(params.plugin_grant_id.uuid.high_bits, params.plugin_grant_id.uuid.low_bits),
                    ),
                    target_agent_id: golem_common::model::AgentId::from(params.target_agent_id),
                    confirmed_up_to: golem_common::model::OplogIndex::from_u64(
                        params.confirmed_up_to,
                    ),
                    sending_up_to: golem_common::model::OplogIndex::from_u64(
                        params.sending_up_to,
                    ),
                    last_batch_start: golem_common::model::OplogIndex::from_u64(
                        params.last_batch_start,
                    ),
                })
            }
            oplog::OplogEntry::SetRetryPolicy(params) => {
                let named: golem_common::model::retry_policy::NamedRetryPolicy =
                    params.policy.into();
                Ok(Self::SetRetryPolicy {
                    timestamp: timestamp_from_datetime(params.timestamp),
                    policy: named,
                })
            }
            oplog::OplogEntry::RemoveRetryPolicy(params) => Ok(Self::RemoveRetryPolicy {
                timestamp: timestamp_from_datetime(params.timestamp),
                name: params.name,
            }),
            oplog::OplogEntry::CardRevoked(params) => Ok(Self::CardRevoked {
                timestamp: timestamp_from_datetime(params.timestamp),
                queued_event_index: golem_common::model::OplogIndex::from_u64(
                    params.queued_event_index,
                ),
                card_id: card_id_from_wit(params.card_id),
            }),
            oplog::OplogEntry::CardEventQueued(_params) => Err(
                "Converting CardEventQueued from public WIT to raw oplog entry is not supported"
                    .to_string(),
            ),
            oplog::OplogEntry::CardInstalled(_params) => Err(
                "Converting CardInstalled from public WIT to raw oplog entry is not supported"
                    .to_string(),
            ),
            oplog::OplogEntry::CardInstallFailed(params) => Ok(Self::CardInstallFailed {
                timestamp: timestamp_from_datetime(params.timestamp),
                queued_event_index: golem_common::model::OplogIndex::from_u64(
                    params.queued_event_index,
                ),
                card_id: card_id_from_wit(params.card_id),
                reason: card_install_failure_from_wit(params.reason),
            }),
        }
    }
}

fn multipart_to_raw(multipart: MultipartSnapshotData) -> (Vec<u8>, String) {
    use golem_common::base_model::oplog::multipart::extract_boundary;

    let boundary = extract_boundary(&multipart.mime_type)
        .unwrap_or("boundary")
        .to_string();

    let mut output = Vec::new();
    for part in &multipart.parts {
        output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        output.extend_from_slice(format!("Content-Type: {}\r\n", part.content_type).as_bytes());
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

impl From<golem_common::model::RetryPolicyState> for oplog::RetryPolicyState {
    fn from(state: golem_common::model::RetryPolicyState) -> Self {
        let mut nodes = Vec::new();
        push_wit_state_node(state, &mut nodes);
        Self { nodes }
    }
}

impl From<oplog::RetryPolicyState> for golem_common::model::RetryPolicyState {
    fn from(wit: oplog::RetryPolicyState) -> Self {
        build_wit_state_from_index(&wit.nodes, 0).unwrap_or(Self::Terminal)
    }
}

fn push_wit_state_node(
    state: golem_common::model::RetryPolicyState,
    nodes: &mut Vec<oplog::StateNode>,
) -> i32 {
    use golem_common::model::RetryPolicyState;
    let index = nodes.len() as i32;
    nodes.push(oplog::StateNode::Terminal);

    let node = match state {
        RetryPolicyState::Counter(n) => oplog::StateNode::Counter(n),
        RetryPolicyState::Terminal => oplog::StateNode::Terminal,
        RetryPolicyState::Wrapper(inner) => {
            let inner_idx = push_wit_state_node(*inner, nodes);
            oplog::StateNode::Wrapper(inner_idx)
        }
        RetryPolicyState::CountBox { attempts, inner } => {
            let inner_idx = push_wit_state_node(*inner, nodes);
            oplog::StateNode::CountBox(oplog::CountBoxState {
                attempts,
                inner: inner_idx,
            })
        }
        RetryPolicyState::AndThen {
            left,
            right,
            on_right,
        } => {
            let left_idx = push_wit_state_node(*left, nodes);
            let right_idx = push_wit_state_node(*right, nodes);
            oplog::StateNode::AndThen(oplog::AndThenState {
                left: left_idx,
                right: right_idx,
                on_right,
            })
        }
        RetryPolicyState::Pair(left, right) => {
            let left_idx = push_wit_state_node(*left, nodes);
            let right_idx = push_wit_state_node(*right, nodes);
            oplog::StateNode::Pair(oplog::PairState {
                left: left_idx,
                right: right_idx,
            })
        }
    };

    nodes[index as usize] = node;
    index
}

fn build_wit_state_from_index(
    nodes: &[oplog::StateNode],
    index: i32,
) -> Option<golem_common::model::RetryPolicyState> {
    use golem_common::model::RetryPolicyState;
    if index < 0 || (index as usize) >= nodes.len() {
        return None;
    }

    match &nodes[index as usize] {
        oplog::StateNode::Counter(n) => Some(RetryPolicyState::Counter(*n)),
        oplog::StateNode::Terminal => Some(RetryPolicyState::Terminal),
        oplog::StateNode::Wrapper(inner) => Some(RetryPolicyState::Wrapper(Box::new(
            build_wit_state_from_index(nodes, *inner)?,
        ))),
        oplog::StateNode::CountBox(oplog::CountBoxState { attempts, inner }) => {
            Some(RetryPolicyState::CountBox {
                attempts: *attempts,
                inner: Box::new(build_wit_state_from_index(nodes, *inner)?),
            })
        }
        oplog::StateNode::AndThen(oplog::AndThenState {
            left,
            right,
            on_right,
        }) => Some(RetryPolicyState::AndThen {
            left: Box::new(build_wit_state_from_index(nodes, *left)?),
            right: Box::new(build_wit_state_from_index(nodes, *right)?),
            on_right: *on_right,
        }),
        oplog::StateNode::Pair(oplog::PairState { left, right }) => Some(RetryPolicyState::Pair(
            Box::new(build_wit_state_from_index(nodes, *left)?),
            Box::new(build_wit_state_from_index(nodes, *right)?),
        )),
    }
}

// ============================================================================
// Forward conversions: golem_common model -> bindgen WIT (oplog) types.
// These mirror the reverse conversions above, field-for-field inverted.
// ============================================================================

/// Forward of `oplog_payload_from_wit`. Serializes inline payloads to bytes
/// (fallible) and maps to the WIT `oplog-payload` variant.
fn oplog_payload_to_wit<T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq>(
    payload: golem_common::model::oplog::payload::OplogPayload<T>,
) -> Result<oplog::OplogPayload, String> {
    match payload.try_into_raw()? {
        golem_common::model::oplog::payload::RawOplogPayload::SerializedInline(bytes) => {
            Ok(oplog::OplogPayload::Inline(bytes))
        }
        golem_common::model::oplog::payload::RawOplogPayload::External {
            payload_id,
            md5_hash,
        } => Ok(oplog::OplogPayload::External(oplog::OplogExternalPayload {
            payload_id: payload_id.0.into(),
            md5_hash,
        })),
    }
}

impl From<golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId>
    for oplog::EnvironmentPluginGrantId
{
    fn from(
        value: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
    ) -> Self {
        Self {
            uuid: value.0.into(),
        }
    }
}

impl From<golem_common::model::oplog::AgentError> for oplog::WorkerError {
    fn from(err: golem_common::model::oplog::AgentError) -> Self {
        use golem_common::model::oplog::AgentError;
        match err {
            AgentError::Unknown(msg) => Self::Unknown(msg),
            AgentError::InvalidRequest(msg) => Self::InvalidRequest(msg),
            AgentError::StackOverflow => Self::StackOverflow,
            AgentError::OutOfMemory => Self::OutOfMemory,
            AgentError::ExceededMemoryLimit => Self::ExceededMemoryLimit,
            AgentError::InternalError(msg) => Self::InternalError(msg),
            AgentError::DeterministicTrap(msg) => Self::DeterministicTrap(msg),
            AgentError::TransientError(msg) => Self::TransientError(msg),
            AgentError::PermanentError(msg) => Self::PermanentError(msg),
            AgentError::ExceededTableLimit => Self::ExceededTableLimit,
            AgentError::ExceededHttpCallLimit => Self::ExceededHttpCallLimit,
            AgentError::ExceededRpcCallLimit => Self::ExceededRpcCallLimit,
            AgentError::NodeOutOfFilesystemStorage => Self::NodeOutOfFilesystemStorage,
            AgentError::AgentExceededFilesystemStorageLimit => {
                Self::AgentExceededFilesystemStorageLimit
            }
            AgentError::AgentTerminatedByQuota(inner) => {
                Self::AgentTerminatedByQuota(oplog::AgentTerminatedByQuotaError {
                    environment_id: inner.environment_id.into(),
                    resource_name: inner.resource_name.0,
                })
            }
            AgentError::EphemeralSleepTooLong(inner) => {
                Self::EphemeralSleepTooLong(oplog::EphemeralSleepTooLong {
                    requested_nanos: inner.requested_nanos,
                    max_nanos: inner.max_nanos,
                })
            }
            AgentError::EphemeralFuelExhausted(inner) => {
                Self::EphemeralFuelExhausted(oplog::EphemeralFuelExhausted {
                    overdraft_limit: inner.overdraft_limit,
                })
            }
            AgentError::EphemeralCannotSuspend(inner) => {
                Self::EphemeralCannotSuspend(oplog::EphemeralCannotSuspend {
                    reason: inner.reason,
                })
            }
            AgentError::ReadOnlyViolation(inner) => {
                Self::ReadOnlyViolation(oplog::ReadOnlyViolation {
                    method: inner.method,
                    host_function: inner.host_function,
                })
            }
        }
    }
}

// Note: From<DurableFunctionType> for oplog::WrappedFunctionType is already provided in
// durable_host/durability.rs (as `From<DurableFunctionType> for durability::DurableFunctionType`,
// where `durability::DurableFunctionType` is the same bindgen type as `oplog::WrappedFunctionType`).

impl From<golem_common::model::oplog::SpanData> for oplog::SpanData {
    fn from(span: golem_common::model::oplog::SpanData) -> Self {
        match span {
            golem_common::model::oplog::SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                // The raw model carries `Option<Vec<SpanData>>`, but the WIT
                // `local-span-data.linked-context` is `option<u64>` (an index into the
                // agent-invocation's invocation-context list). There is no value to map
                // into at this conversion boundary, so it is dropped here exactly as the
                // reverse `TryFrom<oplog::SpanData> for SpanData` does (it sets `None`).
                linked_context: _,
                attributes,
                inherited,
            } => Self::LocalSpan(oplog::LocalSpanData {
                span_id: span_id.to_string(),
                start: start.into(),
                parent: parent_id.map(|id| id.to_string()),
                linked_context: None,
                attributes: attributes
                    .into_iter()
                    .map(|(key, value)| oplog::Attribute {
                        key,
                        value: value.into(),
                    })
                    .collect(),
                inherited,
            }),
            golem_common::model::oplog::SpanData::ExternalSpan { span_id } => {
                Self::ExternalSpan(oplog::ExternalSpanData {
                    span_id: span_id.to_string(),
                })
            }
        }
    }
}

impl TryFrom<golem_common::model::oplog::UpdateDescription> for oplog::RawUpdateDescription {
    type Error = String;

    fn try_from(desc: golem_common::model::oplog::UpdateDescription) -> Result<Self, String> {
        use golem_common::model::oplog::UpdateDescription;
        match desc {
            UpdateDescription::Automatic { target_revision } => {
                Ok(Self::Automatic(target_revision.into()))
            }
            UpdateDescription::SnapshotBased {
                target_revision,
                payload,
                mime_type,
            } => Ok(Self::SnapshotBased(oplog::RawSnapshotBasedUpdate {
                target_revision: target_revision.into(),
                payload: oplog_payload_to_wit(payload)?,
                mime_type,
            })),
        }
    }
}

impl From<RawSnapshotData> for crate::preview2::golem_api_1_x::host::Snapshot {
    fn from(value: RawSnapshotData) -> Self {
        Self {
            payload: value.data,
            mime_type: value.mime_type,
        }
    }
}

impl From<crate::preview2::golem_api_1_x::host::Snapshot> for RawSnapshotData {
    fn from(value: crate::preview2::golem_api_1_x::host::Snapshot) -> Self {
        Self {
            data: value.payload,
            mime_type: value.mime_type,
        }
    }
}

impl TryFrom<golem_common::model::oplog::OplogEntry> for oplog::OplogEntry {
    type Error = String;

    fn try_from(value: golem_common::model::oplog::OplogEntry) -> Result<Self, String> {
        use golem_common::model::oplog::OplogEntry as M;
        match value {
            M::Create {
                timestamp,
                agent_id,
                agent_mode,
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
                instance_id,
            } => Ok(Self::Create(oplog::RawCreateParameters {
                timestamp: timestamp.into(),
                agent_id: agent_id.into(),
                agent_mode: match agent_mode {
                    golem_common::model::agent::AgentMode::Durable => oplog::AgentMode::Durable,
                    golem_common::model::agent::AgentMode::Ephemeral => oplog::AgentMode::Ephemeral,
                },
                component_revision: component_revision.into(),
                env,
                environment_id: environment_id.into(),
                created_by: created_by.into(),
                parent: parent.map(|id| id.into()),
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins: initial_active_plugins
                    .into_iter()
                    .map(|g| g.into())
                    .collect(),
                local_agent_config: local_agent_config
                    .into_iter()
                    .map(|entry| {
                        encode_untyped_schema_value(entry.value).map(|value| {
                            oplog::RawLocalAgentConfigEntry {
                                path: entry.path,
                                value,
                            }
                        })
                    })
                    .collect::<Result<_, String>>()?,
                original_phantom_id: original_phantom_id.map(|id| id.into()),
                instance_id: instance_id.into(),
            })),
            M::Start {
                timestamp,
                parent_start_index,
                function_name,
                request,
                durable_function_type,
            } => Ok(Self::Start(oplog::RawStartParameters {
                timestamp: timestamp.into(),
                parent_start_index: parent_start_index.map(|i| i.into()),
                function_name: function_name.to_string(),
                request: request.map(oplog_payload_to_wit).transpose()?,
                durable_function_type: durable_function_type.into(),
            })),
            M::End {
                timestamp,
                start_index,
                response,
                forced_commit,
            } => Ok(Self::End(oplog::RawEndParameters {
                timestamp: timestamp.into(),
                start_index: start_index.into(),
                response: response.map(oplog_payload_to_wit).transpose()?,
                forced_commit,
            })),
            M::Cancelled {
                timestamp,
                start_index,
                partial,
            } => Ok(Self::Cancelled(oplog::RawCancelledParameters {
                timestamp: timestamp.into(),
                start_index: start_index.into(),
                partial: partial.map(oplog_payload_to_wit).transpose()?,
            })),
            M::AgentInvocationStarted {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => Ok(Self::AgentInvocationStarted(
                oplog::RawAgentInvocationStartedParameters {
                    timestamp: timestamp.into(),
                    idempotency_key: idempotency_key.value,
                    payload: oplog_payload_to_wit(payload)?,
                    trace_id: trace_id.to_string(),
                    trace_states,
                    invocation_context: invocation_context.into_iter().map(|s| s.into()).collect(),
                },
            )),
            M::AgentInvocationFinished {
                timestamp,
                result,
                method_name,
                consumed_fuel,
                component_revision,
            } => Ok(Self::AgentInvocationFinished(
                oplog::RawAgentInvocationFinishedParameters {
                    timestamp: timestamp.into(),
                    result: oplog_payload_to_wit(result)?,
                    method_name,
                    consumed_fuel,
                    component_revision: component_revision.get(),
                },
            )),
            M::Suspend { timestamp } => Ok(Self::Suspend(timestamp.into())),
            M::Error {
                timestamp,
                error,
                retry_from,
                inside_atomic_region,
                retry_policy_state,
            } => Ok(Self::Error(oplog::RawErrorParameters {
                timestamp: timestamp.into(),
                error: error.into(),
                retry_from: retry_from.into(),
                inside_atomic_region,
                retry_policy_state: retry_policy_state.map(|s| s.into()),
            })),
            M::NoOp { timestamp } => Ok(Self::NoOp(timestamp.into())),
            M::Jump { timestamp, jump } => Ok(Self::Jump(oplog::JumpParameters {
                timestamp: timestamp.into(),
                jump: oplog::OplogRegion {
                    start: jump.start.into(),
                    end: jump.end.into(),
                },
            })),
            M::Interrupted { timestamp } => Ok(Self::Interrupted(timestamp.into())),
            M::Exited { timestamp } => Ok(Self::Exited(timestamp.into())),
            M::BeginAtomicRegion { timestamp } => Ok(Self::BeginAtomicRegion(timestamp.into())),
            M::EndAtomicRegion {
                timestamp,
                begin_index,
            } => Ok(Self::EndAtomicRegion(oplog::EndAtomicRegionParameters {
                timestamp: timestamp.into(),
                begin_index: begin_index.into(),
            })),
            M::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => Ok(Self::PendingAgentInvocation(
                oplog::RawPendingAgentInvocationParameters {
                    timestamp: timestamp.into(),
                    idempotency_key: idempotency_key.value,
                    payload: oplog_payload_to_wit(payload)?,
                    trace_id: trace_id.to_string(),
                    trace_states,
                    invocation_context: invocation_context.into_iter().map(|s| s.into()).collect(),
                },
            )),
            M::PendingUpdate {
                timestamp,
                description,
            } => Ok(Self::PendingUpdate(oplog::RawPendingUpdateParameters {
                timestamp: timestamp.into(),
                description: description.try_into()?,
            })),
            M::SuccessfulUpdate {
                timestamp,
                target_revision,
                new_component_size,
                new_active_plugins,
            } => Ok(Self::SuccessfulUpdate(
                oplog::RawSuccessfulUpdateParameters {
                    timestamp: timestamp.into(),
                    target_revision: target_revision.into(),
                    new_component_size,
                    new_active_plugins: new_active_plugins.into_iter().map(|g| g.into()).collect(),
                },
            )),
            M::FailedUpdate {
                timestamp,
                target_revision,
                details,
            } => Ok(Self::FailedUpdate(oplog::FailedUpdateParameters {
                timestamp: timestamp.into(),
                target_revision: target_revision.into(),
                details,
            })),
            M::GrowMemory { timestamp, delta } => {
                Ok(Self::GrowMemory(oplog::GrowMemoryParameters {
                    timestamp: timestamp.into(),
                    delta,
                }))
            }
            M::FilesystemStorageUsageUpdate { timestamp, delta } => Ok(
                Self::FilesystemStorageUsageUpdate(oplog::FilesystemStorageUsageUpdateParameters {
                    timestamp: timestamp.into(),
                    delta,
                }),
            ),
            M::CardRevoked {
                timestamp,
                queued_event_index,
                card_id,
            } => Ok(Self::CardRevoked(oplog::CardRevokedParameters {
                timestamp: timestamp.into(),
                queued_event_index: queued_event_index.into(),
                card_id: card_id_to_wit(card_id),
            })),
            M::CardEventQueued { timestamp, event } => {
                Ok(Self::CardEventQueued(oplog::RawCardEventQueuedParameters {
                    timestamp: timestamp.into(),
                    event: raw_queued_card_event_to_wit(event),
                }))
            }
            M::CardInstalled {
                timestamp,
                queued_event_index,
                card,
            } => Ok(Self::CardInstalled(oplog::RawCardInstalledParameters {
                timestamp: timestamp.into(),
                queued_event_index: queued_event_index.map(Into::into),
                card: serde_json::to_vec(&card).map_err(|err| err.to_string())?,
            })),
            M::CardInstallFailed {
                timestamp,
                queued_event_index,
                card_id,
                reason,
            } => Ok(Self::CardInstallFailed(
                oplog::CardInstallFailedParameters {
                    timestamp: timestamp.into(),
                    queued_event_index: queued_event_index.into(),
                    card_id: card_id_to_wit(card_id),
                    reason: card_install_failure_to_wit(reason),
                },
            )),
            M::CreateResource {
                timestamp,
                id,
                resource_type_id,
            } => Ok(Self::CreateResource(oplog::RawCreateResourceParameters {
                timestamp: timestamp.into(),
                id: id.0,
                resource_type_id: oplog::ResourceTypeId {
                    name: resource_type_id.name,
                    owner: resource_type_id.owner,
                },
            })),
            M::DropResource {
                timestamp,
                id,
                resource_type_id,
            } => Ok(Self::DropResource(oplog::RawDropResourceParameters {
                timestamp: timestamp.into(),
                id: id.0,
                resource_type_id: oplog::ResourceTypeId {
                    name: resource_type_id.name,
                    owner: resource_type_id.owner,
                },
            })),
            M::Log {
                timestamp,
                level,
                context,
                message,
            } => Ok(Self::Log(oplog::LogParameters {
                timestamp: timestamp.into(),
                level: level.into(),
                context,
                message,
            })),
            M::Restart { timestamp } => Ok(Self::Restart(timestamp.into())),
            M::ActivatePlugin {
                timestamp,
                plugin_grant_id,
            } => Ok(Self::ActivatePlugin(oplog::RawActivatePluginParameters {
                timestamp: timestamp.into(),
                plugin_grant_id: plugin_grant_id.into(),
            })),
            M::DeactivatePlugin {
                timestamp,
                plugin_grant_id,
            } => Ok(Self::DeactivatePlugin(
                oplog::RawDeactivatePluginParameters {
                    timestamp: timestamp.into(),
                    plugin_grant_id: plugin_grant_id.into(),
                },
            )),
            M::Revert {
                timestamp,
                dropped_region,
            } => Ok(Self::Revert(oplog::RevertParameters {
                timestamp: timestamp.into(),
                dropped_region: oplog::OplogRegion {
                    start: dropped_region.start.into(),
                    end: dropped_region.end.into(),
                },
            })),
            M::CancelPendingInvocation {
                timestamp,
                idempotency_key,
            } => Ok(Self::CancelPendingInvocation(
                oplog::CancelPendingInvocationParameters {
                    timestamp: timestamp.into(),
                    idempotency_key: idempotency_key.value,
                },
            )),
            M::StartSpan {
                timestamp,
                span_id,
                parent,
                linked_context_id,
                attributes,
            } => Ok(Self::StartSpan(oplog::StartSpanParameters {
                timestamp: timestamp.into(),
                span_id: span_id.to_string(),
                parent: parent.map(|id| id.to_string()),
                linked_context_id: linked_context_id.map(|id| id.to_string()),
                attributes: attributes
                    .0
                    .into_iter()
                    .map(|(key, value)| oplog::Attribute {
                        key,
                        value: value.into(),
                    })
                    .collect(),
            })),
            M::FinishSpan { timestamp, span_id } => {
                Ok(Self::FinishSpan(oplog::FinishSpanParameters {
                    timestamp: timestamp.into(),
                    span_id: span_id.to_string(),
                }))
            }
            M::SetSpanAttribute {
                timestamp,
                span_id,
                key,
                value,
            } => Ok(Self::SetSpanAttribute(oplog::SetSpanAttributeParameters {
                timestamp: timestamp.into(),
                span_id: span_id.to_string(),
                key,
                value: value.into(),
            })),
            M::ChangePersistenceLevel {
                timestamp,
                persistence_level,
            } => Ok(Self::ChangePersistenceLevel(
                oplog::ChangePersistenceLevelParameters {
                    timestamp: timestamp.into(),
                    persistence_level: persistence_level.into(),
                },
            )),
            M::BeginRemoteTransaction {
                timestamp,
                transaction_id,
                original_begin_index,
            } => Ok(Self::BeginRemoteTransaction(
                oplog::RawBeginRemoteTransactionParameters {
                    timestamp: timestamp.into(),
                    transaction_id: transaction_id.to_string(),
                    original_begin_index: original_begin_index.map(|idx| idx.into()),
                },
            )),
            M::PreCommitRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(Self::PreCommitRemoteTransaction(
                oplog::RemoteTransactionParameters {
                    timestamp: timestamp.into(),
                    begin_index: begin_index.into(),
                },
            )),
            M::PreRollbackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(Self::PreRollbackRemoteTransaction(
                oplog::RemoteTransactionParameters {
                    timestamp: timestamp.into(),
                    begin_index: begin_index.into(),
                },
            )),
            M::CommittedRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(Self::CommittedRemoteTransaction(
                oplog::RemoteTransactionParameters {
                    timestamp: timestamp.into(),
                    begin_index: begin_index.into(),
                },
            )),
            M::RolledBackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(Self::RolledBackRemoteTransaction(
                oplog::RemoteTransactionParameters {
                    timestamp: timestamp.into(),
                    begin_index: begin_index.into(),
                },
            )),
            M::Snapshot {
                timestamp,
                data,
                mime_type,
                ..
            } => Ok(Self::Snapshot(oplog::RawSnapshotParameters {
                timestamp: timestamp.into(),
                data: oplog_payload_to_wit(data)?,
                mime_type,
            })),
            M::OplogProcessorCheckpoint {
                timestamp,
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
            } => Ok(Self::OplogProcessorCheckpoint(
                oplog::RawOplogProcessorCheckpointParameters {
                    timestamp: timestamp.into(),
                    plugin_grant_id: plugin_grant_id.into(),
                    target_agent_id: target_agent_id.into(),
                    confirmed_up_to: confirmed_up_to.into(),
                    sending_up_to: sending_up_to.into(),
                    last_batch_start: last_batch_start.into(),
                },
            )),
            M::SetRetryPolicy { timestamp, policy } => {
                Ok(Self::SetRetryPolicy(oplog::SetRetryPolicyParameters {
                    timestamp: timestamp.into(),
                    policy: policy.into(),
                }))
            }
            M::RemoveRetryPolicy { timestamp, name } => Ok(Self::RemoveRetryPolicy(
                oplog::RemoveRetryPolicyParameters {
                    timestamp: timestamp.into(),
                    name,
                },
            )),
        }
    }
}
