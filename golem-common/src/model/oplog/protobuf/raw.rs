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

use golem_api_grpc::proto::golem::worker::RawCardExpiredParameters;

use super::cards::{
    card_holder_from_proto, card_holder_to_proto, deserialize_stored_card,
    invocation_wallet_pin_from_proto, invocation_wallet_pin_to_proto,
    raw_card_install_failure_from_proto, raw_card_install_failure_to_proto,
    raw_queued_card_event_from_proto, raw_queued_card_event_to_proto,
};
use super::spans::{
    raw_span_attributes_from_proto, raw_span_attributes_to_proto, raw_span_finished_from_proto,
    raw_span_finished_to_proto, raw_span_started_from_proto, raw_span_started_to_proto,
    span_data_from_proto, span_data_to_proto,
};
use super::{
    decode_attachment_control, decode_attachment_observation, host_stream_kind_from_proto,
    host_stream_kind_to_proto, oplog_error_kind_from_proto, oplog_error_kind_to_proto,
};
use crate::base_model::OplogIndex;
use crate::base_model::agent::AgentMode;
use crate::model::card::CardId;
use crate::model::invocation_context::TraceId;
use crate::model::oplog::payload::OplogPayload;
use crate::model::oplog::raw_types::{
    CreateParameters, DurableStreamEventSummary, DurableStreamOutcome,
};
use crate::model::oplog::{
    AgentResourceId, DurableFunctionType, FailedSnapshotAssistedUpdateDetails, LogLevel,
    OplogEntry, PublicOplogEntry, SnapshotAssistedUpdateDetails,
};
use crate::resource_runtime::ResourceTypeId;

impl From<DurableStreamEventSummary>
    for golem_api_grpc::proto::golem::worker::RawDurableStreamEventSummary
{
    fn from(value: DurableStreamEventSummary) -> Self {
        use golem_api_grpc::proto::golem::worker::raw_durable_stream_event_summary::{
            Kind, Outcome,
        };

        let (kind, item_count, outcome) = match value {
            DurableStreamEventSummary::Registered => (Kind::Registered, None, None),
            DurableStreamEventSummary::Items { item_count } => {
                (Kind::Items, Some(item_count), None)
            }
            DurableStreamEventSummary::End { outcome } => (Kind::End, None, Some(outcome)),
            DurableStreamEventSummary::Cancelled => (Kind::Cancelled, None, None),
            DurableStreamEventSummary::SessionResult => (Kind::SessionResult, None, None),
            DurableStreamEventSummary::SessionFinished { outcome } => {
                (Kind::SessionFinished, None, Some(outcome))
            }
            DurableStreamEventSummary::SessionCancellation => {
                (Kind::SessionCancellation, None, None)
            }
            DurableStreamEventSummary::SessionExpired => (Kind::SessionExpired, None, None),
        };
        Self {
            kind: kind as i32,
            item_count,
            outcome: outcome.map(|outcome| match outcome {
                DurableStreamOutcome::Success => Outcome::Success as i32,
                DurableStreamOutcome::Error => Outcome::Error as i32,
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::RawDurableStreamEventSummary>
    for DurableStreamEventSummary
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::RawDurableStreamEventSummary,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::raw_durable_stream_event_summary::{
            Kind, Outcome,
        };

        let kind = Kind::try_from(value.kind).map_err(|error| error.to_string())?;
        let outcome = value
            .outcome
            .map(
                |outcome| match Outcome::try_from(outcome).map_err(|error| error.to_string())? {
                    Outcome::Success => Ok::<_, String>(DurableStreamOutcome::Success),
                    Outcome::Error => Ok(DurableStreamOutcome::Error),
                },
            )
            .transpose()?;
        match (kind, value.item_count, outcome) {
            (Kind::Registered, None, None) => Ok(Self::Registered),
            (Kind::Items, Some(item_count), None) => Ok(Self::Items { item_count }),
            (Kind::End, None, Some(outcome)) => Ok(Self::End { outcome }),
            (Kind::Cancelled, None, None) => Ok(Self::Cancelled),
            (Kind::SessionResult, None, None) => Ok(Self::SessionResult),
            (Kind::SessionFinished, None, Some(outcome)) => Ok(Self::SessionFinished { outcome }),
            (Kind::SessionCancellation, None, None) => Ok(Self::SessionCancellation),
            (Kind::SessionExpired, None, None) => Ok(Self::SessionExpired),
            _ => Err("Invalid durable stream event summary fields".to_string()),
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

fn durable_function_type_to_proto(
    dft: DurableFunctionType,
) -> golem_api_grpc::proto::golem::worker::WrappedFunctionType {
    use golem_api_grpc::proto::golem::worker::wrapped_function_type;
    match dft {
        DurableFunctionType::ReadLocal => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::ReadLocal as i32,
            }
        }
        DurableFunctionType::WriteLocal => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteLocal as i32,
            }
        }
        DurableFunctionType::ReadRemote => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::ReadRemote as i32,
            }
        }
        DurableFunctionType::WriteRemote => {
            golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                r#type: wrapped_function_type::Type::WriteRemote as i32,
            }
        }
    }
}

fn durable_function_type_from_proto(
    wft: golem_api_grpc::proto::golem::worker::WrappedFunctionType,
) -> Result<DurableFunctionType, String> {
    use golem_api_grpc::proto::golem::worker::wrapped_function_type;
    match wrapped_function_type::Type::try_from(wft.r#type).map_err(|err| err.to_string())? {
        wrapped_function_type::Type::ReadLocal => Ok(DurableFunctionType::ReadLocal),
        wrapped_function_type::Type::WriteLocal => Ok(DurableFunctionType::WriteLocal),
        wrapped_function_type::Type::ReadRemote => Ok(DurableFunctionType::ReadRemote),
        wrapped_function_type::Type::WriteRemote => Ok(DurableFunctionType::WriteRemote),
    }
}

fn update_description_to_proto(
    desc: crate::model::oplog::raw_types::UpdateDescription,
) -> Result<golem_api_grpc::proto::golem::worker::RawUpdateDescription, String> {
    use golem_api_grpc::proto::golem::worker::raw_update_description::Description;
    use golem_api_grpc::proto::golem::worker::{
        RawSnapshotAssistedAutomaticUpdate, RawSnapshotBasedUpdate, RawUpdateDescription,
    };

    use crate::model::oplog::raw_types::UpdateDescription;
    match desc {
        UpdateDescription::Automatic { target_revision } => Ok(RawUpdateDescription {
            description: Some(Description::AutomaticTargetRevision(target_revision.into())),
        }),
        UpdateDescription::SnapshotAssistedAutomatic {
            target_revision,
            source_component_revision,
            source_revision_start_index,
            snapshot_index,
            snapshot_revision,
            filesystem_snapshot,
        } => Ok(RawUpdateDescription {
            description: Some(Description::SnapshotAssistedAutomatic(
                RawSnapshotAssistedAutomaticUpdate {
                    target_revision: target_revision.into(),
                    source_component_revision: source_component_revision.into(),
                    source_revision_start_index: source_revision_start_index.into(),
                    snapshot_index: snapshot_index.into(),
                    snapshot_revision: snapshot_revision.into(),
                    filesystem_snapshot: filesystem_snapshot.map(String::from),
                },
            )),
        }),
        UpdateDescription::SnapshotBased {
            target_revision,
            payload,
            mime_type,
            filesystem_snapshot,
        } => Ok(RawUpdateDescription {
            description: Some(Description::SnapshotBased(RawSnapshotBasedUpdate {
                target_revision: target_revision.into(),
                payload: Some(oplog_payload_to_proto(payload)?),
                mime_type,
                filesystem_snapshot: filesystem_snapshot.map(String::from),
            })),
        }),
    }
}

fn raw_snapshot_fault_to_proto(
    fault: crate::model::oplog::raw_types::SnapshotFault,
) -> golem_api_grpc::proto::golem::worker::RawSnapshotFault {
    use golem_api_grpc::proto::golem::worker::RawSnapshotFault;

    use crate::model::oplog::raw_types::SnapshotFault;
    match fault {
        SnapshotFault::Unavailable => RawSnapshotFault::Unavailable,
        SnapshotFault::Incompatible => RawSnapshotFault::Incompatible,
    }
}

fn raw_snapshot_fault_from_proto(
    value: i32,
) -> Result<crate::model::oplog::raw_types::SnapshotFault, String> {
    use golem_api_grpc::proto::golem::worker::RawSnapshotFault;

    use crate::model::oplog::raw_types::SnapshotFault;
    match RawSnapshotFault::try_from(value)
        .map_err(|_| format!("Invalid snapshot fault: {value}"))?
    {
        RawSnapshotFault::Unavailable => Ok(SnapshotFault::Unavailable),
        RawSnapshotFault::Incompatible => Ok(SnapshotFault::Incompatible),
    }
}

fn update_description_from_proto(
    proto: golem_api_grpc::proto::golem::worker::RawUpdateDescription,
) -> Result<crate::model::oplog::raw_types::UpdateDescription, String> {
    use golem_api_grpc::proto::golem::worker::raw_update_description::Description;

    use crate::model::oplog::raw_types::UpdateDescription;
    match proto
        .description
        .ok_or("Missing description in RawUpdateDescription")?
    {
        Description::AutomaticTargetRevision(rev) => Ok(UpdateDescription::Automatic {
            target_revision: rev.try_into().map_err(|e: String| e)?,
        }),
        Description::SnapshotAssistedAutomatic(update) => {
            Ok(UpdateDescription::SnapshotAssistedAutomatic {
                target_revision: update.target_revision.try_into().map_err(|e: String| e)?,
                source_component_revision: update
                    .source_component_revision
                    .try_into()
                    .map_err(|e: String| e)?,
                source_revision_start_index: OplogIndex::from_u64(
                    update.source_revision_start_index,
                ),
                snapshot_index: OplogIndex::from_u64(update.snapshot_index),
                snapshot_revision: update.snapshot_revision.try_into().map_err(|e: String| e)?,
                filesystem_snapshot: update
                    .filesystem_snapshot
                    .map(|name| name.parse())
                    .transpose()?,
            })
        }
        Description::SnapshotBased(snap) => Ok(UpdateDescription::SnapshotBased {
            target_revision: snap.target_revision.try_into().map_err(|e: String| e)?,
            payload: oplog_payload_from_proto(
                snap.payload
                    .ok_or("Missing payload in SnapshotBasedUpdate")?,
            )?,
            mime_type: snap.mime_type,
            filesystem_snapshot: snap
                .filesystem_snapshot
                .map(|name| name.parse())
                .transpose()?,
        }),
    }
}

#[allow(deprecated)]
impl TryFrom<OplogEntry> for golem_api_grpc::proto::golem::worker::RawOplogEntry {
    type Error = String;

    fn try_from(
        value: OplogEntry,
    ) -> Result<golem_api_grpc::proto::golem::worker::RawOplogEntry, String> {
        use golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry;
        use golem_api_grpc::proto::golem::worker::{
            RawActivatePluginParameters, RawAgentInvocationFinishedParameters,
            RawAgentInvocationStartedParameters, RawBeginRemoteTransactionParameters,
            RawCancelPendingInvocationParameters, RawCancelledParameters, RawCardDerivedParameters,
            RawCardEventQueuedParameters, RawCardInstallFailedParameters,
            RawCardInstalledParameters, RawCardRevokedCascadeParameters, RawCardRevokedParameters,
            RawCardTransferConfirmedParameters, RawCardTransferStartedParameters,
            RawCardTransferredParameters, RawCompletionDeliveredParameters,
            RawCompletionDiscardedParameters, RawCreateParameters, RawCreateResourceParameters,
            RawDeactivatePluginParameters, RawDropResourceParameters,
            RawDurableStreamRecordParameters, RawEndParameters, RawEnvVar, RawErrorParameters,
            RawFailedSnapshotAssistedUpdateDetails, RawFailedUpdateParameters,
            RawGrowMemoryParameters, RawHostStreamFrameParameters, RawJumpParameters,
            RawLogParameters, RawOplogProcessorCheckpointParameters, RawOplogRegion,
            RawPendingAgentInvocationParameters, RawPendingUpdateParameters,
            RawRemoteTransactionParameters, RawRemoveRetryPolicyParameters, RawResourceTypeId,
            RawRevertParameters, RawSetRetryPolicyParameters, RawSnapshotAssistedUpdateDetails,
            RawSnapshotConfirmedParameters, RawSnapshotParameters, RawStartParameters,
            RawSuccessfulUpdateParameters, RawTimestampOnly,
        };

        let timestamp = value.timestamp();
        let entity_parent_start_index = value
            .entity_parent_start_index()
            .map(|index| index.as_u64());
        let proto_ts: prost_types::Timestamp = timestamp.into();

        let entry = match value {
            OplogEntry::Create { parameters, .. } => {
                let CreateParameters {
                    agent_id,
                    owner_kind,
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
                } = *parameters;
                Entry::Create(RawCreateParameters {
                    agent_id: Some(agent_id.into()),
                    owner_kind: owner_kind.into(),
                    agent_mode: golem_api_grpc::proto::golem::component::AgentMode::from(agent_mode)
                        as i32,
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
                    instance_id: Some(instance_id.into()),
                })
            }
            OplogEntry::Start {
                parent_start_index,
                function_name,
                invocation_id,
                observational_owner,
                request,
                durable_function_type,
                span_started,
                denial,
                ..
            } => Entry::Start(RawStartParameters {
                parent_start_index: parent_start_index.map(|id| id.as_u64()),
                function_name: function_name.to_string(),
                invocation_id: invocation_id.map(Into::into),
                observational_owner: observational_owner.map(|id| id.as_u64()),
                request: request.map(oplog_payload_to_proto).transpose()?,
                durable_function_type: Some(durable_function_type_to_proto(durable_function_type)),
                span_started: span_started.map(|span| raw_span_started_to_proto(*span)),
                denial,
            }),
            OplogEntry::End {
                start_index,
                response,
                forced_commit,
                span_finished,
                span_attributes,
                ..
            } => Entry::End(RawEndParameters {
                start_index: start_index.as_u64(),
                response: response.map(oplog_payload_to_proto).transpose()?,
                forced_commit,
                span_finished: span_finished.map(raw_span_finished_to_proto),
                span_attributes: span_attributes.map(raw_span_attributes_to_proto),
            }),
            OplogEntry::Cancelled {
                start_index,
                partial,
                span_finished,
                ..
            } => Entry::Cancelled(RawCancelledParameters {
                start_index: start_index.as_u64(),
                partial: partial.map(oplog_payload_to_proto).transpose()?,
                span_finished: span_finished.map(raw_span_finished_to_proto),
            }),
            OplogEntry::CompletionDiscarded { start_index, .. } => {
                Entry::CompletionDiscarded(RawCompletionDiscardedParameters {
                    start_index: start_index.as_u64(),
                })
            }
            OplogEntry::CompletionDelivered { start_index, .. } => {
                Entry::CompletionDelivered(RawCompletionDeliveredParameters {
                    start_index: start_index.as_u64(),
                })
            }
            OplogEntry::AgentInvocationStarted {
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                wallet_pin,
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
                wallet_pin: Some(invocation_wallet_pin_to_proto(*wallet_pin)),
            }),
            OplogEntry::AgentInvocationFinished {
                result,
                method_name,
                consumed_fuel,
                component_revision,
                ..
            } => Entry::AgentInvocationFinished(RawAgentInvocationFinishedParameters {
                result: Some(oplog_payload_to_proto(result)?),
                method_name,
                consumed_fuel,
                component_revision: component_revision.into(),
            }),
            OplogEntry::Suspend { .. } => Entry::Suspend(RawTimestampOnly {}),
            OplogEntry::Error {
                kind,
                error,
                retry_from,
                retry_policy_state,
                ..
            } => Entry::Error(RawErrorParameters {
                error: Some(error.into()),
                kind: oplog_error_kind_to_proto(kind) as i32,
                retry_from: retry_from.into(),
                retry_policy_state: retry_policy_state
                    .map(|s| crate::serialization::serialize(&s))
                    .transpose()?,
            }),
            OplogEntry::RecoverySucceeded { .. } => Entry::RecoverySucceeded(RawTimestampOnly {}),
            OplogEntry::Jump { jump, .. } => Entry::Jump(RawJumpParameters {
                jump: Some(RawOplogRegion {
                    start: jump.start.into(),
                    end: jump.end.into(),
                }),
            }),
            OplogEntry::Interrupted { .. } => Entry::Interrupted(RawTimestampOnly {}),
            OplogEntry::Exited { .. } => Entry::Exited(RawTimestampOnly {}),
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
            OplogEntry::PendingUpdate {
                description,
                update_attempt_index,
                ..
            } => Entry::PendingUpdate(RawPendingUpdateParameters {
                description: Some(update_description_to_proto(description)?),
                update_attempt_index: update_attempt_index.map(Into::into),
            }),
            OplogEntry::SuccessfulUpdate {
                target_revision,
                new_component_size,
                new_total_linear_memory_size,
                new_active_plugins,
                snapshot_assisted_details,
                ..
            } => Entry::SuccessfulUpdate(RawSuccessfulUpdateParameters {
                target_revision: target_revision.into(),
                new_component_size,
                new_total_linear_memory_size,
                new_active_plugins: new_active_plugins.into_iter().map(Into::into).collect(),
                snapshot_assisted_details: snapshot_assisted_details.map(|details| {
                    RawSnapshotAssistedUpdateDetails {
                        pending_update_index: details.pending_update_index.into(),
                        source_component_revision: details.source_component_revision.into(),
                        source_revision_start_index: details.source_revision_start_index.into(),
                        snapshot_index: details.snapshot_index.into(),
                    }
                }),
            }),
            OplogEntry::FailedUpdate {
                target_revision,
                details,
                snapshot_assisted_details,
                update_attempt_index,
                snapshot_fault,
                ..
            } => Entry::FailedUpdate(RawFailedUpdateParameters {
                target_revision: target_revision.into(),
                details,
                snapshot_assisted_details: snapshot_assisted_details.map(|details| {
                    RawFailedSnapshotAssistedUpdateDetails {
                        pending_update_index: details.pending_update_index.into(),
                        source_component_revision: details.source_component_revision.into(),
                        source_revision_start_index: details.source_revision_start_index.into(),
                        snapshot_index: details.snapshot_index.into(),
                    }
                }),
                update_attempt_index: update_attempt_index.map(Into::into),
                snapshot_fault: snapshot_fault
                    .map(|fault| raw_snapshot_fault_to_proto(fault) as i32),
            }),
            OplogEntry::GrowMemory { delta, .. } => {
                Entry::GrowMemory(RawGrowMemoryParameters { delta })
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
                parent_start_index,
                level,
                context,
                message,
                trace_context,
                ..
            } => Entry::Log(RawLogParameters {
                level: Into::<golem_api_grpc::proto::golem::worker::OplogLogLevel>::into(level)
                    as i32,
                context,
                message,
                parent_start_index: parent_start_index.map(|index| index.as_u64()),
                trace_context: trace_context.map(Into::into),
            }),
            OplogEntry::Restart { .. } => Entry::Restart(RawTimestampOnly {}),
            OplogEntry::Resumed { .. } => Entry::Resumed(RawTimestampOnly {}),
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
            OplogEntry::BeginRemoteTransaction {
                transaction_id,
                original_begin_index,
                ..
            } => Entry::BeginRemoteTransaction(RawBeginRemoteTransactionParameters {
                transaction_id: String::from(transaction_id),
                original_begin_index: original_begin_index.into(),
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
                data,
                mime_type,
                active_cards,
                wallet_generation,
                filesystem_snapshot,
                ..
            } => Entry::Snapshot(RawSnapshotParameters {
                data: Some(oplog_payload_to_proto(data)?),
                mime_type,
                active_cards: active_cards
                    .into_iter()
                    .map(|card| crate::serialization::serialize(&card))
                    .collect::<Result<Vec<_>, _>>()?,
                wallet_generation,
                filesystem_snapshot: filesystem_snapshot.map(String::from),
            }),
            OplogEntry::SnapshotConfirmed {
                filesystem_snapshot,
                ..
            } => Entry::SnapshotConfirmed(RawSnapshotConfirmedParameters {
                filesystem_snapshot: filesystem_snapshot.into(),
            }),
            OplogEntry::SpanClosed {
                resource_start_index,
                span_finished,
                ..
            } => Entry::SpanClosed(
                golem_api_grpc::proto::golem::worker::RawSpanClosedParameters {
                    resource_start_index: resource_start_index.map(|index| index.as_u64()),
                    span_finished: Some(raw_span_finished_to_proto(span_finished)),
                },
            ),
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
                    policy: Some((*policy).into()),
                })
            }
            OplogEntry::RemoveRetryPolicy { name, .. } => {
                Entry::RemoveRetryPolicy(RawRemoveRetryPolicyParameters { name })
            }
            OplogEntry::CardRevoked {
                timestamp,
                queued_event_index,
                card_id,
                wallet_generation,
                ..
            } => Entry::CardRevoked(RawCardRevokedParameters {
                timestamp: Some(timestamp.into()),
                queued_event_index: queued_event_index.into(),
                card_id: Some(card_id.0.into()),
                wallet_generation,
            }),
            OplogEntry::CardEventQueued {
                timestamp, event, ..
            } => Entry::CardEventQueued(RawCardEventQueuedParameters {
                timestamp: Some(timestamp.into()),
                event: Some(raw_queued_card_event_to_proto(*event)?),
            }),
            OplogEntry::CardInstalled {
                timestamp,
                queued_event_index,
                card,
                wallet_generation,
                ..
            } => Entry::CardInstalled(RawCardInstalledParameters {
                timestamp: Some(timestamp.into()),
                queued_event_index: queued_event_index.map(Into::into),
                card: crate::serialization::serialize(&card)?,
                wallet_generation,
            }),
            OplogEntry::CardInstallFailed {
                timestamp,
                queued_event_index,
                card_id,
                reason,
                ..
            } => Entry::CardInstallFailed(RawCardInstallFailedParameters {
                timestamp: Some(timestamp.into()),
                queued_event_index: queued_event_index.into(),
                card_id: Some(card_id.0.into()),
                reason: raw_card_install_failure_to_proto(reason) as i32,
            }),
            OplogEntry::CardDerived {
                timestamp,
                card,
                wallet_generation,
                transfer_id,
                ..
            } => Entry::CardDerived(RawCardDerivedParameters {
                timestamp: Some(timestamp.into()),
                card: crate::serialization::serialize(&card)?,
                wallet_generation,
                transfer_id: transfer_id.map(Into::into),
            }),
            OplogEntry::CardTransferStarted {
                timestamp,
                transfer_id,
                card_id,
                source_holder,
                target_holder,
                source_wallet_generation,
                ..
            } => Entry::CardTransferStarted(RawCardTransferStartedParameters {
                timestamp: Some(timestamp.into()),
                transfer_id: Some(transfer_id.into()),
                card_id: Some(card_id.0.into()),
                source_holder: Some(card_holder_to_proto(source_holder)),
                target_holder: Some(card_holder_to_proto(target_holder)),
                source_wallet_generation,
            }),
            OplogEntry::CardTransferred {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                card,
                target_wallet_generation,
                ..
            } => {
                if card.card_id() != installed_card_id {
                    return Err(
                        "Transferred card payload does not match installed_card_id".to_string()
                    );
                }
                Entry::CardTransferred(RawCardTransferredParameters {
                    timestamp: Some(timestamp.into()),
                    transfer_id: Some(transfer_id.into()),
                    source_card_id: Some(source_card_id.0.into()),
                    installed_card_id: Some(installed_card_id.0.into()),
                    target_holder: Some(card_holder_to_proto(target_holder)),
                    card: crate::serialization::serialize(&card)?,
                    target_wallet_generation,
                })
            }
            OplogEntry::CardRevokedCascade {
                timestamp,
                revoked_card_ids,
                affected_wallets,
                local_wallet_generation,
                ..
            } => Entry::CardRevokedCascade(RawCardRevokedCascadeParameters {
                timestamp: Some(timestamp.into()),
                revoked_card_ids: revoked_card_ids.into_iter().map(|id| id.0.into()).collect(),
                affected_wallets: affected_wallets
                    .into_iter()
                    .map(card_holder_to_proto)
                    .collect(),
                local_wallet_generation,
            }),
            OplogEntry::CardTransferConfirmed {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => Entry::CardTransferConfirmed(RawCardTransferConfirmedParameters {
                timestamp: Some(timestamp.into()),
                transfer_id: Some(transfer_id.into()),
                source_card_id: Some(source_card_id.0.into()),
                installed_card_id: Some(installed_card_id.0.into()),
                target_holder: Some(card_holder_to_proto(target_holder)),
            }),
            OplogEntry::CardExpired {
                timestamp,
                card_id,
                wallet_generation,
                ..
            } => Entry::CardExpired(RawCardExpiredParameters {
                timestamp: Some(timestamp.into()),
                card_id: Some(card_id.0.into()),
                wallet_generation,
            }),
            OplogEntry::HostStreamFrame {
                parent_start_index,
                kind,
                payload,
                ..
            } => Entry::HostStreamFrame(RawHostStreamFrameParameters {
                parent_start_index: parent_start_index.as_u64(),
                kind: host_stream_kind_to_proto(kind) as i32,
                payload: Some(oplog_payload_to_proto(payload)?),
            }),
            OplogEntry::StreamRegistered {
                record, summary, ..
            } => Entry::StreamRegistered(RawDurableStreamRecordParameters {
                record: Some(oplog_payload_to_proto(record)?),
                summary: summary.map(Into::into),
            }),
            OplogEntry::ToolAttachmentControl {
                attachment,
                transition,
                ..
            } => {
                use crate::schema::IntoTypedSchemaValue as _;
                Entry::ToolAttachmentControl(
                    golem_api_grpc::proto::golem::worker::RawToolAttachmentControlParameters {
                        attachment: Some(
                            attachment
                                .into_typed_schema_value()
                                .map_err(|error| error.to_string())?
                                .try_into()?,
                        ),
                        transition: Some(
                            transition
                                .into_typed_schema_value()
                                .map_err(|error| error.to_string())?
                                .try_into()?,
                        ),
                    },
                )
            }
            OplogEntry::ToolAttachmentRead {
                observation,
                payload,
                ..
            } => {
                use crate::schema::IntoTypedSchemaValue as _;
                Entry::ToolAttachmentRead(
                    golem_api_grpc::proto::golem::worker::RawToolAttachmentReadParameters {
                        observation: Some(
                            (*observation)
                                .into_typed_schema_value()
                                .map_err(|error| error.to_string())?
                                .try_into()?,
                        ),
                        payload: payload.map(oplog_payload_to_proto).transpose()?,
                    },
                )
            }
            OplogEntry::StreamItems {
                record, summary, ..
            } => Entry::StreamItems(RawDurableStreamRecordParameters {
                record: Some(oplog_payload_to_proto(record)?),
                summary: summary.map(Into::into),
            }),
            OplogEntry::StreamEnd {
                record, summary, ..
            } => Entry::StreamEnd(RawDurableStreamRecordParameters {
                record: Some(oplog_payload_to_proto(record)?),
                summary: summary.map(Into::into),
            }),
            OplogEntry::StreamCancel {
                record, summary, ..
            } => Entry::StreamCancel(RawDurableStreamRecordParameters {
                record: Some(oplog_payload_to_proto(record)?),
                summary: summary.map(Into::into),
            }),
            OplogEntry::StreamSession {
                record, summary, ..
            } => Entry::StreamSession(RawDurableStreamRecordParameters {
                record: Some(oplog_payload_to_proto(record)?),
                summary: summary.map(Into::into),
            }),
        };

        Ok(golem_api_grpc::proto::golem::worker::RawOplogEntry {
            timestamp: Some(proto_ts),
            entity_parent_start_index,
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

        let entity_parent_start_index = match value.entity_parent_start_index {
            Some(0) => return Err("Invalid entity parent Start index: 0".to_string()),
            Some(index) => Some(OplogIndex::from_u64(index)),
            None => None,
        };
        let timestamp: crate::model::Timestamp = value
            .timestamp
            .ok_or("Missing timestamp in RawOplogEntry")?
            .into();

        match value.entry.ok_or("Missing entry in RawOplogEntry")? {
            Entry::Create(p) => {
                let agent_id = p.agent_id.ok_or("Missing agent_id")?.try_into()?;
                let owner_kind = crate::model::agent::OwnerKind::try_from(p.owner_kind)?;
                let agent_mode: AgentMode =
                    golem_api_grpc::proto::golem::component::AgentMode::try_from(p.agent_mode)
                        .map_err(|e| format!("Invalid agent_mode: {e}"))?
                        .into();
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
                    let proto_uuid: golem_schema::proto::golem::common::Uuid = u;
                    uuid::Uuid::from(proto_uuid)
                });
                let instance_id = p.instance_id.ok_or("Missing instance_id")?.into();

                Ok(OplogEntry::Create {
                    timestamp,
                    parameters: Box::new(CreateParameters {
                        agent_id,
                        owner_kind,
                        agent_mode,
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
                        instance_id,
                    }),
                })
            }
            Entry::Start(p) => {
                let function_name =
                    crate::model::oplog::payload::host_functions::HostFunctionName::from(
                        p.function_name.as_str(),
                    );
                let request = p.request.map(oplog_payload_from_proto).transpose()?;
                let durable_function_type = durable_function_type_from_proto(
                    p.durable_function_type
                        .ok_or("Missing durable_function_type")?,
                )?;
                Ok(OplogEntry::Start {
                    timestamp,
                    parent_start_index: p
                        .parent_start_index
                        .map(crate::base_model::OplogIndex::from_u64),
                    function_name,
                    invocation_id: p.invocation_id.map(Into::into),
                    observational_owner: p
                        .observational_owner
                        .map(crate::base_model::OplogIndex::from_u64),
                    request,
                    durable_function_type,
                    span_started: p
                        .span_started
                        .map(raw_span_started_from_proto)
                        .transpose()?
                        .map(Box::new),
                    denial: p.denial,
                })
            }
            Entry::End(p) => {
                let response = p.response.map(oplog_payload_from_proto).transpose()?;
                Ok(OplogEntry::End {
                    timestamp,
                    start_index: crate::base_model::OplogIndex::from_u64(p.start_index),
                    response,
                    forced_commit: p.forced_commit,
                    span_finished: p
                        .span_finished
                        .map(raw_span_finished_from_proto)
                        .transpose()?,
                    span_attributes: p
                        .span_attributes
                        .map(raw_span_attributes_from_proto)
                        .transpose()?,
                })
            }
            Entry::Cancelled(p) => {
                let partial = p.partial.map(oplog_payload_from_proto).transpose()?;
                Ok(OplogEntry::Cancelled {
                    timestamp,
                    start_index: crate::base_model::OplogIndex::from_u64(p.start_index),
                    partial,
                    span_finished: p
                        .span_finished
                        .map(raw_span_finished_from_proto)
                        .transpose()?,
                })
            }
            Entry::CompletionDiscarded(p) => Ok(OplogEntry::CompletionDiscarded {
                timestamp,
                start_index: crate::base_model::OplogIndex::from_u64(p.start_index),
            }),
            Entry::CompletionDelivered(p) => Ok(OplogEntry::CompletionDelivered {
                timestamp,
                start_index: crate::base_model::OplogIndex::from_u64(p.start_index),
            }),
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
                    wallet_pin: Box::new(invocation_wallet_pin_from_proto(
                        p.wallet_pin.ok_or("Missing wallet_pin")?,
                    )?),
                })
            }
            Entry::AgentInvocationFinished(p) => {
                let result = oplog_payload_from_proto(p.result.ok_or("Missing result payload")?)?;
                let component_revision: crate::model::component::ComponentRevision =
                    p.component_revision.try_into().map_err(|e: String| e)?;
                Ok(OplogEntry::AgentInvocationFinished {
                    timestamp,
                    result,
                    method_name: p.method_name,
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
                    entity_parent_start_index,
                    kind: oplog_error_kind_from_proto(p.kind)?,
                    error,
                    retry_from,
                    retry_policy_state,
                })
            }
            Entry::RecoverySucceeded(_) => Ok(OplogEntry::RecoverySucceeded { timestamp }),
            Entry::Jump(p) => {
                let jump = p.jump.ok_or("Missing jump region")?;
                Ok(OplogEntry::Jump {
                    timestamp,
                    entity_parent_start_index,
                    jump: crate::model::regions::OplogRegion {
                        start: OplogIndex::from_u64(jump.start),
                        end: OplogIndex::from_u64(jump.end),
                    },
                })
            }
            Entry::Interrupted(_) => Ok(OplogEntry::Interrupted { timestamp }),
            Entry::Exited(_) => Ok(OplogEntry::Exited { timestamp }),
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
                    update_attempt_index: p.update_attempt_index.map(OplogIndex::from_u64),
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
                    new_total_linear_memory_size: p.new_total_linear_memory_size,
                    new_active_plugins,
                    snapshot_assisted_details: p
                        .snapshot_assisted_details
                        .map(|details| {
                            Ok::<_, String>(SnapshotAssistedUpdateDetails {
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
                })
            }
            Entry::FailedUpdate(p) => {
                let target_revision: crate::model::component::ComponentRevision =
                    p.target_revision.try_into().map_err(|e: String| e)?;
                Ok(OplogEntry::FailedUpdate {
                    timestamp,
                    target_revision,
                    details: p.details,
                    snapshot_assisted_details: p
                        .snapshot_assisted_details
                        .map(|details| {
                            Ok::<_, String>(FailedSnapshotAssistedUpdateDetails {
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
                    update_attempt_index: p.update_attempt_index.map(OplogIndex::from_u64),
                    snapshot_fault: p
                        .snapshot_fault
                        .map(raw_snapshot_fault_from_proto)
                        .transpose()?,
                })
            }
            Entry::GrowMemory(p) => Ok(OplogEntry::GrowMemory {
                timestamp,
                delta: p.delta,
            }),
            Entry::CreateResource(p) => {
                let rt = p.resource_type_id.ok_or("Missing resource_type_id")?;
                Ok(OplogEntry::CreateResource {
                    timestamp,
                    entity_parent_start_index,
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
                    entity_parent_start_index,
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
                    parent_start_index: p.parent_start_index.map(OplogIndex::from_u64),
                    level,
                    context: p.context,
                    message: p.message,
                    trace_context: p.trace_context.map(TryInto::try_into).transpose()?,
                })
            }
            Entry::Restart(_) => Ok(OplogEntry::Restart { timestamp }),
            Entry::Resumed(_) => Ok(OplogEntry::Resumed { timestamp }),
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
            Entry::BeginRemoteTransaction(p) => {
                let transaction_id = crate::model::TransactionId::from(p.transaction_id);
                let original_begin_index = OplogIndex::from_u64(p.original_begin_index);
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
                    active_cards: p
                        .active_cards
                        .into_iter()
                        .map(|card| deserialize_stored_card(&card, "active card"))
                        .collect::<Result<Vec<_>, _>>()?,
                    wallet_generation: p.wallet_generation,
                    filesystem_snapshot: p
                        .filesystem_snapshot
                        .map(|name| name.parse())
                        .transpose()?,
                })
            }
            Entry::SnapshotConfirmed(p) => Ok(OplogEntry::SnapshotConfirmed {
                timestamp,
                filesystem_snapshot: p.filesystem_snapshot.parse()?,
            }),
            Entry::SpanClosed(p) => Ok(OplogEntry::SpanClosed {
                timestamp,
                entity_parent_start_index,
                resource_start_index: p
                    .resource_start_index
                    .map(crate::base_model::OplogIndex::from_u64),
                span_finished: raw_span_finished_from_proto(
                    p.span_finished.ok_or("Missing span_finished")?,
                )?,
            }),
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
                Ok(OplogEntry::SetRetryPolicy {
                    timestamp,
                    entity_parent_start_index,
                    policy: Box::new(policy),
                })
            }
            Entry::RemoveRetryPolicy(p) => Ok(OplogEntry::RemoveRetryPolicy {
                timestamp,
                entity_parent_start_index,
                name: p.name,
            }),
            Entry::CardRevoked(p) => Ok(OplogEntry::CardRevoked {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                queued_event_index: OplogIndex::from_u64(p.queued_event_index),
                card_id: CardId(p.card_id.ok_or("Missing card_id")?.into()),
                wallet_generation: p.wallet_generation,
            }),
            Entry::CardEventQueued(p) => Ok(OplogEntry::CardEventQueued {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                event: Box::new(raw_queued_card_event_from_proto(
                    p.event.ok_or("Missing event")?,
                )?),
            }),
            Entry::CardInstalled(p) => Ok(OplogEntry::CardInstalled {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                queued_event_index: p.queued_event_index.map(OplogIndex::from_u64),
                card: Box::new(deserialize_stored_card(&p.card, "installed card")?),
                wallet_generation: p.wallet_generation,
            }),
            Entry::CardInstallFailed(p) => Ok(OplogEntry::CardInstallFailed {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                queued_event_index: OplogIndex::from_u64(p.queued_event_index),
                card_id: CardId(p.card_id.ok_or("Missing card_id")?.into()),
                reason: raw_card_install_failure_from_proto(
                    golem_api_grpc::proto::golem::worker::RawCardInstallFailure::try_from(p.reason)
                        .map_err(|e| format!("Invalid raw card install failure: {e}"))?,
                )?,
            }),
            Entry::CardDerived(p) => Ok(OplogEntry::CardDerived {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                card: Box::new(deserialize_stored_card(&p.card, "derived card")?),
                wallet_generation: p.wallet_generation,
                transfer_id: p.transfer_id.map(Into::into),
            }),
            Entry::CardTransferStarted(p) => Ok(OplogEntry::CardTransferStarted {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                transfer_id: p.transfer_id.ok_or("Missing transfer_id")?.into(),
                card_id: CardId(p.card_id.ok_or("Missing card_id")?.into()),
                source_holder: card_holder_from_proto(
                    p.source_holder.ok_or("Missing source_holder")?,
                )?,
                target_holder: card_holder_from_proto(
                    p.target_holder.ok_or("Missing target_holder")?,
                )?,
                source_wallet_generation: p.source_wallet_generation,
            }),
            Entry::CardTransferred(p) => {
                let installed_card_id = CardId(
                    p.installed_card_id
                        .ok_or("Missing installed_card_id")?
                        .into(),
                );
                let card = deserialize_stored_card(&p.card, "transferred card")?;
                if card.card_id() != installed_card_id {
                    return Err(
                        "Transferred card payload does not match installed_card_id".to_string()
                    );
                }
                Ok(OplogEntry::CardTransferred {
                    timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                    entity_parent_start_index,
                    transfer_id: p.transfer_id.ok_or("Missing transfer_id")?.into(),
                    source_card_id: CardId(
                        p.source_card_id.ok_or("Missing source_card_id")?.into(),
                    ),
                    installed_card_id,
                    target_holder: card_holder_from_proto(
                        p.target_holder.ok_or("Missing target_holder")?,
                    )?,
                    card: Box::new(card),
                    target_wallet_generation: p.target_wallet_generation,
                })
            }
            Entry::CardRevokedCascade(p) => Ok(OplogEntry::CardRevokedCascade {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                revoked_card_ids: p
                    .revoked_card_ids
                    .into_iter()
                    .map(|id| CardId(id.into()))
                    .collect(),
                affected_wallets: p
                    .affected_wallets
                    .into_iter()
                    .map(card_holder_from_proto)
                    .collect::<Result<Vec<_>, _>>()?,
                local_wallet_generation: p.local_wallet_generation,
            }),
            Entry::CardTransferConfirmed(p) => Ok(OplogEntry::CardTransferConfirmed {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                transfer_id: p.transfer_id.ok_or("Missing transfer_id")?.into(),
                source_card_id: CardId(p.source_card_id.ok_or("Missing source_card_id")?.into()),
                installed_card_id: CardId(
                    p.installed_card_id
                        .ok_or("Missing installed_card_id")?
                        .into(),
                ),
                target_holder: card_holder_from_proto(
                    p.target_holder.ok_or("Missing target_holder")?,
                )?,
            }),
            Entry::CardExpired(p) => Ok(OplogEntry::CardExpired {
                timestamp: p.timestamp.map(Into::into).unwrap_or(timestamp),
                entity_parent_start_index,
                card_id: CardId(p.card_id.ok_or("Missing card_id")?.into()),
                wallet_generation: p.wallet_generation,
            }),
            Entry::HostStreamFrame(p) => Ok(OplogEntry::HostStreamFrame {
                timestamp,
                parent_start_index: crate::base_model::OplogIndex::from_u64(p.parent_start_index),
                kind: host_stream_kind_from_proto(p.kind)?,
                payload: oplog_payload_from_proto(p.payload.ok_or("Missing payload")?)?,
            }),
            Entry::StreamRegistered(p) => Ok(OplogEntry::StreamRegistered {
                timestamp,
                entity_parent_start_index,
                summary: p.summary.map(TryInto::try_into).transpose()?,
                record: oplog_payload_from_proto(p.record.ok_or("Missing record")?)?,
            }),
            Entry::ToolAttachmentControl(p) => {
                let (attachment, transition) = decode_attachment_control(
                    &p.attachment.ok_or("Missing attachment")?.try_into()?,
                    &p.transition.ok_or("Missing transition")?.try_into()?,
                )?;
                Ok(OplogEntry::ToolAttachmentControl {
                    timestamp,
                    entity_parent_start_index,
                    attachment,
                    transition,
                })
            }
            Entry::ToolAttachmentRead(p) => {
                let observation = decode_attachment_observation(
                    &p.observation.ok_or("Missing observation")?.try_into()?,
                )?;
                observation
                    .validate_payload_presence(p.payload.is_some())
                    .map_err(|error| format!("Invalid attachment read payload: {error:?}"))?;
                Ok(OplogEntry::ToolAttachmentRead {
                    timestamp,
                    entity_parent_start_index,
                    observation: Box::new(observation),
                    payload: p.payload.map(oplog_payload_from_proto).transpose()?,
                })
            }
            Entry::StreamItems(p) => Ok(OplogEntry::StreamItems {
                timestamp,
                entity_parent_start_index,
                summary: p.summary.map(TryInto::try_into).transpose()?,
                record: oplog_payload_from_proto(p.record.ok_or("Missing record")?)?,
            }),
            Entry::StreamEnd(p) => Ok(OplogEntry::StreamEnd {
                timestamp,
                entity_parent_start_index,
                summary: p.summary.map(TryInto::try_into).transpose()?,
                record: oplog_payload_from_proto(p.record.ok_or("Missing record")?)?,
            }),
            Entry::StreamCancel(p) => Ok(OplogEntry::StreamCancel {
                timestamp,
                entity_parent_start_index,
                summary: p.summary.map(TryInto::try_into).transpose()?,
                record: oplog_payload_from_proto(p.record.ok_or("Missing record")?)?,
            }),
            Entry::StreamSession(p) => Ok(OplogEntry::StreamSession {
                timestamp,
                entity_parent_start_index,
                summary: p.summary.map(TryInto::try_into).transpose()?,
                record: oplog_payload_from_proto(p.record.ok_or("Missing record")?)?,
            }),
        }
    }
}
