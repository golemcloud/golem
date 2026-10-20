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

impl<Ctx: WorkerCtx> Clone for DirectWorkerInvocationRpc<Ctx> {
    fn clone(&self) -> Self {
        Self {
            remote_rpc: self.remote_rpc.clone(),
            direct_invocation_auth: self.direct_invocation_auth.clone(),
            active_agents: self.active_agents.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            card_service: self.card_service.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            quota_service: self.quota_service.clone(),
            worker_fork: self.worker_fork.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            rdbms_service: self.rdbms_service.clone(),
            oplog_service: self.oplog_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
            resource_limits: self.resource_limits.clone(),
            shutdown_token: self.shutdown_token.clone(),
            environment_state_service: self.environment_state_service.clone(),
            agent_filesystem_snapshots: self.agent_filesystem_snapshots.clone(),
            native_tool_catalog: self.native_tool_catalog.clone(),
            agent_types_service: self.agent_types_service.clone(),
            agent_webhooks_service: self.agent_webhooks_service.clone(),
            external_durable_streams: self.external_durable_streams.clone(),
            http_connection_pool: self.http_connection_pool.clone(),
            websocket_connection_pool: self.websocket_connection_pool.clone(),
            mcp_transport: self.mcp_transport.clone(),
            extra_deps: self.extra_deps.clone(),
            leak_sentinel: self.leak_sentinel.clone(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        remote_rpc: Arc<RemoteInvocationRpc>,
        direct_invocation_auth: Arc<dyn DirectInvocationAuthService>,
        active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        card_service: Arc<dyn card::CardService>,
        component_service: Arc<dyn component::ComponentService>,
        worker_fork: Arc<dyn worker_fork::WorkerForkService>,
        worker_service: Arc<dyn worker::WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService,
        >,
        promise_service: Arc<dyn promise::PromiseService>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
        quota_service: Arc<dyn crate::services::quota::QuotaService>,
        key_value_service: Arc<dyn key_value::KeyValueService>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        oplog_service: Arc<dyn oplog::OplogService>,
        scheduler_service: Arc<dyn scheduler::SchedulerService>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx>>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        resource_limits: Arc<dyn ResourceLimits>,
        shutdown_token: tokio_util::sync::CancellationToken,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_filesystem_snapshots: Arc<AgentFilesystemSnapshots>,
        native_tool_catalog: Arc<crate::native_tool::NativeToolCatalog<Ctx>>,
        agent_types_service: Arc<dyn agent_types::AgentTypesService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        external_durable_streams: Arc<dyn ExternalDurableStreamService>,
        http_connection_pool: Option<HttpConnectionPool>,
        websocket_connection_pool: WebSocketConnectionPool,
        mcp_transport: Arc<crate::services::mcp::McpTransport>,
        extra_deps: Ctx::ExtraDeps,
        leak_sentinel: Arc<()>,
    ) -> Self {
        Self {
            remote_rpc,
            direct_invocation_auth,
            active_agents,
            engine,
            linker,
            runtime,
            card_service,
            component_service,
            shard_manager_service,
            quota_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            oplog_service,
            scheduler_service,
            worker_activator,
            events,
            file_loader,
            oplog_processor_plugin,
            resource_limits,
            shutdown_token,
            environment_state_service,
            agent_filesystem_snapshots,
            native_tool_catalog,
            agent_types_service,
            agent_webhooks_service,
            external_durable_streams,
            http_connection_pool,
            websocket_connection_pool,
            mcp_transport,
            extra_deps,
            leak_sentinel,
        }
    }
    /// Rewrites the `OwnedAgentId` so that `environment_id` comes from the
    /// target component's metadata rather than from the caller. This ensures
    /// that auth checks, shard routing, and all downstream code use the
    /// component-authoritative environment.
    async fn canonicalize_owned_agent_id(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<OwnedAgentId, RpcError> {
        let component = self
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await
            .map_err(|e| RpcError::RemoteInternalError {
                details: format!("Failed to resolve target component metadata: {e}"),
            })?;
        Ok(OwnedAgentId::new(
            component.environment_id,
            &owned_agent_id.agent_id,
        ))
    }

    async fn validate_method_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        method_parameters: &SchemaValue,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<bool, RpcError> {
        let component_revision = method_validation_revision(freshness_disposition, || async {
            Ok(Worker::<Ctx>::get_latest_metadata(self, owned_agent_id)
                .await?
                .map(|metadata| metadata.last_known_status.component_revision))
        })
        .await?;
        let component = self
            .component_service()
            .get_metadata(owned_agent_id.component_id(), component_revision)
            .await?;
        let parsed_agent_id =
            ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                .map_err(|details| RpcError::ProtocolError { details })?;
        validate_agent_method_invocation(
            &component.metadata,
            Some(&parsed_agent_id),
            method_name,
            method_parameters,
        )
        .map_err(Into::into)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Ensuring local target worker exists");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Method(AgentMethodName(method_name.to_string())),
                    auth_ctx,
                )
                .await?;

            let worker = Worker::get_or_create_suspended(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                Principal::Agent(AgentPrincipal {
                    agent_id: self_agent_id.clone(),
                }),
            )
            .await?;

            let fingerprint = worker.get_initial_worker_metadata().fingerprint;
            Ok(Box::new(LoggingDemand::new(
                owned_agent_id.agent_id(),
                fingerprint,
            )))
        } else {
            self.remote_rpc
                .create_demand(
                    owned_agent_id,
                    method_name,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await
        }
    }

    async fn invoke_and_await(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(RpcError::ProtocolError {
                details: "KnownFresh requires an idempotency key".to_string(),
            });
        }

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Local direct agent invoke_and_await");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                    auth_ctx,
                )
                .await?;

            if self
                .validate_method_invocation(
                    owned_agent_id,
                    &method_name,
                    &method_parameters,
                    freshness_disposition,
                )
                .await?
            {
                return Err(RpcError::ProtocolError {
                    details: "live streams require the attached streaming RPC".to_string(),
                });
            }

            let principal = caller_agent_principal(self_agent_id);
            let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
            Worker::<Ctx>::validate_invocation_freshness(
                self,
                owned_agent_id,
                &idempotency_key,
                freshness_disposition,
            )
            .await?;
            let worker = Worker::get_or_create_suspended_with_freshness(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                principal.clone(),
                freshness_disposition,
            )
            .await?;

            let invocation = AgentInvocation::AgentMethod {
                idempotency_key,
                method_name,
                input: method_parameters,
                invocation_context: self_stack,
                principal,
                scope_card,
            };

            let output = worker.invoke_and_await(invocation).await?;

            match output.result {
                AgentInvocationResult::AgentMethod { output } => Ok(output),
                _ => Err(RpcError::RemoteInternalError {
                    details:
                        "Expected a result from agent invoke_and_await but got a non-method result"
                            .to_string(),
                }),
            }
        } else {
            self.remote_rpc
                .invoke_and_await(
                    owned_agent_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    freshness_disposition,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                    scope_card,
                )
                .await
        }
    }

    async fn invoke_and_await_streaming(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: IdempotencyKey,
        method_name: String,
        method_parameters: ProtoSchemaValue,
        input_mappings: Vec<DurableStreamMapping>,
        expected_callee_fingerprint: AgentFingerprint,
        attempt_id: uuid::Uuid,
        origin_invocation: StreamInvocationIdentity,
        accepted_inputs: tokio::sync::oneshot::Sender<InvocationAccepted>,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<DurableRpcInvocationResult, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;
        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_err()
        {
            return self
                .remote_rpc
                .invoke_and_await_streaming(
                    owned_agent_id,
                    idempotency_key,
                    method_name,
                    method_parameters,
                    input_mappings,
                    expected_callee_fingerprint,
                    attempt_id,
                    origin_invocation,
                    accepted_inputs,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                    scope_card,
                )
                .await;
        }

        self.direct_invocation_auth
            .check(
                self_created_by,
                owned_agent_id,
                AgentVerb::Invoke,
                AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                auth_ctx,
            )
            .await?;

        Worker::<Ctx>::validate_invocation_freshness(
            self,
            owned_agent_id,
            &idempotency_key,
            InvocationFreshnessDisposition::MayExist,
        )
        .await?;
        let principal = caller_agent_principal(self_agent_id);
        let (worker, _response_lease) = Worker::get_or_create_suspended_for_response(
            self,
            owned_agent_id,
            Some(self_env.to_vec()),
            config.clone(),
            None,
            Some(self_agent_id.clone()),
            &self_stack,
            principal.clone(),
            InvocationFreshnessDisposition::MayExist,
        )
        .await?;
        if worker.get_initial_worker_metadata().fingerprint != expected_callee_fingerprint {
            return Err(RpcError::ProtocolError {
                details: "expected callee fingerprint does not match the active agent incarnation"
                    .to_string(),
            });
        }
        let status = worker.get_last_known_status().await;
        let component = self
            .component_service()
            .get_metadata(
                owned_agent_id.component_id(),
                Some(status.component_revision),
            )
            .await?;
        let input_encoded_len = method_parameters.encoded_len();
        let input = decode_invocation_input(method_parameters)
            .map_err(|details| RpcError::ProtocolError { details })?;
        let parsed_agent_id =
            ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                .map_err(|details| RpcError::ProtocolError { details })?;
        if !validate_agent_method_invocation(
            &component.metadata,
            Some(&parsed_agent_id),
            &method_name,
            &input,
        )? {
            return Err(RpcError::ProtocolError {
                details: "durable streaming invocation requires a streaming agent method"
                    .to_string(),
            });
        }

        let start = InvocationStart {
            agent_id: Some(owned_agent_id.agent_id().into()),
            method_name: Some(method_name.clone()),
            input: None,
            idempotency_key: Some(idempotency_key.clone().into()),
            context: Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                parent: Some(self_agent_id.clone().into()),
                env: HashMap::from_iter(self_env.to_vec()),
                tracing: Some(self_stack.clone().into()),
            }),
            auth_ctx: Some(auth_ctx.clone().into()),
            principal: Some(principal.clone().into()),
            environment_id: Some(owned_agent_id.environment_id.into()),
            config: config.into_iter().map(Into::into).collect(),
            component_owner_account_id: None,
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(attempt_id.into()),
            expected_callee_fingerprint: Some(expected_callee_fingerprint.0.into()),
            durable_input_mappings: input_mappings,
            origin_invocation: Some(origin_invocation),
            scope_card: scope_card
                .as_ref()
                .map(golem_api_grpc::proto::golem::worker::EncodedScopeCard::try_from)
                .transpose()
                .map_err(|details| RpcError::ProtocolError { details })?,
            external_tool: None,
        };
        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: idempotency_key.clone(),
            method_name,
            input,
            invocation_context: self_stack,
            principal,
            scope_card,
        };
        let (acceptance_committed, accepted) = tokio::sync::oneshot::channel();
        let request = build_durable_streaming_request(
            &start,
            &component.metadata,
            component.revision,
            expected_callee_fingerprint,
            invocation,
            input_encoded_len,
            acceptance_committed,
            self.config()
                .limits
                .live_stream_event_broadcast_capacity
                .get(),
        )?;
        let acceptance = worker
            .clone()
            .accept_durable_streaming_invocation(request)
            .await?;
        accepted.await.map_err(|_| RpcError::RemoteInternalError {
            details: "durable streaming acceptance was not committed".to_string(),
        })?;
        let input_mappings = worker
            .durable_stream_producer()
            .await?
            .materialize_bindings(&acceptance.prepared.stream_mappings)
            .await
            .map_err(|error| RpcError::RemoteInternalError {
                details: error.to_string(),
            })?;
        let _ = accepted_inputs.send(InvocationAccepted {
            agent_id: start.agent_id.clone(),
            idempotency_key: start.idempotency_key.clone(),
            component_revision: Some(component.revision.get()),
            attachment_id: (!acceptance.joined_origin_observer)
                .then(|| acceptance.prepared.attempt.attachment_id.0.into()),
            attempt_id: (!acceptance.joined_origin_observer)
                .then(|| acceptance.prepared.attempt.attempt_id.0.into()),
            epoch: if acceptance.joined_origin_observer {
                0
            } else {
                acceptance.streams.attachment_epoch()
            },
            stream_mappings: input_mappings
                .iter()
                .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                .collect(),
            environment_id: Some(
                acceptance
                    .prepared
                    .attempt
                    .session_key
                    .callee_environment_id
                    .into(),
            ),
            callee_fingerprint: Some(
                acceptance
                    .prepared
                    .attempt
                    .expected_callee_fingerprint
                    .0
                    .into(),
            ),
            method_name: start.method_name.clone(),
            joined_origin_observer: acceptance.joined_origin_observer,
            tool_name: None,
            command_path: Vec::new(),
            terminal_cursor_stream_ids: Vec::new(),
        });
        acceptance
            .streams
            .recover_nested_input_mappings()
            .await
            .map_err(|error| RpcError::RemoteInternalError {
                details: error.to_string(),
            })?;
        let result = acceptance.streams.wait_persisted_result();
        let completion = worker.await_enqueued_invocation(idempotency_key);
        tokio::pin!(result);
        tokio::pin!(completion);
        let result = tokio::select! {
            result = &mut result => result
                .map_err(|error| RpcError::RemoteInternalError {
                details: error.to_string(),
            })?,
            output = &mut completion => {
                let output = output?;
                if !matches!(output.result, AgentInvocationResult::AgentMethod { .. }) {
                    return Err(RpcError::RemoteInternalError {
                        details: "durable streaming invocation returned a non-method result".to_string(),
                    });
                }
                acceptance
                    .streams
                    .persisted_result()
                    .await
                    .map_err(|error| RpcError::RemoteInternalError {
                details: error.to_string(),
            })?
                    .ok_or_else(|| RpcError::RemoteInternalError {
                        details: "durable streaming invocation completed without a persisted result".to_string(),
                    })?
            }
        };
        Ok(DurableRpcInvocationResult {
            output_mappings: result.proto_mappings(),
            value: result.value,
        })
    }

    async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<RpcError>> {
        let key = request.operation.key();
        let target = if request.operation.targets_consumer() {
            OwnedAgentId::new(key.consumer_environment_id, &key.consumer)
        } else {
            OwnedAgentId::new(key.producer_environment_id, &key.producer)
        };
        if self.shard_service().check_worker(&target.agent_id).is_err() {
            debug!(target = %target, "Routing durable stream attachment control to a remote shard");
            return self
                .remote_rpc
                .control_durable_stream_attachment(request, auth_ctx)
                .await;
        }

        debug!(target = %target, "Routing durable stream attachment control within the local shard");

        self.direct_invocation_auth
            .check(
                auth_ctx.actor_account_id(),
                &target,
                AgentVerb::Invoke,
                AgentResourcePattern::Any,
                auth_ctx,
            )
            .await?;
        let (worker, _response_lease) = Worker::find_durable_stream_worker(self, &target)
            .await
            .map_err(|error| error.map_other(RpcError::from))?
            .ok_or_else(|| WorkerExecutorError::worker_not_found(target.agent_id()))
            .map_err(RpcError::from)?;
        worker
            .control_durable_stream_attachment(request)
            .await
            .map_err(|error| error.map_other(RpcError::from))
    }

    async fn read_durable_stream_segment(
        &self,
        request: DurableStreamReadRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<RpcError>> {
        let producer = match &request {
            DurableStreamReadRequest::AttachedConsumer(request) => OwnedAgentId::new(
                request.attachment.producer_environment_id,
                &request.attachment.producer,
            ),
            DurableStreamReadRequest::AuthorizedExport(request) => OwnedAgentId::new(
                request.handle.producer_environment_id,
                &request.handle.producer,
            ),
        };
        if self
            .shard_service()
            .check_worker(&producer.agent_id)
            .is_err()
        {
            debug!(producer = %producer, "Routing durable stream segment read to a remote shard");
            return self
                .remote_rpc
                .read_durable_stream_segment(request, auth_ctx)
                .await;
        }

        debug!(producer = %producer, "Routing durable stream segment read within the local shard");

        match &request {
            DurableStreamReadRequest::AttachedConsumer(_) => {
                self.direct_invocation_auth
                    .check(
                        auth_ctx.actor_account_id(),
                        &producer,
                        AgentVerb::View,
                        AgentResourcePattern::Any,
                        auth_ctx,
                    )
                    .await?;
            }
            DurableStreamReadRequest::AuthorizedExport(_) => auth_ctx
                .authorize_system_only("read authorized durable stream export")
                .map_err(|error| RpcError::Denied {
                    details: error.to_string(),
                })?,
        }
        let (worker, _response_lease) = Worker::find_durable_stream_worker(self, &producer)
            .await
            .map_err(|error| error.map_other(RpcError::from))?
            .ok_or_else(|| WorkerExecutorError::worker_not_found(producer.agent_id()))
            .map_err(RpcError::from)?;
        match request {
            DurableStreamReadRequest::AttachedConsumer(request) => {
                let events = worker
                    .read_durable_stream_segment(*request)
                    .await
                    .map_err(|error| error.map_other(RpcError::from))?;
                golem_common::serialization::serialize(&events)
                    .map_err(WorkerExecutorError::runtime)
                    .map_err(RpcError::from)
                    .map_err(Into::into)
            }
            DurableStreamReadRequest::AuthorizedExport(request) => worker
                .read_durable_stream_by_handle(*request)
                .await
                .map_err(|error| error.map_other(RpcError::from)),
        }
    }

    async fn invoke(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(RpcError::ProtocolError {
                details: "KnownFresh requires an idempotency key".to_string(),
            });
        }

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Local direct agent invoke (fire-and-forget)");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                    auth_ctx,
                )
                .await?;

            if self
                .validate_method_invocation(
                    owned_agent_id,
                    &method_name,
                    &method_parameters,
                    freshness_disposition,
                )
                .await?
            {
                return Err(RpcError::ProtocolError {
                    details: "live streams cannot be used in fire-and-forget invocations"
                        .to_string(),
                });
            }

            let principal = caller_agent_principal(self_agent_id);
            let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
            Worker::<Ctx>::validate_invocation_freshness(
                self,
                owned_agent_id,
                &idempotency_key,
                freshness_disposition,
            )
            .await?;
            let worker = Worker::get_or_create_suspended_with_freshness(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                principal.clone(),
                freshness_disposition,
            )
            .await?;

            let invocation = AgentInvocation::AgentMethod {
                idempotency_key,
                method_name,
                input: method_parameters,
                invocation_context: self_stack,
                principal,
                scope_card: None,
            };

            match worker.invoke_and_start(invocation).await? {
                crate::worker::ResultOrSubscription::Finished(Err(err)) => Err(err.into()),
                crate::worker::ResultOrSubscription::Finished(Ok(_)) => Ok(()),
                crate::worker::ResultOrSubscription::Pending(_) => Ok(()),
            }
        } else {
            self.remote_rpc
                .invoke(
                    owned_agent_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    freshness_disposition,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await
        }
    }
}
