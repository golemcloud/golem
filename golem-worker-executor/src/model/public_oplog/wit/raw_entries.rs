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

use super::*;
use golem_common::model::oplog::OplogEntry as M;

golem_common::oplog_entry_metadata!(generate_entry_conversion;
from (oplog::OplogEntry => golem_common::model::oplog::OplogEntry)
before (value) {}
{
    Create { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        parameters: Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: golem_common::model::AgentId::from(params.agent_id),
            owner_kind: match params.owner_kind {
                oplog::OwnerKind::ComponentAgent => {
                    golem_common::model::agent::OwnerKind::ComponentAgent
                }
                oplog::OwnerKind::EphemeralExternalTool => {
                    golem_common::model::agent::OwnerKind::EphemeralExternalTool
                }
            },
            agent_mode: match params.agent_mode {
                oplog::AgentMode::Durable => golem_common::model::agent::AgentMode::Durable,
                oplog::AgentMode::Ephemeral => {
                    golem_common::model::agent::AgentMode::Ephemeral
                }
            },
            component_revision:
                golem_common::model::component::ComponentRevision::try_from(
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
                Ok(golem_common::model::agent::UntypedAgentConfigEntry {
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
    } }
    Start { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        parent_start_index: params
        .parent_start_index
        .map(golem_common::base_model::OplogIndex::from_u64),
        function_name: golem_common::model::oplog::payload::host_functions::HostFunctionName::from(
            params.function_name.as_str(),
        ),
        invocation_id: params.invocation_id.map(durable_invocation_id_from_wit),
        observational_owner: params
        .observational_owner
        .map(golem_common::base_model::OplogIndex::from_u64),
        request: params.request.map(oplog_payload_from_wit),
        durable_function_type: params.durable_function_type.into(),
        span_started: params.span_started.map(|span| raw_span_started_from_wit(span).map(Box::new)).transpose()?,
        denial: params.denial,
    } }
    End { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
        response: params.response.map(oplog_payload_from_wit),
        forced_commit: params.forced_commit,
        span_finished: params.span_finished.map(raw_span_finished_from_wit).transpose()?,
        span_attributes: params.span_attributes.map(raw_span_attributes_from_wit).transpose()?,
    } }
    Cancelled { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
        partial: params.partial.map(oplog_payload_from_wit),
        span_finished: params.span_finished.map(raw_span_finished_from_wit).transpose()?,
    } }
    AgentInvocationStarted { override (oplog::OplogEntry::AgentInvocationStarted(params)) => {
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
                        wallet_pin: Box::new(invocation_wallet_pin_from_wit(params.wallet_pin)?),
                    })
                } }
    AgentInvocationFinished { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        result: oplog_payload_from_wit(params.result),
        method_name: params.method_name,
        consumed_fuel: params.consumed_fuel,
        component_revision: params
        .component_revision
        .try_into()
        .map_err(|e: String| e)?,
    } }
    Suspend { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    Error { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        kind: params.kind.into(),
        error: params.error.into(),
        retry_from: golem_common::model::OplogIndex::from_u64(params.retry_from),
        retry_policy_state: params.retry_policy_state.map(|s| {
            let internal: golem_common::model::RetryPolicyState = s.into();
            internal
        }),
    } }
    RecoverySucceeded { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    Jump { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        jump: golem_common::model::regions::OplogRegion {
            start: golem_common::model::OplogIndex::from_u64(params.jump.start),
            end: golem_common::model::OplogIndex::from_u64(params.jump.end),
        },
    } }
    Interrupted { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    Exited { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    PendingAgentInvocation { override (oplog::OplogEntry::PendingAgentInvocation(params)) => {
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
                } }
    PendingUpdate { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        description: params.description.try_into()?,
        update_attempt_index: params
        .update_attempt_index
        .map(golem_common::model::oplog::OplogIndex::from_u64),
    } }
    SuccessfulUpdate { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        target_revision: golem_common::model::component::ComponentRevision::try_from(
            params.target_revision,
        )
        .map_err(|e| e.to_string())?,
        new_component_size: params.new_component_size,
        new_total_linear_memory_size: None,
        new_active_plugins: params
        .new_active_plugins
        .into_iter()
        .map(|v| golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(uuid::Uuid::from_u64_pair(v.uuid.high_bits, v.uuid.low_bits)))
        .collect(),
        snapshot_assisted_details: params
        .snapshot_assisted_details
        .map(|details| {
            Ok::<_, String>(
                golem_common::model::oplog::SnapshotAssistedUpdateDetails {
                    pending_update_index: golem_common::model::oplog::OplogIndex::from_u64(
                        details.pending_update_index,
                    ),
                    source_component_revision: details
                        .source_component_revision
                        .try_into()?,
                    source_revision_start_index: golem_common::model::oplog::OplogIndex::from_u64(
                        details.source_revision_start_index,
                    ),
                    snapshot_index: golem_common::model::oplog::OplogIndex::from_u64(details.snapshot_index),
                },
            )
        })
        .transpose()?,
    } }
    FailedUpdate { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        target_revision: golem_common::model::component::ComponentRevision::try_from(
            params.target_revision,
        )
        .map_err(|e| e.to_string())?,
        details: params.details,
        update_attempt_index: params
        .update_attempt_index
        .map(golem_common::model::oplog::OplogIndex::from_u64),
        snapshot_assisted_details: params
            .snapshot_assisted_details
            .map(|details| {
                Ok::<_, String>(golem_common::model::oplog::FailedSnapshotAssistedUpdateDetails {
                    pending_update_index: golem_common::model::oplog::OplogIndex::from_u64(details.pending_update_index),
                    source_component_revision: golem_common::model::component::ComponentRevision::try_from(details.source_component_revision)
                        .map_err(|e| e.to_string())?,
                    source_revision_start_index: golem_common::model::oplog::OplogIndex::from_u64(details.source_revision_start_index),
                    snapshot_index: golem_common::model::oplog::OplogIndex::from_u64(details.snapshot_index),
                })
            })
            .transpose()?,
        // The WIT record of a failed update has no snapshot fault.
        snapshot_fault: None,
    } }
    GrowMemory { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        delta: params.delta,
    } }
    CreateResource { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        id: golem_common::model::oplog::AgentResourceId(params.id),
        resource_type_id: golem_common::resource_runtime::ResourceTypeId {
            name: params.resource_type_id.name,
            owner: params.resource_type_id.owner,
        },
    } }
    DropResource { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        id: golem_common::model::oplog::AgentResourceId(params.id),
        resource_type_id: golem_common::resource_runtime::ResourceTypeId {
            name: params.resource_type_id.name,
            owner: params.resource_type_id.owner,
        },
    } }
    Log { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        parent_start_index: None,
        level: params.level.into(),
        context: params.context,
        message: params.message,
        trace_context: params
        .trace_context
        .map(log_trace_context_from_wit)
        .transpose()?,
    } }
    Restart { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    Resumed { record (ts) {
        timestamp: timestamp_from_datetime(ts.timestamp),
    } }
    ActivatePlugin { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        plugin_grant_id: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(
            uuid::Uuid::from_u64_pair(params.plugin_grant_id.uuid.high_bits, params.plugin_grant_id.uuid.low_bits),
        ),
    } }
    DeactivatePlugin { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        plugin_grant_id: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId(
            uuid::Uuid::from_u64_pair(params.plugin_grant_id.uuid.high_bits, params.plugin_grant_id.uuid.low_bits),
        ),
    } }
    Revert { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        dropped_region: golem_common::model::regions::OplogRegion {
            start: golem_common::model::OplogIndex::from_u64(params.dropped_region.start),
            end: golem_common::model::OplogIndex::from_u64(params.dropped_region.end),
        },
    } }
    CancelPendingInvocation { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        idempotency_key: golem_common::model::IdempotencyKey::new(
            params.idempotency_key,
        ),
    } }
    BeginRemoteTransaction { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        transaction_id: golem_common::model::TransactionId::from(params.transaction_id),
        original_begin_index: golem_common::model::OplogIndex::from_u64(params.original_begin_index),
    } }
    PreCommitRemoteTransaction { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
    } }
    PreRollbackRemoteTransaction { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
    } }
    CommittedRemoteTransaction { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
    } }
    RolledBackRemoteTransaction { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        begin_index: golem_common::model::OplogIndex::from_u64(params.begin_index),
    } }
    Snapshot { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        data: oplog_payload_from_wit(params.data),
        mime_type: params.mime_type,
        active_cards: params
        .active_cards
        .into_iter()
        .map(|card| serde_json::from_slice(&card).map_err(|err| err.to_string()))
        .collect::<Result<Vec<_>, _>>()?,
        wallet_generation: params.wallet_generation,
        filesystem_snapshot: params
        .filesystem_snapshot
        .map(|name| name.parse())
        .transpose()?,
    } }
    OplogProcessorCheckpoint { record (params) {
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
    } }
    SetRetryPolicy { override (oplog::OplogEntry::SetRetryPolicy(params)) => {
                    let named: golem_common::model::retry_policy::NamedRetryPolicy =
                        params.policy.try_into()?;
                    Ok(Self::SetRetryPolicy {
                        timestamp: timestamp_from_datetime(params.timestamp),
                        entity_parent_start_index: None,
                        policy: Box::new(named),
                    })
                } }
    RemoveRetryPolicy { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        name: params.name,
    } }
    CardEventQueued { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        event: Box::new(raw_queued_card_event_from_wit(params.event)?),
    } }
    CardInstalled { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        queued_event_index: params
        .queued_event_index
        .map(golem_common::model::OplogIndex::from_u64),
        card: serde_json::from_slice(&params.card).map_err(|err| err.to_string())?,
        wallet_generation: params.wallet_generation,
    } }
    CardInstallFailed { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        queued_event_index: golem_common::model::OplogIndex::from_u64(
            params.queued_event_index,
        ),
        card_id: card_id_from_wit(params.card_id),
        reason: card_install_failure_from_wit(params.reason),
    } }
    CardRevoked { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        queued_event_index: golem_common::model::OplogIndex::from_u64(
            params.queued_event_index,
        ),
        card_id: card_id_from_wit(params.card_id),
        wallet_generation: params.wallet_generation,
    } }
    CardExpired { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        card_id: card_id_from_wit(params.card_id),
        wallet_generation: params.wallet_generation,
    } }
    CardDerived { absent }
    CardTransferStarted { absent }
    CardTransferred { absent }
    CardRevokedCascade { absent }
    CardTransferConfirmed { absent }
    HostStreamFrame { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        parent_start_index: golem_common::base_model::OplogIndex::from_u64(
            params.parent_start_index,
        ),
        kind: params.kind.into(),
        payload: oplog_payload_from_wit(params.payload),
    } }
    ToolAttachmentRead { override (oplog::OplogEntry::ToolAttachmentRead(params)) => {
                    use golem_common::schema::FromSchema as _;
                    let observation = golem_schema::schema::wit::decode_typed(&params.observation).map_err(|error| error.to_string())?;
                    let observation = golem_common::base_model::oplog::attachment::ToolAttachmentReadObservation::from_value(observation.value()).map_err(|error| error.to_string())?;
                    observation.validate_payload_presence(params.payload.is_some())
                        .map_err(|error| format!("Invalid attachment read payload: {error:?}"))?;
                    Ok(Self::ToolAttachmentRead {
                        timestamp: timestamp_from_datetime(params.timestamp),
                        entity_parent_start_index: params.entity_parent_start_index.map(golem_common::base_model::OplogIndex::from_u64),
                        observation: Box::new(observation),
                        payload: params.payload.map(oplog_payload_from_wit),
                    })
                } }
    ToolAttachmentControl { override (oplog::OplogEntry::ToolAttachmentControl(params)) => {
                    use golem_common::schema::FromSchema as _;
                    use golem_common::base_model::oplog::attachment::{ToolAttachmentId, ToolAttachmentControl};
                    let attachment = golem_schema::schema::wit::decode_typed(&params.attachment).map_err(|error| error.to_string())?;
                    let transition = golem_schema::schema::wit::decode_typed(&params.transition).map_err(|error| error.to_string())?;
                    Ok(Self::ToolAttachmentControl {
                        timestamp: timestamp_from_datetime(params.timestamp),
                        entity_parent_start_index: params.entity_parent_start_index.map(golem_common::base_model::OplogIndex::from_u64),
                        attachment: ToolAttachmentId::from_value(attachment.value()).map_err(|error| error.to_string())?,
                        transition: ToolAttachmentControl::from_value(transition.value()).map_err(|error| error.to_string())?,
                    })
                } }
    StreamRegistered { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        record: oplog_payload_from_wit(params.record),
        summary: params.summary.map(durable_stream_summary_from_wit),
    } }
    StreamItems { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        record: oplog_payload_from_wit(params.record),
        summary: params.summary.map(durable_stream_summary_from_wit),
    } }
    StreamEnd { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        record: oplog_payload_from_wit(params.record),
        summary: params.summary.map(durable_stream_summary_from_wit),
    } }
    StreamCancel { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        record: oplog_payload_from_wit(params.record),
        summary: params.summary.map(durable_stream_summary_from_wit),
    } }
    StreamSession { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        entity_parent_start_index: None,
        record: oplog_payload_from_wit(params.record),
        summary: params.summary.map(durable_stream_summary_from_wit),
    } }
    CompletionDiscarded { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
    } }
    CompletionDelivered { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        start_index: golem_common::base_model::OplogIndex::from_u64(params.start_index),
    } }
    SnapshotConfirmed { record (params) {
        timestamp: timestamp_from_datetime(params.timestamp),
        filesystem_snapshot: params.filesystem_snapshot.parse()?,
    } }
});

golem_common::oplog_entry_metadata!(generate_entry_conversion;
raw (golem_common::model::oplog::OplogEntry => oplog::OplogEntry)
before (value) {}
{
    Create { override (M::Create {
                    timestamp,
                    parameters,
                }) => {
                    let golem_common::model::oplog::CreateParameters {
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
                    Ok(Self::Create(oplog::RawCreateParameters {
                        timestamp: timestamp.into(),
                        agent_id: agent_id.into(),
                        owner_kind: match owner_kind {
                            golem_common::model::agent::OwnerKind::ComponentAgent => {
                                oplog::OwnerKind::ComponentAgent
                            }
                            golem_common::model::agent::OwnerKind::EphemeralExternalTool => {
                                oplog::OwnerKind::EphemeralExternalTool
                            }
                        },
                        agent_mode: match agent_mode {
                            golem_common::model::agent::AgentMode::Durable => oplog::AgentMode::Durable,
                            golem_common::model::agent::AgentMode::Ephemeral => {
                                oplog::AgentMode::Ephemeral
                            }
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
                    }))
                } }
    Start { record () => RawStartParameters {
        timestamp: timestamp.into(),
        parent_start_index: parent_start_index.map(|i| i.into()),
        function_name: function_name.to_string(),
        invocation_id: invocation_id.map(durable_invocation_id_to_wit),
        observational_owner: observational_owner.map(|i| i.into()),
        request: request.map(oplog_payload_to_wit).transpose()?,
        durable_function_type: durable_function_type.into(),
        span_started: span_started.map(|span| raw_span_started_to_wit(*span)),
        denial: denial,
    } }
    End { record () => RawEndParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
        response: response.map(oplog_payload_to_wit).transpose()?,
        forced_commit: forced_commit,
        span_finished: span_finished.map(raw_span_finished_to_wit),
        span_attributes: span_attributes.map(raw_span_attributes_to_wit),
    } }
    Cancelled { record () => RawCancelledParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
        partial: partial.map(oplog_payload_to_wit).transpose()?,
        span_finished: span_finished.map(raw_span_finished_to_wit),
    } }
    AgentInvocationStarted { record () => RawAgentInvocationStartedParameters {
        timestamp: timestamp.into(),
        idempotency_key: idempotency_key.value,
        payload: oplog_payload_to_wit(payload)?,
        trace_id: trace_id.to_string(),
        trace_states: trace_states,
        invocation_context: invocation_context.into_iter().map(|s| s.into()).collect(),
        wallet_pin: invocation_wallet_pin_to_wit(*wallet_pin),
    } }
    AgentInvocationFinished { record () => RawAgentInvocationFinishedParameters {
        timestamp: timestamp.into(),
        result: oplog_payload_to_wit(result)?,
        method_name: method_name,
        consumed_fuel: consumed_fuel,
        component_revision: component_revision.get(),
    } }
    Suspend { override (M::Suspend { timestamp }) => Ok(Self::Suspend(timestamp.into())) }
    Error { record (entity_parent_start_index) => RawErrorParameters {
        timestamp: timestamp.into(),
        kind: kind.into(),
        error: error.into(),
        retry_from: retry_from.into(),
        retry_policy_state: retry_policy_state.map(|s| s.into()),
    } }
    RecoverySucceeded { override (M::RecoverySucceeded { timestamp }) => Ok(Self::RecoverySucceeded(timestamp.into())) }
    Jump { record (entity_parent_start_index) => JumpParameters {
        timestamp: timestamp.into(),
        jump: oplog::OplogRegion {
            start: jump.start.into(),
            end: jump.end.into(),
        },
    } }
    Interrupted { override (M::Interrupted { timestamp }) => Ok(Self::Interrupted(timestamp.into())) }
    Exited { override (M::Exited { timestamp }) => Ok(Self::Exited(timestamp.into())) }
    PendingAgentInvocation { record () => RawPendingAgentInvocationParameters {
        timestamp: timestamp.into(),
        idempotency_key: idempotency_key.value,
        payload: oplog_payload_to_wit(payload)?,
        trace_id: trace_id.to_string(),
        trace_states: trace_states,
        invocation_context: invocation_context.into_iter().map(|s| s.into()).collect(),
    } }
    PendingUpdate { record () => RawPendingUpdateParameters {
        timestamp: timestamp.into(),
        description: description.try_into()?,
        update_attempt_index: update_attempt_index.map(Into::into),
    } }
    SuccessfulUpdate { record (new_total_linear_memory_size) => RawSuccessfulUpdateParameters {
        timestamp: timestamp.into(),
        target_revision: target_revision.into(),
        new_component_size: new_component_size,
        new_active_plugins: new_active_plugins.into_iter().map(|g| g.into()).collect(),
        snapshot_assisted_details: snapshot_assisted_details.map(|details| {
            oplog::SnapshotAssistedUpdateDetails {
                pending_update_index: details.pending_update_index.into(),
                source_component_revision: details.source_component_revision.into(),
                source_revision_start_index: details.source_revision_start_index.into(),
                snapshot_index: details.snapshot_index.into(),
            }
        }),
    } }
    FailedUpdate { record (snapshot_fault) => FailedUpdateParameters {
        timestamp: timestamp.into(),
        target_revision: target_revision.into(),
        details: details,
        update_attempt_index: update_attempt_index.map(Into::into),
        snapshot_assisted_details: snapshot_assisted_details.map(|details| {
            oplog::FailedSnapshotAssistedUpdateDetails {
                pending_update_index: details.pending_update_index.into(),
                source_component_revision: details.source_component_revision.into(),
                source_revision_start_index: details.source_revision_start_index.into(),
                snapshot_index: details.snapshot_index.into(),
            }
        }),
    } }
    GrowMemory { record () => GrowMemoryParameters {
        timestamp: timestamp.into(),
        delta: delta,
    } }
    CreateResource { record (entity_parent_start_index) => RawCreateResourceParameters {
        timestamp: timestamp.into(),
        id: id.0,
        resource_type_id: oplog::ResourceTypeId {
            name: resource_type_id.name,
            owner: resource_type_id.owner,
        },
    } }
    DropResource { record (entity_parent_start_index) => RawDropResourceParameters {
        timestamp: timestamp.into(),
        id: id.0,
        resource_type_id: oplog::ResourceTypeId {
            name: resource_type_id.name,
            owner: resource_type_id.owner,
        },
    } }
    Log { record (parent_start_index) => LogParameters {
        timestamp: timestamp.into(),
        level: level.into(),
        context: context,
        message: message,
        trace_context: trace_context.map(log_trace_context_to_wit),
    } }
    Restart { override (M::Restart { timestamp }) => Ok(Self::Restart(timestamp.into())) }
    Resumed { override (M::Resumed { timestamp }) => Ok(Self::Resumed(timestamp.into())) }
    ActivatePlugin { record () => RawActivatePluginParameters {
        timestamp: timestamp.into(),
        plugin_grant_id: plugin_grant_id.into(),
    } }
    DeactivatePlugin { record () => RawDeactivatePluginParameters {
        timestamp: timestamp.into(),
        plugin_grant_id: plugin_grant_id.into(),
    } }
    Revert { record () => RevertParameters {
        timestamp: timestamp.into(),
        dropped_region: oplog::OplogRegion {
            start: dropped_region.start.into(),
            end: dropped_region.end.into(),
        },
    } }
    CancelPendingInvocation { record () => CancelPendingInvocationParameters {
        timestamp: timestamp.into(),
        idempotency_key: idempotency_key.value,
    } }
    BeginRemoteTransaction { record () => RawBeginRemoteTransactionParameters {
        timestamp: timestamp.into(),
        transaction_id: transaction_id.to_string(),
        original_begin_index: original_begin_index.into(),
    } }
    PreCommitRemoteTransaction { record () => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    PreRollbackRemoteTransaction { record () => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    CommittedRemoteTransaction { record () => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    RolledBackRemoteTransaction { record () => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    Snapshot { record () => RawSnapshotParameters {
        timestamp: timestamp.into(),
        data: oplog_payload_to_wit(data)?,
        mime_type: mime_type,
        active_cards: active_cards
        .into_iter()
        .map(|card| serde_json::to_vec(&card).map_err(|err| err.to_string()))
        .collect::<Result<Vec<_>, _>>()?,
        wallet_generation: wallet_generation,
        filesystem_snapshot: filesystem_snapshot.map(String::from),
    } }
    OplogProcessorCheckpoint { record () => RawOplogProcessorCheckpointParameters {
        timestamp: timestamp.into(),
        plugin_grant_id: plugin_grant_id.into(),
        target_agent_id: target_agent_id.into(),
        confirmed_up_to: confirmed_up_to.into(),
        sending_up_to: sending_up_to.into(),
        last_batch_start: last_batch_start.into(),
    } }
    SetRetryPolicy { record (entity_parent_start_index) => SetRetryPolicyParameters {
        timestamp: timestamp.into(),
        policy: (*policy).into(),
    } }
    RemoveRetryPolicy { record (entity_parent_start_index) => RemoveRetryPolicyParameters {
        timestamp: timestamp.into(),
        name: name,
    } }
    CardEventQueued { record (entity_parent_start_index) => CardEventQueuedParameters {
        timestamp: timestamp.into(),
        event: raw_queued_card_event_to_wit(*event)?,
    } }
    CardInstalled { record (entity_parent_start_index) => RawCardInstalledParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.map(Into::into),
        card: serde_json::to_vec(&card).map_err(|err| err.to_string())?,
        wallet_generation: wallet_generation,
    } }
    CardInstallFailed { record (entity_parent_start_index) => CardInstallFailedParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.into(),
        card_id: card_id_to_wit(card_id),
        reason: card_install_failure_to_wit(reason),
    } }
    CardRevoked { record (entity_parent_start_index) => CardRevokedParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.into(),
        card_id: card_id_to_wit(card_id),
        wallet_generation: wallet_generation,
    } }
    CardExpired { record (entity_parent_start_index) => CardExpiredParameters {
        timestamp: timestamp.into(),
        card_id: card_id_to_wit(card_id),
        wallet_generation: wallet_generation,
    } }
    CardDerived { override (M::CardDerived { .. }) => Err(
                    "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                        .to_string(),
                ) }
    CardTransferStarted { override (M::CardTransferStarted { .. }) => Err(
                    "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                        .to_string(),
                ) }
    CardTransferred { override (M::CardTransferred { .. }) => Err(
                    "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                        .to_string(),
                ) }
    CardRevokedCascade { override (M::CardRevokedCascade { .. }) => Err(
                    "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                        .to_string(),
                ) }
    CardTransferConfirmed { override (M::CardTransferConfirmed { .. }) => Err(
                    "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                        .to_string(),
                ) }
    HostStreamFrame { record () => RawHostStreamFrameParameters {
        timestamp: timestamp.into(),
        parent_start_index: parent_start_index.into(),
        kind: kind.into(),
        payload: oplog_payload_to_wit(payload)?,
    } }
    ToolAttachmentRead { override (M::ToolAttachmentRead {
                    timestamp,
                    entity_parent_start_index,
                    observation,
                    payload,
                }) => {
                    use golem_common::schema::IntoTypedSchemaValue as _;
                    Ok(Self::ToolAttachmentRead(
                        oplog::RawToolAttachmentReadParameters {
                            timestamp: timestamp.into(),
                            entity_parent_start_index: entity_parent_start_index.map(Into::into),
                            observation: encode_public_typed_schema_value(
                                (*observation)
                                    .into_typed_schema_value()
                                    .map_err(|error| error.to_string())?,
                            )?,
                            payload: payload.map(oplog_payload_to_wit).transpose()?,
                        },
                    ))
                } }
    ToolAttachmentControl { override (M::ToolAttachmentControl {
                    timestamp,
                    entity_parent_start_index,
                    attachment,
                    transition,
                }) => {
                    use golem_common::schema::IntoTypedSchemaValue as _;
                    Ok(Self::ToolAttachmentControl(
                        oplog::RawToolAttachmentControlParameters {
                            timestamp: timestamp.into(),
                            entity_parent_start_index: entity_parent_start_index.map(Into::into),
                            attachment: encode_public_typed_schema_value(
                                attachment
                                    .into_typed_schema_value()
                                    .map_err(|error| error.to_string())?,
                            )?,
                            transition: encode_public_typed_schema_value(
                                transition
                                    .into_typed_schema_value()
                                    .map_err(|error| error.to_string())?,
                            )?,
                        },
                    ))
                } }
    StreamRegistered { record (entity_parent_start_index) => RawDurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: oplog_payload_to_wit(record)?,
        summary: summary.map(durable_stream_summary_to_wit),
    } }
    StreamItems { record (entity_parent_start_index) => RawDurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: oplog_payload_to_wit(record)?,
        summary: summary.map(durable_stream_summary_to_wit),
    } }
    StreamEnd { record (entity_parent_start_index) => RawDurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: oplog_payload_to_wit(record)?,
        summary: summary.map(durable_stream_summary_to_wit),
    } }
    StreamCancel { record (entity_parent_start_index) => RawDurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: oplog_payload_to_wit(record)?,
        summary: summary.map(durable_stream_summary_to_wit),
    } }
    StreamSession { record (entity_parent_start_index) => RawDurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: oplog_payload_to_wit(record)?,
        summary: summary.map(durable_stream_summary_to_wit),
    } }
    CompletionDiscarded { record () => RawCompletionDiscardedParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
    } }
    CompletionDelivered { record () => RawCompletionDeliveredParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
    } }
    SnapshotConfirmed { record () => SnapshotConfirmedParameters {
        timestamp: timestamp.into(),
        filesystem_snapshot: filesystem_snapshot.into(),
    } }
});
