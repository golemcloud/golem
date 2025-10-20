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

use crate::durable_host::http::serialized::{
    SerializableErrorCode, SerializableHttpRequest, SerializableResponse,
};
use crate::durable_host::rdbms::serialized::RdbmsRequest;
use crate::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddress,
    SerializableIpAddresses, SerializableStreamError,
};
use crate::durable_host::wasm_rpc::serialized::{
    EnrichedSerializableInvokeRequest, EnrichedSerializableScheduleInvocationRequest,
    SerializableInvokeRequest, SerializableInvokeResult, SerializableScheduleId,
    SerializableScheduleInvocationRequest,
};
use crate::preview2::golem_api_1_x::host::ForkResult;
use crate::services::component::ComponentService;
use crate::services::oplog::OplogService;
use crate::services::plugins::PluginsService;
use crate::services::rdbms::mysql::types as mysql_types;
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::postgres::types as postgres_types;
use crate::services::rdbms::postgres::PostgresType;
use crate::services::rdbms::RdbmsIntoValueAndType;
use crate::services::rpc::RpcError;
use async_trait::async_trait;
use bincode::Decode;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::model::agent::{AgentId, DataValue, RegisteredAgentType};
use golem_common::model::lucene::Query;
use golem_common::model::oplog::{OplogEntry, OplogIndex, SpanData, UpdateDescription};
use golem_common::model::public_oplog::{
    ActivatePluginParameters, BeginRemoteTransactionParameters, CancelInvocationParameters,
    ChangePersistenceLevelParameters, ChangeRetryPolicyParameters, CreateParameters,
    DeactivatePluginParameters, EndRegionParameters, ErrorParameters,
    ExportedFunctionCompletedParameters, ExportedFunctionInvokedParameters,
    ExportedFunctionParameters, FailedUpdateParameters, FinishSpanParameters, GrowMemoryParameters,
    ImportedFunctionInvokedParameters, JumpParameters, LogParameters, ManualUpdateParameters,
    PendingUpdateParameters, PendingWorkerInvocationParameters, PluginInstallationDescription,
    PublicAttribute, PublicExternalSpanData, PublicLocalSpanData, PublicOplogEntry, PublicSpanData,
    PublicUpdateDescription, PublicWorkerInvocation, RemoteTransactionParameters,
    ResourceParameters, RevertParameters, SetSpanAttributeParameters,
    SnapshotBasedUpdateParameters, StartSpanParameters, SuccessfulUpdateParameters,
    TimestampParameter,
};
use golem_common::model::{Empty, OwnedWorkerId, PromiseId, WorkerId, WorkerInvocation};
use golem_common::serialization::try_deserialize as core_try_deserialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::RevertWorkerTarget;
use golem_wasm::analysis::analysed_type::{
    case, field, list, option, record, result, result_err, str, u64, unit_case, variant,
};
use golem_wasm::analysis::{AnalysedFunctionParameter, AnalysedType};
use golem_wasm::{IntoValue, IntoValueAndType, Value, ValueAndType, WitValue};
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;
use golem_common::model::component::{ComponentId, ComponentRevision};

pub struct PublicOplogChunk {
    pub entries: Vec<PublicOplogEntry>,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentRevision,
    pub first_index_in_chunk: OplogIndex,
    pub last_index: OplogIndex,
}

pub async fn get_public_oplog_chunk(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    plugins: Arc<dyn PluginsService>,
    projects: Arc<dyn ProjectService>,
    owned_worker_id: &OwnedWorkerId,
    initial_component_version: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
) -> Result<PublicOplogChunk, String> {
    let raw_entries = oplog_service
        .read(owned_worker_id, initial_oplog_index, count as u64)
        .await;

    let last_index = oplog_service.get_last_index(owned_worker_id).await;

    let mut entries = Vec::new();
    let mut current_component_version = initial_component_version;
    let mut next_oplog_index = initial_oplog_index;
    let mut first_index_in_chunk = None;

    for (index, raw_entry) in raw_entries {
        if first_index_in_chunk.is_none() {
            first_index_in_chunk = Some(index);
        }
        if let Some(version) = raw_entry.specifies_component_version() {
            current_component_version = version;
        }

        let entry = PublicOplogEntry::from_oplog_entry(
            index,
            raw_entry,
            oplog_service.clone(),
            components.clone(),
            plugins.clone(),
            projects.clone(),
            owned_worker_id,
            current_component_version,
        )
        .await?;
        entries.push(entry);
        next_oplog_index = index.next();
    }

    Ok(PublicOplogChunk {
        entries,
        next_oplog_index,
        current_component_version,
        first_index_in_chunk: first_index_in_chunk.unwrap_or(initial_oplog_index),
        last_index,
    })
}

pub struct PublicOplogSearchResult {
    pub entries: Vec<(OplogIndex, PublicOplogEntry)>,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentRevision,
    pub last_index: OplogIndex,
}

pub async fn search_public_oplog(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    plugin_service: Arc<dyn PluginsService>,
    project_service: Arc<dyn ProjectService>,
    owned_worker_id: &OwnedWorkerId,
    initial_component_version: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    query: &str,
) -> Result<PublicOplogSearchResult, String> {
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_version = initial_component_version;

    let query = Query::parse(query)?;

    loop {
        let chunk = get_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            plugin_service.clone(),
            project_service.clone(),
            owned_worker_id,
            current_component_version,
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
        current_component_version = chunk.current_component_version;

        if current_index >= last_index || results.len() >= count {
            break;
        }
    }

    Ok(PublicOplogSearchResult {
        entries: results,
        next_oplog_index: current_index,
        current_component_version,
        last_index,
    })
}

pub async fn find_component_version_at(
    oplog_service: Arc<dyn OplogService>,
    owned_worker_id: &OwnedWorkerId,
    start: OplogIndex,
) -> Result<ComponentRevision, WorkerExecutorError> {
    let mut initial_component_version = ComponentRevision::INITIAL;
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

        if let Some(version) = entry.and_then(|entry| entry.specifies_component_version()) {
            initial_component_version = version;
        }

        current = current.next();
    }

    Ok(initial_component_version)
}

#[async_trait]
pub trait PublicOplogEntryOps: Sized {
    async fn from_oplog_entry(
        oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        plugins: Arc<dyn PluginsService>,
        projects: Arc<dyn ProjectService>,
        owned_worker_id: &OwnedWorkerId,
        component_version: ComponentRevision,
    ) -> Result<Self, String>;
}

#[async_trait]
impl PublicOplogEntryOps for PublicOplogEntry {
    async fn from_oplog_entry(
        oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        plugins: Arc<dyn PluginsService>,
        projects: Arc<dyn ProjectService>,
        owned_worker_id: &OwnedWorkerId,
        component_version: ComponentRevision,
    ) -> Result<Self, String> {
        match value {
            OplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                environment_id,
                created_by,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                wasi_config_vars,
            } => {
                let project_owner = projects
                    .get_project_owner(&project_id)
                    .await
                    .map_err(|err| err.to_string())?;
                let mut initial_plugins = BTreeSet::new();
                for installation_id in initial_active_plugins {
                    let (installation, definition) = plugins
                        .get(
                            &project_owner,
                            &worker_id.component_id,
                            component_version,
                            &installation_id,
                        )
                        .await
                        .map_err(|err| err.to_string())?;
                    let desc = PluginInstallationDescription::from_definition_and_installation(
                        definition,
                        installation,
                    );
                    initial_plugins.insert(desc);
                }
                Ok(PublicOplogEntry::Create(CreateParameters {
                    timestamp,
                    worker_id,
                    component_version,
                    args,
                    env: env.into_iter().collect(),
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins: initial_plugins,
                    wasi_config_vars: wasi_config_vars.into(),
                }))
            }
            OplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                response,
                durable_function_type,
            } => {
                let request_bytes = oplog_service
                    .download_payload(owned_worker_id, &request)
                    .await?;
                let response_bytes = oplog_service
                    .download_payload(owned_worker_id, &response)
                    .await?;
                let request = encode_host_function_request_as_value(
                    oplog_index,
                    components,
                    &function_name,
                    &request_bytes,
                )
                .await?;
                let response = encode_host_function_response_as_value(
                    oplog_index,
                    &function_name,
                    &response_bytes,
                )?;
                Ok(PublicOplogEntry::ImportedFunctionInvoked(
                    ImportedFunctionInvokedParameters {
                        timestamp,
                        function_name,
                        request,
                        response,
                        durable_function_type: durable_function_type.into(),
                    },
                ))
            }
            OplogEntry::ExportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                idempotency_key,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let payload_bytes = oplog_service
                    .download_payload(owned_worker_id, &request)
                    .await?;
                let proto_params: Vec<golem_wasm::protobuf::Val> =
                    core_try_deserialize(&payload_bytes)?.unwrap_or_default();
                let params = proto_params
                    .into_iter()
                    .map(Value::try_from)
                    .collect::<Result<Vec<_>, _>>()?;

                let metadata = components
                    .get_metadata(
                        &owned_worker_id.worker_id.component_id,
                        Some(component_version),
                    )
                    .await
                    .map_err(|err| err.to_string())?;
                let function = metadata.metadata.find_function(&function_name)?.ok_or(
                    format!("Exported function {function_name} not found in component {} version {component_version}", owned_worker_id.component_id())
                )?;

                let param_types: Box<dyn Iterator<Item = &AnalysedFunctionParameter>> =
                    Box::new(function.analysed_export.parameters.iter());

                let request = param_types
                    .zip(params)
                    .map(|(param, value)| ValueAndType::new(value, param.typ.clone()))
                    .collect();

                Ok(PublicOplogEntry::ExportedFunctionInvoked(
                    ExportedFunctionInvokedParameters {
                        timestamp,
                        function_name,
                        request,
                        idempotency_key,
                        trace_id,
                        trace_states,
                        invocation_context: encode_span_data(&invocation_context),
                    },
                ))
            }
            OplogEntry::ExportedFunctionCompleted {
                timestamp,
                response,
                consumed_fuel,
            } => {
                let payload_bytes = oplog_service
                    .download_payload(owned_worker_id, &response)
                    .await?;
                let value_and_type: Option<ValueAndType> = try_deserialize(
                    oplog_index,
                    "ExportedFunctionCompleted payload",
                    &payload_bytes,
                )?;
                Ok(PublicOplogEntry::ExportedFunctionCompleted(
                    ExportedFunctionCompletedParameters {
                        timestamp,
                        response: value_and_type,
                        consumed_fuel,
                    },
                ))
            }
            OplogEntry::Suspend { timestamp } => {
                Ok(PublicOplogEntry::Suspend(TimestampParameter { timestamp }))
            }
            OplogEntry::Error {
                timestamp,
                error,
                retry_from,
            } => Ok(PublicOplogEntry::Error(ErrorParameters {
                timestamp,
                error: error.to_string(""),
                retry_from,
            })),
            OplogEntry::NoOp { timestamp } => {
                Ok(PublicOplogEntry::NoOp(TimestampParameter { timestamp }))
            }
            OplogEntry::Jump { timestamp, jump } => {
                Ok(PublicOplogEntry::Jump(JumpParameters { timestamp, jump }))
            }
            OplogEntry::Interrupted { timestamp } => {
                Ok(PublicOplogEntry::Interrupted(TimestampParameter {
                    timestamp,
                }))
            }
            OplogEntry::Exited { timestamp } => {
                Ok(PublicOplogEntry::Exited(TimestampParameter { timestamp }))
            }
            OplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            } => Ok(PublicOplogEntry::ChangeRetryPolicy(
                ChangeRetryPolicyParameters {
                    timestamp,
                    new_policy: new_policy.into(),
                },
            )),
            OplogEntry::BeginAtomicRegion { timestamp } => {
                Ok(PublicOplogEntry::BeginAtomicRegion(TimestampParameter {
                    timestamp,
                }))
            }
            OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndAtomicRegion(EndRegionParameters {
                timestamp,
                begin_index,
            })),
            OplogEntry::BeginRemoteWrite { timestamp } => {
                Ok(PublicOplogEntry::BeginRemoteWrite(TimestampParameter {
                    timestamp,
                }))
            }
            OplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndRemoteWrite(EndRegionParameters {
                timestamp,
                begin_index,
            })),
            OplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
            } => {
                let invocation = match invocation {
                    WorkerInvocation::ExportedFunction {
                        idempotency_key,
                        full_function_name,
                        function_input,
                        invocation_context,
                    } => {
                        let metadata = components
                            .get_metadata(
                                &owned_worker_id.worker_id.component_id,
                                Some(component_version),
                            )
                            .await
                            .map_err(|err| err.to_string())?;

                        let function = metadata.metadata.find_function(&full_function_name)?;

                        // It is not guaranteed that we can resolve the enqueued invocation's parameter types because
                        // we only know the current component version. If the client enqueued an update earlier and assumes
                        // it will succeed, it is possible that it enqueues an invocation using a future API.
                        //
                        // If we cannot resolve the type, we leave the `function_input` field empty in the public oplog.
                        let mut params = None;
                        if let Some(function) = function {
                            if function.analysed_export.parameters.len() == function_input.len() {
                                params = Some(
                                    function
                                        .analysed_export
                                        .parameters
                                        .iter()
                                        .zip(function_input)
                                        .map(|(param, value)| {
                                            ValueAndType::new(value, param.typ.clone())
                                        })
                                        .collect(),
                                );
                            }
                        }

                        let span_data = invocation_context.to_oplog_data();

                        PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                            idempotency_key,
                            full_function_name,
                            function_input: params,
                            trace_id: invocation_context.trace_id.clone(),
                            trace_states: invocation_context.trace_states.clone(),
                            invocation_context: encode_span_data(&span_data),
                        })
                    }
                    WorkerInvocation::ManualUpdate { target_version } => {
                        PublicWorkerInvocation::ManualUpdate(ManualUpdateParameters {
                            target_version,
                        })
                    }
                };
                Ok(PublicOplogEntry::PendingWorkerInvocation(
                    PendingWorkerInvocationParameters {
                        timestamp,
                        invocation,
                    },
                ))
            }
            OplogEntry::PendingUpdate {
                timestamp,
                description,
            } => {
                let target_version = *description.target_version();
                let public_description = match description {
                    UpdateDescription::Automatic { .. } => {
                        PublicUpdateDescription::Automatic(Empty {})
                    }
                    UpdateDescription::SnapshotBased { payload, .. } => {
                        let bytes = oplog_service
                            .download_payload(owned_worker_id, &payload)
                            .await?;
                        PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                            payload: bytes.to_vec(),
                        })
                    }
                };
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
                    timestamp,
                    target_version,
                    description: public_description,
                }))
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
                new_component_size,
                new_active_plugins,
            } => {
                let project_owner = projects
                    .get_project_owner(&owned_worker_id.project_id)
                    .await
                    .map_err(|err| err.to_string())?;
                let mut new_plugins = BTreeSet::new();
                for installation_id in new_active_plugins {
                    let (installation, definition) = plugins
                        .get(
                            &project_owner,
                            &owned_worker_id.worker_id.component_id,
                            target_version,
                            &installation_id,
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    let desc = PluginInstallationDescription::from_definition_and_installation(
                        definition,
                        installation,
                    );
                    new_plugins.insert(desc);
                }
                Ok(PublicOplogEntry::SuccessfulUpdate(
                    SuccessfulUpdateParameters {
                        timestamp,
                        target_version,
                        new_component_size,
                        new_active_plugins: new_plugins,
                    },
                ))
            }
            OplogEntry::FailedUpdate {
                timestamp,
                target_version,
                details,
            } => Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
                timestamp,
                target_version,
                details,
            })),
            OplogEntry::GrowMemory { timestamp, delta } => {
                Ok(PublicOplogEntry::GrowMemory(GrowMemoryParameters {
                    timestamp,
                    delta,
                }))
            }
            OplogEntry::CreateResource {
                timestamp,
                id,
                resource_type_id,
            } => Ok(PublicOplogEntry::CreateResource(ResourceParameters {
                timestamp,
                id,
                name: resource_type_id.name,
                owner: resource_type_id.owner,
            })),
            OplogEntry::DropResource {
                timestamp,
                id,
                resource_type_id,
            } => Ok(PublicOplogEntry::DropResource(ResourceParameters {
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
            } => Ok(PublicOplogEntry::Log(LogParameters {
                timestamp,
                level,
                context,
                message,
            })),
            OplogEntry::Restart { timestamp } => {
                Ok(PublicOplogEntry::Restart(TimestampParameter { timestamp }))
            }
            OplogEntry::ActivatePlugin { timestamp, plugin } => {
                let project_owner = projects
                    .get_project_owner(&owned_worker_id.project_id)
                    .await
                    .map_err(|err| err.to_string())?;
                let (installation, definition) = plugins
                    .get(
                        &project_owner,
                        &owned_worker_id.worker_id.component_id,
                        component_version,
                        &plugin,
                    )
                    .await
                    .map_err(|err| err.to_string())?;
                let desc = PluginInstallationDescription::from_definition_and_installation(
                    definition,
                    installation,
                );
                Ok(PublicOplogEntry::ActivatePlugin(ActivatePluginParameters {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::DeactivatePlugin { timestamp, plugin } => {
                let project_owner = projects
                    .get_project_owner(&owned_worker_id.project_id)
                    .await
                    .map_err(|err| err.to_string())?;
                let (installation, definition) = plugins
                    .get(
                        &project_owner,
                        &owned_worker_id.worker_id.component_id,
                        component_version,
                        &plugin,
                    )
                    .await
                    .map_err(|err| err.to_string())?;
                let desc = PluginInstallationDescription::from_definition_and_installation(
                    definition,
                    installation,
                );
                Ok(PublicOplogEntry::DeactivatePlugin(
                    DeactivatePluginParameters {
                        timestamp,
                        plugin: desc,
                    },
                ))
            }
            OplogEntry::Revert {
                timestamp,
                dropped_region,
            } => Ok(PublicOplogEntry::Revert(RevertParameters {
                timestamp,
                dropped_region,
            })),
            OplogEntry::CancelPendingInvocation {
                timestamp,
                idempotency_key,
            } => Ok(PublicOplogEntry::CancelInvocation(
                CancelInvocationParameters {
                    timestamp,
                    idempotency_key,
                },
            )),
            OplogEntry::StartSpan {
                timestamp,
                span_id,
                parent_id,
                linked_context_id,
                attributes,
            } => Ok(PublicOplogEntry::StartSpan(StartSpanParameters {
                timestamp,
                span_id,
                parent_id,
                linked_context: linked_context_id,
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| PublicAttribute {
                        key: k,
                        value: v.into(),
                    })
                    .collect(),
            })),
            OplogEntry::FinishSpan { timestamp, span_id } => {
                Ok(PublicOplogEntry::FinishSpan(FinishSpanParameters {
                    timestamp,
                    span_id,
                }))
            }
            OplogEntry::SetSpanAttribute {
                timestamp,
                span_id,
                key,
                value,
            } => Ok(PublicOplogEntry::SetSpanAttribute(
                SetSpanAttributeParameters {
                    timestamp,
                    span_id,
                    key,
                    value: value.into(),
                },
            )),
            OplogEntry::ChangePersistenceLevel { timestamp, level } => Ok(
                PublicOplogEntry::ChangePersistenceLevel(ChangePersistenceLevelParameters {
                    timestamp,
                    persistence_level: level,
                }),
            ),
            OplogEntry::BeginRemoteTransaction {
                timestamp,
                transaction_id,
                ..
            } => Ok(PublicOplogEntry::BeginRemoteTransaction(
                BeginRemoteTransactionParameters {
                    timestamp,
                    transaction_id,
                },
            )),
            OplogEntry::PreCommitRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreCommitRemoteTransaction(
                RemoteTransactionParameters {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::PreRollbackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreRollbackRemoteTransaction(
                RemoteTransactionParameters {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::CommittedRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::CommittedRemoteTransaction(
                RemoteTransactionParameters {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::RolledBackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::RolledBackRemoteTransaction(
                RemoteTransactionParameters {
                    timestamp,
                    begin_index,
                },
            )),
        }
    }
}

fn try_deserialize<T: Decode<()>>(
    oplog_idx: OplogIndex,
    what: &str,
    data: &[u8],
) -> Result<T, String> {
    core_try_deserialize(data)
        .map_err(|err| format!("Oplog entry #{oplog_idx} - {what}: {err}"))?
        .ok_or("Unexpected oplog payload, cannot deserialize".to_string())
}

fn no_payload() -> Result<ValueAndType, String> {
    Ok(ValueAndType::new(Value::Option(None), option(str())))
}

async fn try_resolve_agent_id(
    component_service: Arc<dyn ComponentService>,
    worker_id: &WorkerId,
) -> Option<AgentId> {
    if let Ok(component) = component_service
        .get_metadata(&worker_id.component_id, None)
        .await
    {
        AgentId::parse(&worker_id.worker_name, &component.metadata).ok()
    } else {
        None
    }
}

async fn enrich_serializable_invoke_request(
    components: Arc<dyn ComponentService>,
    payload: SerializableInvokeRequest,
) -> EnrichedSerializableInvokeRequest {
    let agent_id = try_resolve_agent_id(components, &payload.remote_worker_id).await;
    EnrichedSerializableInvokeRequest {
        remote_worker_id: payload.remote_worker_id,
        remote_agent_type: agent_id
            .as_ref()
            .map(|agent_id| agent_id.agent_type.clone()),
        remote_agent_parameters: agent_id.map(|agent_id| agent_id.parameters),
        idempotency_key: payload.idempotency_key,
        function_name: payload.function_name,
        function_params: payload.function_params,
    }
}

async fn enrich_serializable_schedule_invocation_request(
    components: Arc<dyn ComponentService>,
    payload: SerializableScheduleInvocationRequest,
) -> EnrichedSerializableScheduleInvocationRequest {
    let agent_id = try_resolve_agent_id(components, &payload.remote_worker_id).await;
    EnrichedSerializableScheduleInvocationRequest {
        remote_worker_id: payload.remote_worker_id,
        remote_agent_type: agent_id
            .as_ref()
            .map(|agent_id| agent_id.agent_type.clone()),
        remote_agent_parameters: agent_id.map(|agent_id| agent_id.parameters),
        idempotency_key: payload.idempotency_key,
        function_name: payload.function_name,
        function_params: payload.function_params,
        datetime: payload.datetime,
    }
}

async fn encode_host_function_request_as_value(
    oplog_index: OplogIndex,
    components: Arc<dyn ComponentService>,
    function_name: &str,
    bytes: &[u8],
) -> Result<ValueAndType, String> {
    let what = format!("{function_name} input");
    match function_name {
        "golem::rpc::future-invoke-result::get" => {
            let payload: SerializableInvokeRequest = try_deserialize(oplog_index, &what, bytes)?;
            let payload = enrich_serializable_invoke_request(components, payload).await;
            Ok(payload.into_value_and_type())
        }
        "http::types::future_incoming_response::get" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem io::poll::poll" => {
            let count: usize = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(Value::U64(count as u64), u64()))
        }
        "golem io::poll::ready" => no_payload(),
        "golem blobstore::container::object_info" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container_and_object(payload.0, payload.1))
        }
        "golem blobstore::container::delete_objects" => {
            let payload: (String, Vec<String>) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container_and_objects(payload.0, payload.1))
        }
        "golem blobstore::container::list_objects" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::container::get_data" => {
            let payload: (String, String, u64, u64) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::String(payload.1),
                    Value::U64(payload.2),
                    Value::U64(payload.3),
                ]),
                record(vec![
                    field("container", str()),
                    field("object", str()),
                    field("begin", u64()),
                    field("end", u64()),
                ]),
            ))
        }
        "golem blobstore::container::write_data" => {
            let payload: (String, String, u64) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::String(payload.1),
                    Value::U64(payload.2),
                ]),
                record(vec![
                    field("container", str()),
                    field("object", str()),
                    field("length", u64()),
                ]),
            ))
        }
        "golem blobstore::container::delete_object" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container_and_object(payload.0, payload.1))
        }
        "golem blobstore::container::has_object" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container_and_object(payload.0, payload.1))
        }
        "golem blobstore::container::clear" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::blobstore::copy_object" => {
            let payload: (String, String, String, String) =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::String(payload.1),
                    Value::String(payload.2),
                    Value::String(payload.3),
                ]),
                record(vec![
                    field("src_container", str()),
                    field("src_object", str()),
                    field("dest_container", str()),
                    field("dest_object", str()),
                ]),
            ))
        }
        "golem blobstore::blobstore::delete_container" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::blobstore::create_container" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::blobstore::get_container" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::blobstore::container_exists" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(container(payload))
        }
        "golem blobstore::blobstore::move_object" => {
            let payload: (String, String, String, String) =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::String(payload.1),
                    Value::String(payload.2),
                    Value::String(payload.3),
                ]),
                record(vec![
                    field("src_container", str()),
                    field("src_object", str()),
                    field("dest_container", str()),
                    field("dest_object", str()),
                ]),
            ))
        }
        "golem_environment::get_arguments" => no_payload(),
        "golem_environment::get_environment" => no_payload(),
        "golem_environment::initial_cwd" => no_payload(),
        "monotonic_clock::resolution" => no_payload(),
        "monotonic_clock::now" => no_payload(),
        "monotonic_clock::subscribe_duration" => {
            let duration_ns: u64 = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(Value::U64(duration_ns), u64()))
        }
        "wall_clock::now" => no_payload(),
        "wall_clock::resolution" => no_payload(),
        "golem::api::create_promise" => no_payload(),
        "golem::api::complete_promise" => {
            let payload: PromiseId = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::get-promise-result::get" => no_payload(),
        "golem::api::update-worker" => {
            let payload: (WorkerId, ComponentRevision, UpdateMode) =
                try_deserialize(oplog_index, &what, bytes)?;
            let agent_id = try_resolve_agent_id(components, &payload.0).await;

            Ok(ValueAndType::new(
                Value::Record(vec![
                    payload.0.into_value(),
                    agent_id
                        .as_ref()
                        .map(|agent_id| agent_id.agent_type.clone())
                        .into_value(),
                    agent_id.map(|agent_id| agent_id.parameters).into_value(),
                    payload.1.into_value(),
                    Value::String(format!("{:?}", payload.2)),
                ]),
                record(vec![
                    field("worker_id", WorkerId::get_type()),
                    field("agent_type", Option::<String>::get_type()),
                    field("agent_parameters", Option::<DataValue>::get_type()),
                    field("component_version", u64()),
                    field("update_mode", str()),
                ]),
            ))
        }
        "golem::api::fork-worker" => {
            let payload: (WorkerId, WorkerId, OplogIndex) =
                try_deserialize(oplog_index, &what, bytes)?;
            let source_agent_id = try_resolve_agent_id(components.clone(), &payload.0).await;
            let target_agent_id = try_resolve_agent_id(components, &payload.1).await;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    payload.0.into_value(),
                    source_agent_id
                        .as_ref()
                        .map(|agent_id| agent_id.agent_type.clone())
                        .into_value(),
                    source_agent_id
                        .map(|agent_id| agent_id.parameters)
                        .into_value(),
                    payload.1.into_value(),
                    target_agent_id
                        .as_ref()
                        .map(|agent_id| agent_id.agent_type.clone())
                        .into_value(),
                    target_agent_id
                        .map(|agent_id| agent_id.parameters)
                        .into_value(),
                    payload.2.into_value(),
                ]),
                record(vec![
                    field("source_worker_id", WorkerId::get_type()),
                    field("source_agent_type", Option::<String>::get_type()),
                    field("source_agent_parameters", Option::<DataValue>::get_type()),
                    field("target_worker_id", WorkerId::get_type()),
                    field("target_agent_type", Option::<String>::get_type()),
                    field("target_agent_parameters", Option::<DataValue>::get_type()),
                    field("oplog_idx_cut_off", u64()),
                ]),
            ))
        }
        "golem::api::fork" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::revert-worker" => {
            let payload: (WorkerId, RevertWorkerTarget) =
                try_deserialize(oplog_index, &what, bytes)?;
            let agent_id = try_resolve_agent_id(components, &payload.0).await;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    payload.0.into_value(),
                    agent_id
                        .as_ref()
                        .map(|agent_id| agent_id.agent_type.clone())
                        .into_value(),
                    agent_id.map(|agent_id| agent_id.parameters).into_value(),
                    payload.1.into_value(),
                ]),
                record(vec![
                    field("worker_id", WorkerId::get_type()),
                    field("agent_type", Option::<String>::get_type()),
                    field("agent_parameters", Option::<DataValue>::get_type()),
                    field("target", RevertWorkerTarget::get_type()),
                ]),
            ))
        }
        "http::types::incoming_body_stream::skip" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::read" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::blocking_read" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::blocking_skip" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual::delete" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket_and_key(payload.0, payload.1))
        }
        "golem keyvalue::eventual::get" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket_and_key(payload.0, payload.1))
        }
        "golem keyvalue::eventual::set" => {
            let payload: (String, String, u64) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::String(payload.1),
                    Value::U64(payload.2),
                ]),
                record(vec![
                    field("bucket", str()),
                    field("key", str()),
                    field("value", u64()),
                ]),
            ))
        }
        "golem keyvalue::eventual::exists" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket_and_key(payload.0, payload.1))
        }
        "golem keyvalue::eventual_batch::set_many" => {
            let payload: (String, Vec<(String, u64)>) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(ValueAndType::new(
                Value::Record(vec![
                    Value::String(payload.0),
                    Value::List(
                        payload
                            .1
                            .into_iter()
                            .map(|(key, value)| {
                                Value::Record(vec![Value::String(key), Value::U64(value)])
                            })
                            .collect(),
                    ),
                ]),
                record(vec![
                    field("bucket", str()),
                    field(
                        "key_values",
                        list(record(vec![field("key", str()), field("length", u64())])),
                    ),
                ]),
            ))
        }
        "golem keyvalue::eventual_batch::get_many" => {
            let payload: (String, Vec<String>) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket_and_keys(payload.0, payload.1))
        }
        "golem keyvalue::eventual_batch::get_keys" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket(payload))
        }
        "golem keyvalue::eventual_batch::delete_many" => {
            let payload: (String, Vec<String>) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(bucket_and_keys(payload.0, payload.1))
        }
        "golem random::insecure::get_insecure_random_bytes" => no_payload(),
        "golem random::insecure::get_insecure_random_u64" => no_payload(),
        "golem random::insecure_seed::insecure_seed" => no_payload(),
        "golem random::get_random_bytes" => no_payload(),
        "golem random::get_random_u64" => no_payload(),
        "sockets::ip_name_lookup::resolve_addresses" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke" => {
            let payload: SerializableInvokeRequest = try_deserialize(oplog_index, &what, bytes)?;
            let payload = enrich_serializable_invoke_request(components, payload).await;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke-and-await"
        | "golem::rpc::wasm-rpc::invoke-and-await result" => {
            let payload: SerializableInvokeRequest = try_deserialize(oplog_index, &what, bytes)?;
            let payload = enrich_serializable_invoke_request(components, payload).await;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => no_payload(),
        "filesystem::types::descriptor::stat" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "filesystem::types::descriptor::stat_at" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem api::generate_idempotency_key" => no_payload(),
        "golem http::types::future_trailers::get" => {
            let payload: SerializableHttpRequest = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke idempotency key" => no_payload(),
        "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => no_payload(),
        "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => no_payload(),
        "golem::rpc::wasm-rpc::schedule_invocation" => {
            let payload: SerializableScheduleInvocationRequest =
                try_deserialize(oplog_index, &what, bytes)?;
            let payload =
                enrich_serializable_schedule_invocation_request(components, payload).await;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::cancellation-token::cancel" => {
            let payload: SerializableScheduleId = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::resolve_component_id" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::resolve_worker_id_strict" => {
            let payload: (String, String) = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "rdbms::mysql::db-connection::query"
        | "rdbms::mysql::db-connection::execute"
        | "rdbms::mysql::db-connection::query-stream"
        | "rdbms::mysql::db-transaction::query"
        | "rdbms::mysql::db-transaction::execute"
        | "rdbms::mysql::db-transaction::query-stream" => {
            let payload: Option<RdbmsRequest<MysqlType>> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::mysql::db-result-stream::get-columns"
        | "rdbms::mysql::db-result-stream::get-next" => no_payload(),
        "rdbms::postgres::db-connection::query"
        | "rdbms::postgres::db-connection::execute"
        | "rdbms::postgres::db-connection::query-stream"
        | "rdbms::postgres::db-transaction::query"
        | "rdbms::postgres::db-transaction::execute"
        | "rdbms::postgres::db-transaction::query-stream" => {
            let payload: Option<RdbmsRequest<PostgresType>> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::postgres::db-result-stream::get-columns"
        | "rdbms::postgres::db-result-stream::get-next" => no_payload(),
        "golem_agent::get_all_agent_types" => no_payload(),
        "golem_agent::get_agent_type" => {
            let payload: String = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        _ => {
            // For everything else we assume that payload is a serialized ValueAndType
            let payload: ValueAndType = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload)
        }
    }
}

#[allow(clippy::type_complexity)]
fn encode_host_function_response_as_value(
    oplog_index: OplogIndex,
    function_name: &str,
    bytes: &[u8],
) -> Result<ValueAndType, String> {
    let what = format!("{function_name} output");
    match function_name {
        "golem::rpc::future-invoke-result::get" => {
            let payload: SerializableInvokeResult = try_deserialize(oplog_index, &what, bytes)?;
            match payload {
                SerializableInvokeResult::Failed(error) => Ok(ValueAndType::new(
                    Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(error.into_value())),
                    },
                    variant(vec![
                        case("Failed", SerializableError::get_type()),
                        unit_case("Pending"),
                        unit_case("Completed"),
                    ]),
                )),
                SerializableInvokeResult::Pending => Ok(ValueAndType::new(
                    Value::Variant {
                        case_idx: 1,
                        case_value: None,
                    },
                    variant(vec![
                        case("Failed", SerializableError::get_type()),
                        unit_case("Pending"),
                        unit_case("Completed"),
                    ]),
                )),
                SerializableInvokeResult::Completed(Ok(Some(value))) => {
                    let ValueAndType { value, typ } = value;
                    Ok(ValueAndType::new(
                        Value::Variant {
                            case_idx: 2,
                            case_value: Some(Box::new(Value::Result(Ok(Some(Box::new(value)))))),
                        },
                        variant(vec![
                            case("Failed", SerializableError::get_type()),
                            unit_case("Pending"),
                            case("Completed", result(typ, RpcError::get_type())),
                        ]),
                    ))
                }
                SerializableInvokeResult::Completed(Ok(None)) => Ok(ValueAndType::new(
                    Value::Variant {
                        case_idx: 2,
                        case_value: Some(Box::new(Value::Result(Ok(None)))),
                    },
                    variant(vec![
                        case("Failed", SerializableError::get_type()),
                        unit_case("Pending"),
                        case("Completed", result_err(RpcError::get_type())),
                    ]),
                )),
                SerializableInvokeResult::Completed(Err(rpc_error)) => Ok(ValueAndType::new(
                    Value::Variant {
                        case_idx: 2,
                        case_value: Some(Box::new(Value::Result(Err(Some(Box::new(
                            rpc_error.into_value(),
                        )))))),
                    },
                    variant(vec![
                        case("Failed", SerializableError::get_type()),
                        unit_case("Pending"),
                        case("Completed", result(record(vec![]), RpcError::get_type())),
                    ]),
                )),
            }
        }
        "http::types::future_incoming_response::get" => {
            let payload: SerializableResponse = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem io::poll::poll" => {
            let payload: Result<Vec<u32>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem io::poll::ready" => {
            let payload: Result<bool, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::object_info" => {
            let payload: Result<crate::services::blob_store::ObjectMetadata, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::delete_objects" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::list_objects" => {
            let payload: Result<Vec<String>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::get_data" => {
            let payload: Result<Vec<u8>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::write_data" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::delete_object" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::has_object" => {
            let payload: Result<bool, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::container::clear" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::copy_object" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::delete_container" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::create_container" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::get_container" => {
            let payload: Result<Option<u64>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::container_exists" => {
            let payload: Result<bool, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem blobstore::blobstore::move_object" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem_environment::get_arguments" => {
            let payload: Result<Vec<String>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem_environment::get_environment" => {
            let payload: Result<Vec<(String, String)>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem_environment::initial_cwd" => {
            let payload: Result<Option<String>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "monotonic_clock::resolution" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "monotonic_clock::now" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "monotonic_clock::subscribe_duration" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "wall_clock::now" => {
            let payload: Result<SerializableDateTime, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "wall_clock::resolution" => {
            let payload: Result<SerializableDateTime, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::create_promise" => {
            let payload: Result<PromiseId, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::get-promise-result::get" => {
            let payload: Result<Option<Vec<u8>>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::complete_promise" => {
            let payload: Result<bool, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::update-worker" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::fork-worker" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::fork" => {
            let payload: Result<ForkResult, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::revert-worker" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::skip" => {
            let payload: Result<u64, SerializableStreamError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::read" => {
            let payload: Result<Vec<u8>, SerializableStreamError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::blocking_read" => {
            let payload: Result<Vec<u8>, SerializableStreamError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "http::types::incoming_body_stream::blocking_skip" => {
            let payload: Result<u64, SerializableStreamError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual::delete" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual::get" => {
            let payload: Result<Option<Vec<u8>>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual::set" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual::exists" => {
            let payload: Result<bool, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual_batch::set_many" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual_batch::get_many" => {
            let payload: Result<Vec<Option<Vec<u8>>>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual_batch::get_keys" => {
            let payload: Result<Vec<String>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem keyvalue::eventual_batch::delete_many" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem random::insecure::get_insecure_random_bytes" => {
            let payload: Result<Vec<u8>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem random::insecure::get_insecure_random_u64" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem random::insecure_seed::insecure_seed" => {
            let payload: Result<(u64, u64), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem random::get_random_bytes" => {
            let payload: Result<Vec<u8>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem random::get_random_u64" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "sockets::ip_name_lookup::resolve_addresses" => {
            let payload: Result<SerializableIpAddresses, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke-and-await"
        | "golem::rpc::wasm-rpc::invoke-and-await result" => {
            let payload: Result<Result<Option<ValueAndType>, SerializableError>, String> =
                try_deserialize(oplog_index, &what, bytes);

            match payload {
                Err(_) => {
                    let _payload: Result<WitValue, SerializableError> =
                        try_deserialize(oplog_index, &what, bytes)?;
                    no_payload()
                }
                Ok(Ok(Some(payload))) => {
                    let ValueAndType { value, typ } = payload;
                    Ok(ValueAndType::new(
                        Value::Result(Ok(Some(Box::new(value)))),
                        result(typ, SerializableError::get_type()),
                    ))
                }
                Ok(Ok(None)) => Ok(ValueAndType::new(
                    Value::Result(Ok(None)),
                    result_err(SerializableError::get_type()),
                )),
                Ok(Err(error)) => Ok(ValueAndType::new(
                    Value::Result(Err(Some(Box::new(error.into_value())))),
                    result_err(SerializableError::get_type()),
                )),
            }
        }
        "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => {
            let payload: Result<WorkerId, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "filesystem::types::descriptor::stat" => {
            let payload: Result<SerializableFileTimes, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "filesystem::types::descriptor::stat_at" => {
            let payload: Result<SerializableFileTimes, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem api::generate_idempotency_key" => {
            let payload = try_deserialize::<Result<(u64, u64), SerializableError>>(
                oplog_index,
                &what,
                bytes,
            )?
            .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
            Ok(payload.into_value_and_type())
        }
        "golem http::types::future_trailers::get" => {
            let payload: Result<
                Option<Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableErrorCode>, ()>>,
                SerializableError,
            > = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke idempotency key" => {
            let payload = try_deserialize::<Result<(u64, u64), SerializableError>>(
                oplog_index,
                &what,
                bytes,
            )?
            .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => {
            let payload = try_deserialize::<Result<(u64, u64), SerializableError>>(
                oplog_index,
                &what,
                bytes,
            )?
            .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => {
            let payload = try_deserialize::<Result<(u64, u64), SerializableError>>(
                oplog_index,
                &what,
                bytes,
            )?
            .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::wasm-rpc::schedule_invocation" => {
            let payload: Result<SerializableScheduleId, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::rpc::cancellation-token::cancel" => {
            let payload: Result<(), SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::resolve_component_id" => {
            let payload: Result<Option<ComponentId>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem::api::resolve_worker_id_strict" => {
            let payload: Result<Option<WorkerId>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "rdbms::mysql::db-connection::execute" | "rdbms::mysql::db-transaction::execute" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "rdbms::mysql::db-connection::query" | "rdbms::mysql::db-transaction::query" => {
            let payload: Result<crate::services::rdbms::DbResult<MysqlType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::mysql::db-connection::query-stream"
        | "rdbms::mysql::db-transaction::query-stream" => {
            let payload: Result<RdbmsRequest<MysqlType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::mysql::db-result-stream::get-columns" => {
            let payload: Result<Vec<mysql_types::DbColumn>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::mysql::db-result-stream::get-next" => {
            let payload: Result<
                Option<Vec<crate::services::rdbms::DbRow<mysql_types::DbValue>>>,
                SerializableError,
            > = try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::postgres::db-connection::execute" | "rdbms::postgres::db-transaction::execute" => {
            let payload: Result<u64, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "rdbms::postgres::db-connection::query" | "rdbms::postgres::db-transaction::query" => {
            let payload: Result<crate::services::rdbms::DbResult<PostgresType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::postgres::db-connection::query-stream"
        | "rdbms::postgres::db-transaction::query-stream" => {
            let payload: Result<RdbmsRequest<PostgresType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::postgres::db-result-stream::get-columns" => {
            let payload: Result<Vec<postgres_types::DbColumn>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "rdbms::postgres::db-result-stream::get-next" => {
            let payload: Result<
                Option<Vec<crate::services::rdbms::DbRow<postgres_types::DbValue>>>,
                SerializableError,
            > = try_deserialize(oplog_index, &what, bytes)?;
            Ok(RdbmsIntoValueAndType::into_value_and_type(payload))
        }
        "golem_agent::get_agent_type" => {
            let payload: Result<Option<RegisteredAgentType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        "golem_agent::get_all_agent_types" => {
            let payload: Result<Vec<RegisteredAgentType>, SerializableError> =
                try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload.into_value_and_type())
        }
        _ => {
            // For everything else we assume that payload is a serialized ValueAndType
            let payload: ValueAndType = try_deserialize(oplog_index, &what, bytes)?;
            Ok(payload)
        }
    }
}

impl IntoValue for SerializableIpAddress {
    fn into_value(self) -> Value {
        let addr = match self {
            SerializableIpAddress::IPv4 { address } => IpAddr::V4(address.into()),
            SerializableIpAddress::IPv6 { address } => IpAddr::V6(address.into()),
        };
        Value::String(addr.to_string())
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

fn container(container: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(container)]),
        record(vec![field("container", str())]),
    )
}

fn container_and_object(container: String, object: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(container), Value::String(object)]),
        record(vec![field("container", str()), field("object", str())]),
    )
}

fn container_and_objects(container: String, objects: Vec<String>) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![
            Value::String(container),
            Value::List(objects.into_iter().map(Value::String).collect()),
        ]),
        record(vec![
            field("container", str()),
            field("objects", list(str())),
        ]),
    )
}

fn bucket(bucket: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(bucket)]),
        record(vec![field("bucket", str())]),
    )
}

fn bucket_and_key(bucket: String, key: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(bucket), Value::String(key)]),
        record(vec![field("bucket", str()), field("key", str())]),
    )
}

fn bucket_and_keys(bucket: String, keys: Vec<String>) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![
            Value::String(bucket),
            Value::List(keys.into_iter().map(Value::String).collect()),
        ]),
        record(vec![field("bucket", str()), field("keys", list(str()))]),
    )
}

fn encode_span_data(spans: &[SpanData]) -> Vec<Vec<PublicSpanData>> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for span in spans.iter().rev() {
        match span {
            SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited,
            } => {
                let linked_context = if let Some(linked_context) = linked_context {
                    let mut encoded_linked_context = encode_span_data(linked_context);

                    // Before merging encoded_linked_context into result, we need to adjust the indices in it
                    for spans in encoded_linked_context.iter_mut() {
                        for span in spans.iter_mut() {
                            match span {
                                PublicSpanData::LocalSpan(local_span) => {
                                    if let Some(idx) = local_span.linked_context.as_mut() {
                                        *idx += (result.len() as u64) + 1;
                                    }
                                }
                                PublicSpanData::ExternalSpan(_) => {}
                            }
                        }
                    }

                    result.extend(encoded_linked_context);

                    let id = result.len() as u64 + 1;
                    Some(id)
                } else {
                    None
                };
                let span_data = PublicSpanData::LocalSpan(PublicLocalSpanData {
                    span_id: span_id.clone(),
                    start: *start,
                    parent_id: parent_id.clone(),
                    linked_context,
                    attributes: attributes
                        .iter()
                        .map(|(k, v)| PublicAttribute {
                            key: k.clone(),
                            value: v.clone().into(),
                        })
                        .collect(),
                    inherited: *inherited,
                });
                current.insert(0, span_data);
            }
            SpanData::ExternalSpan { span_id } => {
                let span_data = PublicSpanData::ExternalSpan(PublicExternalSpanData {
                    span_id: span_id.clone(),
                });
                current.insert(0, span_data);
            }
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
    result
}
