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

pub(super) fn host_response_to_public_value(
    response: HostResponse,
) -> Result<TypedSchemaValue, String> {
    match response {
        HostResponse::EntityInvocation(HostResponseEntityInvocation { result: Ok(value) }) => {
            Ok(value)
        }
        response => response
            .into_typed_schema_value()
            .map_err(|error| error.to_string()),
    }
}

#[async_trait]
impl PublicOplogEntryOps for PublicOplogEntry {
    async fn from_oplog_entry(
        oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        agent_type_name: Option<&AgentTypeName>,
        component_revision: ComponentRevision,
    ) -> Result<Self, String> {
        let owner_kind = if OwnerKind::is_reserved_instance_name(&owned_agent_id.agent_id.agent_id)
        {
            OwnerKind::EphemeralExternalTool
        } else {
            OwnerKind::ComponentAgent
        };
        match value {
            OplogEntry::Create {
                timestamp,
                parameters,
            } => {
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
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let initial_plugins = metadata
                    .metadata
                    .owner_plugins(owner_kind, agent_type_name)
                    .unwrap_or_default()
                    .iter()
                    .filter(|&p| initial_active_plugins.contains(&p.environment_plugin_grant_id))
                    .cloned()
                    .map(make_plugin_installation_description)
                    .collect();

                let local_agent_config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        let typed =
                            lac.enrich_with_type(&metadata.metadata, owner_kind, agent_type_name)?;
                        Ok::<_, String>(PublicTypedAgentConfigEntry {
                            path: typed.path,
                            value: typed.value,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(PublicOplogEntry::Create(CreateParams {
                    timestamp,
                    agent_id,
                    owner_kind,
                    agent_mode,
                    component_revision,
                    env: env.into_iter().collect(),
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins: initial_plugins,
                    local_agent_config,
                    original_phantom_id,
                    instance_id,
                }))
            }
            OplogEntry::Start {
                timestamp,
                parent_start_index,
                function_name,
                invocation_id,
                observational_owner,
                request,
                durable_function_type,
                span_started,
                denial,
            } => {
                let request_value = if let Some(request_payload) = request {
                    let host_request: HostRequest = oplog_service
                        .download_payload(owned_agent_id, agent_mode, request_payload)
                        .await?;

                    let request_value = match host_request {
                        HostRequest::EntityInvocation(request) => request.input,
                        HostRequest::McpToolCall(request) => request.input,
                        HostRequest::GolemRpcInvoke(inner) => HostRequest::GolemRpcInvoke(
                            enrich_golem_rpc_invoke(components, inner).await,
                        )
                        .into_typed_schema_value()
                        .map_err(|error| error.to_string())?,
                        HostRequest::GolemRpcScheduledInvocation(inner) => {
                            HostRequest::GolemRpcScheduledInvocation(
                                enrich_golem_rpc_scheduled_invocation(components, inner).await,
                            )
                            .into_typed_schema_value()
                            .map_err(|error| error.to_string())?
                        }
                        other => other
                            .into_typed_schema_value()
                            .map_err(|error| error.to_string())?,
                    };
                    Some(request_value)
                } else {
                    None
                };

                Ok(PublicOplogEntry::Start(StartParams {
                    timestamp,
                    parent_start_index,
                    function_name: function_name.to_string(),
                    invocation_id,
                    observational_owner,
                    request: request_value,
                    durable_function_type: durable_function_type.into(),
                    span_started: span_started.map(|span| public_span_started(*span)),
                    denial,
                }))
            }
            OplogEntry::End {
                timestamp,
                start_index,
                response,
                forced_commit,
                span_finished,
                span_attributes,
            } => {
                let response_value = if let Some(response_payload) = response {
                    let host_response: HostResponse = oplog_service
                        .download_payload(owned_agent_id, agent_mode, response_payload)
                        .await?;
                    Some(host_response_to_public_value(host_response)?)
                } else {
                    None
                };

                Ok(PublicOplogEntry::End(EndParams {
                    timestamp,
                    start_index,
                    response: response_value,
                    forced_commit,
                    span_finished: span_finished.map(public_span_finished),
                    span_attributes: span_attributes.map(public_span_attributes),
                }))
            }
            OplogEntry::Cancelled {
                timestamp,
                start_index,
                partial,
                span_finished,
            } => {
                let partial_value = if let Some(partial_payload) = partial {
                    let host_response: HostResponse = oplog_service
                        .download_payload(owned_agent_id, agent_mode, partial_payload)
                        .await?;
                    Some(host_response_to_public_value(host_response)?)
                } else {
                    None
                };

                Ok(PublicOplogEntry::Cancelled(CancelledParams {
                    timestamp,
                    start_index,
                    partial: partial_value,
                    span_finished: span_finished.map(public_span_finished),
                }))
            }
            OplogEntry::CompletionDiscarded {
                timestamp,
                start_index,
            } => Ok(PublicOplogEntry::CompletionDiscarded(
                CompletionDiscardedParams {
                    timestamp,
                    start_index,
                },
            )),
            OplogEntry::CompletionDelivered {
                timestamp,
                start_index,
            } => Ok(PublicOplogEntry::CompletionDelivered(
                CompletionDeliveredParams {
                    timestamp,
                    start_index,
                },
            )),
            OplogEntry::AgentInvocationStarted {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                wallet_pin,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                let invocation_context_stack = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation = AgentInvocation::from_parts(
                    idempotency_key,
                    invocation_payload,
                    invocation_context_stack,
                );
                let public_invocation = agent_invocation_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    invocation,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationStarted(
                    AgentInvocationStartedParams {
                        timestamp,
                        invocation: public_invocation,
                        wallet_pin: golem_common::model::card::PublicInvocationWalletPin {
                            wallet_token: wallet_pin.wallet_token,
                            scope_card_id: wallet_pin.scope_card_id,
                        },
                    },
                ))
            }
            OplogEntry::AgentInvocationFinished {
                timestamp,
                result,
                method_name,
                consumed_fuel,
                component_revision: entry_component_revision,
            } => {
                let invocation_result: AgentInvocationResult = oplog_service
                    .download_payload(owned_agent_id, agent_mode, result)
                    .await?;

                let public_result = agent_invocation_result_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    method_name.clone(),
                    invocation_result,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationFinished(
                    AgentInvocationFinishedParams {
                        timestamp,
                        result: public_result,
                        method_name,
                        consumed_fuel,
                        component_revision: entry_component_revision,
                    },
                ))
            }
            OplogEntry::Suspend { timestamp } => {
                Ok(PublicOplogEntry::Suspend(SuspendParams { timestamp }))
            }
            OplogEntry::Error {
                timestamp,
                kind,
                error,
                retry_from,
                retry_policy_state,
                ..
            } => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp,
                kind,
                error: error.to_string(""),
                retry_from,
                retry_policy_state: retry_policy_state.map(Into::into),
            })),
            OplogEntry::RecoverySucceeded { timestamp } => Ok(PublicOplogEntry::RecoverySucceeded(
                RecoverySucceededParams { timestamp },
            )),
            OplogEntry::Jump {
                timestamp, jump, ..
            } => Ok(PublicOplogEntry::Jump(JumpParams { timestamp, jump })),
            OplogEntry::Interrupted { timestamp } => {
                Ok(PublicOplogEntry::Interrupted(InterruptedParams {
                    timestamp,
                }))
            }
            OplogEntry::Exited { timestamp } => {
                Ok(PublicOplogEntry::Exited(ExitedParams { timestamp }))
            }
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                let invocation_context_stack = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation = AgentInvocation::from_parts(
                    idempotency_key,
                    invocation_payload,
                    invocation_context_stack,
                );
                let public_invocation = agent_invocation_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    invocation,
                )
                .await?;

                Ok(PublicOplogEntry::PendingAgentInvocation(
                    PendingAgentInvocationParams {
                        timestamp,
                        invocation: public_invocation,
                    },
                ))
            }
            OplogEntry::PendingUpdate {
                timestamp,
                description,
                update_attempt_index,
            } => {
                let target_revision = *description.target_revision();
                let public_description = match description {
                    UpdateDescription::Automatic { .. } => {
                        PublicUpdateDescription::Automatic(Empty {})
                    }
                    UpdateDescription::SnapshotAssistedAutomatic { .. } => {
                        PublicUpdateDescription::SnapshotAssistedAutomatic(
                            SnapshotAssistedAutomaticUpdateParameters {},
                        )
                    }
                    UpdateDescription::SnapshotBased {
                        payload,
                        mime_type,
                        filesystem_snapshot,
                        ..
                    } => {
                        let bytes = oplog_service
                            .download_payload(owned_agent_id, agent_mode, payload)
                            .await?;
                        PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                            payload: bytes,
                            mime_type,
                            filesystem_snapshot: filesystem_snapshot.map(String::from),
                        })
                    }
                };
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                    timestamp,
                    target_revision,
                    description: public_description,
                    update_attempt_index: update_attempt_index.unwrap_or(oplog_index),
                }))
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_revision,
                new_component_size,
                new_total_linear_memory_size: _,
                new_active_plugins,
                snapshot_assisted_details,
            } => {
                let metadata = components
                    .get_metadata(owned_agent_id.agent_id.component_id, Some(target_revision))
                    .await
                    .map_err(|err| err.to_string())?;

                let new_plugins = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .unwrap_or_default()
                    .iter()
                    .filter(|&p| new_active_plugins.contains(&p.environment_plugin_grant_id))
                    .cloned()
                    .map(make_plugin_installation_description)
                    .collect();

                Ok(PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                    timestamp,
                    target_revision,
                    new_component_size,
                    new_active_plugins: new_plugins,
                    snapshot_assisted_details: snapshot_assisted_details.map(|details| {
                        PublicSnapshotAssistedUpdateDetails {
                            pending_update_index: details.pending_update_index,
                            source_component_revision: details.source_component_revision,
                            source_revision_start_index: details.source_revision_start_index,
                            snapshot_index: details.snapshot_index,
                        }
                    }),
                }))
            }
            OplogEntry::FailedUpdate {
                timestamp,
                target_revision,
                details,
                snapshot_assisted_details,
                update_attempt_index,
                snapshot_fault: _,
            } => Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp,
                target_revision,
                details,
                update_attempt_index,
                snapshot_assisted_details: snapshot_assisted_details.map(|details| {
                    PublicFailedSnapshotAssistedUpdateDetails {
                        pending_update_index: details.pending_update_index,
                        source_component_revision: details.source_component_revision,
                        source_revision_start_index: details.source_revision_start_index,
                        snapshot_index: details.snapshot_index,
                    }
                }),
            })),
            OplogEntry::GrowMemory { timestamp, delta } => {
                Ok(PublicOplogEntry::GrowMemory(GrowMemoryParams {
                    timestamp,
                    delta,
                }))
            }
            OplogEntry::CreateResource {
                timestamp,
                id,
                resource_type_id,
                ..
            } => Ok(PublicOplogEntry::CreateResource(CreateResourceParams {
                timestamp,
                id,
                name: resource_type_id.name,
                owner: resource_type_id.owner,
            })),
            OplogEntry::DropResource {
                timestamp,
                id,
                resource_type_id,
                ..
            } => Ok(PublicOplogEntry::DropResource(DropResourceParams {
                timestamp,
                id,
                name: resource_type_id.name,
                owner: resource_type_id.owner,
            })),

            OplogEntry::Log {
                timestamp,
                level,
                context,
                message,
                trace_context,
                ..
            } => Ok(PublicOplogEntry::Log(LogParams {
                timestamp,
                level,
                context,
                message,
                trace_context,
            })),
            OplogEntry::Restart { timestamp } => {
                Ok(PublicOplogEntry::Restart(RestartParams { timestamp }))
            }
            OplogEntry::Resumed { timestamp } => {
                Ok(PublicOplogEntry::Resumed(ResumedParams { timestamp }))
            }
            OplogEntry::ActivatePlugin {
                timestamp,
                plugin_grant_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = metadata
                    .metadata
                    .owner_plugins(owner_kind, agent_type_name)
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::DeactivatePlugin {
                timestamp,
                plugin_grant_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = metadata
                    .metadata
                    .owner_plugins(owner_kind, agent_type_name)
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::Revert {
                timestamp,
                dropped_region,
            } => Ok(PublicOplogEntry::Revert(RevertParams {
                timestamp,
                dropped_region,
            })),
            OplogEntry::CancelPendingInvocation {
                timestamp,
                idempotency_key,
            } => Ok(PublicOplogEntry::CancelPendingInvocation(
                CancelPendingInvocationParams {
                    timestamp,
                    idempotency_key,
                },
            )),
            OplogEntry::BeginRemoteTransaction {
                timestamp,
                transaction_id,
                ..
            } => Ok(PublicOplogEntry::BeginRemoteTransaction(
                BeginRemoteTransactionParams {
                    timestamp,
                    transaction_id,
                },
            )),
            OplogEntry::PreCommitRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreCommitRemoteTransaction(
                PreCommitRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::PreRollbackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreRollbackRemoteTransaction(
                PreRollbackRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::CommittedRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::CommittedRemoteTransaction(
                CommittedRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::RolledBackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::RolledBackRemoteTransaction(
                RolledBackRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::Snapshot {
                timestamp,
                data,
                mime_type,
                filesystem_snapshot,
                ..
            } => {
                let bytes: Vec<u8> = oplog_service
                    .download_payload(owned_agent_id, agent_mode, data)
                    .await?;

                let snapshot_data = raw_snapshot_to_public(RawSnapshotData {
                    data: bytes,
                    mime_type,
                });

                Ok(PublicOplogEntry::Snapshot(SnapshotParams {
                    timestamp,
                    data: snapshot_data,
                    filesystem_snapshot: filesystem_snapshot.map(String::from),
                }))
            }
            OplogEntry::SnapshotConfirmed {
                timestamp,
                filesystem_snapshot,
            } => Ok(PublicOplogEntry::SnapshotConfirmed(
                SnapshotConfirmedParams {
                    timestamp,
                    filesystem_snapshot: filesystem_snapshot.into(),
                },
            )),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp,
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = metadata
                    .metadata
                    .owner_plugins(owner_kind, agent_type_name)
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::OplogProcessorCheckpoint(
                    OplogProcessorCheckpointParams {
                        timestamp,
                        plugin: desc,
                        target_agent_id,
                        confirmed_up_to,
                        sending_up_to,
                        last_batch_start,
                    },
                ))
            }
            OplogEntry::SetRetryPolicy {
                timestamp, policy, ..
            } => Ok(PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
                timestamp,
                policy: (*policy).into(),
            })),
            OplogEntry::RemoveRetryPolicy {
                timestamp, name, ..
            } => Ok(PublicOplogEntry::RemoveRetryPolicy(
                RemoveRetryPolicyParams { timestamp, name },
            )),
            OplogEntry::CardRevoked {
                timestamp,
                queued_event_index,
                card_id,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardRevoked(CardRevokedParams {
                timestamp,
                queued_event_index,
                card_id,
                wallet_generation,
            })),
            OplogEntry::CardExpired {
                timestamp,
                card_id,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardExpired(CardExpiredParams {
                timestamp,
                card_id,
                wallet_generation,
            })),
            OplogEntry::HostStreamFrame {
                timestamp,
                parent_start_index,
                kind,
                payload,
            } => {
                let host_request: HostRequest = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                Ok(PublicOplogEntry::HostStreamFrame(HostStreamFrameParams {
                    timestamp,
                    parent_start_index,
                    kind,
                    payload: host_request
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamRegistered {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamRegistered(StreamRegisteredParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::ToolAttachmentControl {
                timestamp,
                attachment,
                transition,
                ..
            } => Ok(PublicOplogEntry::ToolAttachmentControl(
                golem_common::model::oplog::public_oplog_entry::ToolAttachmentControlParams {
                    timestamp,
                    attachment: attachment
                        .into_typed_schema_value()
                        .map_err(|error| error.to_string())?,
                    transition: transition
                        .into_typed_schema_value()
                        .map_err(|error| error.to_string())?,
                },
            )),
            OplogEntry::ToolAttachmentRead {
                timestamp,
                observation,
                payload,
                ..
            } => {
                let payload = match payload {
                    Some(payload) => Some(
                        oplog_service
                            .download_payload(owned_agent_id, agent_mode, payload)
                            .await?,
                    ),
                    None => None,
                };
                Ok(PublicOplogEntry::ToolAttachmentRead(
                    golem_common::model::oplog::public_oplog_entry::ToolAttachmentReadParams {
                        timestamp,
                        observation: (*observation)
                            .into_typed_schema_value()
                            .map_err(|error| error.to_string())?,
                        payload,
                    },
                ))
            }
            OplogEntry::StreamItems {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamItems(StreamItemsParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamEnd {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamEnd(StreamEndParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamCancel {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamCancel(StreamCancelParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamSession {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamSession(StreamSessionParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::CardEventQueued {
                timestamp, event, ..
            } => Ok(PublicOplogEntry::CardEventQueued(CardEventQueuedParams {
                timestamp,
                event: (*event).into(),
            })),
            OplogEntry::CardInstalled {
                timestamp,
                queued_event_index,
                card,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardInstalled(CardInstalledParams {
                timestamp,
                queued_event_index,
                card_id: card.card_id(),
                wallet_generation,
            })),
            OplogEntry::CardInstallFailed {
                timestamp,
                queued_event_index,
                card_id,
                reason,
                ..
            } => Ok(PublicOplogEntry::CardInstallFailed(
                CardInstallFailedParams {
                    timestamp,
                    queued_event_index,
                    card_id,
                    reason,
                },
            )),
            OplogEntry::CardDerived {
                timestamp,
                card,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardDerived(CardDerivedParams {
                timestamp,
                card_id: card.card_id(),
                parent_ids: card.parent_ids().to_vec(),
                wallet_generation,
            })),
            OplogEntry::CardTransferStarted {
                timestamp,
                transfer_id,
                card_id,
                target_holder,
                source_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardTransferStarted(
                CardTransferStartedParams {
                    timestamp,
                    transfer_id,
                    card_id,
                    target_holder,
                    source_wallet_generation,
                },
            )),
            OplogEntry::CardTransferred {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                target_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardTransferred(CardTransferredParams {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                target_wallet_generation,
            })),
            OplogEntry::CardRevokedCascade {
                timestamp,
                revoked_card_ids,
                local_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardRevokedCascade(
                CardRevokedCascadeParams {
                    timestamp,
                    revoked_card_ids,
                    local_wallet_generation,
                },
            )),
            OplogEntry::CardTransferConfirmed {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => Ok(PublicOplogEntry::CardTransferConfirmed(
                CardTransferConfirmedParams {
                    timestamp,
                    transfer_id,
                    source_card_id,
                    installed_card_id,
                    target_holder,
                },
            )),
        }
    }
}

fn make_plugin_installation_description(
    installation: InstalledPlugin,
) -> PluginInstallationDescription {
    PluginInstallationDescription {
        environment_plugin_grant_id: installation.environment_plugin_grant_id,
        plugin_priority: installation.priority,
        plugin_name: installation.plugin_name,
        plugin_version: installation.plugin_version,
        parameters: installation.parameters,
    }
}
