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
    DurableStreamingTaskParams, FutureInvokeResultEntry, FutureInvokeResultState,
    PreparedRpcInvocation, RpcCallTarget, WasmRpcEntry, WasmRpcEntryPayload,
    await_streaming_rpc_acceptance, bake_closed_async_error, caller_durable_rpc_streams,
    complete_rpc_preparation_failure, construct_ephemeral_wasm_rpc_resource,
    construct_wasm_rpc_resource, current_streaming_origin, denied_rpc_failure,
    discard_owned_rpc_input, embedded_invocation_span_builder, ensure_rpc_target_activated,
    find_agent_method, invocation_metadata, known_fresh_dispatch_allowed,
    persist_async_invoke_denial_from_begun, persist_async_validation_rejection,
    persist_invoke_and_await_denial_from_begun, persist_invoke_denial_from_begun,
    prepare_rpc_invocation, prepare_rpc_invocation_identity, reconstruct_wasm_rpc_resource,
    recorded_rpc_denial, reject_non_await_scope_card, remote_method_uses_streams,
    replay_async_rpc_denial, replay_rpc_denial, resolve_method_and_lift_input,
    restore_embedded_span, rpc_input_root, rpc_output_root, rpc_span_finished, run_invoke,
    run_invoke_and_await, schema_value_to_wire_output, spawn_invoke_and_await_task,
    spawn_streaming_invoke_and_await_task, start_rpc_binding_call, start_rpc_connection_call,
    streaming_target_fingerprint,
};
use super::{EphemeralRpcTarget, dispatch_target};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::concurrent::record_released_resource_span;
use crate::durable_host::durable_session::durable_stream_mapping_from_proto;
use crate::durable_host::durable_session::strip_streams;
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
use crate::durable_host::golem::v1x::agent_operation_denied;
use crate::durable_host::permissions::resolve_invocation_scope_card;
use crate::preview2::golem::agent::common::AgentError as WitAgentError;
use crate::preview2::golem::agent::host::AsyncInvocationWithMetadata;
use crate::preview2::golem::agent::host::CancelableScheduledInvocationReceipt;
use crate::preview2::golem::agent::host::HostWasmRpc;
use crate::preview2::golem::agent::host::InvocationMetadata;
use crate::preview2::golem::agent::host::InvocationResultWithMetadata;
use crate::preview2::golem::agent::host::RpcError;
use crate::preview2::golem::agent::host::ScheduledInvocationReceipt;
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::{ResolvedTarget, keyed_target};
use crate::workerctx::InvocationContextManagement;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use futures::FutureExt;
use futures::future::Either;
use golem_common::base_model::agent::AgentMode;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::InvocationRequestKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::agent::{AgentTypeName, ParsedAgentKey, RegisteredAgentType};
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentName;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::HostResponseGolemRpcCreate;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::HostResponseGolemRpcUnitOrFailure;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcBindSelf;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvoke;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::model::oplog::host_functions::{
    GolemAgentLookupAgentId, GolemRpcWasmRpcGetById, GolemRpcWasmRpcUpsert,
};
use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::oplog::{HostRequestGolemApiAgentId, HostResponseGolemAgentLookupAgentId};
use golem_common::model::oplog::{HostRequestGolemRpcBindSelf, HostResponseGolemRpcUnit};
use golem_common::model::oplog::{HostRequestGolemRpcGetById, HostResponseGolemRpcCreated};
use golem_common::schema::agent::AgentTypeSchema;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::component::Component;
use std::collections::BTreeMap;
use std::sync::Arc;
use wasmtime::component::Resource;

/// The deployed agent type that a client invokes, with what each of its calls needs.
pub(super) struct RemoteAgentType {
    pub(super) registered: RegisteredAgentType,
    pub(super) agent_type: Arc<AgentTypeSchema>,
    pub(super) method_streams: Arc<BTreeMap<String, bool>>,
    pub(super) owner: AgentOwnerPattern,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Looks up the deployed agent type of the name, durably. `Ok(Err(_))` is an RPC error for
    /// the guest; the caller still owns the guest's other inputs.
    pub(super) async fn remote_agent_type(
        &mut self,
        agent_type_name: &str,
    ) -> anyhow::Result<Result<RemoteAgentType, RpcError>> {
        let Some(registered) = self
            .get_agent_type_schema_model(AgentTypeName(agent_type_name.to_string()))
            .await?
        else {
            return Ok(Err(RpcError::RemoteAgentError(WitAgentError::InvalidType(
                agent_type_name.to_string(),
            ))));
        };
        let remote_component = self
            .component_service()
            .get_metadata(
                registered.implemented_by.component_id,
                Some(registered.implemented_by.component_revision),
            )
            .await?;
        RemoteAgentType::new(
            registered,
            &remote_component,
            self.owner_component_metadata(),
        )
        .map(Ok)
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// The ID of the keyed agent of the constructor, when that agent exists. The type lookup and
    /// the existence check are durable, so replay sees the answer that the live call saw.
    pub(crate) async fn lookup_keyed_agent_id(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<Option<core_wire::AgentId>, RpcError>> {
        let remote = match self.remote_agent_type(&agent_type_name).await? {
            Ok(remote) => remote,
            Err(error) => {
                discard_owned_rpc_input(constructor, self);
                return Ok(Err(error));
            }
        };
        let parameters = match schema_value_tree_to_typed_constructor_parameters(
            constructor,
            &remote.agent_type,
            self,
        ) {
            Ok(parameters) => parameters,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(format!("Invalid constructor input: {err}")),
                )));
            }
        };
        if remote.registered.agent_type.mode == AgentMode::Ephemeral {
            return Ok(Ok(None));
        }
        let constructor = match ParsedAgentKey::try_new(AgentTypeName(agent_type_name), parameters)
        {
            Ok(constructor) => constructor,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(err),
                )));
            }
        };
        let target = match keyed_target(
            self.owned_agent_id.environment_id,
            remote.registered.type_id,
            remote.registered.implemented_by.component_id,
            constructor,
        ) {
            Ok(target) => target,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(err.to_string()),
                )));
            }
        };
        let agent_id = target.owned_agent_id.agent_id;
        let mut handle = DurableCallSession::<GolemAgentLookupAgentId, NotCancellable>::start(
            self,
            HostRequestGolemApiAgentId { agent_id },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }
            let result = self.live_keyed_agent_lookup(&target).await;
            handle
                .complete(self, HostResponseGolemAgentLookupAgentId { result })
                .await?
        };
        Ok(response
            .result
            .map(|agent_id| agent_id.map(Into::into))
            .map_err(|error| InternalRpcError::from(error).into()))
    }

    async fn live_keyed_agent_lookup(
        &mut self,
        target: &ResolvedTarget,
    ) -> Result<Option<AgentId>, SerializableRpcError> {
        let agent_id = target.owned_agent_id.agent_id;
        let denied = agent_operation_denied(
            self,
            &agent_id,
            AgentVerb::View,
            AgentResourcePattern::Empty,
        )
        .await
        .map_err(|error| SerializableRpcError::from(InternalRpcError::from(error)))?;
        if denied {
            return Err(SerializableRpcError::Denied {
                details: format!("Not allowed to {:?} agent {agent_id}", AgentVerb::View),
            });
        }
        let Some(metadata) = self
            .state
            .worker_service
            .get(&target.owned_agent_id)
            .await
            .map_err(|error| SerializableRpcError::from(InternalRpcError::from(error)))?
        else {
            return Ok(None);
        };
        let requested_key = target
            .new_owner
            .as_ref()
            .and_then(|owner| owner.canonical_key(AgentMode::Durable));
        if metadata.initial_worker_metadata.canonical_key == requested_key {
            Ok(Some(agent_id))
        } else {
            Err(SerializableRpcError::from(InternalRpcError::from(
                WorkerExecutorError::AgentKeyCollision {
                    agent_id,
                    requested_key: requested_key.unwrap_or_default(),
                },
            )))
        }
    }
}

impl RemoteAgentType {
    /// `remote_component` is the revision of the implementing component that the registered
    /// type names; `owner_component` is the caller's component.
    pub(super) fn new(
        registered: RegisteredAgentType,
        remote_component: &Component,
        owner_component: &Component,
    ) -> anyhow::Result<Self> {
        let method_streams = Arc::new(
            registered
                .agent_type
                .methods
                .iter()
                .map(|method| {
                    remote_component
                        .metadata
                        .agent_method_stream_metadata(
                            &registered.agent_type.type_name,
                            &method.name,
                        )
                        .map(|metadata| (method.name.clone(), metadata.uses_streams()))
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Streaming classification for remote agent method '{}' is missing",
                                method.name
                            )
                        })
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?,
        );
        let owner = AgentOwnerPattern::Agent {
            account: registered.implemented_by.account_email.clone(),
            application: owner_component.application_name.clone(),
            environment: owner_component.environment_name.clone(),
            component: ComponentName(registered.implemented_by.component_name.clone()),
            agent: AgentOwnerLeafPattern::AgentTypeWildcard(
                registered.agent_type.type_name.clone(),
            ),
        };
        Ok(Self {
            agent_type: Arc::new(registered.agent_type.clone()),
            registered,
            method_streams,
            owner,
        })
    }
}

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn upsert(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<Resource<WasmRpcEntry>, RpcError>> {
        self.check_read_only_allows("golem::rpc::wasm-rpc::upsert")
            .map_err(wasmtime::Error::from)?;
        let remote = match self.remote_agent_type(&agent_type_name).await? {
            Ok(remote) => remote,
            Err(error) => {
                discard_owned_rpc_input(constructor, self);
                return Ok(Err(error));
            }
        };
        let parameters = match schema_value_tree_to_typed_constructor_parameters(
            constructor,
            &remote.agent_type,
            self,
        ) {
            Ok(parameters) => parameters,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(format!("Invalid constructor input: {err}")),
                )));
            }
        };
        let constructor = match ParsedAgentKey::try_new(AgentTypeName(agent_type_name), parameters)
        {
            Ok(constructor) => constructor,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(err),
                )));
            }
        };
        let environment_id = self.owned_agent_id.environment_id;
        let component_id = remote.registered.implemented_by.component_id;
        let type_id = remote.registered.type_id;
        let RemoteAgentType {
            registered,
            agent_type,
            method_streams,
            owner,
        } = remote;

        if registered.agent_type.mode == AgentMode::Ephemeral {
            // A client of an ephemeral agent type is a logical proxy: every invocation runs on
            // a new one-shot instance. Its logical ID never addresses an instance.
            let logical_id = AgentId::ephemeral_proxy(environment_id, type_id, &constructor)
                .map_err(anyhow::Error::msg)?;
            let mut logical_remote_agent_id = OwnedAgentId::new(environment_id, &logical_id);
            let (handle, span) = start_rpc_connection_call::<_, GolemRpcWasmRpcUpsert>(
                self,
                HostRequestGolemRpcCreate {
                    remote_agent_id: logical_remote_agent_id.agent_id,
                },
                false,
            )
            .await?;
            let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());
            let live = if handle.is_live() {
                Some(handle)
            } else {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        logical_remote_agent_id.environment_id = response.target_environment_id;
                        None
                    }
                    CallReplayOutcome::Incomplete(live) => Some(live),
                }
            };
            if let Some(live) = live {
                live.complete(
                    self,
                    HostResponseGolemRpcCreate {
                        result: Ok(AgentFingerprint(uuid::Uuid::nil())),
                        target_environment_id: logical_remote_agent_id.environment_id,
                    },
                )
                .await?;
            }
            return construct_ephemeral_wasm_rpc_resource(
                self,
                logical_remote_agent_id,
                EphemeralRpcTarget {
                    type_id,
                    component_id,
                    constructor,
                },
                vec![],
                vec![],
                span,
                agent_type,
                method_streams,
                owner,
                creation_index,
            )
            .map(Ok);
        }

        let target = match keyed_target(environment_id, type_id, component_id, constructor) {
            Ok(target) => target,
            Err(err) => {
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(err.to_string()),
                )));
            }
        };
        let (handle, span) = start_rpc_connection_call::<_, GolemRpcWasmRpcUpsert>(
            self,
            HostRequestGolemRpcCreate {
                remote_agent_id: target.owned_agent_id.agent_id,
            },
            true,
        )
        .await?;
        bind_rpc_connection(
            self,
            handle,
            span,
            Box::new(target),
            agent_type,
            component_id,
            method_streams,
            owner,
        )
        .await
    }

    async fn get_by_id(
        &mut self,
        agent_type_name: String,
        agent_id: core_wire::AgentId,
    ) -> anyhow::Result<Result<Resource<WasmRpcEntry>, RpcError>> {
        self.check_read_only_allows("golem::rpc::wasm-rpc::get-by-id")
            .map_err(wasmtime::Error::from)?;
        let agent_id = AgentId::from(agent_id);
        let name = AgentTypeName(agent_type_name);
        let request = HostRequestGolemRpcGetById {
            remote_agent_id: agent_id,
            agent_type_name: name.clone(),
        };
        let (mut handle, span, recorded) = start_rpc_binding_call::<_, GolemRpcWasmRpcGetById>(
            self,
            request.clone(),
            &agent_id,
            true,
        )
        .await?;
        let start_index = handle.start_index();
        if recorded != request {
            return Err(handle.trap(WorkerExecutorError::unexpected_oplog_entry(
                start_index,
                format!("get-by-id {request:?}"),
                format!("get-by-id {recorded:?}"),
            )));
        }
        let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());
        let target_environment_id = self.owned_agent_id.environment_id;
        let response = 'binding: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'binding response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }
            let lookup_result = {
                let auth_ctx = handle.agent_auth_ctx().clone();
                let lookup = async {
                    let metadata = self
                        .state
                        .worker_service
                        .get(&OwnedAgentId::new(target_environment_id, &agent_id))
                        .await?
                        .filter(|metadata| {
                            metadata.initial_worker_metadata.agent_type_name.as_ref() == Some(&name)
                                && metadata.initial_worker_metadata.environment_id
                                    == target_environment_id
                        })
                        .ok_or_else(|| WorkerExecutorError::worker_not_found(agent_id))?;
                    let initial = metadata.initial_worker_metadata;
                    let revision = metadata
                        .last_known_status
                        .ok_or_else(|| {
                            WorkerExecutorError::runtime(
                                "Cannot resolve the existing agent's revision",
                            )
                        })?
                        .component_revision;
                    let component = self
                        .component_service()
                        .get_metadata(initial.component_id, Some(revision))
                        .await?;
                    let agent_type = component
                        .metadata
                        .find_agent_type_by_name(&name)
                        .ok_or_else(|| WorkerExecutorError::worker_not_found(agent_id))?
                        .clone();
                    let type_id = agent_id
                        .type_id()
                        .ok_or_else(|| WorkerExecutorError::worker_not_found(agent_id))?;
                    let registered = RegisteredAgentType {
                        type_id,
                        agent_type,
                        implemented_by:
                            golem_common::model::agent::RegisteredAgentTypeImplementer {
                                component_id: component.id,
                                component_revision: component.revision,
                                component_name: component.component_name.0.clone(),
                                account_id: component.account_id,
                                account_email: component.account_email.clone(),
                            },
                    };
                    let remote = RemoteAgentType::new(
                        registered.clone(),
                        &component,
                        self.owner_component_metadata(),
                    )
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                    let mut owner = remote.owner;
                    if let AgentOwnerPattern::Agent { agent, .. } = &mut owner {
                        *agent = AgentOwnerLeafPattern::agent(agent_id, Some(name.clone()));
                    }
                    auth_ctx
                        .authorize_permission(
                            &crate::durable_host::authorization::targets::agent_target(
                                owner,
                                AgentVerb::Invoke,
                                AgentResourcePattern::Any,
                            ),
                        )
                        .map_err(|error| {
                            WorkerExecutorError::permission_denied(error.to_string())
                        })?;
                    Ok::<_, WorkerExecutorError>((initial.fingerprint, registered))
                };
                let interrupt = self.create_interrupt_signal().map(Error::from);
                tokio::pin!(lookup);
                match futures::future::select(lookup, interrupt).await {
                    Either::Left((result, _)) => Ok(result),
                    Either::Right((error, _)) => Err(error),
                }
            };
            let result = match lookup_result {
                Ok(result) => result,
                Err(error) => return Err(handle.trap(error)),
            };
            let (result, agent_type, type_id) = match result {
                Ok((fingerprint, registered)) => (
                    Ok((agent_id, fingerprint)),
                    Some(golem_common::schema::RegisteredAgentTypeSchema {
                        agent_type: registered.agent_type,
                        implemented_by: registered.implemented_by,
                    }),
                    Some(registered.type_id),
                ),
                Err(
                    error @ (WorkerExecutorError::AgentNotFound { .. }
                    | WorkerExecutorError::PermissionDenied { .. }),
                ) => (Err(InternalRpcError::from(error).into()), None, None),
                Err(error) => return Err(handle.trap(error)),
            };
            handle
                .complete(
                    self,
                    HostResponseGolemRpcCreated {
                        result,
                        agent_type,
                        type_id,
                        target_environment_id,
                    },
                )
                .await?
        };
        let (bound_id, fingerprint) = match response.result {
            Ok(binding) => binding,
            Err(error) => {
                finish_span_in_memory(self, span.span_id())?;
                return Ok(Err(InternalRpcError::from(error).into()));
            }
        };
        let (Some(schema), Some(type_id)) = (response.agent_type, response.type_id) else {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                start_index,
                "get-by-id End with the recorded agent type",
                "get-by-id End without the recorded agent type",
            )
            .into());
        };
        let component = self
            .component_service()
            .get_metadata(
                schema.implemented_by.component_id,
                Some(schema.implemented_by.component_revision),
            )
            .await?;
        let remote = RemoteAgentType::new(
            RegisteredAgentType {
                type_id,
                agent_type: schema.agent_type,
                implemented_by: schema.implemented_by,
            },
            &component,
            self.owner_component_metadata(),
        )?;
        reconstruct_wasm_rpc_resource(
            self,
            bound_id,
            response.target_environment_id,
            fingerprint,
            vec![],
            vec![],
            span,
            remote.agent_type,
            remote.registered.implemented_by.component_id,
            remote.method_streams,
            remote.owner,
            creation_index,
        )
        .await
        .map(Ok)
    }

    async fn bind_self(
        &mut self,
    ) -> anyhow::Result<Result<(core_wire::AgentId, Resource<WasmRpcEntry>), RpcError>> {
        let golem_common::model::agent::ResolvedOwnerContext::Agent(agent) = self.owner_context()
        else {
            return Ok(Err(RpcError::RemoteAgentError(WitAgentError::InvalidType(
                "self binding requires a durable agent owner".to_string(),
            ))));
        };
        if self.agent_mode() != AgentMode::Durable {
            return Ok(Err(RpcError::RemoteAgentError(WitAgentError::InvalidType(
                agent.agent_type.0.clone(),
            ))));
        }
        let component = self.owner_component_metadata();
        let agent_type = Arc::new(
            component
                .metadata
                .find_agent_type_by_name(&agent.agent_type)
                .ok_or_else(|| anyhow::anyhow!("current agent type is missing"))?
                .clone(),
        );
        let streams = Arc::new(
            agent_type
                .methods
                .iter()
                .map(|method| {
                    component
                        .metadata
                        .agent_method_stream_metadata(&agent_type.type_name, &method.name)
                        .map(|metadata| (method.name.clone(), metadata.uses_streams()))
                        .ok_or_else(|| {
                            anyhow::anyhow!("current agent method streaming metadata is missing")
                        })
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?,
        );
        let remote_agent_id = *self.agent_id();
        let environment_id = self.owned_agent_id.environment_id;
        let fingerprint = self
            .public_state
            .worker()
            .get_initial_worker_metadata()
            .fingerprint;
        let owner = crate::durable_host::authorization::targets::agent_owner(self);
        let (handle, span, binding) = start_rpc_binding_call::<_, GolemRpcWasmRpcBindSelf>(
            self,
            HostRequestGolemRpcBindSelf {
                remote_agent_id,
                target_environment_id: environment_id,
                fingerprint,
            },
            &remote_agent_id,
            false,
        )
        .await?;
        let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());
        let live = if handle.is_live() {
            Some(handle)
        } else {
            match handle.replay(self).await? {
                CallReplayOutcome::Replayed(_) => None,
                CallReplayOutcome::Incomplete(live) => Some(live),
            }
        };
        if let Some(live) = live {
            live.complete(self, HostResponseGolemRpcUnit {}).await?;
        }
        let bound_id = binding.remote_agent_id;
        let own_component_id = self.owner_component_metadata().id;
        let resource = reconstruct_wasm_rpc_resource(
            self,
            binding.remote_agent_id,
            binding.target_environment_id,
            binding.fingerprint,
            vec![],
            vec![],
            span,
            agent_type,
            own_component_id,
            streams,
            owner,
            creation_index,
        )
        .await?;
        Ok(Ok((bound_id.into(), resource)))
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
    ) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
        let scope_card = match scope_card {
            Some(handle) => match resolve_invocation_scope_card(self, &handle) {
                Ok(card) => Some(card),
                Err(details) => {
                    discard_owned_rpc_input(input, self);
                    return Ok(Err(RpcError::Denied(details)));
                }
            },
            None => None,
        };
        let prepared = match prepare_rpc_invocation(
            self,
            &self_,
            "golem::rpc::wasm-rpc::invoke-and-await",
            true,
            method_name,
            input,
            true,
        )? {
            Ok(prepared) => prepared,
            Err(err) => return Ok(Err(err)),
        };

        let target = self.outbound_agent_method_target(
            &self_,
            &prepared.logical_remote_agent_id,
            &prepared.method_name,
        )?;
        self.record_outbound_rpc_call()?;

        if prepared.is_streaming() {
            let begun = DurableCallSession::<
                GolemRpcWasmRpcInvokeAndAwaitResult,
                NotCancellable,
            >::begin_with_agent_authority(self, DurableFunctionType::WriteRemote)
            .await?;
            let parent_key = self.state.get_current_idempotency_key().ok_or_else(|| {
                anyhow::anyhow!("durable streaming RPC requires a caller invocation key")
            })?;
            let live_streaming_origin = current_streaming_origin(self, parent_key);
            let call_target = RpcCallTarget::new(
                begun.idempotency_key_position(self),
                &prepared.logical_remote_agent_id,
                prepared.ephemeral.as_ref(),
                &prepared.method_name,
            );
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Replay(handle) => handle,
                ResolvedCall::Live(begun) => {
                    if let Err(error) = self.authorize_live_permission(&target).await? {
                        return persist_invoke_and_await_denial_from_begun(
                            self,
                            begun,
                            prepared.logical_remote_agent_id,
                            prepared.method_name,
                            RpcError::Denied(error.to_string()),
                        )
                        .await;
                    }
                    let request_prepared = prepared.clone();
                    let request_target = call_target.clone();
                    let request_scope_card = scope_card.clone();
                    let request_streaming_origin = live_streaming_origin.clone();
                    let span_started = embedded_invocation_span_builder(
                        self,
                        &prepared.connection_span_id,
                        &prepared.method_name,
                    )?;
                    begun
                        .start_live_with_index_and_span(self, move |start_index| {
                            let (idempotency_key, remote_agent_id) =
                                request_target.at(start_index)?;
                            let started = span_started(&idempotency_key);
                            Ok((
                                request_prepared.invoke_request(
                                    &remote_agent_id,
                                    &idempotency_key,
                                    request_scope_card.as_ref(),
                                    Some(request_streaming_origin),
                                ),
                                started,
                            ))
                        })
                        .await?
                }
            };
            let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
            let freshness_disposition =
                if known_fresh_dispatch_allowed(handle.is_live(), prepared.is_ephemeral()) {
                    InvocationFreshnessDisposition::KnownFresh
                } else {
                    InvocationFreshnessDisposition::MayExist
                };
            if !handle.is_live() && recorded_rpc_denial(self, &handle).await?.is_some() {
                let start_index = handle.start_index();
                let response = replay_rpc_denial(self, handle).await?;
                return Ok(Err(denied_rpc_failure(start_index, response.result)?.into()));
            }
            let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    handle.start_index(),
                    "streaming RPC Start carrying span_started",
                    format!(
                        "streaming RPC Start at {} without span metadata",
                        handle.start_index()
                    ),
                )
            })?;
            let span = restore_embedded_span(self, &started)?;
            if self.is_live() && !handle.is_live() {
                let start_index = handle.start_index();
                handle = match handle.replay(self).await? {
                    CallReplayOutcome::Incomplete(handle) => handle,
                    CallReplayOutcome::Replayed(_) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            start_index,
                            "incomplete RPC before a newly authored invocation span",
                            "completed RPC",
                        )
                        .into());
                    }
                };
            }
            let logical_streaming_origin = handle
                .recorded_request(self)
                .await?
                .logical_streaming_origin
                .ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        handle.start_index(),
                        "streaming RPC Start with logical origin",
                        format!(
                            "streaming RPC Start at {} without logical origin",
                            handle.start_index()
                        ),
                    )
                })?;
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
            }
            let mut auth_ctx = handle.is_live().then(|| handle.agent_auth_ctx().clone());
            let (invocation_id, target_fingerprint) = match streaming_target_fingerprint(
                self,
                &self_,
                handle.start_index(),
                &prepared,
                &remote_agent_id,
                auth_ctx.as_ref(),
                idempotency_key.clone(),
                freshness_disposition,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    let finished = rpc_span_finished(span.span_id(), &Err::<(), _>(error.clone()));
                    complete_rpc_preparation_failure(
                        self,
                        handle,
                        HostResponseGolemRpcInvokeAndAwait {
                            result: Err(error.clone().into()),
                        },
                        finished,
                    )
                    .await?;
                    return Ok(Err(error.into()));
                }
                Err(error) => {
                    return Err(handle.trap(error));
                }
            };
            let metadata = invocation_metadata(&invocation_id);
            let stream_auth_ctx = auth_ctx.clone().unwrap_or_else(|| self.agent_auth_ctx());
            let (streams, origin_invocation) = caller_durable_rpc_streams(
                self,
                &remote_agent_id,
                target_fingerprint,
                invocation_id.id(),
                logical_streaming_origin,
                stream_auth_ctx,
            )
            .await?;
            let caller_revision = self.owner_component_metadata().revision;
            let input_root = rpc_input_root(&prepared);
            let output_root = rpc_output_root(&prepared);
            let input = streams
                .materialize_agent_input(
                    &prepared.input_value,
                    &prepared.remote_agent_type.schema,
                    &input_root,
                    caller_revision,
                )
                .await
                .map_err(SessionError::into_trap)?;
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(persisted) => {
                        let result = match persisted.result {
                            Ok(scalar) => Ok(streams
                                .replay_remote_result()
                                .await
                                .map_err(SessionError::into_trap)?
                                .unwrap_or(scalar)),
                            Err(error) => Err(InternalRpcError::from(error)),
                        };
                        finish_span_in_memory(self, span.span_id())?;
                        return match result {
                            Ok(value) => Ok(Ok(InvocationResultWithMetadata {
                                metadata,
                                result: schema_value_to_wire_output(&value, self)?,
                            })),
                            Err(error) => Ok(Err(error.into())),
                        };
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        auth_ctx = Some(handle.agent_auth_ctx().clone());
                    }
                }
            }
            let auth_ctx = auth_ctx.expect("a live durable RPC call has captured agent authority");
            if !prepared.is_ephemeral()
                && let Err(error) =
                    ensure_rpc_target_activated(self, &self_, &auth_ctx, &prepared.method_name)
                        .await
            {
                return Err(handle.trap(error));
            }
            let attempt_id = streams
                .caller_attempt_id()
                .await
                .map_err(SessionError::into_trap)?;
            let input_mappings = input.proto_mappings();
            let (accepted_inputs, acceptance) = tokio::sync::oneshot::channel();
            let interrupt_signal = self.create_interrupt_signal();
            let remote_result = {
                let rpc = self.rpc();
                let agent_id = *self.agent_id();
                let caller = golem_common::model::caller::Caller::entry(self.self_principal());
                let created_by = self.created_by();
                let stack = self.clone_as_inherited_stack(span.span_id());
                let interrupt_signal = interrupt_signal.map(Error::from);
                let call = Box::pin(await_streaming_rpc_acceptance(
                    &streams,
                    None,
                    acceptance,
                    rpc.invoke_and_await_streaming(
                        dispatch_target(&remote_agent_id, prepared.ephemeral.as_ref()),
                        idempotency_key,
                        prepared.method_name.clone(),
                        input.value,
                        input_mappings,
                        target_fingerprint,
                        attempt_id.0,
                        origin_invocation,
                        accepted_inputs,
                        created_by,
                        &agent_id,
                        &prepared.env,
                        stack,
                        &caller,
                        prepared.config,
                        &auth_ctx,
                        scope_card,
                    ),
                ));
                match futures::future::select(call, interrupt_signal).await {
                    Either::Left((Ok(result), _)) => result,
                    Either::Left((Err(error), _)) => {
                        return Err(handle.trap(error.into_trap()));
                    }
                    Either::Right((error, _)) => {
                        return Err(handle.trap(error));
                    }
                }
            };
            let result = match remote_result {
                Ok(remote_result) => {
                    let output_mappings = remote_result
                        .output_mappings
                        .into_iter()
                        .map(durable_stream_mapping_from_proto)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(anyhow::Error::msg)?;
                    let value = {
                        let interrupt_signal = self.create_interrupt_signal().map(Error::from);
                        let materialize = streams.materialize_remote_result(
                            remote_result.value,
                            output_mappings,
                            &prepared.remote_agent_type.schema,
                            &output_root,
                        );
                        tokio::pin!(materialize);
                        match futures::future::select(materialize, interrupt_signal).await {
                            Either::Left((result, _)) => result.map_err(SessionError::into_trap)?,
                            Either::Right((error, _)) => {
                                return Err(handle.trap(error));
                            }
                        }
                    };
                    if self.secret_holds_allowed_for_value(&value).await? {
                        Ok(value)
                    } else {
                        Err(InternalRpcError::Denied {
                            details: "permission denied".to_string(),
                        })
                    }
                }
                Err(error) => Err(error),
            };
            let span_finished = rpc_span_finished(span.span_id(), &result);
            handle
                .complete_with_span(
                    self,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: result.clone().map(strip_streams).map_err(Into::into),
                    },
                    span_finished,
                )
                .await?;
            finish_span_in_memory(self, span.span_id())?;
            return match result {
                Ok(value) => Ok(Ok(InvocationResultWithMetadata {
                    metadata,
                    result: schema_value_to_wire_output(&value, self)?,
                })),
                Err(error) => Ok(Err(error.into())),
            };
        }

        let begun =
            DurableCallSession::<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>::
                begin_with_agent_authority(self, DurableFunctionType::WriteRemote)
                .await?;
        let call_target = RpcCallTarget::new(
            begun.idempotency_key_position(self),
            &prepared.logical_remote_agent_id,
            prepared.ephemeral.as_ref(),
            &prepared.method_name,
        );
        let mut handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let Err(error) = self.authorize_live_permission(&target).await? {
                    return persist_invoke_and_await_denial_from_begun(
                        self,
                        begun,
                        prepared.logical_remote_agent_id,
                        prepared.method_name,
                        RpcError::Denied(error.to_string()),
                    )
                    .await;
                }
                let span_started = embedded_invocation_span_builder(
                    self,
                    &prepared.connection_span_id,
                    &prepared.method_name,
                )?;
                let request_prepared = prepared.clone();
                let request_target = call_target.clone();
                let request_scope_card = scope_card.clone();
                begun
                    .start_live_with_index_and_span(self, move |start_index| {
                        let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
                        let started = span_started(&idempotency_key);
                        Ok((
                            request_prepared.invoke_request(
                                &remote_agent_id,
                                &idempotency_key,
                                request_scope_card.as_ref(),
                                None,
                            ),
                            started,
                        ))
                    })
                    .await?
            }
            ResolvedCall::Replay(handle) => {
                if recorded_rpc_denial(self, &handle).await?.is_some() {
                    let start_index = handle.start_index();
                    let response = replay_rpc_denial(self, handle).await?;
                    return Ok(Err(denied_rpc_failure(start_index, response.result)?.into()));
                }
                handle
            }
        };

        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let freshness = if known_fresh_dispatch_allowed(handle.is_live(), prepared.is_ephemeral()) {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let identity = prepare_rpc_invocation_identity(
            self,
            &self_,
            handle.start_index(),
            &remote_agent_id,
            &idempotency_key,
            &prepared.method_name,
            freshness,
        )
        .await
        .map_err(|error| handle.trap(error))?;
        let dispatched_scope_card = scope_card.clone();
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                "dispatched RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;
        let (invocation_id, _) = match identity {
            Ok(identity) => identity,
            Err(error) => {
                let finished = rpc_span_finished(span.span_id(), &Err::<(), _>(error.clone()));
                complete_rpc_preparation_failure(
                    self,
                    handle,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: Err(error.clone().into()),
                    },
                    finished,
                )
                .await?;
                return Ok(Err(error.into()));
            }
        };
        let metadata = invocation_metadata(&invocation_id);
        match run_invoke_and_await(
            self,
            self_,
            prepared,
            remote_agent_id,
            idempotency_key,
            span,
            handle,
            dispatched_scope_card,
        )
        .await?
        {
            Ok(value) => Ok(Ok(InvocationResultWithMetadata {
                metadata,
                result: schema_value_to_wire_output(&value, self)?,
            })),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
    ) -> anyhow::Result<Result<InvocationMetadata, RpcError>> {
        let input = match reject_non_await_scope_card(&scope_card, input, "invoke", self) {
            Ok(input) => input,
            Err(details) => return Ok(Err(RpcError::Denied(details))),
        };
        let prepared = match prepare_rpc_invocation(
            self,
            &self_,
            "golem::rpc::wasm-rpc::invoke",
            false,
            method_name,
            input,
            false,
        )? {
            Ok(prepared) => prepared,
            Err(err) => return Ok(Err(err)),
        };

        let target = self.outbound_agent_method_target(
            &self_,
            &prepared.logical_remote_agent_id,
            &prepared.method_name,
        )?;
        self.record_outbound_rpc_call()?;

        let begun =
            DurableCallSession::<GolemRpcWasmRpcInvoke, NotCancellable>::begin_with_agent_authority(
                self,
                DurableFunctionType::WriteRemote,
            )
            .await?;
        let call_target = RpcCallTarget::new(
            begun.idempotency_key_position(self),
            &prepared.logical_remote_agent_id,
            prepared.ephemeral.as_ref(),
            &prepared.method_name,
        );
        let mut handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let Err(error) = self.authorize_live_permission(&target).await? {
                    return persist_invoke_denial_from_begun(
                        self,
                        begun,
                        prepared.logical_remote_agent_id,
                        prepared.method_name,
                        RpcError::Denied(error.to_string()),
                    )
                    .await;
                }
                let span_started = embedded_invocation_span_builder(
                    self,
                    &prepared.connection_span_id,
                    &prepared.method_name,
                )?;
                let request_prepared = prepared.clone();
                let request_target = call_target.clone();
                begun
                    .start_live_with_index_and_span(self, move |start_index| {
                        let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
                        let started = span_started(&idempotency_key);
                        Ok((
                            request_prepared.invoke_request(
                                &remote_agent_id,
                                &idempotency_key,
                                None,
                                None,
                            ),
                            started,
                        ))
                    })
                    .await?
            }
            ResolvedCall::Replay(handle) => {
                if recorded_rpc_denial(self, &handle).await?.is_some() {
                    let start_index = handle.start_index();
                    let response = replay_rpc_denial(self, handle).await?;
                    return Ok(Err(denied_rpc_failure(start_index, response.result)?.into()));
                }
                handle
            }
        };

        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let freshness = if known_fresh_dispatch_allowed(handle.is_live(), prepared.is_ephemeral()) {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let identity = prepare_rpc_invocation_identity(
            self,
            &self_,
            handle.start_index(),
            &remote_agent_id,
            &idempotency_key,
            &prepared.method_name,
            freshness,
        )
        .await
        .map_err(|error| handle.trap(error))?;
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                "fire-and-forget RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;
        let (invocation_id, _) = match identity {
            Ok(identity) => identity,
            Err(error) => {
                let finished = rpc_span_finished(span.span_id(), &Err::<(), _>(error.clone()));
                complete_rpc_preparation_failure(
                    self,
                    handle,
                    HostResponseGolemRpcUnitOrFailure {
                        result: Err(error.clone().into()),
                    },
                    finished,
                )
                .await?;
                return Ok(Err(error.into()));
            }
        };
        let metadata = invocation_metadata(&invocation_id);
        match run_invoke(
            self,
            self_,
            prepared,
            remote_agent_id,
            idempotency_key,
            span,
            handle,
        )
        .await?
        {
            Ok(()) => Ok(Ok(metadata)),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn async_invoke_and_await(
        &mut self,
        this: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
    ) -> anyhow::Result<AsyncInvocationWithMetadata> {
        let scope_card = match scope_card {
            Some(handle) => resolve_invocation_scope_card(self, &handle).map(Some),
            None => Ok(None),
        };
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::async-invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let own_agent_id = self.owned_agent_id().clone();

        let (
            logical_remote_agent_id,
            connection_span_id,
            remote_agent_type,
            remote_component_id,
            remote_method_streams,
            env,
            config,
            ephemeral,
        ) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.span_id.clone(),
                payload.remote_agent_type.clone(),
                payload.remote_component_id,
                payload.remote_method_streams.clone(),
                env,
                config,
                payload.ephemeral.clone(),
            )
        };
        if ephemeral.is_none() && logical_remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        let scope_card = match scope_card {
            Ok(scope_card) => scope_card,
            Err(details) => {
                discard_owned_rpc_input(input, self);
                return persist_async_validation_rejection(
                    self,
                    &connection_span_id,
                    &logical_remote_agent_id,
                    ephemeral.as_ref(),
                    &method_name,
                    InternalRpcError::Denied { details },
                )
                .await;
            }
        };

        // Resolve the method and lift the input before opening the remote invocation. A rejection
        // is recorded as its own short local operation so its invocation span closes when the
        // rejected attempt is produced, independently of whether the baked future is consumed.
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input, self) {
                Ok(parts) => parts,
                Err(rpc_error) => {
                    return persist_async_validation_rejection(
                        self,
                        &connection_span_id,
                        &logical_remote_agent_id,
                        ephemeral.as_ref(),
                        &method_name,
                        rpc_error,
                    )
                    .await;
                }
            };
        let method = find_agent_method(&remote_agent_type, &method_name)?;
        let streaming = remote_method_uses_streams(&remote_method_streams, &method.name)?;

        let target =
            self.outbound_agent_method_target(&this, &logical_remote_agent_id, &method_name)?;
        self.record_outbound_rpc_call()?;

        if streaming {
            let begun = DurableCallSession::<
                GolemRpcWasmRpcInvokeAndAwaitResult,
                Cancellable,
            >::begin_with_agent_authority(self, DurableFunctionType::WriteRemote)
            .await?;
            let parent_key = self.state.get_current_idempotency_key().ok_or_else(|| {
                anyhow::anyhow!("durable streaming RPC requires a caller invocation key")
            })?;
            let live_streaming_origin = current_streaming_origin(self, parent_key);
            let call_target = RpcCallTarget::new(
                begun.idempotency_key_position(self),
                &logical_remote_agent_id,
                ephemeral.as_ref(),
                &method_name,
            );
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    if let Err(error) = self.authorize_live_permission(&target).await? {
                        return persist_async_invoke_denial_from_begun(
                            self,
                            begun,
                            &this,
                            logical_remote_agent_id,
                            method_name,
                            RpcError::Denied(error.to_string()),
                        )
                        .await;
                    }
                    let request_target = call_target.clone();
                    let request_method_name = method_name.clone();
                    let request_input = strip_streams(input_value.clone());
                    let request_scope_card = scope_card.clone();
                    let request_streaming_origin = live_streaming_origin.clone();
                    let span_started =
                        embedded_invocation_span_builder(self, &connection_span_id, &method_name)?;
                    begun
                        .start_live_with_index_and_span(self, move |start_index| {
                            let (idempotency_key, remote_agent_id) =
                                request_target.at(start_index)?;
                            let started = span_started(&idempotency_key);
                            Ok((
                                HostRequestGolemRpcInvoke {
                                    remote_agent_id: remote_agent_id.agent_id(),
                                    request_key: InvocationRequestKey::Internal(idempotency_key),
                                    method_name: request_method_name,
                                    input: request_input,
                                    logical_streaming_origin: Some(request_streaming_origin),
                                    remote_agent_type: None,
                                    remote_agent_parameters: None,
                                    scope_card: request_scope_card,
                                },
                                started,
                            ))
                        })
                        .await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
            let freshness_disposition =
                if known_fresh_dispatch_allowed(handle.is_live(), ephemeral.is_some()) {
                    InvocationFreshnessDisposition::KnownFresh
                } else {
                    InvocationFreshnessDisposition::MayExist
                };
            if !handle.is_live() && recorded_rpc_denial(self, &handle).await?.is_some() {
                return replay_async_rpc_denial(self, handle).await;
            }
            let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    handle.start_index(),
                    "streaming RPC Start carrying span_started",
                    format!(
                        "streaming RPC Start at {} without span metadata",
                        handle.start_index()
                    ),
                )
            })?;
            let span = restore_embedded_span(self, &started)?;
            if self.is_live() && !handle.is_live() {
                let start_index = handle.start_index();
                handle = match handle.replay(self).await? {
                    CallReplayOutcome::Incomplete(handle) => handle,
                    CallReplayOutcome::Replayed(_) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            start_index,
                            "incomplete RPC before a newly authored invocation span",
                            "completed RPC",
                        )
                        .into());
                    }
                };
            }
            let logical_streaming_origin = handle
                .recorded_request(self)
                .await?
                .logical_streaming_origin
                .ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        handle.start_index(),
                        "streaming RPC Start with logical origin",
                        format!(
                            "streaming RPC Start at {} without logical origin",
                            handle.start_index()
                        ),
                    )
                })?;
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                request_key: InvocationRequestKey::Internal(idempotency_key.clone()),
                method_name: method_name.clone(),
                input: strip_streams(input_value.clone()),
                logical_streaming_origin: Some(logical_streaming_origin.clone()),
                remote_agent_type: None,
                remote_agent_parameters: None,
                scope_card: scope_card.clone(),
            };
            let prepared = PreparedRpcInvocation {
                logical_remote_agent_id,
                ephemeral,
                connection_span_id,
                env: env.clone(),
                config: config.clone(),
                method_name: method_name.clone(),
                input_value: input_value.clone(),
                streaming: true,
                remote_agent_type: remote_agent_type.clone(),
                remote_component_id,
            };
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
            }
            let auth_ctx = handle.is_live().then(|| handle.take_agent_auth_ctx());
            let (invocation_id, target_fingerprint) = match streaming_target_fingerprint(
                self,
                &this,
                handle.start_index(),
                &prepared,
                &remote_agent_id,
                auth_ctx.as_ref(),
                idempotency_key.clone(),
                freshness_disposition,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    let finished = rpc_span_finished(span.span_id(), &Err::<(), _>(error.clone()));
                    complete_rpc_preparation_failure(
                        self,
                        handle,
                        HostResponseGolemRpcInvokeAndAwait {
                            result: Err(error.clone().into()),
                        },
                        finished,
                    )
                    .await?;
                    return bake_closed_async_error(self.table(), error);
                }
                Err(error) => return Err(handle.trap(error)),
            };
            let metadata = invocation_metadata(&invocation_id);
            let stream_auth_ctx = auth_ctx.clone().unwrap_or_else(|| self.agent_auth_ctx());
            let (streams, origin_invocation) = caller_durable_rpc_streams(
                self,
                &remote_agent_id,
                target_fingerprint,
                invocation_id.id(),
                logical_streaming_origin,
                stream_auth_ctx,
            )
            .await?;
            let caller_revision = self.owner_component_metadata().revision;
            let input = streams
                .materialize_agent_input(
                    &input_value,
                    &remote_agent_type.schema,
                    &rpc_input_root(&prepared),
                    caller_revision,
                )
                .await
                .map_err(SessionError::into_trap)?;
            let attempt_id = streams
                .caller_attempt_id()
                .await
                .map_err(SessionError::into_trap)?;
            let params = DurableStreamingTaskParams {
                streams,
                input_mappings: input.proto_mappings(),
                input: input.value,
                expected_callee_fingerprint: target_fingerprint,
                attempt_id: attempt_id.0,
                origin_invocation,
                output_graph: Arc::new(remote_agent_type.schema.clone()),
                output_root: rpc_output_root(&prepared),
            };
            let deferred_activation = self
                .table()
                .get(&this)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?
                .target_activation
                .deferred_activation();
            let remote_target = dispatch_target(&remote_agent_id, prepared.ephemeral.as_ref());
            let task = handle.is_live().then(|| {
                spawn_streaming_invoke_and_await_task(
                    self,
                    remote_target.clone(),
                    idempotency_key,
                    method_name,
                    params.clone(),
                    env.clone(),
                    config.clone(),
                    deferred_activation.clone(),
                    span.span_id(),
                    scope_card.clone(),
                    auth_ctx
                        .clone()
                        .expect("a live durable RPC call has captured agent authority"),
                )
            });
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: task.map(|task| Arc::new(tokio::sync::Mutex::new(task))),
                    streaming: Some(params),
                    request,
                    invocation_id,
                    remote_target,
                    env,
                    config,
                    deferred_activation,
                    expected_callee_fingerprint: None,
                    retry_properties: prepared
                        .retry_properties("invoke-and-await", &remote_agent_id),
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    retry_auth_ctx: None,
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            return Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata: Some(metadata),
            });
        }

        let (deferred_activation, expected_callee_fingerprint) = {
            let target_activation = &self
                .table()
                .get(&this)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?
                .target_activation;
            (
                target_activation.deferred_activation(),
                target_activation.target_fingerprint(),
            )
        };

        // Open the single durable host call for this async RPC as a `WriteRemote` — the same
        // durable function type as the synchronous `invoke_and_await`. The remote idempotency key
        // is derived from the call's own `Start` index, which the request is built from when the
        // eager `Start` is appended. Each concurrently-created future appends its own eager
        // `Start`, so its key is distinct.
        // The read-only side-effect guard was already applied at the top of this function and is
        // re-applied by `begin`.
        let begun = DurableCallSession::<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>::
            begin_with_agent_authority(self, DurableFunctionType::WriteRemote)
            .await?;
        let call_target = RpcCallTarget::new(
            begun.idempotency_key_position(self),
            &logical_remote_agent_id,
            ephemeral.as_ref(),
            &method_name,
        );
        // Self-call is only rejected for durable targets: an ephemeral
        // invocation always creates a new instance with a freshly derived
        // identity, so it can never target the caller's own invocation queue.
        let self_call = ephemeral.is_none() && logical_remote_agent_id == own_agent_id;
        let mut handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let Err(error) = self.authorize_live_permission(&target).await? {
                    return persist_async_invoke_denial_from_begun(
                        self,
                        begun,
                        &this,
                        logical_remote_agent_id,
                        method_name,
                        RpcError::Denied(error.to_string()),
                    )
                    .await;
                }
                if self_call {
                    return Err(anyhow::anyhow!(
                        "RPC calls to the same agent are not supported"
                    ));
                }
                let span_started =
                    embedded_invocation_span_builder(self, &connection_span_id, &method_name)?;
                let request_target = call_target.clone();
                let request_method_name = method_name.clone();
                let request_input = input_value.clone();
                let request_scope_card = scope_card.clone();
                begun
                    .start_live_with_index_and_span(self, move |start_index| {
                        let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
                        let started = span_started(&idempotency_key);
                        Ok((
                            HostRequestGolemRpcInvoke {
                                remote_agent_id: remote_agent_id.agent_id(),
                                request_key: InvocationRequestKey::Internal(idempotency_key),
                                method_name: request_method_name,
                                input: request_input,
                                logical_streaming_origin: None,
                                remote_agent_type: None,
                                remote_agent_parameters: None,
                                scope_card: request_scope_card,
                            },
                            started,
                        ))
                    })
                    .await?
            }
            ResolvedCall::Replay(handle) => {
                if recorded_rpc_denial(self, &handle).await?.is_some() {
                    return replay_async_rpc_denial(self, handle).await;
                }
                if self_call {
                    return Err(anyhow::anyhow!(
                        "RPC calls to the same agent are not supported"
                    ));
                }
                handle
            }
        };
        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let freshness = if known_fresh_dispatch_allowed(handle.is_live(), ephemeral.is_some()) {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let identity = prepare_rpc_invocation_identity(
            self,
            &this,
            handle.start_index(),
            &remote_agent_id,
            &idempotency_key,
            &method_name,
            freshness,
        )
        .await
        .map_err(|error| handle.trap(error))?;
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                "RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;
        let (invocation_id, _) = match identity {
            Ok(identity) => identity,
            Err(error) => {
                let finished = rpc_span_finished(span.span_id(), &Err::<(), _>(error.clone()));
                complete_rpc_preparation_failure(
                    self,
                    handle,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: Err(error.clone().into()),
                    },
                    finished,
                )
                .await?;
                return bake_closed_async_error(self.table(), error);
            }
        };
        let metadata = invocation_metadata(&invocation_id);

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            request_key: InvocationRequestKey::Internal(idempotency_key.clone()),
            method_name: method_name.clone(),
            input: input_value.clone(),
            logical_streaming_origin: None,
            remote_agent_type: None,
            remote_agent_parameters: None,
            scope_card: scope_card.clone(),
        };

        let remote_target = dispatch_target(&remote_agent_id, ephemeral.as_ref());
        let retry_properties = golem_common::model::RetryContext::rpc(
            "invoke-and-await",
            &remote_agent_id,
            remote_component_id,
            &remote_agent_type.type_name,
            &method_name,
        );
        if handle.is_live() {
            let mut handle = handle;

            let initial_freshness_disposition = InvocationFreshnessDisposition::MayExist;

            let auth_ctx = handle.take_agent_auth_ctx();
            let task = spawn_invoke_and_await_task(
                self,
                &remote_target,
                idempotency_key,
                method_name,
                input_value,
                env.clone(),
                config.clone(),
                deferred_activation.clone(),
                expected_callee_fingerprint,
                retry_properties.clone(),
                span.span_id(),
                handle.trap_context().retry_from,
                handle.start_index(),
                initial_freshness_disposition,
                scope_card,
                auth_ctx.clone(),
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: Some(Arc::new(tokio::sync::Mutex::new(task))),
                    streaming: None,
                    request,
                    invocation_id,
                    remote_target: remote_target.clone(),
                    env,
                    config,
                    deferred_activation,
                    expected_callee_fingerprint,
                    retry_properties: retry_properties.clone(),
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    retry_auth_ctx: Some(auth_ctx),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata: Some(metadata),
            })
        } else {
            // Replay: claim the eager `Start` from the oplog now. The RPC is not re-dispatched here.
            // On a normal replay `get` replays the matching `End`, and `cancel` / `drop` consume the
            // matching `Cancelled`. If the worker crashed after this `Start` but before its terminal,
            // `get`'s `replay_access` returns `Incomplete` (a read-only `WriteRemote` call is safe to
            // re-execute) and `get` re-dispatches the RPC there to complete the existing `Start`.
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: None,
                    streaming: None,
                    request,
                    invocation_id,
                    remote_target: remote_target.clone(),
                    env,
                    config,
                    deferred_activation,
                    expected_callee_fingerprint,
                    retry_properties: retry_properties.clone(),
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    retry_auth_ctx: None,
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata: Some(metadata),
            })
        }
    }

    async fn schedule_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
    ) -> anyhow::Result<Result<ScheduledInvocationReceipt, RpcError>> {
        match self
            .schedule_cancelable_invocation_impl(
                this,
                scheduled_time,
                method_name,
                input,
                scope_card,
                "schedule-invocation",
            )
            .await?
        {
            Ok(cancellation_token) => {
                let entry = self.table().delete(cancellation_token)?;
                Ok(Ok(ScheduledInvocationReceipt {
                    metadata: entry.metadata,
                }))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
        scope_card: Option<Resource<PermissionCardHandleRep>>,
    ) -> anyhow::Result<Result<CancelableScheduledInvocationReceipt, RpcError>> {
        match self
            .schedule_cancelable_invocation_impl(
                this,
                scheduled_time,
                method_name,
                input,
                scope_card,
                "schedule-cancelable-invocation",
            )
            .await?
        {
            Ok(cancellation_token) => {
                let metadata = self.table().get(&cancellation_token)?.metadata.clone();
                Ok(Ok(CancelableScheduledInvocationReceipt {
                    metadata,
                    cancellation_token,
                }))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    async fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        let entry = self.table().delete(rep)?;
        if let Ok(payload) = entry.payload.downcast::<WasmRpcEntryPayload>() {
            record_released_resource_span(self, payload.creation_index, &payload.span_id).await?;
            finish_span_in_memory(self, &payload.span_id)?;
        }
        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    fn record_outbound_rpc_call(&mut self) -> anyhow::Result<()> {
        if self.state.is_live() {
            self.state
                .check_and_increment_rpc_call_count()
                .map_err(wasmtime::Error::from)?;
            self.record_monthly_rpc_call()?;
        }
        Ok(())
    }
}

/// Completes the connection call of a durable client: replays a recorded binding, or binds the
/// client live.
async fn bind_rpc_connection<Ctx: WorkerCtx, F>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: DurableCallSession<F, NotCancellable>,
    span: Arc<InvocationContextSpan>,
    binding: Box<ResolvedTarget>,
    agent_type: Arc<AgentTypeSchema>,
    component_id: ComponentId,
    method_streams: Arc<BTreeMap<String, bool>>,
    owner: AgentOwnerPattern,
) -> anyhow::Result<Result<Resource<WasmRpcEntry>, RpcError>>
where
    F: golem_common::model::oplog::payload::HostPayloadPair<
            Req = HostRequestGolemRpcCreate,
            Resp = HostResponseGolemRpcCreate,
        >,
{
    let creation_index = (!ctx.state.snapshotting_mode).then(|| handle.start_index());
    let handle = if handle.is_live() {
        handle
    } else {
        match handle.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => {
                let fingerprint = match response.result {
                    Ok(fingerprint) => fingerprint,
                    Err(error) => {
                        finish_span_in_memory(ctx, span.span_id())?;
                        return Ok(Err(InternalRpcError::from(error).into()));
                    }
                };
                return reconstruct_wasm_rpc_resource(
                    ctx,
                    binding.owned_agent_id.agent_id,
                    response.target_environment_id,
                    fingerprint,
                    vec![],
                    vec![],
                    span,
                    agent_type,
                    component_id,
                    method_streams,
                    owner,
                    creation_index,
                )
                .await
                .map(Ok);
            }
            CallReplayOutcome::Incomplete(live) => live,
        }
    };
    construct_wasm_rpc_resource(
        ctx,
        handle,
        binding,
        &[],
        vec![],
        span,
        agent_type,
        component_id,
        method_streams,
        owner,
        creation_index,
    )
    .await
}
