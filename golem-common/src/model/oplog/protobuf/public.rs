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

use std::collections::BTreeSet;

use golem_api_grpc::proto::golem::worker::{oplog_entry, wrapped_function_type};

use super::cards::{
    card_holder_from_proto, card_holder_to_proto, card_install_failure_from_proto,
    card_install_failure_to_proto, public_invocation_wallet_pin_from_proto,
    public_invocation_wallet_pin_to_proto, public_queued_card_event_from_proto,
    public_queued_card_event_to_proto,
};
use super::invocations::public_agent_invocation_result_to_raw;
use super::spans::{
    public_span_attributes_from_proto, public_span_attributes_to_proto,
    public_span_attributes_to_raw, public_span_finished_from_proto, public_span_finished_to_proto,
    public_span_finished_to_raw, public_span_started_from_proto, public_span_started_to_proto,
    public_span_started_to_raw,
};
use super::{
    decode_attachment_control, decode_attachment_observation, host_stream_kind_from_proto,
    host_stream_kind_to_proto, oplog_error_kind_from_proto, oplog_error_kind_to_proto,
};
use crate::base_model::OplogIndex;
use crate::base_model::durable_stream::{
    StreamCancelRecord, StreamEndRecord, StreamItemsRecord, StreamRegisteredRecord,
    StreamSessionRecord,
};
use crate::model::Empty;
use crate::model::card::CardId;
use crate::model::oplog::payload::OplogPayload;
use crate::model::oplog::payload::host_functions::{
    HostFunctionName, host_request_from_typed_schema_value,
};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginRemoteTransactionParams, CancelPendingInvocationParams, CancelledParams,
    CardDerivedParams, CardEventQueuedParams, CardExpiredParams, CardInstallFailedParams,
    CardInstalledParams, CardRevokedCascadeParams, CardRevokedParams, CardTransferConfirmedParams,
    CardTransferStartedParams, CardTransferredParams, CommittedRemoteTransactionParams,
    CompletionDeliveredParams, CompletionDiscardedParams, CreateParams, CreateResourceParams,
    DeactivatePluginParams, DropResourceParams, EndParams, ErrorParams, ExitedParams,
    FailedUpdateParams, GrowMemoryParams, HostStreamFrameParams, InterruptedParams, JumpParams,
    LogParams, OplogProcessorCheckpointParams, PendingAgentInvocationParams, PendingUpdateParams,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, RecoverySucceededParams,
    RemoveRetryPolicyParams, RestartParams, ResumedParams, RevertParams,
    RolledBackRemoteTransactionParams, SetRetryPolicyParams, SnapshotConfirmedParams,
    SnapshotParams, StartParams, StreamCancelParams, StreamEndParams, StreamItemsParams,
    StreamRegisteredParams, StreamSessionParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::raw_types::{CreateParameters, DurableStreamEventSummary};
use crate::model::oplog::{
    AgentError, AgentResourceId, DurableFunctionType, FailedSnapshotAssistedUpdateDetails,
    JsonSnapshotData, MultipartPartData, MultipartSnapshotData, MultipartSnapshotPart,
    OmittedOplogEntry, OplogEntry, PublicDurableFunctionType,
    PublicFailedSnapshotAssistedUpdateDetails, PublicOplogEntry, PublicOplogEntryAttribution,
    PublicOplogEntryWithIndex, PublicOplogPageEntry, PublicRetryPolicyState,
    PublicSnapshotAssistedUpdateDetails, PublicSnapshotData, PublicTypedAgentConfigEntry,
    PublicUpdateDescription, RawSnapshotData, SnapshotAssistedAutomaticUpdateParameters,
    SnapshotAssistedUpdateDetails, SnapshotBasedUpdateParameters,
};
use crate::model::regions::OplogRegion;
use crate::resource_runtime::ResourceTypeId;

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
                owner_kind: crate::model::agent::OwnerKind::try_from(create.owner_kind)?,
                agent_mode: golem_api_grpc::proto::golem::component::AgentMode::try_from(
                    create.agent_mode,
                )
                .map_err(|e| format!("Invalid agent_mode: {e}"))?
                .into(),
                component_revision: create.component_revision.try_into()?,
                env: create.env.into_iter().collect(),
                local_agent_config: create
                    .config
                    .into_iter()
                    .map(PublicTypedAgentConfigEntry::try_from)
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
                instance_id: create
                    .instance_id
                    .map(|id| id.into())
                    .ok_or("Missing instance_id in Create entry")?,
            })),
            oplog_entry::Entry::Start(start) => Ok(PublicOplogEntry::Start(StartParams {
                timestamp: start.timestamp.ok_or("Missing timestamp field")?.into(),
                parent_start_index: start
                    .parent_start_index
                    .map(crate::base_model::OplogIndex::from_u64),
                function_name: start.function_name,
                invocation_id: start.invocation_id.map(Into::into),
                observational_owner: start
                    .observational_owner
                    .map(crate::base_model::OplogIndex::from_u64),
                request: start.request.map(TryInto::try_into).transpose()?,
                durable_function_type: start
                    .durable_function_type
                    .ok_or("Missing durable_function_type field")?
                    .try_into()?,
                span_started: start
                    .span_started
                    .map(public_span_started_from_proto)
                    .transpose()?,
                denial: start.denial,
            })),
            oplog_entry::Entry::End(end) => Ok(PublicOplogEntry::End(EndParams {
                timestamp: end.timestamp.ok_or("Missing timestamp field")?.into(),
                start_index: crate::base_model::OplogIndex::from_u64(end.start_index),
                response: end.response.map(TryInto::try_into).transpose()?,
                forced_commit: end.forced_commit,
                span_finished: end
                    .span_finished
                    .map(public_span_finished_from_proto)
                    .transpose()?,
                span_attributes: end
                    .span_attributes
                    .map(public_span_attributes_from_proto)
                    .transpose()?,
            })),
            oplog_entry::Entry::Cancelled(cancelled) => {
                Ok(PublicOplogEntry::Cancelled(CancelledParams {
                    timestamp: cancelled.timestamp.ok_or("Missing timestamp field")?.into(),
                    start_index: crate::base_model::OplogIndex::from_u64(cancelled.start_index),
                    partial: cancelled.partial.map(TryInto::try_into).transpose()?,
                    span_finished: cancelled
                        .span_finished
                        .map(public_span_finished_from_proto)
                        .transpose()?,
                }))
            }
            oplog_entry::Entry::CompletionDiscarded(completion_discarded) => Ok(
                PublicOplogEntry::CompletionDiscarded(CompletionDiscardedParams {
                    timestamp: completion_discarded
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    start_index: crate::base_model::OplogIndex::from_u64(
                        completion_discarded.start_index,
                    ),
                }),
            ),
            oplog_entry::Entry::CompletionDelivered(completion_delivered) => Ok(
                PublicOplogEntry::CompletionDelivered(CompletionDeliveredParams {
                    timestamp: completion_delivered
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    start_index: crate::base_model::OplogIndex::from_u64(
                        completion_delivered.start_index,
                    ),
                }),
            ),
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
                    wallet_pin: public_invocation_wallet_pin_from_proto(
                        agent_invocation_started
                            .wallet_pin
                            .ok_or("Missing wallet_pin field")?,
                    )?,
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
                    method_name: agent_invocation_finished.method_name,
                    consumed_fuel: agent_invocation_finished.consumed_fuel,
                    component_revision: agent_invocation_finished.component_revision.try_into()?,
                }),
            ),
            oplog_entry::Entry::Suspend(suspend) => Ok(PublicOplogEntry::Suspend(SuspendParams {
                timestamp: suspend.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Error(error) => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp: error.timestamp.ok_or("Missing timestamp field")?.into(),
                kind: oplog_error_kind_from_proto(error.kind)?,
                error: error.error,
                retry_from: OplogIndex::from_u64(error.retry_from),
                retry_policy_state: error
                    .retry_policy_state
                    .map(TryInto::try_into)
                    .transpose()?,
            })),
            oplog_entry::Entry::RecoverySucceeded(recovery_succeeded) => Ok(
                PublicOplogEntry::RecoverySucceeded(RecoverySucceededParams {
                    timestamp: recovery_succeeded
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }),
            ),
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
                    update_attempt_index: OplogIndex::from_u64(pending_update.update_attempt_index),
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
                    snapshot_assisted_details: successful_update
                        .snapshot_assisted_details
                        .map(|details| {
                            Ok::<_, String>(PublicSnapshotAssistedUpdateDetails {
                                pending_update_index: OplogIndex::from_u64(
                                    details.pending_update_index,
                                ),
                                source_component_revision: details
                                    .source_component_revision
                                    .try_into()?,
                                source_revision_start_index: OplogIndex::from_u64(
                                    details.source_revision_start_index,
                                ),
                                snapshot_index: OplogIndex::from_u64(details.snapshot_index),
                            })
                        })
                        .transpose()?,
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
                    update_attempt_index: failed_update
                        .update_attempt_index
                        .map(OplogIndex::from_u64),
                    snapshot_assisted_details: failed_update
                        .snapshot_assisted_details
                        .map(|details| {
                            Ok::<_, String>(PublicFailedSnapshotAssistedUpdateDetails {
                                pending_update_index: OplogIndex::from_u64(
                                    details.pending_update_index,
                                ),
                                source_component_revision: details
                                    .source_component_revision
                                    .try_into()?,
                                source_revision_start_index: OplogIndex::from_u64(
                                    details.source_revision_start_index,
                                ),
                                snapshot_index: OplogIndex::from_u64(details.snapshot_index),
                            })
                        })
                        .transpose()?,
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
                trace_context: log.trace_context.map(TryInto::try_into).transpose()?,
            })),
            oplog_entry::Entry::Restart(restart) => Ok(PublicOplogEntry::Restart(RestartParams {
                timestamp: restart.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Resumed(resumed) => Ok(PublicOplogEntry::Resumed(ResumedParams {
                timestamp: resumed.timestamp.ok_or("Missing timestamp field")?.into(),
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
                    filesystem_snapshot: snapshot.filesystem_snapshot,
                }))
            }
            oplog_entry::Entry::SnapshotConfirmed(value) => Ok(
                PublicOplogEntry::SnapshotConfirmed(SnapshotConfirmedParams {
                    timestamp: value.timestamp.ok_or("Missing timestamp field")?.into(),
                    filesystem_snapshot: value.filesystem_snapshot,
                }),
            ),
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
            oplog_entry::Entry::CardRevoked(params) => {
                Ok(PublicOplogEntry::CardRevoked(CardRevokedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    queued_event_index: OplogIndex::from_u64(params.queued_event_index),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    wallet_generation: params.wallet_generation,
                }))
            }
            oplog_entry::Entry::CardEventQueued(params) => {
                Ok(PublicOplogEntry::CardEventQueued(CardEventQueuedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    event: public_queued_card_event_from_proto(
                        params.event.ok_or("Missing event field")?,
                    )?,
                }))
            }
            oplog_entry::Entry::CardInstalled(params) => {
                Ok(PublicOplogEntry::CardInstalled(CardInstalledParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    queued_event_index: params.queued_event_index.map(OplogIndex::from_u64),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    wallet_generation: params.wallet_generation,
                }))
            }
            oplog_entry::Entry::CardInstallFailed(params) => Ok(
                PublicOplogEntry::CardInstallFailed(CardInstallFailedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    queued_event_index: OplogIndex::from_u64(params.queued_event_index),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    reason: card_install_failure_from_proto(
                        golem_api_grpc::proto::golem::worker::CardInstallFailure::try_from(
                            params.reason,
                        )
                        .map_err(|e| format!("Invalid card install failure: {e}"))?,
                    )?,
                }),
            ),
            oplog_entry::Entry::CardDerived(params) => {
                Ok(PublicOplogEntry::CardDerived(CardDerivedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    parent_ids: params
                        .parent_ids
                        .into_iter()
                        .map(|id| CardId(id.into()))
                        .collect(),
                    wallet_generation: params.wallet_generation,
                }))
            }
            oplog_entry::Entry::CardTransferStarted(params) => Ok(
                PublicOplogEntry::CardTransferStarted(CardTransferStartedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    transfer_id: params
                        .transfer_id
                        .ok_or("Missing transfer_id field")?
                        .into(),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    target_holder: card_holder_from_proto(
                        params.target_holder.ok_or("Missing target_holder field")?,
                    )?,
                    source_wallet_generation: params.source_wallet_generation,
                }),
            ),
            oplog_entry::Entry::CardTransferred(params) => {
                Ok(PublicOplogEntry::CardTransferred(CardTransferredParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    transfer_id: params
                        .transfer_id
                        .ok_or("Missing transfer_id field")?
                        .into(),
                    source_card_id: CardId(
                        params
                            .source_card_id
                            .ok_or("Missing source_card_id field")?
                            .into(),
                    ),
                    installed_card_id: CardId(
                        params
                            .installed_card_id
                            .ok_or("Missing installed_card_id field")?
                            .into(),
                    ),
                    target_holder: card_holder_from_proto(
                        params.target_holder.ok_or("Missing target_holder field")?,
                    )?,
                    target_wallet_generation: params.target_wallet_generation,
                }))
            }
            oplog_entry::Entry::CardRevokedCascade(params) => Ok(
                PublicOplogEntry::CardRevokedCascade(CardRevokedCascadeParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    revoked_card_ids: params
                        .revoked_card_ids
                        .into_iter()
                        .map(|id| CardId(id.into()))
                        .collect(),
                    local_wallet_generation: params.local_wallet_generation,
                }),
            ),
            oplog_entry::Entry::CardTransferConfirmed(params) => Ok(
                PublicOplogEntry::CardTransferConfirmed(CardTransferConfirmedParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    transfer_id: params
                        .transfer_id
                        .ok_or("Missing transfer_id field")?
                        .into(),
                    source_card_id: CardId(
                        params
                            .source_card_id
                            .ok_or("Missing source_card_id field")?
                            .into(),
                    ),
                    installed_card_id: CardId(
                        params
                            .installed_card_id
                            .ok_or("Missing installed_card_id field")?
                            .into(),
                    ),
                    target_holder: card_holder_from_proto(
                        params.target_holder.ok_or("Missing target_holder field")?,
                    )?,
                }),
            ),
            oplog_entry::Entry::CardExpired(params) => {
                Ok(PublicOplogEntry::CardExpired(CardExpiredParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    card_id: CardId(params.card_id.ok_or("Missing card_id field")?.into()),
                    wallet_generation: params.wallet_generation,
                }))
            }
            oplog_entry::Entry::HostStreamFrame(params) => {
                Ok(PublicOplogEntry::HostStreamFrame(HostStreamFrameParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    parent_start_index: crate::base_model::OplogIndex::from_u64(
                        params.parent_start_index,
                    ),
                    kind: host_stream_kind_from_proto(params.kind)?,
                    payload: params.payload.ok_or("Missing payload field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::ToolAttachmentControl(params) => {
                let attachment = params
                    .attachment
                    .ok_or("Missing attachment field")?
                    .try_into()?;
                let transition = params
                    .transition
                    .ok_or("Missing transition field")?
                    .try_into()?;
                decode_attachment_control(&attachment, &transition)?;
                Ok(PublicOplogEntry::ToolAttachmentControl(
                    crate::model::oplog::public_oplog_entry::ToolAttachmentControlParams {
                        timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                        attachment,
                        transition,
                    },
                ))
            }
            oplog_entry::Entry::ToolAttachmentRead(params) => {
                let observation = params
                    .observation
                    .ok_or("Missing observation field")?
                    .try_into()?;
                decode_attachment_observation(&observation)?
                    .validate_payload_presence(params.payload.is_some())
                    .map_err(|error| format!("Invalid attachment read payload: {error:?}"))?;
                Ok(PublicOplogEntry::ToolAttachmentRead(
                    crate::model::oplog::public_oplog_entry::ToolAttachmentReadParams {
                        timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                        observation,
                        payload: params.payload,
                    },
                ))
            }
            oplog_entry::Entry::StreamRegistered(params) => {
                Ok(PublicOplogEntry::StreamRegistered(StreamRegisteredParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    record: params.record.ok_or("Missing record field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::StreamItems(params) => {
                Ok(PublicOplogEntry::StreamItems(StreamItemsParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    record: params.record.ok_or("Missing record field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::StreamEnd(params) => {
                Ok(PublicOplogEntry::StreamEnd(StreamEndParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    record: params.record.ok_or("Missing record field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::StreamCancel(params) => {
                Ok(PublicOplogEntry::StreamCancel(StreamCancelParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    record: params.record.ok_or("Missing record field")?.try_into()?,
                }))
            }
            oplog_entry::Entry::StreamSession(params) => {
                Ok(PublicOplogEntry::StreamSession(StreamSessionParams {
                    timestamp: params.timestamp.ok_or("Missing timestamp field")?.into(),
                    record: params.record.ok_or("Missing record field")?.try_into()?,
                }))
            }
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
                        owner_kind: create.owner_kind.into(),
                        agent_mode: golem_api_grpc::proto::golem::component::AgentMode::from(
                            create.agent_mode,
                        ) as i32,
                        component_revision: create.component_revision.into(),
                        env: create.env.into_iter().collect(),
                        config: create
                            .local_agent_config
                            .into_iter()
                            .map(TryInto::try_into)
                            .collect::<Result<_, _>>()?,
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
                        instance_id: Some(create.instance_id.into()),
                    },
                )),
            },
            PublicOplogEntry::Start(start) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Start(
                    golem_api_grpc::proto::golem::worker::StartParameters {
                        timestamp: Some(start.timestamp.into()),
                        parent_start_index: start.parent_start_index.map(|id| id.as_u64()),
                        function_name: start.function_name.clone(),
                        invocation_id: start.invocation_id.map(Into::into),
                        observational_owner: start.observational_owner.map(|id| id.as_u64()),
                        request: start.request.map(TryInto::try_into).transpose()?,
                        durable_function_type: Some(start.durable_function_type.into()),
                        span_started: start.span_started.map(public_span_started_to_proto),
                        denial: start.denial,
                    },
                )),
            },
            PublicOplogEntry::End(end) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::End(
                    golem_api_grpc::proto::golem::worker::EndParameters {
                        timestamp: Some(end.timestamp.into()),
                        start_index: end.start_index.as_u64(),
                        response: end.response.map(TryInto::try_into).transpose()?,
                        forced_commit: end.forced_commit,
                        span_finished: end.span_finished.map(public_span_finished_to_proto),
                        span_attributes: end.span_attributes.map(public_span_attributes_to_proto),
                    },
                )),
            },
            PublicOplogEntry::Cancelled(cancelled) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Cancelled(
                        golem_api_grpc::proto::golem::worker::CancelledParameters {
                            timestamp: Some(cancelled.timestamp.into()),
                            start_index: cancelled.start_index.as_u64(),
                            partial: cancelled.partial.map(TryInto::try_into).transpose()?,
                            span_finished: cancelled
                                .span_finished
                                .map(public_span_finished_to_proto),
                        },
                    )),
                }
            }
            PublicOplogEntry::CompletionDiscarded(completion_discarded) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CompletionDiscarded(
                        golem_api_grpc::proto::golem::worker::CompletionDiscardedParameters {
                            timestamp: Some(completion_discarded.timestamp.into()),
                            start_index: completion_discarded.start_index.as_u64(),
                        },
                    )),
                }
            }
            PublicOplogEntry::CompletionDelivered(completion_delivered) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CompletionDelivered(
                        golem_api_grpc::proto::golem::worker::CompletionDeliveredParameters {
                            timestamp: Some(completion_delivered.timestamp.into()),
                            start_index: completion_delivered.start_index.as_u64(),
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
                            wallet_pin: Some(public_invocation_wallet_pin_to_proto(
                                agent_invocation_started.wallet_pin,
                            )),
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
                            method_name: agent_invocation_finished.method_name,
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
                        kind: oplog_error_kind_to_proto(error.kind) as i32,
                        error: error.error,
                        retry_from: error.retry_from.0,
                        retry_policy_state: error.retry_policy_state.map(Into::into),
                    },
                )),
            },
            PublicOplogEntry::RecoverySucceeded(recovery_succeeded) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::RecoverySucceeded(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(recovery_succeeded.timestamp.into()),
                        },
                    )),
                }
            }
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
                            update_attempt_index: pending_update.update_attempt_index.into(),
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
                            snapshot_assisted_details: successful_update
                                .snapshot_assisted_details
                                .map(|details| {
                                    golem_api_grpc::proto::golem::worker::SnapshotAssistedUpdateDetails {
                                        pending_update_index: details.pending_update_index.into(),
                                        source_component_revision: details.source_component_revision.into(),
                                        source_revision_start_index: details.source_revision_start_index.into(),
                                        snapshot_index: details.snapshot_index.into(),
                                    }
                                }),
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
                            update_attempt_index: failed_update
                                .update_attempt_index
                                .map(Into::into),
                            snapshot_assisted_details: failed_update
                                .snapshot_assisted_details
                                .map(|details| {
                                    golem_api_grpc::proto::golem::worker::FailedSnapshotAssistedUpdateDetails {
                                        pending_update_index: details.pending_update_index.into(),
                                        source_component_revision: details.source_component_revision.into(),
                                        source_revision_start_index: details.source_revision_start_index.into(),
                                        snapshot_index: details.snapshot_index.into(),
                                    }
                                }),
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
                        trace_context: log.trace_context.map(Into::into),
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
            PublicOplogEntry::Resumed(resumed) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Resumed(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(resumed.timestamp.into()),
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
                            filesystem_snapshot: snapshot.filesystem_snapshot,
                        },
                    )),
                }
            }
            PublicOplogEntry::SnapshotConfirmed(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::SnapshotConfirmed(
                        golem_api_grpc::proto::golem::worker::SnapshotConfirmedParameters {
                            timestamp: Some(params.timestamp.into()),
                            filesystem_snapshot: params.filesystem_snapshot,
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
            PublicOplogEntry::CardRevoked(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardRevoked(
                        golem_api_grpc::proto::golem::worker::CardRevokedParameters {
                            timestamp: Some(params.timestamp.into()),
                            queued_event_index: params.queued_event_index.into(),
                            card_id: Some(params.card_id.0.into()),
                            wallet_generation: params.wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardEventQueued(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardEventQueued(
                        golem_api_grpc::proto::golem::worker::CardEventQueuedParameters {
                            timestamp: Some(params.timestamp.into()),
                            event: Some(public_queued_card_event_to_proto(params.event)),
                        },
                    )),
                }
            }
            PublicOplogEntry::CardInstalled(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardInstalled(
                        golem_api_grpc::proto::golem::worker::CardInstalledParameters {
                            timestamp: Some(params.timestamp.into()),
                            queued_event_index: params.queued_event_index.map(Into::into),
                            card_id: Some(params.card_id.0.into()),
                            wallet_generation: params.wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardInstallFailed(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardInstallFailed(
                        golem_api_grpc::proto::golem::worker::CardInstallFailedParameters {
                            timestamp: Some(params.timestamp.into()),
                            queued_event_index: params.queued_event_index.into(),
                            card_id: Some(params.card_id.0.into()),
                            reason: card_install_failure_to_proto(params.reason) as i32,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardDerived(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardDerived(
                        golem_api_grpc::proto::golem::worker::CardDerivedParameters {
                            timestamp: Some(params.timestamp.into()),
                            card_id: Some(params.card_id.0.into()),
                            parent_ids: params
                                .parent_ids
                                .into_iter()
                                .map(|id| id.0.into())
                                .collect(),
                            wallet_generation: params.wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardTransferStarted(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardTransferStarted(
                        golem_api_grpc::proto::golem::worker::CardTransferStartedParameters {
                            timestamp: Some(params.timestamp.into()),
                            transfer_id: Some(params.transfer_id.into()),
                            card_id: Some(params.card_id.0.into()),
                            target_holder: Some(card_holder_to_proto(params.target_holder)),
                            source_wallet_generation: params.source_wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardTransferred(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardTransferred(
                        golem_api_grpc::proto::golem::worker::CardTransferredParameters {
                            timestamp: Some(params.timestamp.into()),
                            transfer_id: Some(params.transfer_id.into()),
                            source_card_id: Some(params.source_card_id.0.into()),
                            installed_card_id: Some(params.installed_card_id.0.into()),
                            target_holder: Some(card_holder_to_proto(params.target_holder)),
                            target_wallet_generation: params.target_wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardRevokedCascade(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardRevokedCascade(
                        golem_api_grpc::proto::golem::worker::CardRevokedCascadeParameters {
                            timestamp: Some(params.timestamp.into()),
                            revoked_card_ids: params
                                .revoked_card_ids
                                .into_iter()
                                .map(|id| id.0.into())
                                .collect(),
                            local_wallet_generation: params.local_wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::CardTransferConfirmed(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardTransferConfirmed(
                        golem_api_grpc::proto::golem::worker::CardTransferConfirmedParameters {
                            timestamp: Some(params.timestamp.into()),
                            transfer_id: Some(params.transfer_id.into()),
                            source_card_id: Some(params.source_card_id.0.into()),
                            installed_card_id: Some(params.installed_card_id.0.into()),
                            target_holder: Some(card_holder_to_proto(params.target_holder)),
                        },
                    )),
                }
            }
            PublicOplogEntry::CardExpired(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CardExpired(
                        golem_api_grpc::proto::golem::worker::CardExpiredParameters {
                            timestamp: Some(params.timestamp.into()),
                            card_id: Some(params.card_id.0.into()),
                            wallet_generation: params.wallet_generation,
                        },
                    )),
                }
            }
            PublicOplogEntry::HostStreamFrame(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::HostStreamFrame(
                        golem_api_grpc::proto::golem::worker::HostStreamFrameParameters {
                            timestamp: Some(params.timestamp.into()),
                            parent_start_index: params.parent_start_index.as_u64(),
                            kind: host_stream_kind_to_proto(params.kind) as i32,
                            payload: Some(params.payload.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::StreamRegistered(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StreamRegistered(
                        golem_api_grpc::proto::golem::worker::DurableStreamRecordParameters {
                            timestamp: Some(params.timestamp.into()),
                            record: Some(params.record.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::ToolAttachmentControl(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ToolAttachmentControl(
                        golem_api_grpc::proto::golem::worker::ToolAttachmentControlParameters {
                            timestamp: Some(params.timestamp.into()),
                            attachment: Some(params.attachment.try_into()?),
                            transition: Some(params.transition.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::ToolAttachmentRead(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ToolAttachmentRead(
                        golem_api_grpc::proto::golem::worker::ToolAttachmentReadParameters {
                            timestamp: Some(params.timestamp.into()),
                            observation: Some(params.observation.try_into()?),
                            payload: params.payload,
                        },
                    )),
                }
            }
            PublicOplogEntry::StreamItems(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StreamItems(
                        golem_api_grpc::proto::golem::worker::DurableStreamRecordParameters {
                            timestamp: Some(params.timestamp.into()),
                            record: Some(params.record.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::StreamEnd(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StreamEnd(
                        golem_api_grpc::proto::golem::worker::DurableStreamRecordParameters {
                            timestamp: Some(params.timestamp.into()),
                            record: Some(params.record.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::StreamCancel(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StreamCancel(
                        golem_api_grpc::proto::golem::worker::DurableStreamRecordParameters {
                            timestamp: Some(params.timestamp.into()),
                            record: Some(params.record.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::StreamSession(params) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::StreamSession(
                        golem_api_grpc::proto::golem::worker::DurableStreamRecordParameters {
                            timestamp: Some(params.timestamp.into()),
                            record: Some(params.record.try_into()?),
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
        match wrapped_function_type::Type::try_from(value.r#type).map_err(|err| err.to_string())? {
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
        }
    }
}

impl From<PublicDurableFunctionType> for golem_api_grpc::proto::golem::worker::WrappedFunctionType {
    fn from(value: PublicDurableFunctionType) -> Self {
        match value {
            PublicDurableFunctionType::ReadLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadLocal as i32,
                }
            }
            PublicDurableFunctionType::WriteLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteLocal as i32,
                }
            }
            PublicDurableFunctionType::ReadRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadRemote as i32,
                }
            }
            PublicDurableFunctionType::WriteRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemote as i32,
                }
            }
        }
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
            golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotAssistedAutomatic(_) => {
                Ok(PublicUpdateDescription::SnapshotAssistedAutomatic(
                    SnapshotAssistedAutomaticUpdateParameters {},
                ))
            }
            golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                snapshot_based,
            ) => Ok(PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload: snapshot_based.payload,
                mime_type: snapshot_based.mime_type,
                filesystem_snapshot: snapshot_based.filesystem_snapshot,
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
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            },
            PublicUpdateDescription::SnapshotAssistedAutomatic(_) => {
                golem_api_grpc::proto::golem::worker::UpdateDescription {
                    description: Some(
                        golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotAssistedAutomatic(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                }
            }
            PublicUpdateDescription::SnapshotBased(snapshot_based) => {
                golem_api_grpc::proto::golem::worker::UpdateDescription {
                    description: Some(
                        golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                            golem_api_grpc::proto::golem::worker::SnapshotBasedUpdateParameters {
                                payload: snapshot_based.payload,
                                mime_type: snapshot_based.mime_type,
                                filesystem_snapshot: snapshot_based.filesystem_snapshot,
                            }
                        ),
                    ),
                }
            }
        }
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
            attribution: value
                .attribution
                .ok_or("Missing field: attribution")?
                .try_into()?,
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
            attribution: Some(value.attribution.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OplogPageEntry> for PublicOplogPageEntry {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::OplogPageEntry,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::oplog_page_entry::Entry;

        match value.entry.ok_or("Missing field: entry")? {
            Entry::Included(entry) => Ok(PublicOplogPageEntry::Included(entry.try_into()?)),
            Entry::Omitted(omitted) => Ok(PublicOplogPageEntry::Omitted(OmittedOplogEntry {
                oplog_index: OplogIndex::from_u64(omitted.oplog_index),
                attribution: omitted
                    .attribution
                    .ok_or("Missing field: attribution")?
                    .try_into()?,
                timestamp: omitted.timestamp.ok_or("Missing field: timestamp")?.into(),
                entry_type: omitted.entry_type,
                size: omitted.size,
                component_revision: omitted.component_revision.try_into()?,
            })),
        }
    }
}

impl TryFrom<PublicOplogPageEntry> for golem_api_grpc::proto::golem::worker::OplogPageEntry {
    type Error = String;

    fn try_from(value: PublicOplogPageEntry) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::oplog_page_entry::Entry;

        let entry = match value {
            PublicOplogPageEntry::Included(entry) => Entry::Included(entry.try_into()?),
            PublicOplogPageEntry::Omitted(omitted) => {
                Entry::Omitted(golem_api_grpc::proto::golem::worker::OmittedOplogEntry {
                    oplog_index: omitted.oplog_index.into(),
                    attribution: Some(omitted.attribution.into()),
                    timestamp: Some(omitted.timestamp.into()),
                    entry_type: omitted.entry_type,
                    size: omitted.size,
                    component_revision: omitted.component_revision.into(),
                })
            }
        };
        Ok(Self { entry: Some(entry) })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicOplogEntryAttribution>
    for PublicOplogEntryAttribution
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicOplogEntryAttribution,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_oplog_entry_attribution::Attribution;

        match value.attribution.ok_or("Missing oplog entry attribution")? {
            Attribution::Agent(_) => Ok(Self::agent()),
            Attribution::Entity(context) => Ok(Self::entity(context.try_into()?)),
        }
    }
}

impl From<PublicOplogEntryAttribution>
    for golem_api_grpc::proto::golem::worker::PublicOplogEntryAttribution
{
    fn from(value: PublicOplogEntryAttribution) -> Self {
        use golem_api_grpc::proto::golem::worker::public_oplog_entry_attribution::Attribution;

        let attribution = match value {
            PublicOplogEntryAttribution::Agent(_) => {
                Attribution::Agent(golem_schema::proto::golem::common::Empty {})
            }
            PublicOplogEntryAttribution::Entity(context) => Attribution::Entity(context.into()),
        };
        Self {
            attribution: Some(attribution),
        }
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
                State::Terminal(golem_schema::proto::golem::common::Empty {})
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
                crate::model::oplog::PublicRetryPolicyStateCounter { count: n },
            )),
            State::Terminal(_) => Ok(PublicRetryPolicyState::Terminal(
                crate::base_model::Empty {},
            )),
            State::Wrapper(w) => {
                let inner = w.inner.ok_or("Missing inner in Wrapper")?;
                Ok(PublicRetryPolicyState::Wrapper(
                    crate::model::oplog::PublicRetryPolicyStateWrapper {
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::CountBox(cb) => {
                let inner = cb.inner.ok_or("Missing inner in CountBox")?;
                Ok(PublicRetryPolicyState::CountBox(
                    crate::model::oplog::PublicRetryPolicyStateCountBox {
                        attempts: cb.attempts,
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::TimeBox(tb) => {
                let inner = tb.inner.ok_or("Missing inner in TimeBox")?;
                Ok(PublicRetryPolicyState::Wrapper(
                    crate::model::oplog::PublicRetryPolicyStateWrapper {
                        inner: Box::new((*inner).try_into()?),
                    },
                ))
            }
            State::AndThen(at) => {
                let left = at.left.ok_or("Missing left in AndThen")?;
                let right = at.right.ok_or("Missing right in AndThen")?;
                Ok(PublicRetryPolicyState::AndThen(
                    crate::model::oplog::PublicRetryPolicyStateAndThen {
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
                    crate::model::oplog::PublicRetryPolicyStatePair {
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
                parameters: Box::new(CreateParameters {
                    agent_id: create.agent_id,
                    owner_kind: create.owner_kind,
                    agent_mode: create.agent_mode,
                    component_revision: create.component_revision,
                    env: create.env.into_iter().collect(),
                    environment_id: create.environment_id,
                    created_by: create.created_by,
                    local_agent_config: create
                        .local_agent_config
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<_>, _>>()?,
                    parent: create.parent,
                    component_size: create.component_size,
                    initial_total_linear_memory_size: create.initial_total_linear_memory_size,
                    initial_active_plugins: create
                        .initial_active_plugins
                        .into_iter()
                        .map(|p| p.environment_plugin_grant_id)
                        .collect(),
                    original_phantom_id: create.original_phantom_id,
                    instance_id: create.instance_id,
                }),
            }),
            PublicOplogEntry::Start(start) => {
                let durable_function_type = match start.durable_function_type {
                    PublicDurableFunctionType::ReadLocal(_) => DurableFunctionType::ReadLocal,
                    PublicDurableFunctionType::WriteLocal(_) => DurableFunctionType::WriteLocal,
                    PublicDurableFunctionType::ReadRemote(_) => DurableFunctionType::ReadRemote,
                    PublicDurableFunctionType::WriteRemote(_) => DurableFunctionType::WriteRemote,
                };

                let request = start
                    .request
                    .map(|value| {
                        host_request_from_typed_schema_value(&start.function_name, value)
                            .map(|req| OplogPayload::Inline(Box::new(req)))
                    })
                    .transpose()?;

                Ok(OplogEntry::Start {
                    timestamp: start.timestamp,
                    parent_start_index: start.parent_start_index,
                    function_name: HostFunctionName::from(start.function_name.as_str()),
                    invocation_id: start.invocation_id,
                    observational_owner: start.observational_owner,
                    request,
                    durable_function_type,
                    span_started: start
                        .span_started
                        .map(public_span_started_to_raw)
                        .map(Box::new),
                    denial: start.denial,
                })
            }
            PublicOplogEntry::End(end) => {
                // The public type carries the response as a `ValueAndType`, but we cannot map it
                // back to a typed `HostResponse` here without remembering the originating Start's
                // function name. This converter is only used for snapshot import in tests.
                // Reconstructing a typed inline response would require pairing this `End` with its
                // `Start` in the same conversion pass, which is out of scope.
                let response = end
                    .response
                    .map(|_| Err::<OplogPayload<_>, _>("Converting non-empty End response from public to raw oplog entry is not supported".to_string()))
                    .transpose()?;
                Ok(OplogEntry::End {
                    timestamp: end.timestamp,
                    start_index: end.start_index,
                    response,
                    forced_commit: end.forced_commit,
                    span_finished: end.span_finished.map(public_span_finished_to_raw),
                    span_attributes: end.span_attributes.map(public_span_attributes_to_raw),
                })
            }
            PublicOplogEntry::Cancelled(cancelled) => {
                let partial = cancelled
                    .partial
                    .map(|_| Err::<OplogPayload<_>, _>("Converting non-empty Cancelled partial response from public to raw oplog entry is not supported".to_string()))
                    .transpose()?;
                Ok(OplogEntry::Cancelled {
                    timestamp: cancelled.timestamp,
                    start_index: cancelled.start_index,
                    partial,
                    span_finished: cancelled.span_finished.map(public_span_finished_to_raw),
                })
            }
            PublicOplogEntry::CompletionDiscarded(completion_discarded) => {
                Ok(OplogEntry::CompletionDiscarded {
                    timestamp: completion_discarded.timestamp,
                    start_index: completion_discarded.start_index,
                })
            }
            PublicOplogEntry::CompletionDelivered(completion_delivered) => {
                Ok(OplogEntry::CompletionDelivered {
                    timestamp: completion_delivered.timestamp,
                    start_index: completion_delivered.start_index,
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
                    method_name: finished.method_name,
                    consumed_fuel: finished.consumed_fuel,
                    component_revision: finished.component_revision,
                })
            }
            PublicOplogEntry::Suspend(p) => Ok(OplogEntry::Suspend {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Error(error) => Ok(OplogEntry::Error {
                timestamp: error.timestamp,
                entity_parent_start_index: None,
                kind: error.kind,
                error: AgentError::Unknown(error.error),
                retry_from: error.retry_from,
                retry_policy_state: error.retry_policy_state.map(Into::into),
            }),
            PublicOplogEntry::RecoverySucceeded(p) => Ok(OplogEntry::RecoverySucceeded {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Jump(jump) => Ok(OplogEntry::Jump {
                timestamp: jump.timestamp,
                entity_parent_start_index: None,
                jump: jump.jump,
            }),
            PublicOplogEntry::Interrupted(p) => Ok(OplogEntry::Interrupted {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Exited(p) => Ok(OplogEntry::Exited {
                timestamp: p.timestamp,
            }),
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
                    new_total_linear_memory_size: None,
                    new_active_plugins,
                    snapshot_assisted_details: p.snapshot_assisted_details.map(|details| {
                        SnapshotAssistedUpdateDetails {
                            pending_update_index: details.pending_update_index,
                            source_component_revision: details.source_component_revision,
                            source_revision_start_index: details.source_revision_start_index,
                            snapshot_index: details.snapshot_index,
                        }
                    }),
                })
            }
            PublicOplogEntry::FailedUpdate(p) => Ok(OplogEntry::FailedUpdate {
                timestamp: p.timestamp,
                target_revision: p.target_revision,
                details: p.details,
                update_attempt_index: p.update_attempt_index,
                snapshot_assisted_details: p.snapshot_assisted_details.map(|details| {
                    FailedSnapshotAssistedUpdateDetails {
                        pending_update_index: details.pending_update_index,
                        source_component_revision: details.source_component_revision,
                        source_revision_start_index: details.source_revision_start_index,
                        snapshot_index: details.snapshot_index,
                    }
                }),
                snapshot_fault: None,
            }),
            PublicOplogEntry::GrowMemory(p) => Ok(OplogEntry::GrowMemory {
                timestamp: p.timestamp,
                delta: p.delta,
            }),
            PublicOplogEntry::CreateResource(p) => Ok(OplogEntry::CreateResource {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                id: p.id,
                resource_type_id: ResourceTypeId {
                    owner: p.owner,
                    name: p.name,
                },
            }),
            PublicOplogEntry::DropResource(p) => Ok(OplogEntry::DropResource {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                id: p.id,
                resource_type_id: ResourceTypeId {
                    owner: p.owner,
                    name: p.name,
                },
            }),
            PublicOplogEntry::Log(p) => Ok(OplogEntry::Log {
                timestamp: p.timestamp,
                parent_start_index: None,
                level: p.level,
                context: p.context,
                message: p.message,
                trace_context: p.trace_context,
            }),
            PublicOplogEntry::Restart(p) => Ok(OplogEntry::Restart {
                timestamp: p.timestamp,
            }),
            PublicOplogEntry::Resumed(p) => Ok(OplogEntry::Resumed {
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
                        use crate::model::oplog::MultipartPartData;

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
                    active_cards: Vec::new(),
                    wallet_generation: 0,
                    filesystem_snapshot: p
                        .filesystem_snapshot
                        .map(|name| name.parse())
                        .transpose()?,
                })
            }
            PublicOplogEntry::SnapshotConfirmed(p) => Ok(OplogEntry::SnapshotConfirmed {
                timestamp: p.timestamp,
                filesystem_snapshot: p.filesystem_snapshot.parse()?,
            }),
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
                entity_parent_start_index: None,
                policy: Box::new(p.policy.into()),
            }),
            PublicOplogEntry::RemoveRetryPolicy(p) => Ok(OplogEntry::RemoveRetryPolicy {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                name: p.name,
            }),
            PublicOplogEntry::CardRevoked(p) => Ok(OplogEntry::CardRevoked {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                queued_event_index: p.queued_event_index,
                card_id: p.card_id,
                wallet_generation: p.wallet_generation,
            }),
            PublicOplogEntry::CardEventQueued(_) => {
                Err("Converting CardEventQueued from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardInstalled(_) => {
                Err("Converting CardInstalled from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardDerived(_) => {
                Err("Converting CardDerived from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardTransferStarted(_) => {
                Err("Converting CardTransferStarted from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardTransferred(_) => {
                Err("Converting CardTransferred from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardRevokedCascade(_) => {
                Err("Converting CardRevokedCascade from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardTransferConfirmed(_) => {
                Err("Converting CardTransferConfirmed from public to raw oplog entry is not supported".to_string())
            }
            PublicOplogEntry::CardInstallFailed(p) => Ok(OplogEntry::CardInstallFailed {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                queued_event_index: p.queued_event_index,
                card_id: p.card_id,
                reason: p.reason,
            }),
            PublicOplogEntry::CardExpired(p) => Ok(OplogEntry::CardExpired {
                timestamp: p.timestamp,
                entity_parent_start_index: None,
                card_id: p.card_id,
                wallet_generation: p.wallet_generation,
            }),
            // The concrete payload type cannot be recovered from the rendered schema value
            // (there is no owning function name to key it on), so it is preserved as a
            // `Custom` host request.
            PublicOplogEntry::HostStreamFrame(p) => Ok(OplogEntry::HostStreamFrame {
                timestamp: p.timestamp,
                parent_start_index: p.parent_start_index,
                kind: p.kind,
                payload: OplogPayload::Inline(Box::new(
                    crate::model::oplog::payload::HostRequest::from(p.payload),
                )),
            }),
            PublicOplogEntry::StreamRegistered(p) => {
                use crate::schema::FromSchema as _;
                Ok(OplogEntry::StreamRegistered {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    summary: Some(DurableStreamEventSummary::Registered),
                    record: OplogPayload::Inline(Box::new(StreamRegisteredRecord::from_value(
                        p.record.value(),
                    ).map_err(|error| error.to_string())?)),
                })
            }
            PublicOplogEntry::ToolAttachmentControl(p) => {
                let (attachment, transition) = decode_attachment_control(&p.attachment, &p.transition)?;
                Ok(OplogEntry::ToolAttachmentControl {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    attachment,
                    transition,
                })
            }
            PublicOplogEntry::ToolAttachmentRead(p) => {
                let observation = decode_attachment_observation(&p.observation)?;
                observation.validate_payload_presence(p.payload.is_some())
                    .map_err(|error| format!("Invalid attachment read payload: {error:?}"))?;
                Ok(OplogEntry::ToolAttachmentRead {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    observation: Box::new(observation),
                    payload: p.payload.map(|bytes| OplogPayload::Inline(Box::new(bytes))),
                })
            }
            PublicOplogEntry::StreamItems(p) => {
                use crate::schema::FromSchema as _;
                let record = StreamItemsRecord::from_value(p.record.value())
                    .map_err(|error| error.to_string())?;
                Ok(OplogEntry::StreamItems {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    summary: Some(DurableStreamEventSummary::items(&record)),
                    record: OplogPayload::Inline(Box::new(record)),
                })
            }
            PublicOplogEntry::StreamEnd(p) => {
                use crate::schema::FromSchema as _;
                let record = StreamEndRecord::from_value(p.record.value())
                    .map_err(|error| error.to_string())?;
                Ok(OplogEntry::StreamEnd {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    summary: Some(DurableStreamEventSummary::end(&record)),
                    record: OplogPayload::Inline(Box::new(record)),
                })
            }
            PublicOplogEntry::StreamCancel(p) => {
                use crate::schema::FromSchema as _;
                Ok(OplogEntry::StreamCancel {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    summary: Some(DurableStreamEventSummary::Cancelled),
                    record: OplogPayload::Inline(Box::new(StreamCancelRecord::from_value(
                        p.record.value(),
                    ).map_err(|error| error.to_string())?)),
                })
            }
            PublicOplogEntry::StreamSession(p) => {
                use crate::schema::FromSchema as _;
                let record = StreamSessionRecord::from_value(p.record.value())
                    .map_err(|error| error.to_string())?;
                Ok(OplogEntry::StreamSession {
                    timestamp: p.timestamp,
                    entity_parent_start_index: None,
                    summary: DurableStreamEventSummary::session(&record),
                    record: OplogPayload::Inline(Box::new(record)),
                })
            }
        }
    }
}
