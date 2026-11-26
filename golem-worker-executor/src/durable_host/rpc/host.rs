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

use super::EphemeralRpcTarget;
use super::{
    WasmRpcEntry, WasmRpcEntryPayload, construct_ephemeral_wasm_rpc_resource,
    construct_wasm_rpc_resource, discard_owned_rpc_input, reconstruct_wasm_rpc_resource,
    start_rpc_binding_call, start_rpc_connection_call,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::concurrent::record_released_resource_span;
use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
use crate::durable_host::golem::v1x::agent_operation_denied;
use crate::preview2::golem::agent::common::AgentError as WitAgentError;
use crate::preview2::golem::agent::host::HostWasmRpc;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::HasWorker;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::{ResolvedTarget, keyed_target};
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use futures::FutureExt;
use futures::future::Either;
use golem_common::base_model::agent::AgentMode;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::{AgentTypeName, ParsedAgentKey, RegisteredAgentType};
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentName;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostResponseGolemRpcCreate;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcBindSelf;
use golem_common::model::oplog::host_functions::{
    GolemAgentLookupAgentId, GolemRpcWasmRpcGetById, GolemRpcWasmRpcUpsert,
};
use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::oplog::{HostRequestGolemApiAgentId, HostResponseGolemAgentLookupAgentId};
use golem_common::model::oplog::{HostRequestGolemRpcBindSelf, HostResponseGolemRpcUnit};
use golem_common::model::oplog::{HostRequestGolemRpcGetById, HostResponseGolemRpcCreated};
use golem_common::schema::agent::AgentTypeSchema;
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
    pub(super) fn record_outbound_rpc_call(&mut self) -> anyhow::Result<()> {
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
