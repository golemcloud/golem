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

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        owned_agent_id: OwnedAgentId,
        owner_context: ResolvedOwnerContext,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        quota_service: Arc<dyn QuotaService>,
        event_service: Arc<dyn WorkerEventService>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Ctx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        card_service: Arc<dyn CardService>,
        card_interest_index: Arc<CardInterestIndex>,
        component_service: Arc<dyn ComponentService>,
        resource_limits: Arc<AtomicResourceEntry>,
        config: Arc<GolemConfig>,
        filesystem: crate::workerctx::WorkerFilesystemContext,
        linear_memory: LinearMemoryTracker,
        worker_config: AgentConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        worker_fork: Arc<dyn WorkerForkService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        shard_service: Arc<dyn ShardService>,
        http_connection_pool: Option<HttpConnectionPool>,
        websocket_connection_pool: websocket::WebSocketConnectionPool,
        pending_update: Option<HydratedUpdate>,
        original_phantom_id: Option<Uuid>,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        runtime: OwnerRuntime,
        entity_execution_mode: Option<InvocationExecutionMode>,
        owner_execution: Arc<OwnerExecution>,
        owner_resources: Arc<OwnerRuntimeResources>,
        entity_reconstruction_claim_hook: Option<
            Arc<dyn crate::workerctx::EntityReconstructionClaimHook>,
        >,
        replay_admission_hook: Option<Arc<dyn crate::workerctx::ReplayAdmissionHook>>,
        filesystem_capability: FilesystemCapability,
        executable: crate::workerctx::WorkerCtxExecutable,
        entity_activation: Option<Arc<golem_common::model::entity::EntityActivation>>,
    ) -> Result<Self, WorkerExecutorError> {
        let crate::workerctx::WorkerFilesystemContext {
            generation_handle: filesystem_generation_handle,
            preopen: filesystem_preopen,
        } = filesystem;
        if matches!(runtime, OwnerRuntime::Agent) != entity_execution_mode.is_none() {
            return Err(WorkerExecutorError::runtime(
                "Entity execution mode must be supplied exactly for entity Stores",
            ));
        }
        if runtime == OwnerRuntime::Agent && filesystem_capability != FilesystemCapability::Capable
        {
            return Err(WorkerExecutorError::runtime(
                "The primary Store must be filesystem-capable",
            ));
        }

        if runtime != OwnerRuntime::Agent {
            linear_memory.begin_reconstruction_usage();
        }
        filesystem_generation_handle
            .attach_usage_tracker(
                &linear_memory,
                filesystem_capability == FilesystemCapability::Capable,
            )
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;

        debug!(
            "Worker {} initialized with skipped regions {}",
            owned_agent_id.agent_id, worker_config.skipped_regions
        );

        debug!(
            "Worker {} starting replay from component revision {}",
            owned_agent_id.agent_id, worker_config.component_revision_for_replay
        );
        if let crate::workerctx::WorkerCtxExecutable::Component(component) = &executable {
            if component.revision != worker_config.component_revision_for_replay {
                return Err(WorkerExecutorError::runtime(format!(
                    "Executable component revision {} does not match context revision {}",
                    component.revision, worker_config.component_revision_for_replay
                )));
            }
            if runtime == OwnerRuntime::Agent && component.id != owned_agent_id.component_id() {
                return Err(WorkerExecutorError::runtime(
                    "Primary Store executable must be the owner component",
                ));
            }
        } else if runtime == OwnerRuntime::Agent {
            return Err(WorkerExecutorError::runtime(
                "Primary Store requires a component executable",
            ));
        }
        match (&runtime, &entity_activation) {
            (OwnerRuntime::Agent, None) => {}
            (OwnerRuntime::Entity(entity), Some(activation)) => {
                if entity != &activation.entity() {
                    return Err(WorkerExecutorError::runtime(
                        "Entity Store activation does not match its runtime selector",
                    ));
                }
            }
            (OwnerRuntime::Agent, Some(_)) => {
                return Err(WorkerExecutorError::runtime(
                    "Primary Store cannot carry an entity activation",
                ));
            }
            (OwnerRuntime::Entity(_), _) => {
                return Err(WorkerExecutorError::runtime(
                    "Entity Store requires an activation matching its runtime selector",
                ));
            }
        }
        match (&executable, entity_activation.as_deref()) {
            (crate::workerctx::WorkerCtxExecutable::Component(component), Some(activation))
                if activation.executable_opt().is_some_and(|target| {
                    target.component_id == component.id
                        && target.component_revision == component.revision
                }) => {}
            (
                crate::workerctx::WorkerCtxExecutable::Native {
                    host_tool_id,
                    implementation_version,
                },
                Some(activation),
            ) if matches!(
                activation.source(),
                golem_common::model::entity::EntityActivationSource::Host {
                    host_tool_id: expected_id,
                    implementation_version: expected_version,
                } if expected_id == host_tool_id && expected_version == implementation_version
            ) => {}
            (crate::workerctx::WorkerCtxExecutable::Component(_), None)
                if runtime == OwnerRuntime::Agent => {}
            _ => {
                return Err(WorkerExecutorError::runtime(
                    "Context executable does not match its validated activation source",
                ));
            }
        }
        match (&runtime, &worker_config.owner_component_metadata) {
            (OwnerRuntime::Agent, None) | (OwnerRuntime::Entity(_), Some(_)) => {}
            (OwnerRuntime::Agent, Some(_)) => {
                return Err(WorkerExecutorError::runtime(
                    "Primary Store cannot carry separate owner component metadata",
                ));
            }
            (OwnerRuntime::Entity(_), None) => {
                return Err(WorkerExecutorError::runtime(
                    "Entity Store requires owner component metadata pinned at dispatch",
                ));
            }
        }
        let component_metadata = match &executable {
            crate::workerctx::WorkerCtxExecutable::Component(component) => {
                component.as_ref().clone()
            }
            crate::workerctx::WorkerCtxExecutable::Native { .. } => worker_config
                .owner_component_metadata
                .as_deref()
                .expect("native entity Store has owner component metadata")
                .clone(),
        };

        let executable_component_metadata = match &executable {
            crate::workerctx::WorkerCtxExecutable::Component(component) => Some(component),
            crate::workerctx::WorkerCtxExecutable::Native { .. } => None,
        };
        if executable_component_metadata
            .is_some_and(|component| component.metadata.has_shared_linear_memory())
        {
            return Err(WorkerExecutorError::worker_creation_failed(
                owned_agent_id.agent_id.clone(),
                SHARED_LINEAR_MEMORY_ERROR,
            ));
        }

        let initial_linear_memory = executable_component_metadata
            .map(|component| component.metadata.initial_linear_memory_bytes())
            .unwrap_or(0);
        if initial_linear_memory > resource_limits.max_memory_limit() as u64 {
            return Err(WorkerExecutorError::worker_creation_failed(
                owned_agent_id.agent_id.clone(),
                format!(
                    "Linear memories require {initial_linear_memory} bytes, exceeding the per-agent limit of {} bytes",
                    resource_limits.max_memory_limit()
                ),
            ));
        }

        let agent_type_provision_configs = match &runtime {
            OwnerRuntime::Agent => owner_context.agent().and_then(|agent_id| {
                component_metadata
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
                    .cloned()
            }),
            OwnerRuntime::Entity(_) => None,
        };
        let agent_config = if matches!(runtime, OwnerRuntime::Agent) {
            let provisioned_config = match &owner_context {
                ResolvedOwnerContext::Agent(_) => agent_type_provision_configs
                    .as_ref()
                    .map(|c| c.config.clone())
                    .unwrap_or_default(),
                ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                    component_metadata
                        .metadata
                        .component_provision_config()
                        .config
                        .clone()
                }
            };
            effective_agent_config(
                worker_config.initial_agent_config.clone(),
                provisioned_config,
            )?
        } else {
            worker_config
                .initial_agent_config
                .iter()
                .map(|entry| (entry.path.clone(), entry.value.clone()))
                .collect()
        };

        let stdin = ManagedStdIn::disabled();
        let stdout = ManagedStdOut::from_stdout(tokio::io::stdout());
        let stderr = ManagedStdErr::from_stderr(tokio::io::stderr());
        let suspend_threshold = match execution_status.read().unwrap().agent_mode() {
            AgentMode::Durable => None,
            AgentMode::Ephemeral => Some(config.suspend.ephemeral_max_sleep),
        };
        let (wasi, io_ctx, table) = wasi_host::create_context(
            &[] as &[&str],
            stdin.clone(),
            stdout,
            stderr,
            |duration| wasmtime::Error::from(SuspendForSleep(duration)),
            suspend_threshold,
            config.agent_sockets.enabled,
        )
        .map_err(|e| WorkerExecutorError::runtime(format!("Could not create WASI context: {e}")))?;
        let mut wasi_http = WasiHttpCtx::new();
        wasi_http.connection_pool = http_connection_pool.clone();
        let http_hooks = DurableHttpHooks {
            connection_pool: http_connection_pool,
            is_replay: Arc::new(AtomicBool::new(false)),
        };
        let (replay_state, owner_wallet) = owner_execution.replay_and_wallet().await?;
        let worker = invocation_queue
            .upgrade()
            .expect("worker must remain alive while creating its context");
        let published_authority_generation = worker.published_authority_generation();
        let tail_work = match &runtime {
            OwnerRuntime::Agent => owner_execution.primary_tail_work_tracker(),
            OwnerRuntime::Entity(_) => tail_work::TailWorkTracker::new(),
        };
        let state = PrivateDurableWorkerState::new(
            owner_context,
            oplog_service,
            oplog.clone(),
            promise_service.clone(),
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            card_service,
            card_interest_index,
            component_service,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            config.clone(),
            owned_agent_id.clone(),
            rpc,
            worker_proxy,
            replay_state,
            owner_wallet,
            runtime.clone(),
            entity_execution_mode,
            tail_work,
            component_metadata,
            worker_config.owner_component_metadata,
            worker_fork,
            file_loader,
            worker_config.created_by,
            worker_config.created_by_email,
            worker_config.initial_agent_config,
            agent_config,
            worker_config.authority_wallet,
            worker_config.entity_start_index,
            shard_service,
            pending_update,
            original_phantom_id,
            worker_config.replay.snapshot.map(|(index, _)| index),
            worker_config.replay.role.clone(),
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            resource_limits.clone(),
            published_authority_generation,
        )
        .await?;
        if state.is_live() {
            linear_memory.switch_to_live();
        }
        let weak_worker = Arc::downgrade(&worker);
        let memory_limits = linear_memory.clone();
        linear_memory.set_limit_exceeded_callback(Arc::new(move || {
            if let Some(worker) = weak_worker.upgrade() {
                worker.request_memory_limit_interrupt(memory_limits.clone());
            }
        }));
        let store_alive_guard = StoreAliveGuard::new(&runtime);

        Ok(DurableWorkerCtx {
            table: Arc::new(Mutex::new(table)),
            wasi: Arc::new(Mutex::new(wasi)),
            io_ctx: Arc::new(Mutex::new(io_ctx)),
            stdin,
            wasi_http,
            http_hooks,
            owned_agent_id: owned_agent_id.clone(),
            runtime,
            executable,
            filesystem: filesystem_capability,
            entity_invocation_scope: None,
            entity_tool_operation: None,
            entity_cancellation: None,
            entity_body_finalization: Default::default(),
            tool_trap_disposal: Arc::new(AtomicBool::new(false)),
            producer_disposal_tasks: Some(crate::worker::tasks::NativeReconstructionTasks::new(
                &worker.tasks,
            )?),
            primary_invocation_start_index: None,
            invocation_principal: None,
            owner_execution,
            runtime_suspension: None,
            _owner_resources: owner_resources,
            entity_reconstruction_claim_hook,
            replay_admission_hook,
            websocket_connection_pool,
            public_state: PublicDurableWorkerState {
                promise_service: promise_service.clone(),
                event_service,
                invocation_queue,
                oplog: oplog.clone(),
            },
            state,
            filesystem_generation_handle,
            filesystem_preopen: Some(AgentDescriptor::new(filesystem_preopen, PathBuf::new())),
            execution_status,
            stream_runtime_teardown: Arc::new(AtomicBool::new(false)),
            resource_limits,
            linear_memory,
            agent_export_funcs: AgentExportFuncs::default(),
            _store_alive_guard: store_alive_guard,
        })
    }
}

impl PrivateDurableWorkerState {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        owner_context: ResolvedOwnerContext,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        promise_service: Arc<dyn PromiseService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        quota_service: Arc<dyn QuotaService>,
        card_service: Arc<dyn CardService>,
        card_interest_index: Arc<CardInterestIndex>,
        component_service: Arc<dyn ComponentService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        config: Arc<GolemConfig>,
        owned_agent_id: OwnedAgentId,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        replay_state: ReplayState,
        owner_wallet: Arc<crate::worker::owner_wallet::OwnerWallet>,
        runtime: OwnerRuntime,
        entity_execution_mode: Option<InvocationExecutionMode>,
        tail_work: tail_work::TailWorkTracker,
        component_metadata: Component,
        owner_component_metadata: Option<Arc<Component>>,
        worker_fork: Arc<dyn WorkerForkService>,
        file_loader: Arc<FileLoader>,
        created_by: AccountId,
        created_by_email: AccountEmail,
        initial_agent_config: Vec<TypedAgentConfigEntry>,
        agent_config: HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
        initial_authority_wallet: Option<Vec<StoredCard>>,
        entity_start_index: Option<OplogIndex>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<HydratedUpdate>,
        original_phantom_id: Option<Uuid>,
        last_snapshot_index: Option<OplogIndex>,
        baseline_role: BaselineRole,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        resource_limit_entry: Arc<AtomicResourceEntry>,
        published_authority_generation: Arc<AtomicU64>,
    ) -> Result<Self, WorkerExecutorError> {
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        let dropped_call_events = tokio::sync::mpsc::unbounded_channel();
        let entity_replay_wallet = match (&runtime, entity_execution_mode) {
            (
                OwnerRuntime::Entity(_),
                Some(
                    InvocationExecutionMode::ReplayingCompleted
                    | InvocationExecutionMode::ReplayingIncomplete,
                ),
            ) => {
                let start = entity_start_index.ok_or_else(|| {
                    WorkerExecutorError::runtime("historical entity Store has no recorded Start")
                })?;
                Some(owner_wallet.entity_replay_wallet(start)?)
            }
            _ => None,
        };
        let entity_wallet_generation = match &entity_replay_wallet {
            Some(entity) => entity.receipt().result.await?.wallet.generation,
            _ => owner_wallet.snapshot().generation,
        };
        let (agent_wallet_cards, wallet_generation) = match &runtime {
            OwnerRuntime::Agent => {
                let historical = owner_wallet.initial_historical_snapshot();
                (historical.cards, historical.generation)
            }
            OwnerRuntime::Entity(_) => (
                initial_authority_wallet
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|card| (card.card_id(), Arc::new(card)))
                    .collect(),
                entity_wallet_generation,
            ),
        };
        let wallet_id_hash = CardHolder::Agent(golem_common::model::card::AgentCardHolder {
            agent_id: owned_agent_id.agent_id.clone(),
        })
        .wallet_id_hash();
        let agent_effective_surface = match (&runtime, &owner_context) {
            (OwnerRuntime::Agent, ResolvedOwnerContext::Agent(agent_id)) => {
                let context =
                    agent_monomorphization_context(&component_metadata, &owned_agent_id, agent_id);
                golem_common::model::card::agent_effective_surface_from_wallet(
                    &context,
                    agent_wallet_cards.values().map(Arc::as_ref),
                )
            }
            (
                OwnerRuntime::Agent,
                ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline,
            ) => component_baseline_effective_surface_from_wallet(
                &component_metadata,
                &owned_agent_id,
                agent_wallet_cards.values().map(Arc::as_ref),
            ),
            (OwnerRuntime::Entity(_), ResolvedOwnerContext::Agent(agent_id)) => {
                let owner_component_metadata = owner_component_metadata
                    .as_deref()
                    .expect("entity Store has owner component metadata");
                let context = agent_monomorphization_context(
                    owner_component_metadata,
                    &owned_agent_id,
                    agent_id,
                );
                golem_common::model::card::agent_effective_surface_from_wallet(
                    &context,
                    agent_wallet_cards.values().map(Arc::as_ref),
                )
            }
            (
                OwnerRuntime::Entity(_),
                ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline,
            ) => component_baseline_effective_surface_from_wallet(
                owner_component_metadata
                    .as_deref()
                    .expect("entity Store has owner component metadata"),
                &owned_agent_id,
                agent_wallet_cards.values().map(Arc::as_ref),
            ),
        };
        let local_live_tail = matches!(entity_execution_mode, Some(InvocationExecutionMode::Live));
        let snapshot_replay_purpose =
            baseline_role.purpose(pending_update.as_ref().map(|update| &update.reference));
        let completion_marker_recorder =
            concurrent::CompletionMarkerRecorder::new(oplog.clone(), replay_state.clone());
        Ok(Self {
            oplog_service,
            oplog,
            owner_context,
            http_call_count: 0,
            per_invocation_http_call_limit,
            rpc_call_count: 0,
            per_invocation_rpc_call_limit,
            scope_card_mint_ordinal: 0,
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            card_service,
            card_interest_index,
            component_service,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            agent_config,
            owned_agent_id,
            current_idempotency_key: None,
            entity_logical_key_position: None,
            rpc,
            worker_proxy,
            resources: HashMap::new(),
            last_resource_id: AgentResourceId::INITIAL,
            entity_execution_mode,
            local_live_tail: Arc::new(AtomicBool::new(local_live_tail)),
            active_custom_invocations: HashMap::new(),
            invocation_ordinals: HashMap::new(),
            tool_invocation_attempt_ordinals: HashMap::new(),
            custom_invocation_scopes: HashMap::new(),
            next_custom_invocation_scope_id: 1,
            open_p3_http_responses: HashMap::new(),
            pending_p3_http_request_transmissions: HashMap::new(),
            open_websocket_connections: HashMap::new(),
            open_filesystem_output_streams: HashMap::new(),
            open_filesystem_input_streams: HashSet::new(),
            file_stream_pollables: HashSet::new(),
            tcp_connect_replay: sockets::tcp::TcpConnectReplay::default(),
            open_tcp_input_streams: HashSet::new(),
            open_tcp_output_streams: HashSet::new(),
            p2_timer_deadlines: HashMap::new(),
            tcp_taken_streams: HashMap::new(),
            snapshotting_mode: false,
            invocation_strictness: InvocationStrictness::Normal,
            read_only_method_name: None,
            operator_authorized_oplog_processor_invocation: Arc::new(AtomicBool::new(false)),
            component_metadata,
            owner_component_metadata,
            authority_wallet: owner_wallet.snapshot(),
            card_boundary: Weak::new(),
            replay_applications: VecDeque::new(),
            owner_wallet,
            entity_replay_wallet,
            agent_effective_surface,
            agent_wallet_cards,
            invocation_scope_card: None,
            invocation_scope_root_cards: BTreeMap::new(),
            invocation_scope_handles: HashMap::new(),
            inherited_invocation_scope: None,
            scope_interest: None,
            owner_revoked_card_ids: HashSet::new(),
            wallet_id_hash,
            wallet_generation,
            published_authority_generation,
            processed_authority_generation: 0,
            authority_initialized: false,
            next_authority_expiration: None,
            replay_state,
            invocation_context,
            current_span_id,
            forward_trace_context_headers: true,
            set_outgoing_http_idempotency_key: true,
            worker_fork,
            file_loader,
            created_by,
            created_by_email,
            initial_agent_config,
            config,
            cached_agent_config_retry_policies: None,
            runtime_retry_policy_mutations: BTreeMap::new(),
            rpc_pollable_to_parent: HashMap::new(),
            shard_service,
            automatic_update_unsettled: pending_update.as_ref().is_some_and(|update| {
                matches!(
                    update.description,
                    UpdateDescription::Automatic { .. }
                        | UpdateDescription::SnapshotAssistedAutomatic { .. }
                )
            }),
            pending_update: tokio::sync::Mutex::new(pending_update),
            current_retry_point: OplogIndex::INITIAL,
            transaction_protocols: HashMap::new(),
            live_host_calls: Arc::new(AtomicUsize::new(0)),
            tail_work,
            invocation_deadline_exceeded: Arc::new(AtomicBool::new(false)),
            tail_work_deadline_exceeded: Arc::new(AtomicBool::new(false)),
            dropped_call_events,
            dropped_call_event_backlog: VecDeque::new(),
            completion_marker_recorder,
            current_phantom_id: original_phantom_id,
            last_snapshot_index,
            baseline_role,
            snapshot_replay_purpose,
            authoritative_replay_failure: Arc::new(AtomicBool::new(false)),
            resource_limit_entry,
        })
    }
}
