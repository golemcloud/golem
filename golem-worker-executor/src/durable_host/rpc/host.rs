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
    PreparedRpcInvocation, RpcCallTarget, RpcTargetAdmission, WasmRpcEntry, WasmRpcEntryPayload,
    await_streaming_rpc_acceptance, bake_recorded_async_invoke_denial, caller_durable_rpc_streams,
    construct_ephemeral_wasm_rpc_resource, construct_wasm_rpc_resource, current_streaming_origin,
    denied_rpc_failure, discard_owned_rpc_config, discard_owned_rpc_input,
    embedded_invocation_span_builder, ensure_rpc_target_activated, find_agent_method,
    invocation_metadata, known_fresh_dispatch_allowed, persist_async_invoke_denial,
    persist_async_invoke_denial_from_begun, persist_async_validation_rejection,
    persist_invoke_and_await_denial, persist_invoke_and_await_denial_from_begun,
    persist_invoke_denial, persist_invoke_denial_from_begun, prepare_rpc_invocation,
    reconstruct_wasm_rpc_resource, recorded_rpc_denial, reject_non_await_scope_card,
    remote_method_uses_streams, replay_async_rpc_denial, replay_rpc_denial,
    resolve_method_and_lift_input, restore_embedded_span, rpc_input_root, rpc_output_root,
    rpc_span_finished, run_invoke, run_invoke_and_await, schema_value_to_wire_output,
    spawn_invoke_and_await_task, spawn_streaming_invoke_and_await_task, start_rpc_connection_call,
    streaming_target_fingerprint,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::durable_session::durable_stream_mapping_from_proto;
use crate::durable_host::durable_session::strip_streams;
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
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
use crate::workerctx::InvocationContextManagement;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use futures::FutureExt;
use futures::future::Either;
use golem_common::base_model::agent::AgentMode;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::HostRequestGolemRpcResource;
use golem_common::model::oplog::HostResponseGolemRpcCreate;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::HostResponseGolemRpcUnit;
use golem_common::model::oplog::SpanFinished;
use golem_common::model::oplog::SpanOutcome;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcDrop;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvoke;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::schema::agent::AgentTypeSchema;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::decode_typed_rejecting_quota_with;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::Arc;
use wasmtime::component::Resource;

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
        config: Vec<
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        <Self as HostWasmRpc>::create(self, agent_type_name, constructor, phantom_id, config)
            .await?
            .map_err(|error| anyhow::anyhow!(InternalRpcError::from(error).to_string()))
    }

    async fn create(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
        config: Vec<
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Result<Resource<WasmRpcEntry>, RpcError>> {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let Some(registered_agent_type) = self
            .get_agent_type_schema_model(golem_common::model::agent::AgentTypeName(
                agent_type_name.clone(),
            ))
            .await?
        else {
            discard_owned_rpc_input(constructor, self);
            discard_owned_rpc_config(config, self);
            return Ok(Err(RpcError::RemoteAgentError(WitAgentError::InvalidType(
                agent_type_name,
            ))));
        };

        let input = match schema_value_tree_to_typed_constructor_parameters(
            constructor,
            &registered_agent_type.agent_type,
            self,
        ) {
            Ok(input) => input,
            Err(err) => {
                discard_owned_rpc_config(config, self);
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidInput(format!("Invalid constructor input: {err}")),
                )));
            }
        };

        let component_id: golem_common::model::component::ComponentId =
            registered_agent_type.implemented_by.component_id;
        let component_revision = registered_agent_type.implemented_by.component_revision;
        let remote_component = self
            .component_service()
            .get_metadata(component_id, Some(component_revision))
            .await?;
        let remote_method_streams = Arc::new(
            registered_agent_type
                .agent_type
                .methods
                .iter()
                .map(|method| {
                    remote_component
                        .metadata
                        .agent_method_stream_metadata(
                            &registered_agent_type.agent_type.type_name,
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
        let agent_mode = registered_agent_type.agent_type.mode;
        let owner_component = self.owner_component_metadata();
        let remote_owner = AgentOwnerPattern::Agent {
            account: registered_agent_type.implemented_by.account_email.clone(),
            application: owner_component.application_name.clone(),
            environment: owner_component.environment_name.clone(),
            component: golem_common::model::component::ComponentName(
                registered_agent_type.implemented_by.component_name.clone(),
            ),
            agent: AgentOwnerLeafPattern::AgentTypeWildcard(
                golem_common::model::agent::AgentTypeName(agent_type_name.clone()),
            ),
        };

        // Share the canonical agent type through `WasmRpcEntryPayload`. Every
        // subsequent RPC entry resolves the per-method input/output schema from
        // this cached value to drive the typed flow. `registered_agent_type` is
        // owned and no longer used, so move its agent type into the `Arc` rather
        // than cloning the whole schema graph.
        let remote_agent_type: Arc<AgentTypeSchema> = Arc::new(registered_agent_type.agent_type);

        let agent_id = match golem_common::model::agent::ParsedAgentId::try_new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        ) {
            Ok(agent_id) => agent_id,
            Err(err) => {
                discard_owned_rpc_config(config, self);
                return Ok(Err(RpcError::RemoteAgentError(
                    WitAgentError::InvalidAgentId(err.to_string()),
                )));
            }
        };
        let remote_agent_id =
            match golem_common::model::AgentId::from_agent_id(component_id, &agent_id) {
                Ok(agent_id) => agent_id,
                Err(err) => {
                    discard_owned_rpc_config(config, self);
                    return Ok(Err(RpcError::RemoteAgentError(
                        WitAgentError::InvalidAgentId(err.to_string()),
                    )));
                }
            };

        // Each config value is a guest-owned `typed-schema-value` and never
        // legally carries a quota token. Decode through the rejecting path so any
        // owned `quota-token` handle is deleted from the resource table rather
        // than leaked, and drain every config value before surfacing the first
        // error so a handle in a later entry cannot leak when an earlier one is
        // rejected.
        let mut decoded_config = Vec::with_capacity(config.len());
        let mut config_error: Option<anyhow::Error> = None;
        for c in config {
            match decode_typed_rejecting_quota_with(c.value, self) {
                Ok(typed) => {
                    if config_error.is_none() {
                        // The config value travels as a self-contained
                        // `golem:core@2.0.0` typed-schema-value. Render the inner
                        // `SchemaValue` as plain (schema-guided) JSON, matching
                        // the `AgentConfigEntryDto` service-boundary contract: the
                        // DTO carries plain user JSON which
                        // `parse_worker_creation_agent_config` decodes with the
                        // schema graph (`from_json_value`).
                        match golem_schema::schema::render::to_json_value(
                            typed.graph(),
                            typed.root_type(),
                            typed.value(),
                        ) {
                            Ok(encoded) => decoded_config.push(AgentConfigEntryDto {
                                path: c.path,
                                value: encoded.into(),
                            }),
                            Err(err) => {
                                config_error =
                                    Some(anyhow::anyhow!("Failed serializing agent config: {err}"));
                            }
                        }
                    }
                }
                Err(err) => {
                    if config_error.is_none() {
                        config_error = Some(anyhow::anyhow!("Invalid agent config value: {err}"));
                    }
                }
            }
        }
        if let Some(err) = config_error {
            return Ok(Err(RpcError::RemoteAgentError(
                WitAgentError::InvalidInput(err.to_string()),
            )));
        }
        let config = decoded_config;
        self.check_read_only_allows("golem::rpc::wasm-rpc::new")
            .map_err(wasmtime::Error::from)?;
        let pinned_ephemeral_identity =
            agent_mode == AgentMode::Ephemeral && agent_id.phantom_id.is_some();

        // A phantom-less ephemeral address is a logical proxy: every invocation
        // receives a fresh final identity. A supplied phantom is already a final
        // observation/control identity, so preserve it as a fixed target and let
        // the normal invocation path reject attempts to reuse the terminal agent.
        if agent_mode == AgentMode::Ephemeral && agent_id.phantom_id.is_none() {
            let logical_agent_id = agent_id
                .with_phantom_id(None)
                .map_err(|err| anyhow::anyhow!(err))?;
            let logical_remote_agent_id = AgentId::from_agent_id(component_id, &logical_agent_id)
                .map_err(|err| anyhow::anyhow!(err))?;
            let mut logical_remote_agent_id =
                OwnedAgentId::new(self.owned_agent_id.environment_id, &logical_remote_agent_id);

            let (mut handle, span) = start_rpc_connection_call(
                self,
                HostRequestGolemRpcCreate {
                    remote_agent_id: remote_agent_id.clone(),
                },
                false,
            )
            .await?;
            let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        logical_remote_agent_id.environment_id = response.target_environment_id;
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        handle
                            .complete(
                                self,
                                HostResponseGolemRpcCreate {
                                    target_fingerprint: AgentFingerprint(uuid::Uuid::nil()),
                                    target_environment_id: logical_remote_agent_id.environment_id,
                                },
                            )
                            .await?;
                    }
                }
            } else {
                handle
                    .complete(
                        self,
                        HostResponseGolemRpcCreate {
                            target_fingerprint: AgentFingerprint(uuid::Uuid::nil()),
                            target_environment_id: logical_remote_agent_id.environment_id,
                        },
                    )
                    .await?;
            }

            return construct_ephemeral_wasm_rpc_resource(
                self,
                logical_remote_agent_id,
                logical_agent_id,
                env,
                config,
                span,
                remote_agent_type,
                remote_method_streams,
                component_revision,
                remote_owner,
                creation_index,
            )
            .map(Ok);
        }

        let (handle, span) = start_rpc_connection_call(
            self,
            HostRequestGolemRpcCreate {
                remote_agent_id: remote_agent_id.clone(),
            },
            true,
        )
        .await?;
        let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());

        if !handle.is_live() {
            match handle.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    return reconstruct_wasm_rpc_resource(
                        self,
                        remote_agent_id,
                        response.target_environment_id,
                        response.target_fingerprint,
                        env,
                        config,
                        span,
                        remote_agent_type,
                        remote_method_streams,
                        component_revision,
                        remote_owner,
                        pinned_ephemeral_identity,
                        creation_index,
                    )
                    .await
                    .map(Ok);
                }
                CallReplayOutcome::Incomplete(live) => {
                    return construct_wasm_rpc_resource(
                        self,
                        live,
                        remote_agent_id,
                        &env,
                        config,
                        span,
                        remote_agent_type,
                        remote_method_streams,
                        component_revision,
                        remote_owner,
                        pinned_ephemeral_identity,
                        creation_index,
                    )
                    .await
                    .map(Ok);
                }
            }
        }

        construct_wasm_rpc_resource(
            self,
            handle,
            remote_agent_id,
            &env,
            config.clone(),
            span,
            remote_agent_type,
            remote_method_streams,
            component_revision,
            remote_owner,
            pinned_ephemeral_identity,
            creation_index,
        )
        .await
        .map(Ok)
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

        let admission = match self
            .authorize_and_record_rpc_target_activation(
                &self_,
                &prepared.logical_remote_agent_id,
                &prepared.method_name,
            )
            .await?
        {
            Ok(admission) => admission,
            Err(denial) => {
                if denial.activation_decision.is_some() {
                    return Ok(Err(denial.error));
                }
                return persist_invoke_and_await_denial(
                    self,
                    prepared.logical_remote_agent_id,
                    prepared.method_name,
                    denial.error,
                )
                .await;
            }
        };
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
            let live_streaming_origin = current_streaming_origin(self, parent_key.clone());
            let call_target = RpcCallTarget::new(
                begun.idempotency_key_position(self),
                &prepared.logical_remote_agent_id,
                prepared.ephemeral_logical_agent_id.as_ref(),
            );
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Replay(handle) => handle,
                ResolvedCall::Live(begun) => {
                    if let RpcTargetAdmission::LiveOnly(target) = &admission
                        && let Err(error) = self.authorize_live_permission(target).await?
                    {
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
            let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
            if !handle.is_live() && recorded_rpc_denial(self, &handle).await?.is_some() {
                let response = replay_rpc_denial(self, handle).await?;
                return Ok(Err(denied_rpc_failure(response.result)?.into()));
            }
            let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "streaming RPC Start carrying span_started",
                    format!(
                        "streaming RPC Start at {} without span metadata",
                        handle.start_index()
                    ),
                )
            })?;
            let span = restore_embedded_span(self, &started)?;
            if self.is_live() && !handle.is_live() {
                handle = match handle.replay(self).await? {
                    CallReplayOutcome::Incomplete(handle) => handle,
                    CallReplayOutcome::Replayed(_) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
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
                        "streaming RPC Start with logical origin",
                        format!(
                            "streaming RPC Start at {} without logical origin",
                            handle.start_index()
                        ),
                    )
                })?;
            let mut auth_ctx = handle.is_live().then(|| handle.agent_auth_ctx().clone());
            let (target_fingerprint, _demand) = match streaming_target_fingerprint(
                self,
                &self_,
                &prepared,
                &remote_agent_id,
                auth_ctx.as_ref(),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    return Err(handle.trap(error));
                }
            };
            let stream_auth_ctx = auth_ctx.clone().unwrap_or_else(|| self.agent_auth_ctx());
            let (streams, origin_invocation) = caller_durable_rpc_streams(
                self,
                &remote_agent_id,
                target_fingerprint,
                idempotency_key.clone(),
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
                let agent_id = self.agent_id().clone();
                let created_by = self.created_by();
                let stack = self.clone_as_inherited_stack(span.span_id());
                let interrupt_signal = interrupt_signal.map(Error::from);
                let call = Box::pin(await_streaming_rpc_acceptance(
                    &streams,
                    None,
                    acceptance,
                    rpc.invoke_and_await_streaming(
                        &remote_agent_id,
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
            prepared.ephemeral_logical_agent_id.as_ref(),
        );
        let handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let RpcTargetAdmission::LiveOnly(target) = &admission
                    && let Err(error) = self.authorize_live_permission(target).await?
                {
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
                    let response = replay_rpc_denial(self, handle).await?;
                    return Ok(Err(denied_rpc_failure(response.result)?.into()));
                }
                handle
            }
        };

        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
        let dispatched_scope_card = scope_card.clone();
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "dispatched RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;
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

        let admission = match self
            .authorize_and_record_rpc_target_activation(
                &self_,
                &prepared.logical_remote_agent_id,
                &prepared.method_name,
            )
            .await?
        {
            Ok(admission) => admission,
            Err(denial) => {
                if denial.activation_decision.is_some() {
                    return Ok(Err(denial.error));
                }
                return persist_invoke_denial(
                    self,
                    prepared.logical_remote_agent_id,
                    prepared.method_name,
                    denial.error,
                )
                .await;
            }
        };
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
            prepared.ephemeral_logical_agent_id.as_ref(),
        );
        let handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let RpcTargetAdmission::LiveOnly(target) = &admission
                    && let Err(error) = self.authorize_live_permission(target).await?
                {
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
                    let response = replay_rpc_denial(self, handle).await?;
                    return Ok(Err(denied_rpc_failure(response.result)?.into()));
                }
                handle
            }
        };

        let (idempotency_key, remote_agent_id) = call_target.at(handle.start_index())?;
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "fire-and-forget RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;
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
            remote_method_streams,
            env,
            config,
            ephemeral_logical_agent_id,
        ) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.span_id.clone(),
                payload.remote_agent_type.clone(),
                payload.remote_method_streams.clone(),
                env,
                config,
                payload.ephemeral_logical_agent_id.clone(),
            )
        };
        if ephemeral_logical_agent_id.is_none() && logical_remote_agent_id == own_agent_id {
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
                    ephemeral_logical_agent_id.as_ref(),
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
                        ephemeral_logical_agent_id.as_ref(),
                        &method_name,
                        rpc_error,
                    )
                    .await;
                }
            };
        let method = find_agent_method(&remote_agent_type, &method_name)?;
        let streaming = remote_method_uses_streams(&remote_method_streams, &method.name)?;

        let admission = match self
            .authorize_and_record_rpc_target_activation(
                &this,
                &logical_remote_agent_id,
                &method_name,
            )
            .await?
        {
            Ok(admission) => admission,
            Err(denial) => {
                if let Some(idempotency_key) = denial.activation_decision {
                    return bake_recorded_async_invoke_denial(
                        self,
                        &this,
                        &logical_remote_agent_id,
                        denial.error,
                        idempotency_key,
                    )
                    .await;
                }
                return persist_async_invoke_denial(
                    self,
                    &this,
                    logical_remote_agent_id,
                    method_name,
                    denial.error,
                )
                .await;
            }
        };
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
            let live_streaming_origin = current_streaming_origin(self, parent_key.clone());
            let call_target = RpcCallTarget::new(
                begun.idempotency_key_position(self),
                &logical_remote_agent_id,
                ephemeral_logical_agent_id.as_ref(),
            );
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    if let RpcTargetAdmission::LiveOnly(target) = &admission
                        && let Err(error) = self.authorize_live_permission(target).await?
                    {
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
                                    idempotency_key,
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
            let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
            if !handle.is_live() && recorded_rpc_denial(self, &handle).await?.is_some() {
                return replay_async_rpc_denial(self, handle).await;
            }
            let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "streaming RPC Start carrying span_started",
                    format!(
                        "streaming RPC Start at {} without span metadata",
                        handle.start_index()
                    ),
                )
            })?;
            let span = restore_embedded_span(self, &started)?;
            if self.is_live() && !handle.is_live() {
                handle = match handle.replay(self).await? {
                    CallReplayOutcome::Incomplete(handle) => handle,
                    CallReplayOutcome::Replayed(_) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
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
                        "streaming RPC Start with logical origin",
                        format!(
                            "streaming RPC Start at {} without logical origin",
                            handle.start_index()
                        ),
                    )
                })?;
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: strip_streams(input_value.clone()),
                logical_streaming_origin: Some(logical_streaming_origin.clone()),
                remote_agent_type: None,
                remote_agent_parameters: None,
                scope_card: scope_card.clone(),
            };
            let prepared = PreparedRpcInvocation {
                logical_remote_agent_id,
                ephemeral_logical_agent_id,
                connection_span_id,
                env: env.clone(),
                config: config.clone(),
                method_name: method_name.clone(),
                input_value: input_value.clone(),
                streaming: true,
                remote_agent_type: remote_agent_type.clone(),
            };
            let auth_ctx = handle.is_live().then(|| handle.take_agent_auth_ctx());
            let (target_fingerprint, _demand) = match streaming_target_fingerprint(
                self,
                &this,
                &prepared,
                &remote_agent_id,
                auth_ctx.as_ref(),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => return Err(handle.trap(error)),
            };
            let stream_auth_ctx = auth_ctx.clone().unwrap_or_else(|| self.agent_auth_ctx());
            let (streams, origin_invocation) = caller_durable_rpc_streams(
                self,
                &remote_agent_id,
                target_fingerprint,
                idempotency_key.clone(),
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
            let task = handle.is_live().then(|| {
                spawn_streaming_invoke_and_await_task(
                    self,
                    remote_agent_id.clone(),
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
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            return Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata,
            });
        }

        let deferred_activation = self
            .table()
            .get(&this)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?
            .target_activation
            .deferred_activation();

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
            ephemeral_logical_agent_id.as_ref(),
        );
        // Self-call is only rejected for durable targets: an ephemeral
        // invocation always creates a new instance with a freshly derived
        // identity, so it can never target the caller's own invocation queue.
        let self_call =
            ephemeral_logical_agent_id.is_none() && logical_remote_agent_id == own_agent_id;
        let handle = match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                if let RpcTargetAdmission::LiveOnly(target) = &admission
                    && let Err(error) = self.authorize_live_permission(target).await?
                {
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
                                idempotency_key,
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
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
        let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "RPC Start carrying span_started",
                format!(
                    "RPC Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })?;
        let span = restore_embedded_span(self, &started)?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            logical_streaming_origin: None,
            remote_agent_type: None,
            remote_agent_parameters: None,
            scope_card: scope_card.clone(),
        };

        if handle.is_live() {
            let mut handle = handle;

            let initial_freshness_disposition = if known_fresh_dispatch_allowed(
                self.state.is_live(),
                ephemeral_logical_agent_id.is_some(),
            ) {
                InvocationFreshnessDisposition::KnownFresh
            } else {
                InvocationFreshnessDisposition::MayExist
            };
            if initial_freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
            }

            let auth_ctx = handle.take_agent_auth_ctx();
            let task = spawn_invoke_and_await_task(
                self,
                &remote_agent_id,
                idempotency_key,
                method_name,
                input_value,
                env.clone(),
                config.clone(),
                deferred_activation.clone(),
                span.span_id(),
                handle.start_index(),
                initial_freshness_disposition,
                scope_card,
                auth_ctx,
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: Some(Arc::new(tokio::sync::Mutex::new(task))),
                    streaming: None,
                    request,
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata,
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
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata,
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
        let payload = entry.payload.downcast::<WasmRpcEntryPayload>();
        if let Ok(payload) = payload {
            let Some(creation_index) = payload.creation_index else {
                finish_span_in_memory(self, &payload.span_id)?;
                return Ok(());
            };
            let request = HostRequestGolemRpcResource { creation_index };
            // This record observes guest resource cleanup, not an application write;
            // cleanup remains permitted in read-only invocations.
            let begun = DurableCallSession::<GolemRpcWasmRpcDrop, NotCancellable>::begin(
                self,
                DurableFunctionType::ReadLocal,
            )
            .await?;
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => begun.start_live(self, request).await?,
                ResolvedCall::Replay(handle) => handle,
            };
            if handle.is_live() {
                handle
                    .complete_with_span(
                        self,
                        HostResponseGolemRpcUnit {},
                        SpanFinished {
                            span_id: payload.span_id.clone(),
                            finished_at: golem_common::model::Timestamp::now_utc(),
                            outcome: SpanOutcome::Completed,
                        },
                    )
                    .await?;
            } else {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(_) => {}
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        handle
                            .complete_with_span(
                                self,
                                HostResponseGolemRpcUnit {},
                                SpanFinished {
                                    span_id: payload.span_id.clone(),
                                    finished_at: golem_common::model::Timestamp::now_utc(),
                                    outcome: SpanOutcome::Completed,
                                },
                            )
                            .await?;
                    }
                }
            }
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
