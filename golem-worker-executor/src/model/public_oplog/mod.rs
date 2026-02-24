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

pub mod wit;

use crate::services::component::ComponentService;
use crate::services::oplog::OplogService;
use crate::services::oplog::OplogServiceOps;
use async_trait::async_trait;
use golem_common::model::agent::AgentId;
use golem_common::model::agent::{DataValue, ElementValues};
use golem_common::model::component::{ComponentRevision, InstalledPlugin};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::lucene::Query;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, BeginRemoteWriteParams,
    CancelPendingInvocationParams, ChangePersistenceLevelParams, ChangeRetryPolicyParams,
    CommittedRemoteTransactionParams, CreateParams, CreateResourceParams, DeactivatePluginParams,
    DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams, ErrorParams, ExitedParams,
    FailedUpdateParams, FinishSpanParams, GrowMemoryParams, HostCallParams, InterruptedParams,
    JumpParams, LogParams, NoOpParams, PendingAgentInvocationParams, PendingUpdateParams,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, RestartParams,
    RevertParams, RolledBackRemoteTransactionParams, SetSpanAttributeParams, SnapshotParams,
    StartSpanParams, SuccessfulUpdateParams, SuspendParams,
};
use golem_common::model::oplog::types::encode_span_data;
use golem_common::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, FallibleResultParameters, HostRequest,
    HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation, HostResponse,
    JsonSnapshotData, LoadSnapshotParameters, ManualUpdateParameters, OplogEntry, OplogIndex,
    PluginInstallationDescription, ProcessOplogEntriesParameters, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttribute, PublicOplogEntry, PublicSnapshotData,
    PublicUpdateDescription, RawSnapshotData, SaveSnapshotResultParameters,
    SnapshotBasedUpdateParameters, UpdateDescription,
};
use golem_common::model::{
    AgentInvocation, AgentInvocationPayload, AgentInvocationResult, Empty, OwnedWorkerId, WorkerId,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::IntoValueAndType;
use std::sync::Arc;

pub struct PublicOplogChunk {
    pub entries: Vec<PublicOplogEntry>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub first_index_in_chunk: OplogIndex,
    pub last_index: OplogIndex,
}

pub async fn get_public_oplog_chunk(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_worker_id: &OwnedWorkerId,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
) -> Result<PublicOplogChunk, String> {
    let raw_entries = oplog_service
        .read(owned_worker_id, initial_oplog_index, count as u64)
        .await;

    let last_index = oplog_service.get_last_index(owned_worker_id).await;

    let mut entries = Vec::new();
    let mut current_component_revision = initial_component_revision;
    let mut next_oplog_index = initial_oplog_index;
    let mut first_index_in_chunk = None;

    for (index, raw_entry) in raw_entries {
        if first_index_in_chunk.is_none() {
            first_index_in_chunk = Some(index);
        }
        if let Some(revision) = raw_entry.specifies_component_revision() {
            current_component_revision = revision;
        }

        let entry = PublicOplogEntry::from_oplog_entry(
            index,
            raw_entry,
            oplog_service.clone(),
            components.clone(),
            owned_worker_id,
            current_component_revision,
        )
        .await?;
        entries.push(entry);
        next_oplog_index = index.next();
    }

    Ok(PublicOplogChunk {
        entries,
        next_oplog_index,
        current_component_revision,
        first_index_in_chunk: first_index_in_chunk.unwrap_or(initial_oplog_index),
        last_index,
    })
}

pub struct PublicOplogSearchResult {
    pub entries: Vec<(OplogIndex, PublicOplogEntry)>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub last_index: OplogIndex,
}

pub async fn search_public_oplog(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_worker_id: &OwnedWorkerId,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    query: &str,
) -> Result<PublicOplogSearchResult, String> {
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_revision = initial_component_revision;

    let query = Query::parse(query)?;

    loop {
        let chunk = get_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            owned_worker_id,
            current_component_revision,
            current_index,
            count,
        )
        .await?;

        for (idx, entry) in chunk.entries.into_iter().enumerate() {
            if entry.matches(&query) {
                results.push((
                    OplogIndex::from_u64(u64::from(current_index) + idx as u64),
                    entry,
                ));
            }
        }

        last_index = chunk.last_index;
        current_index = chunk.next_oplog_index;
        current_component_revision = chunk.current_component_revision;

        if current_index >= last_index || results.len() >= count {
            break;
        }
    }

    Ok(PublicOplogSearchResult {
        entries: results,
        next_oplog_index: current_index,
        current_component_revision,
        last_index,
    })
}

pub async fn find_component_revision_at(
    oplog_service: Arc<dyn OplogService>,
    owned_worker_id: &OwnedWorkerId,
    start: OplogIndex,
) -> Result<ComponentRevision, WorkerExecutorError> {
    let mut initial_component_revision = ComponentRevision::INITIAL;
    let last_oplog_index = oplog_service.get_last_index(owned_worker_id).await;
    let mut current = OplogIndex::INITIAL;
    while current < start && current <= last_oplog_index {
        // NOTE: could be reading in pages for optimization
        let entry = oplog_service
            .read(owned_worker_id, current, 1)
            .await
            .iter()
            .next()
            .map(|(_, v)| v.clone());

        if let Some(revision) = entry.and_then(|entry| entry.specifies_component_revision()) {
            initial_component_revision = revision;
        }

        current = current.next();
    }

    Ok(initial_component_revision)
}

#[async_trait]
pub trait PublicOplogEntryOps: Sized {
    async fn from_oplog_entry(
        oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        owned_worker_id: &OwnedWorkerId,
        component_revision: ComponentRevision,
    ) -> Result<Self, String>;
}

#[async_trait]
impl PublicOplogEntryOps for PublicOplogEntry {
    async fn from_oplog_entry(
        _oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        owned_worker_id: &OwnedWorkerId,
        component_revision: ComponentRevision,
    ) -> Result<Self, String> {
        match value {
            OplogEntry::Create {
                timestamp,
                worker_id,
                component_revision,
                env,
                environment_id,
                created_by,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                wasi_config_vars,
                original_phantom_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_worker_id.worker_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let initial_plugins = metadata
                    .installed_plugins
                    .into_iter()
                    .filter(|p| initial_active_plugins.contains(&p.priority))
                    .map(make_plugin_installation_description)
                    .collect();

                Ok(PublicOplogEntry::Create(CreateParams {
                    timestamp,
                    worker_id,
                    component_revision,
                    env: env.into_iter().collect(),
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins: initial_plugins,
                    wasi_config_vars: wasi_config_vars.into(),
                    original_phantom_id,
                }))
            }
            OplogEntry::HostCall {
                timestamp,
                function_name,
                request,
                response,
                durable_function_type,
            } => {
                let host_request: HostRequest = oplog_service
                    .download_payload(owned_worker_id, request)
                    .await?;
                let host_response: HostResponse = oplog_service
                    .download_payload(owned_worker_id, response)
                    .await?;

                // Enriching data
                let host_request = match host_request {
                    HostRequest::GolemRpcInvoke(inner) => HostRequest::GolemRpcInvoke(
                        enrich_golem_rpc_invoke(components, inner).await,
                    ),
                    HostRequest::GolemRpcScheduledInvocation(inner) => {
                        HostRequest::GolemRpcScheduledInvocation(
                            enrich_golem_rpc_scheduled_invocation(components, inner).await,
                        )
                    }
                    other => other,
                };

                Ok(PublicOplogEntry::HostCall(HostCallParams {
                    timestamp,
                    function_name: function_name.to_string(),
                    request: host_request.into_value_and_type(),
                    response: host_response.into_value_and_type(),
                    durable_function_type: durable_function_type.into(),
                }))
            }
            OplogEntry::AgentInvocationStarted {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_worker_id, payload)
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
                    owned_worker_id,
                    component_revision,
                    invocation,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationStarted(
                    AgentInvocationStartedParams {
                        timestamp,
                        invocation: public_invocation,
                    },
                ))
            }
            OplogEntry::AgentInvocationFinished {
                timestamp,
                result,
                consumed_fuel,
            } => {
                let invocation_result: AgentInvocationResult = oplog_service
                    .download_payload(owned_worker_id, result)
                    .await?;

                let public_result = agent_invocation_result_to_public(
                    components.clone(),
                    owned_worker_id,
                    component_revision,
                    invocation_result,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationFinished(
                    AgentInvocationFinishedParams {
                        timestamp,
                        result: public_result,
                        consumed_fuel,
                    },
                ))
            }
            OplogEntry::Suspend { timestamp } => {
                Ok(PublicOplogEntry::Suspend(SuspendParams { timestamp }))
            }
            OplogEntry::Error {
                timestamp,
                error,
                retry_from,
            } => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp,
                error: error.to_string(""),
                retry_from,
            })),
            OplogEntry::NoOp { timestamp } => Ok(PublicOplogEntry::NoOp(NoOpParams { timestamp })),
            OplogEntry::Jump { timestamp, jump } => {
                Ok(PublicOplogEntry::Jump(JumpParams { timestamp, jump }))
            }
            OplogEntry::Interrupted { timestamp } => {
                Ok(PublicOplogEntry::Interrupted(InterruptedParams {
                    timestamp,
                }))
            }
            OplogEntry::Exited { timestamp } => {
                Ok(PublicOplogEntry::Exited(ExitedParams { timestamp }))
            }
            OplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            } => Ok(PublicOplogEntry::ChangeRetryPolicy(
                ChangeRetryPolicyParams {
                    timestamp,
                    new_policy: new_policy.into(),
                },
            )),
            OplogEntry::BeginAtomicRegion { timestamp } => Ok(PublicOplogEntry::BeginAtomicRegion(
                BeginAtomicRegionParams { timestamp },
            )),
            OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                timestamp,
                begin_index,
            })),
            OplogEntry::BeginRemoteWrite { timestamp } => {
                Ok(PublicOplogEntry::BeginRemoteWrite(BeginRemoteWriteParams {
                    timestamp,
                }))
            }
            OplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndRemoteWrite(EndRemoteWriteParams {
                timestamp,
                begin_index,
            })),
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_worker_id, payload)
                    .await?;

                let invocation = AgentInvocation::from_parts(
                    idempotency_key,
                    invocation_payload,
                    InvocationContextStack::fresh(),
                );
                let public_invocation = agent_invocation_to_public(
                    components.clone(),
                    owned_worker_id,
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
            } => {
                let target_revision = *description.target_revision();
                let public_description = match description {
                    UpdateDescription::Automatic { .. } => {
                        PublicUpdateDescription::Automatic(Empty {})
                    }
                    UpdateDescription::SnapshotBased {
                        payload, mime_type, ..
                    } => {
                        let bytes = oplog_service
                            .download_payload(owned_worker_id, payload)
                            .await?;
                        PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                            payload: bytes,
                            mime_type,
                        })
                    }
                };
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                    timestamp,
                    target_revision,
                    description: public_description,
                }))
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_revision,
                new_component_size,
                new_active_plugins,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_worker_id.worker_id.component_id,
                        Some(target_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let new_plugins = metadata
                    .installed_plugins
                    .into_iter()
                    .filter(|p| new_active_plugins.contains(&p.priority))
                    .map(make_plugin_installation_description)
                    .collect();

                Ok(PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                    timestamp,
                    target_revision,
                    new_component_size,
                    new_active_plugins: new_plugins,
                }))
            }
            OplogEntry::FailedUpdate {
                timestamp,
                target_revision,
                details,
            } => Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp,
                target_revision,
                details,
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
            } => Ok(PublicOplogEntry::Log(LogParams {
                timestamp,
                level,
                context,
                message,
            })),
            OplogEntry::Restart { timestamp } => {
                Ok(PublicOplogEntry::Restart(RestartParams { timestamp }))
            }
            OplogEntry::ActivatePlugin {
                timestamp,
                plugin_priority,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_worker_id.worker_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = metadata
                    .installed_plugins
                    .into_iter()
                    .find(|p| p.priority == plugin_priority)
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::DeactivatePlugin {
                timestamp,
                plugin_priority,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_worker_id.worker_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = metadata
                    .installed_plugins
                    .into_iter()
                    .find(|p| p.priority == plugin_priority)
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
            OplogEntry::StartSpan {
                timestamp,
                span_id,
                parent: parent_id,
                linked_context_id,
                attributes,
            } => Ok(PublicOplogEntry::StartSpan(StartSpanParams {
                timestamp,
                span_id,
                parent_id,
                linked_context: linked_context_id,
                attributes: attributes
                    .0
                    .into_iter()
                    .map(|(k, v)| PublicAttribute {
                        key: k,
                        value: v.into(),
                    })
                    .collect(),
            })),
            OplogEntry::FinishSpan { timestamp, span_id } => {
                Ok(PublicOplogEntry::FinishSpan(FinishSpanParams {
                    timestamp,
                    span_id,
                }))
            }
            OplogEntry::SetSpanAttribute {
                timestamp,
                span_id,
                key,
                value,
            } => Ok(PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
                timestamp,
                span_id,
                key,
                value: value.into(),
            })),
            OplogEntry::ChangePersistenceLevel {
                timestamp,
                persistence_level,
            } => Ok(PublicOplogEntry::ChangePersistenceLevel(
                ChangePersistenceLevelParams {
                    timestamp,
                    persistence_level,
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
            } => {
                let bytes: Vec<u8> = oplog_service
                    .download_payload(owned_worker_id, data)
                    .await?;

                let snapshot_data = raw_snapshot_to_public(RawSnapshotData {
                    data: bytes,
                    mime_type,
                });

                Ok(PublicOplogEntry::Snapshot(SnapshotParams {
                    timestamp,
                    data: snapshot_data,
                }))
            }
        }
    }
}

fn raw_snapshot_to_public(snapshot: RawSnapshotData) -> PublicSnapshotData {
    if snapshot.mime_type == "application/json" {
        match serde_json::from_slice(&snapshot.data) {
            Ok(json_value) => PublicSnapshotData::Json(JsonSnapshotData { data: json_value }),
            Err(_) => PublicSnapshotData::Raw(snapshot),
        }
    } else {
        PublicSnapshotData::Raw(snapshot)
    }
}

async fn try_resolve_agent_id(
    component_service: Arc<dyn ComponentService>,
    worker_id: &WorkerId,
) -> Option<AgentId> {
    if let Ok(component) = component_service
        .get_metadata(worker_id.component_id, None)
        .await
    {
        AgentId::parse(&worker_id.worker_name, &component.metadata).ok()
    } else {
        None
    }
}

async fn enrich_golem_rpc_invoke(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcInvoke,
) -> HostRequestGolemRpcInvoke {
    let agent_id = try_resolve_agent_id(components, &payload.remote_worker_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

async fn enrich_golem_rpc_scheduled_invocation(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcScheduledInvocation,
) -> HostRequestGolemRpcScheduledInvocation {
    let agent_id = try_resolve_agent_id(components, &payload.remote_worker_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

async fn agent_invocation_to_public(
    components: Arc<dyn ComponentService>,
    owned_worker_id: &OwnedWorkerId,
    component_revision: ComponentRevision,
    invocation: AgentInvocation,
) -> Result<PublicAgentInvocation, String> {
    match invocation {
        AgentInvocation::AgentInitialization {
            idempotency_key,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_worker_id.worker_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let constructor_schema = metadata
                .metadata
                .agent_types()
                .first()
                .map(|at| at.constructor.input_schema.clone());

            let constructor_parameters = match constructor_schema {
                Some(schema) => DataValue::try_from_untyped(input, schema)
                    .unwrap_or_else(|_| DataValue::Tuple(ElementValues { elements: vec![] })),
                None => DataValue::Tuple(ElementValues { elements: vec![] }),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentInitialization(
                AgentInitializationParameters {
                    idempotency_key,
                    constructor_parameters,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_worker_id.worker_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let method_schema = metadata
                .metadata
                .agent_types()
                .first()
                .and_then(|at| at.methods.iter().find(|m| m.name == method_name))
                .map(|m| m.input_schema.clone());

            let function_input = match method_schema {
                Some(schema) => DataValue::try_from_untyped(input, schema)
                    .unwrap_or_else(|_| DataValue::Tuple(ElementValues { elements: vec![] })),
                None => DataValue::Tuple(ElementValues { elements: vec![] }),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentMethodInvocation(
                AgentMethodInvocationParameters {
                    idempotency_key,
                    method_name,
                    function_input,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::ManualUpdate { target_revision } => Ok(
            PublicAgentInvocation::ManualUpdate(ManualUpdateParameters { target_revision }),
        ),
        AgentInvocation::SaveSnapshot { .. } => Ok(PublicAgentInvocation::SaveSnapshot(Empty {})),
        AgentInvocation::LoadSnapshot { snapshot, .. } => Ok(PublicAgentInvocation::LoadSnapshot(
            LoadSnapshotParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            },
        )),
        AgentInvocation::ProcessOplogEntries {
            idempotency_key, ..
        } => Ok(PublicAgentInvocation::ProcessOplogEntries(
            ProcessOplogEntriesParameters { idempotency_key },
        )),
    }
}

async fn agent_invocation_result_to_public(
    components: Arc<dyn ComponentService>,
    owned_worker_id: &OwnedWorkerId,
    component_revision: ComponentRevision,
    result: AgentInvocationResult,
) -> Result<PublicAgentInvocationResult, String> {
    match result {
        AgentInvocationResult::AgentInitialization => {
            let _ = components;
            let _ = owned_worker_id;
            let _ = component_revision;
            let output_data = DataValue::Tuple(ElementValues { elements: vec![] });

            Ok(PublicAgentInvocationResult::AgentInitialization(
                AgentInvocationOutputParameters {
                    output: output_data,
                },
            ))
        }
        AgentInvocationResult::AgentMethod { output } => {
            // We don't have the method name in the result, so we can't look up the specific schema.
            let output_data = DataValue::Tuple(ElementValues { elements: vec![] });
            let _ = output;

            Ok(PublicAgentInvocationResult::AgentMethod(
                AgentInvocationOutputParameters {
                    output: output_data,
                },
            ))
        }
        AgentInvocationResult::ManualUpdate => {
            Ok(PublicAgentInvocationResult::ManualUpdate(Empty {}))
        }
        AgentInvocationResult::LoadSnapshot { error } => Ok(
            PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters { error }),
        ),
        AgentInvocationResult::SaveSnapshot { snapshot } => Ok(
            PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            }),
        ),
        AgentInvocationResult::ProcessOplogEntries { error } => Ok(
            PublicAgentInvocationResult::ProcessOplogEntries(FallibleResultParameters { error }),
        ),
    }
}

fn make_plugin_installation_description(
    installation: InstalledPlugin,
) -> PluginInstallationDescription {
    PluginInstallationDescription {
        plugin_priority: installation.priority,
        plugin_name: installation.plugin_name,
        plugin_version: installation.plugin_version,
        parameters: installation.parameters,
    }
}
