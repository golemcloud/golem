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

fn encode_tool_error(error: SerializableToolError) -> Result<wire::ToolError, String> {
    Ok(match error {
        SerializableToolError::InvalidToolName(error) => wire::ToolError::InvalidToolName(error),
        SerializableToolError::InvalidCommandPath(path) => {
            wire::ToolError::InvalidCommandPath(path)
        }
        SerializableToolError::InvalidInput(error) => wire::ToolError::InvalidInput(error),
        SerializableToolError::ConstraintViolation(error) => {
            wire::ToolError::ConstraintViolation(error)
        }
        SerializableToolError::InvalidResult(error) => wire::ToolError::InvalidResult(error),
        SerializableToolError::CustomError(error) => {
            wire::ToolError::CustomError(wire::CustomToolError {
                name: error.name,
                payload: encode_public_typed_schema_value(error.payload)?,
            })
        }
    })
}

fn encode_tool_rpc_error(error: SerializableToolRpcError) -> Result<oplog::ToolRpcError, String> {
    Ok(match error {
        SerializableToolRpcError::ProtocolError(error) => oplog::ToolRpcError::ProtocolError(error),
        SerializableToolRpcError::Denied(error) => oplog::ToolRpcError::Denied(error),
        SerializableToolRpcError::NotFound(error) => oplog::ToolRpcError::NotFound(error),
        SerializableToolRpcError::RemoteInternalError(error) => {
            oplog::ToolRpcError::RemoteInternalError(error)
        }
        SerializableToolRpcError::RemoteToolError(error) => {
            oplog::ToolRpcError::RemoteToolError(encode_tool_error(*error)?)
        }
        SerializableToolRpcError::Cancelled => oplog::ToolRpcError::Cancelled,
        SerializableToolRpcError::ResourceExhausted(error) => {
            oplog::ToolRpcError::ResourceExhausted(error)
        }
    })
}

golem_common::oplog_entry_metadata!(generate_entry_conversion;
public (PublicOplogEntry => oplog::PublicOplogEntry)
before (value) { let mut value = value; value.redact_host_managed_values_for_external(); }
{
    Create { record CreateParams => CreateParameters {
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
            golem_common::model::agent::AgentMode::Ephemeral => oplog::AgentMode::Ephemeral,
        },
        component_revision: component_revision.into(),
        env: env.into_iter().collect(),
        created_by: created_by.into(),
        environment_id: environment_id.into(),
        parent: parent.map(|id| id.into()),
        component_size: component_size,
        initial_total_linear_memory_size: initial_total_linear_memory_size,
        initial_active_plugins: initial_active_plugins
        .into_iter()
        .map(|pr| pr.into())
        .collect(),
        local_agent_config: local_agent_config
        .into_iter()
        .map(|lac| {
            Ok(oplog::LocalAgentConfigEntry {
                path: lac.path,
                value: encode_public_typed_schema_value(lac.value)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?,
        original_phantom_id: original_phantom_id.map(|id| id.into()),
        instance_id: instance_id.into(),
    } }
    Start { record StartParams => StartParameters {
        timestamp: timestamp.into(),
        parent_start_index: parent_start_index.map(|c| c.into()),
        function_name: function_name,
        invocation_id: invocation_id.map(durable_invocation_id_to_wit),
        observational_owner: observational_owner.map(|i| i.into()),
        request: request.map(encode_public_typed_schema_value).transpose()?,
        durable_function_type: durable_function_type.into(),
        span_started: span_started.map(public_span_started_to_wit),
        denial: denial,
    } }
    End { record EndParams => EndParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
        response: response.map(encode_public_typed_schema_value).transpose()?,
        forced_commit: forced_commit,
        span_finished: span_finished.map(public_span_finished_to_wit),
        span_attributes: span_attributes.map(public_span_attributes_to_wit),
    } }
    Cancelled { record CancelledParams => CancelledParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
        partial: partial.map(encode_public_typed_schema_value).transpose()?,
        span_finished: span_finished.map(public_span_finished_to_wit),
    } }
    AgentInvocationStarted { record AgentInvocationStartedParams => AgentInvocationStartedParameters {
        timestamp: timestamp.into(),
        invocation: invocation.try_into()?,
        wallet_pin: public_invocation_wallet_pin_to_wit(wallet_pin),
    } }
    AgentInvocationFinished { record AgentInvocationFinishedParams => AgentInvocationFinishedParameters {
        timestamp: timestamp.into(),
        result: result.try_into()?,
        method_name: method_name,
        consumed_fuel: consumed_fuel,
        component_revision: component_revision.get(),
    } }
    Suspend { override (PublicOplogEntry::Suspend(SuspendParams { timestamp })) => Ok({
                    Self::Suspend(timestamp.into())
                }) }
    Error { record ErrorParams => ErrorParameters {
        timestamp: timestamp.into(),
        kind: kind.into(),
        error: error.to_string(),
        retry_from: retry_from.into(),
        retry_policy_state: retry_policy_state.map(|s| {
            let internal: golem_common::model::RetryPolicyState = s.into();
            internal.into()
        }),
    } }
    RecoverySucceeded { override (PublicOplogEntry::RecoverySucceeded(RecoverySucceededParams { timestamp })) => Ok({
                    Self::RecoverySucceeded(timestamp.into())
                }) }
    Jump { record JumpParams => JumpParameters {
        timestamp: timestamp.into(),
        jump: oplog::OplogRegion {
            start: jump.start.into(),
            end: jump.end.into(),
        },
    } }
    Interrupted { override (PublicOplogEntry::Interrupted(InterruptedParams { timestamp })) => Ok({
                    Self::Interrupted(timestamp.into())
                }) }
    Exited { override (PublicOplogEntry::Exited(ExitedParams { timestamp })) => Ok(Self::Exited(timestamp.into())) }
    PendingAgentInvocation { record PendingAgentInvocationParams => PendingAgentInvocationParameters {
        timestamp: timestamp.into(),
        invocation: invocation.try_into()?,
    } }
    PendingUpdate { record PendingUpdateParams => PendingUpdateParameters {
        timestamp: timestamp.into(),
        target_revision: target_revision.into(),
        description: description.into(),
        update_attempt_index: update_attempt_index.into(),
    } }
    SuccessfulUpdate { record SuccessfulUpdateParams => SuccessfulUpdateParameters {
        timestamp: timestamp.into(),
        target_revision: target_revision.into(),
        new_component_size: new_component_size,
        new_active_plugins: new_active_plugins.into_iter().map(|pr| pr.into()).collect(),
        snapshot_assisted_details: snapshot_assisted_details.map(|details| {
            oplog::SnapshotAssistedUpdateDetails {
                pending_update_index: details.pending_update_index.into(),
                source_component_revision: details.source_component_revision.into(),
                source_revision_start_index: details.source_revision_start_index.into(),
                snapshot_index: details.snapshot_index.into(),
            }
        }),
    } }
    FailedUpdate { record FailedUpdateParams => FailedUpdateParameters {
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
    GrowMemory { record GrowMemoryParams => GrowMemoryParameters {
        timestamp: timestamp.into(),
        delta: delta,
    } }
    CreateResource { record CreateResourceParams => CreateResourceParameters {
        timestamp: timestamp.into(),
        id: id.0,
        name: name,
        owner: owner,
    } }
    DropResource { record DropResourceParams => DropResourceParameters {
        timestamp: timestamp.into(),
        id: id.0,
        name: name,
        owner: owner,
    } }
    Log { record LogParams => LogParameters {
        timestamp: timestamp.into(),
        level: level.into(),
        context: context,
        message: message,
        trace_context: trace_context.map(log_trace_context_to_wit),
    } }
    Restart { override (PublicOplogEntry::Restart(RestartParams { timestamp })) => Ok({
                    Self::Restart(timestamp.into())
                }) }
    Resumed { override (PublicOplogEntry::Resumed(ResumedParams { timestamp })) => Ok({
                    Self::Resumed(timestamp.into())
                }) }
    ActivatePlugin { record ActivatePluginParams => ActivatePluginParameters {
        timestamp: timestamp.into(),
        plugin: plugin.into(),
    } }
    DeactivatePlugin { record DeactivatePluginParams => DeactivatePluginParameters {
        timestamp: timestamp.into(),
        plugin: plugin.into(),
    } }
    Revert { record RevertParams => RevertParameters {
        timestamp: timestamp.into(),
        dropped_region: oplog::OplogRegion {
            start: dropped_region.start.into(),
            end: dropped_region.end.into(),
        },
    } }
    CancelPendingInvocation { record CancelPendingInvocationParams => CancelPendingInvocationParameters {
        timestamp: timestamp.into(),
        idempotency_key: idempotency_key.to_string(),
    } }
    BeginRemoteTransaction { record BeginRemoteTransactionParams => BeginRemoteTransactionParameters {
        timestamp: timestamp.into(),
        transaction_id: transaction_id.into(),
    } }
    PreCommitRemoteTransaction { record PreCommitRemoteTransactionParams => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    PreRollbackRemoteTransaction { record PreRollbackRemoteTransactionParams => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    CommittedRemoteTransaction { record CommittedRemoteTransactionParams => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    RolledBackRemoteTransaction { record RolledBackRemoteTransactionParams => RemoteTransactionParameters {
        timestamp: timestamp.into(),
        begin_index: begin_index.into(),
    } }
    Snapshot { override (PublicOplogEntry::Snapshot(SnapshotParams {
                    timestamp,
                    data,
                    filesystem_snapshot,
                })) => Ok({
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
                        filesystem_snapshot,
                    })
                }) }
    OplogProcessorCheckpoint { record OplogProcessorCheckpointParams => OplogProcessorCheckpointParameters {
        timestamp: timestamp.into(),
        plugin: plugin.into(),
        target_agent_id: target_agent_id.into(),
        confirmed_up_to: confirmed_up_to.into(),
        sending_up_to: sending_up_to.into(),
        last_batch_start: last_batch_start.into(),
    } }
    SetRetryPolicy { override (PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams { timestamp, policy })) => Ok({
                    let named: golem_common::model::retry_policy::NamedRetryPolicy = policy.into();
                    let wit_named: golem_common::schema::agent::bindings::golem::api::retry::NamedRetryPolicy = named.into();
                    Self::SetRetryPolicy(oplog::SetRetryPolicyParameters {
                        timestamp: timestamp.into(),
                        policy: wit_named,
                    })
                }) }
    RemoveRetryPolicy { record RemoveRetryPolicyParams => RemoveRetryPolicyParameters {
        timestamp: timestamp.into(),
        name: name,
    } }
    CardEventQueued { record CardEventQueuedParams => CardEventQueuedParameters {
        timestamp: timestamp.into(),
        event: queued_card_event_to_wit(event)?,
    } }
    CardInstalled { record CardInstalledParams => CardInstalledParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.map(Into::into),
        card_id: card_id_to_wit(card_id),
        wallet_generation: wallet_generation,
    } }
    CardInstallFailed { record CardInstallFailedParams => CardInstallFailedParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.into(),
        card_id: card_id_to_wit(card_id),
        reason: card_install_failure_to_wit(reason),
    } }
    CardRevoked { record CardRevokedParams => CardRevokedParameters {
        timestamp: timestamp.into(),
        queued_event_index: queued_event_index.into(),
        card_id: card_id_to_wit(card_id),
        wallet_generation: wallet_generation,
    } }
    CardExpired { record CardExpiredParams => CardExpiredParameters {
        timestamp: timestamp.into(),
        card_id: card_id_to_wit(card_id),
        wallet_generation: wallet_generation,
    } }
    CardDerived { override (PublicOplogEntry::CardDerived(_)) => Err(
                        "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                            .to_string(),
                    ) }
    CardTransferStarted { override (PublicOplogEntry::CardTransferStarted(_)) => Err(
                        "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                            .to_string(),
                    ) }
    CardTransferred { override (PublicOplogEntry::CardTransferred(_)) => Err(
                        "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                            .to_string(),
                    ) }
    CardRevokedCascade { override (PublicOplogEntry::CardRevokedCascade(_)) => Err(
                        "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                            .to_string(),
                    ) }
    CardTransferConfirmed { override (PublicOplogEntry::CardTransferConfirmed(_)) => Err(
                        "permission-card lifecycle entry is not representable in golem:api/oplog@1.5.0"
                            .to_string(),
                    ) }
    HostStreamFrame { record HostStreamFrameParams => HostStreamFrameParameters {
        timestamp: timestamp.into(),
        parent_start_index: parent_start_index.into(),
        kind: kind.into(),
        payload: encode_public_typed_schema_value(payload)?,
    } }
    ToolAttachmentRead { record ToolAttachmentReadParams => ToolAttachmentReadParameters {
        timestamp: timestamp.into(),
        observation: encode_public_typed_schema_value(observation)?,
        payload: payload,
    } }
    ToolAttachmentControl { record ToolAttachmentControlParams => ToolAttachmentControlParameters {
        timestamp: timestamp.into(),
        attachment: encode_public_typed_schema_value(attachment)?,
        transition: encode_public_typed_schema_value(transition)?,
    } }
    StreamRegistered { record StreamRegisteredParams => DurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: encode_public_typed_schema_value(record)?,
    } }
    StreamItems { record StreamItemsParams => DurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: encode_public_typed_schema_value(record)?,
    } }
    StreamEnd { record StreamEndParams => DurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: encode_public_typed_schema_value(record)?,
    } }
    StreamCancel { record StreamCancelParams => DurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: encode_public_typed_schema_value(record)?,
    } }
    StreamSession { record StreamSessionParams => DurableStreamRecordParameters {
        timestamp: timestamp.into(),
        record: encode_public_typed_schema_value(record)?,
    } }
    CompletionDiscarded { record CompletionDiscardedParams => CompletionDiscardedParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
    } }
    CompletionDelivered { record CompletionDeliveredParams => CompletionDeliveredParameters {
        timestamp: timestamp.into(),
        start_index: start_index.into(),
    } }
    SnapshotConfirmed { record SnapshotConfirmedParams => SnapshotConfirmedParameters {
        timestamp: timestamp.into(),
        filesystem_snapshot: filesystem_snapshot,
    } }
});

impl TryFrom<PublicAgentInvocation> for oplog::AgentInvocation {
    type Error = String;

    fn try_from(value: PublicAgentInvocation) -> Result<Self, Self::Error> {
        Ok(match value {
            PublicAgentInvocation::AgentInitialization(params) => {
                Self::AgentInitialization(oplog::AgentInitializationParameters {
                    idempotency_key: params.idempotency_key.value,
                    constructor_parameters: encode_public_typed_schema_value(
                        params.constructor_parameters,
                    )?,
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
                    function_input: encode_public_typed_schema_value(params.function_input)?,
                    trace_id: params.trace_id.to_string(),
                    trace_states: params.trace_states,
                    invocation_context: params
                        .invocation_context
                        .into_iter()
                        .map(|inner| inner.into_iter().map(|span| span.into()).collect())
                        .collect(),
                })
            }
            PublicAgentInvocation::ExternalTool(params) => {
                Self::ExternalTool(oplog::ExternalToolInvocationParameters {
                    idempotency_key: params.idempotency_key.value,
                    tool_name: params.tool_name,
                    command_path: params.command_path,
                    input: encode_public_typed_schema_value(params.input)?,
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
        })
    }
}

impl TryFrom<PublicAgentInvocationResult> for oplog::AgentInvocationResult {
    type Error = String;

    fn try_from(value: PublicAgentInvocationResult) -> Result<Self, Self::Error> {
        Ok(match value {
            PublicAgentInvocationResult::AgentInitialization(AgentInvocationOutputParameters {
                output,
            }) => Self::AgentInitialization(oplog::AgentInvocationOutputParameters {
                output: encode_public_typed_schema_value(output)?,
            }),
            PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                output,
            }) => Self::AgentMethod(oplog::AgentInvocationOutputParameters {
                output: encode_public_typed_schema_value(output)?,
            }),
            PublicAgentInvocationResult::ExternalTool(params) => {
                let result = match params.result {
                    PublicExternalToolResult::Success(result) => Ok(oplog::ToolInvocationResult {
                        result: result
                            .result
                            .map(|value| encode_public_typed_schema_value(*value))
                            .transpose()?,
                    }),
                    PublicExternalToolResult::Failure(error) => Err(encode_tool_rpc_error(error)?),
                };
                Self::ExternalTool(oplog::ExternalToolResultParameters { result })
            }
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
        })
    }
}
