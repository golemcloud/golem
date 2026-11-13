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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) async fn ensure_not_failed<T: HasAll<Ctx> + Send + Sync>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        status: &AgentStatusRecord,
    ) -> Result<(), WorkerExecutorError> {
        match &status.status {
            AgentStatus::Failed => {
                let error_and_retry_count =
                    Ctx::get_last_error_and_retry_count(deps, owned_agent_id, agent_mode, status)
                        .await;
                if let Some(last_error) = error_and_retry_count {
                    if status.last_error_kind == Some(OplogErrorKind::Recovery) {
                        Err(WorkerExecutorError::failed_to_resume_worker(
                            owned_agent_id.agent_id.clone(),
                            WorkerExecutorError::runtime(
                                last_error.error.to_string(&last_error.stderr),
                            ),
                        ))
                    } else {
                        Err(WorkerExecutorError::PreviousInvocationFailed {
                            error: last_error.error,
                            stderr: last_error.stderr,
                        })
                    }
                } else {
                    Err(WorkerExecutorError::runtime(
                        "Previous invocation failed, but failed to get error details",
                    ))
                }
            }
            AgentStatus::Exited => Err(WorkerExecutorError::PreviousInvocationExited),
            _ => Ok(()),
        }
    }
    pub async fn find_durable_stream_worker<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<
        Option<(Arc<Self>, Option<Arc<EphemeralResponseLease>>)>,
        DurableStreamRemoteError<WorkerExecutorError>,
    >
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        loop {
            let worker = match Self::get_exact_existing_suspended(
                deps,
                owned_agent_id,
                Principal::anonymous(),
            )
            .await
            {
                Ok(worker) => worker,
                Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if worker.agent_mode() != AgentMode::Ephemeral {
                return Ok(Some((worker, None)));
            }
            match worker
                .durable_stream_producer
                .retain_response_or_wait_for_archive()
                .await
                .map_err(|error| {
                    DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
                })? {
                Some(lease) => return Ok(Some((worker, Some(lease)))),
                None => continue,
            }
        }
    }
    /// Gets or creates a worker, but does not start it
    pub async fn get_or_create_suspended<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        Self::get_or_create_suspended_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            InvocationFreshnessDisposition::MayExist,
            CreateIntent::UNKEYED_INVOCATION,
        )
        .await
    }
    pub async fn get_or_create_suspended_with_freshness<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
        create_intent: CreateIntent,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        deps.active_agents()
            .get_or_add_with_freshness(
                deps,
                owned_agent_id,
                worker_env,
                worker_agent_config,
                component_revision,
                parent,
                invocation_context_stack,
                principal,
                freshness_disposition,
                create_intent,
            )
            .await
    }
    /// Resolves an already-created owner. Unlike ordinary invocation ingress, this path cannot
    /// create or recreate an owner and is therefore suitable for authority carrying an expected
    /// owner fingerprint.
    pub async fn get_exact_existing_suspended<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        deps.active_agents()
            .get_existing(deps, owned_agent_id, principal)
            .await
    }
    /// Pins an existing owner's response metadata without allowing owner creation.
    pub(crate) async fn get_exact_existing_suspended_for_response<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
    ) -> Result<(Arc<Self>, Option<Arc<EphemeralResponseLease>>), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        loop {
            let worker =
                Self::get_exact_existing_suspended(deps, owned_agent_id, principal.clone()).await?;
            if worker.agent_mode() != AgentMode::Ephemeral {
                return Ok((worker, None));
            }
            if let Some(lease) = worker
                .durable_stream_producer
                .retain_response_or_wait_for_archive()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            {
                return Ok((worker, Some(lease)));
            }
        }
    }
    /// Pins response metadata before normal ephemeral archival, or joins archival and reloads.
    pub(crate) async fn get_or_create_suspended_for_response<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        mut freshness_disposition: InvocationFreshnessDisposition,
        create_intent: CreateIntent,
    ) -> Result<(Arc<Self>, Option<Arc<EphemeralResponseLease>>), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        loop {
            let worker = Self::get_or_create_suspended_with_freshness(
                deps,
                owned_agent_id,
                worker_env.clone(),
                worker_agent_config.clone(),
                component_revision,
                parent.clone(),
                invocation_context_stack,
                principal.clone(),
                freshness_disposition,
                create_intent.clone(),
            )
            .await?;
            if worker.agent_mode() != AgentMode::Ephemeral {
                return Ok((worker, None));
            }
            if let Some(lease) = worker
                .durable_stream_producer
                .retain_response_or_wait_for_archive()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            {
                return Ok((worker, Some(lease)));
            }
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }
    }
    /// Gets or creates a worker and makes sure it is running
    pub async fn get_or_create_running<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::get_or_create_running_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            InvocationFreshnessDisposition::MayExist,
        )
        .await
    }
    /// Loads and starts only the incarnation selected by assignment recovery.
    pub async fn get_existing_running_with_fingerprint<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: AgentFingerprint,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = deps
            .active_agents()
            .get_existing_with_fingerprint(
                deps,
                owned_agent_id,
                Principal::anonymous(),
                Some(expected_fingerprint),
            )
            .await?;

        let lifecycle = deps
            .oplog_service()
            .lock_lifecycle(&owned_agent_id.agent_id)
            .await;
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        let current = deps
            .worker_service()
            .resolve_agent_identity(owned_agent_id)
            .await?;
        if worker.get_initial_worker_metadata().fingerprint != expected_fingerprint
            || current.is_none_or(|identity| identity.fingerprint != expected_fingerprint)
        {
            return Err(WorkerExecutorError::AgentNotFound {
                agent_id: owned_agent_id.agent_id.clone(),
            });
        }

        Self::start_if_needed(worker.clone()).await?;
        drop(lifecycle);
        Ok(worker)
    }
    pub async fn get_or_create_running_with_freshness<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = Self::get_or_create_suspended_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            freshness_disposition,
            CreateIntent::UNKEYED_INVOCATION,
        )
        .await?;
        Self::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }
    pub async fn validate_invocation_freshness<T: HasComponentService + HasWorkerService + Sync>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: &IdempotencyKey,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<(), WorkerExecutorError> {
        if freshness_disposition == InvocationFreshnessDisposition::MayExist
            && let Some(metadata) = deps.worker_service().get(owned_agent_id).await?
        {
            let metadata = metadata.initial_worker_metadata;
            if metadata.agent_type_name.is_none() {
                return Ok(());
            }
            if metadata.agent_mode == AgentMode::Ephemeral {
                crate::metrics::ephemeral::record_invocation_attempt(freshness_disposition);
            }
            return validate_resolved_invocation_identity(
                metadata.agent_mode,
                metadata.original_phantom_id,
                idempotency_key,
                freshness_disposition,
            );
        }
        let component = deps
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await?;
        let parsed_agent_id =
            match ParsedAgentKey::parse(&owned_agent_id.agent_id.agent_id, &component.metadata) {
                Ok(parsed_agent_id) => parsed_agent_id,
                Err(_) if freshness_disposition == InvocationFreshnessDisposition::MayExist => {
                    return Ok(());
                }
                Err(err) => {
                    crate::metrics::ephemeral::record_known_fresh_validation_failure();
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "KnownFresh requires a valid ephemeral agent id: {err}"
                    )));
                }
            };
        let Some(agent_type) = component
            .metadata
            .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
        else {
            if freshness_disposition == InvocationFreshnessDisposition::MayExist {
                return Ok(());
            }
            crate::metrics::ephemeral::record_known_fresh_validation_failure();
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh can only be used for an ephemeral agent invocation",
            ));
        };

        let agent_mode = agent_type.mode;

        if agent_mode == AgentMode::Ephemeral {
            crate::metrics::ephemeral::record_invocation_attempt(freshness_disposition);
        }
        let result = validate_resolved_invocation_identity(
            agent_mode,
            parsed_agent_id.phantom_id,
            idempotency_key,
            freshness_disposition,
        );
        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh && result.is_err() {
            crate::metrics::ephemeral::record_known_fresh_validation_failure();
        }
        result
    }
    pub async fn get_latest_metadata<T: HasAll<Ctx>>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<AgentMetadata>, WorkerExecutorError> {
        let _retired_lifecycle =
            if let Some(worker) = deps.active_agents().try_get(owned_agent_id).await {
                if let Some(status) = worker.state_actor.observe_repaired_status().await? {
                    let mut metadata = worker.get_initial_worker_metadata();
                    metadata.last_known_status = status.as_ref().clone();
                    return Ok(Some(metadata));
                }

                // A failed deletion can leave a cached worker whose actors have already stopped.
                // Serialize with retirement/removal before reconstructing from persisted history.
                let lifecycle = deps
                    .oplog_service()
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await;
                let oplog = worker.oplog();
                if !oplog.is_retired() {
                    return Err(WorkerExecutorError::runtime(
                        "Worker status actor is unavailable",
                    ));
                }
                // Close errors are retained by the deletion attempt. Completion still proves that
                // the writer has drained; metadata reconstruction reports its own storage errors.
                let _ = oplog.closed().await;
                Some(lifecycle)
            } else {
                None
            };
        if let Some(GetWorkerMetadataResult {
            mut initial_worker_metadata,
            last_known_status,
        }) = deps.worker_service().get(owned_agent_id).await?
        {
            // update with latest data from oplog
            let agent_mode = initial_worker_metadata.agent_mode;
            // `Ok(None)` means the oplog is gone - a delete raced this read - and the agent is
            // reported as absent. A status that cannot be *recomputed* is a different thing and
            // is propagated: every caller treats absence as "not here", and reporting a storage
            // outage that way turns it into a not-found, or into validation against the deployed
            // component revision for an agent that is pinned to an older one.
            let Some(last_known_status) = calculate_last_known_status_with_checkpoint(
                deps,
                owned_agent_id,
                initial_worker_metadata.fingerprint,
                agent_mode,
                last_known_status,
            )
            .await
            .map_err(WorkerExecutorError::from)?
            else {
                return Ok(None);
            };

            initial_worker_metadata.last_known_status = last_known_status;

            Ok(Some(initial_worker_metadata))
        } else {
            Ok(None)
        }
    }
    pub(crate) fn unresolved<T: HasAll<Ctx>>(
        deps: &T,
        card_interest_index: Arc<CardInterestIndex>,
        owned_agent_id: OwnedAgentId,
    ) -> Arc<Self> {
        Arc::new(Self {
            owned_agent_id,
            deps: All::from_other(deps),
            card_interest_index,
            instance: Arc::new(Mutex::new(WorkerLifecycle::unresolved())),
            unload_cleanup: StdMutex::new(None),
            resolved: OnceCell::new(),
            unpublished_create: AtomicBool::new(false),
        })
    }
    /// Joins final cleanup after the caller has fenced and stopped the runtime. Unlike the
    /// bounded unload notification, success proves the owned filesystem deletion was verified.
    /// Must be called without lifecycle, cache, or metadata locks held.
    pub async fn wait_for_unload_cleanup(&self) -> Result<(), WorkerExecutorError> {
        let instance = self.instance.lock().await;
        let cleanup = self.unload_cleanup.lock().unwrap().clone();
        let missing_cleanup_error = if instance.runtime_active() {
            Some(WorkerExecutorError::runtime(
                "worker runtime has not stopped before cleanup wait",
            ))
        } else {
            instance
                .cleanup_failure()
                .filter(|_| cleanup.is_none())
                .cloned()
        };
        drop(instance);
        if let Some(error) = missing_cleanup_error {
            return Err(error);
        }
        match cleanup {
            Some(cleanup) => cleanup.wait().await,
            None => Ok(()),
        }
    }
    pub(crate) async fn ensure_created(
        self: &Arc<Self>,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
        creation_mode: WorkerCreationMode,
        create_intent: CreateIntent,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let worker = self.clone();
            let create_intent = create_intent.clone();
            let worker_env = worker_env.clone();
            let worker_agent_config = worker_agent_config.clone();
            let parent = parent.clone();
            let invocation_context_stack = invocation_context_stack.clone();
            let principal = principal.clone();
            let build = async move {
                let start = std::time::Instant::now();
                let mut lifecycle = worker
                    .deps
                    .oplog_service()
                    .lock_lifecycle(&worker.owned_agent_id.agent_id)
                    .await;
                let freshness_disposition = if worker.unpublished_create.load(Ordering::Acquire) {
                    // A failed attempt committed Create. Even a KnownFresh request must reload
                    // that incarnation rather than create another one.
                    InvocationFreshnessDisposition::MayExist
                } else {
                    freshness_disposition
                };
                let metadata = Self::get_or_create_worker_metadata(
                    &worker.deps,
                    &mut lifecycle,
                    &worker.owned_agent_id,
                    component_revision,
                    worker_env,
                    worker_agent_config,
                    parent,
                    &principal,
                    &invocation_context_stack,
                    freshness_disposition,
                    creation_mode,
                    &create_intent,
                )
                .await;
                worker.finish_construction(start, metadata).await
            }
            .boxed();
            let (started, result) = self.initialize_with(build).await;
            match result {
                Err(WorkerExecutorError::AgentNotFound { .. }) if !started => continue,
                result => {
                    result?;
                    if self.initial_worker_metadata.owner_kind != creation_mode.owner_kind() {
                        return Err(WorkerExecutorError::invalid_request(
                            "execution owner kind does not match the requested creation mode",
                        ));
                    }
                    return Ok(());
                }
            }
        }
    }
    pub(crate) async fn ensure_existing_with_fingerprint(
        self: &Arc<Self>,
        _principal: Principal,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> Result<(), WorkerExecutorError> {
        // Existing-only acquisition resolves identity, not execution readiness. Deletion
        // must still reach the retained cleanup owner after a failed unload.
        if self.is_resolved() {
            return Ok(());
        }
        let worker = self.clone();
        let build = async move {
            let start = std::time::Instant::now();
            let mut lifecycle = worker
                .deps
                .oplog_service()
                .lock_lifecycle(&worker.owned_agent_id.agent_id)
                .await;
            // The epoch is read first, as in `get_or_create_worker_metadata`: a worker whose shard
            // has already left the assignment is refused without touching storage, and the oplog
            // opened below asserts the epoch this executor holds.
            let metadata = match owned_shard_epoch(&worker.deps, &worker.owned_agent_id.agent_id) {
                Ok(shard_epoch) => {
                    Self::get_existing_worker_metadata(
                        &worker.deps,
                        &mut lifecycle,
                        &worker.owned_agent_id,
                        expected_fingerprint,
                        shard_epoch,
                    )
                    .await
                }
                Err(error) => Err(error),
            }
            .and_then(|metadata| {
                metadata.ok_or_else(|| {
                    WorkerExecutorError::worker_not_found(worker.owned_agent_id.agent_id())
                })
            });
            worker.finish_construction(start, metadata).await
        }
        .boxed();
        self.initialize_with(build).await.1
    }
    pub(in crate::worker) async fn initialize_with(
        self: &Arc<Self>,
        build: BoxFuture<'static, Result<(ResolvedWorkerData<Ctx>, bool), WorkerExecutorError>>,
    ) -> (bool, Result<(), WorkerExecutorError>) {
        let (started, completion) = {
            let mut instance = self.instance.lock().await;
            match instance.transition() {
                LifecycleTransition::Unresolved(unresolved) => {
                    let (sender, receiver) = oneshot::channel();
                    let completion = async move {
                        receiver.await.unwrap_or_else(|_| {
                            Err(WorkerExecutorError::runtime(
                                "Worker initialization task ended without a result",
                            ))
                        })
                    }
                    .boxed()
                    .shared();
                    let initializing = unresolved.begin_initialization(completion.clone());
                    let worker = self.clone();
                    let invocation_loops = self.active_agents().invocation_loops();
                    let construction = async move {
                        let result = crate::worker::invocation::with_invocation_stack(build).await;
                        let mut instance = worker.instance.lock().await;
                        let published = match instance.transition() {
                            LifecycleTransition::Initializing(initializing) => match result {
                                Ok((data, reconstructed_ephemeral)) => {
                                    worker.unpublished_create.store(false, Ordering::Release);
                                    if reconstructed_ephemeral {
                                        initializing
                                            .complete_inactive(inactive_ephemeral_agent_error());
                                    } else {
                                        initializing.complete_initialization();
                                    }
                                    // Publish immutable data last: cache readers use this cell as
                                    // their lock-free initialization-completion boundary.
                                    worker.resolved.set(data).map_err(|_| {
                                        WorkerExecutorError::runtime("Worker initialized twice")
                                    })
                                }
                                Err(error) => {
                                    initializing.construction_failed();
                                    Err(error)
                                }
                            },
                            _ => Err(WorkerExecutorError::runtime(
                                "Worker initialization no longer owns its lifecycle phase",
                            )),
                        };
                        if published.is_ok() {
                            Self::start_durable_stream_attachment_reconciler(&worker);
                        }
                        drop(instance);
                        let resolved = published.is_ok();
                        let _ = sender.send(published);
                        if resolved {
                            worker.retire_if_shard_left_during_construction().await;
                            if !worker.owner_retirement_requested.is_cancelled() {
                                worker.register_pending_invocation_wakes().await;
                            }
                        }
                    };
                    if !invocation_loops.spawn_construction(construction) {
                        initializing.construction_failed();
                        return (
                            false,
                            Err(WorkerExecutorError::runtime(
                                "Worker initialization rejected because its executor is stopping",
                            )),
                        );
                    }
                    (true, completion)
                }
                LifecycleTransition::Initializing(initializing) => {
                    (false, initializing.completion())
                }
                LifecycleTransition::CleanupFailed(error) => return (false, Err(error.clone())),
                _ if self.resolved.get().is_some() => return (false, Ok(())),
                _ => {
                    return (
                        false,
                        Err(WorkerExecutorError::runtime(
                            "Worker initialization entered an invalid lifecycle state",
                        )),
                    );
                }
            }
        };
        (started, completion.await)
    }
    /// The sweep a delivery runs selects from the resolved agents, so one still being built when
    /// the shard left is not in it: it read the assignment before the revoke, opened its oplog at
    /// the epoch it was granted, and would stay here, unfenced, until the new owner claims the
    /// oplog. This is the sweep's late half for that agent, run once it is published - after the
    /// instance guard is released, because retiring takes it.
    pub(in crate::worker) async fn retire_if_shard_left_during_construction(self: &Arc<Self>) {
        let assignment = self.shard_service().try_get_current_assignment();
        if retired_by_assignment(
            assignment.as_ref(),
            &self.owned_agent_id.agent_id,
            self.oplog().shard_epoch(),
        ) {
            info!("The agent's shard left this executor while the agent was being created");
            let _ = self
                .interrupt_and_retire(InterruptKind::ShardLost, RetirementReason::ShardNotAssigned)
                .await;
        }
    }
    pub(in crate::worker) async fn finish_construction(
        self: &Arc<Self>,
        start: std::time::Instant,
        metadata: Result<GetOrCreateWorkerResult, WorkerExecutorError>,
    ) -> Result<(ResolvedWorkerData<Ctx>, bool), WorkerExecutorError> {
        let mut metadata = match metadata {
            Ok(result) => result,
            Err(err) => {
                crate::metrics::wasm::record_create_worker_failure(&err);
                return Err(err);
            }
        };
        metadata.oplog = Ctx::wrap_oplog(
            self.owned_agent_id.clone(),
            metadata.oplog,
            self.deps.extra_deps(),
        );
        let construction_oplog = metadata.oplog.clone();
        let (worker, reconstructed_ephemeral) = match self.prepare_worker_data(metadata).await {
            Ok(prepared) => prepared,
            Err(error) => {
                // The caller retains the cold lifecycle guard until all attempt-owned actors,
                // transfers and monitors have exited. Persisted data belongs to the next retry.
                let error = match construction_oplog.stop_and_wait().await {
                    Ok(()) => error,
                    Err(cleanup_error) => WorkerExecutorError::runtime(format!(
                        "{error}; failed to stop construction work: {cleanup_error}"
                    )),
                };
                crate::metrics::wasm::record_create_worker_failure(&error);
                return Err(error);
            }
        };
        crate::metrics::wasm::record_create_worker(start.elapsed());

        Ok((worker, reconstructed_ephemeral))
    }
    pub(in crate::worker) async fn prepare_worker_data(
        self: &Arc<Self>,
        metadata: GetOrCreateWorkerResult,
    ) -> Result<(ResolvedWorkerData<Ctx>, bool), WorkerExecutorError> {
        let deps = &self.deps;
        let owned_agent_id = self.owned_agent_id.clone();
        let GetOrCreateWorkerResult {
            initial_worker_metadata,
            current_status,
            mut persisted_status,
            execution_status,
            owner_context,
            snapshot_policy,
            oplog,
            initial_component,
            reconstructed_ephemeral,
            newly_created,
        } = metadata;
        // An unpublished local creation has never admitted a guest invocation. Retrying that
        // same creation is not reconstruction of a previously running ephemeral agent.
        let reconstructed_ephemeral =
            reconstructed_ephemeral && !self.unpublished_create.load(Ordering::Acquire);
        // A reconstructed ephemeral agent that never began executing may still run the work it
        // accepted; nothing of it has run yet, so this is its first execution, not a restart.
        let never_started_admission = reconstructed_ephemeral
            .then(|| {
                never_started_ephemeral_admission(
                    &current_status.load(),
                    (initial_worker_metadata.owner_kind != OwnerKind::EphemeralExternalTool)
                        .then(|| self.initialization_idempotency_key())
                        .as_ref(),
                )
            })
            .flatten();
        let reconstructed_ephemeral = reconstructed_ephemeral && never_started_admission.is_none();
        if newly_created {
            self.unpublished_create.store(true, Ordering::Release);
            let initial_status = current_status.load_full().as_ref().clone();
            deps.worker_service()
                .update_cached_status(
                    &owned_agent_id,
                    initial_worker_metadata.fingerprint,
                    None,
                    initial_status.clone(),
                )
                .await?;
            persisted_status = Some(initial_status);
        } else {
            let reconstructed_status = current_status.load_full();
            deps.worker_service()
                .set_assignment_tracking(
                    &owned_agent_id,
                    initial_worker_metadata.fingerprint,
                    reconstructed_status.as_ref(),
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }

        let current_status_snapshot = current_status.load_full();
        let metrics_status = Arc::new(WorkerStatusMetric::new(current_status_snapshot.status));
        let last_oplog_idx = current_status_snapshot.oplog_idx;
        drop(current_status_snapshot);

        // Invocations already pending when this worker is loaded were enqueued by
        // an earlier request, possibly in an earlier process, so there is no
        // in-process originator to relate them to. They start with no origin rather
        // than being attributed to whichever request happened to trigger the load,
        // which is neither their true origin nor a span that outlives them.
        let queue = Arc::new(RwLock::new(VecDeque::new()));
        let external_invocation_origins = Arc::new(RwLock::new(HashMap::new()));

        let hydrated_invocation_results =
            Arc::new(RwLock::new(HydratedInvocationResultCache::new(
                deps.config().invocation_results.hydrated_cache_capacity,
            )));

        let instance = self.instance.clone();

        // Fetch the account's resource entry and register it with the
        // concurrent-agents semaphore. This must happen before WaitingWorker
        // can acquire a concurrent-agent permit so that the real plan limit
        // is enforced from the very first agent startup for this account.
        // Registration is idempotent — subsequent calls for the same account
        // on the same executor are instant (OnceCell cache hit in ResourceLimitsGrpc).
        let owner_account_id = initial_worker_metadata.created_by;
        let resource_entry = deps
            .resource_limits()
            .initialize_account(owner_account_id)
            .await?;
        let registered_concurrent_account = deps
            .active_agents()
            .register_account_concurrency(owner_account_id, resource_entry.clone())
            .await;

        let read_only_cache_cfg = &deps.config().read_only_cache;
        let read_only_cache = golem_common::cache::Cache::new(
            Some(read_only_cache_cfg.cache_capacity),
            golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
            golem_common::cache::BackgroundEvictionMode::OlderThan {
                ttl: read_only_cache_cfg.max_entry_age,
                period: read_only_cache_cfg.cache_eviction_interval,
            },
            "worker_read_only_cache",
        );

        let current_component = Arc::new(arc_swap::ArcSwap::from(initial_component.clone()));

        let last_known_status_detached = Arc::new(AtomicBool::new(false));
        let lost_shard = tokio::sync::watch::Sender::new(None);
        let status_flusher = status_flusher::AgentStatusFlusher::new(
            owned_agent_id.clone(),
            initial_worker_metadata.fingerprint,
            initial_worker_metadata.agent_mode == AgentMode::Ephemeral,
            deps.config().agent_status_flush.enabled,
            deps.worker_service(),
            deps.active_agents().status_flush_queue(),
            persisted_status,
            current_status.clone(),
            last_known_status_detached.clone(),
            lost_shard.subscribe(),
        );

        let status_checkpointer = status_checkpointer::StatusCheckpointer::new(
            owned_agent_id.clone(),
            initial_worker_metadata.fingerprint,
            initial_worker_metadata.agent_mode == AgentMode::Ephemeral,
            deps.config().agent_status_checkpoint.enabled,
            deps.config().agent_status_checkpoint.min_oplog_delta,
            deps.worker_service(),
            lost_shard.subscribe(),
        );

        let all_deps = All::from_other(deps);
        // Start stale so a newly restored store must reconcile status/oplog
        // authority before it can use lock-free host-call authorization.
        let published_authority_generation = Arc::new(AtomicU64::new(1));

        let state_actor = Arc::new(state_actor::WorkerStateActor::new(
            all_deps.clone(),
            owned_agent_id.clone(),
            initial_worker_metadata.fingerprint,
            initial_worker_metadata.agent_mode,
            initial_worker_metadata.created_by,
            oplog.clone(),
            current_status.clone(),
            last_known_status_detached.clone(),
            metrics_status.clone(),
            status_flusher.clone(),
            published_authority_generation.clone(),
            instance.clone(),
        ));
        let card_event_boundary_lock = Arc::new(Mutex::new(()));
        let runtime_append_tasks = Arc::new(StdMutex::new(Arc::default()));
        let owner_execution = Arc::new(OwnerExecution::new(
            owned_agent_id.clone(),
            oplog.clone(),
            state_actor.owner_commit_controller(),
            card_event_boundary_lock.clone(),
            runtime_append_tasks.clone(),
            self.card_interest_index.clone(),
        ));
        let owner_runtime_resources = Arc::new(OwnerRuntimeResources::new(
            Arc::clone(&resource_entry),
            execution_status.clone(),
        ));
        deps.active_agents()
            .invocation_loops()
            .register_owner_oplog(oplog.clone());

        let worker = ResolvedWorkerData {
            parsed_agent_id: owner_context.agent().cloned(),
            owner_context,
            tasks: oplog
                .task_owner()
                .cloned()
                .unwrap_or_default()
                .for_worker(self.clone()),
            oplog,
            worker_event_service: Arc::new(WorkerEventServiceDefault::new(
                deps.config().limits.event_broadcast_capacity,
                deps.config().limits.event_history_size,
            )),
            queue,
            external_invocation_origins,
            hydrated_invocation_results,
            ephemeral_invocation: StdMutex::new(if reconstructed_ephemeral {
                EphemeralInvocationState::Accepted(None)
            } else {
                match never_started_admission {
                    Some(Some(key)) => EphemeralInvocationState::Accepted(Some(key)),
                    _ => EphemeralInvocationState::Available,
                }
            }),
            cache_retirement_in_progress: AtomicBool::new(false),
            startup_attempt: StartupAttemptTracker::default(),
            linear_memory_grant: StdMutex::new(None),
            interrupt_signal: Arc::default(),
            interrupt_establishment_tail: StdMutex::new(InterruptEstablishment::ready()),
            release_scope: StdMutex::default(),
            monthly_window: StdMutex::new(None),
            monthly_stop: StdMutex::new(None),
            runtime_append_tasks,
            infrastructure_recovery_retry_config: deps.config().retry.clone(),
            infrastructure_recovery_attempt: AtomicU32::new(0),
            execution_status,
            initial_worker_metadata,
            resource_entry,
            registered_concurrent_account,
            last_known_status: current_status,
            metrics_status,
            card_event_boundary_lock,
            stream_session_creation_lock: Arc::new(Mutex::new(())),
            published_authority_generation,
            oom_retry_config: deps.config().memory.oom_retry_config.clone(),
            snapshot_policy,
            state_actor,
            owner_execution,
            owner_runtime_resources,
            last_known_status_detached,
            status_flusher,
            status_checkpointer,
            last_resume_request: Mutex::new(Timestamp::now_utc()),
            snapshot_exclusions: StdMutex::new(SnapshotExclusions::default()),
            startup_linear_memory_bytes: AtomicU64::new(0),
            memory_growth: StdMutex::new(Arc::new(PendingMemoryGrowth::default())),
            memory_limit_interrupt_queued: AtomicBool::new(false),
            current_component,
            read_only_cache,
            read_only_cache_epoch: Arc::new(AtomicU64::new(0)),
            durable_stream_producer: Arc::default(),
            export_fork_receipt: tokio::sync::OnceCell::new(),
            owner_retirement: Arc::new(std::sync::OnceLock::new()),
            owner_cleanup: Mutex::new(OwnerCleanupState::PreRemoval),
            owner_retirement_requested: CancellationToken::new(),
            lost_shard,
            durable_stream_attachment_reconciler: DurableStreamAttachmentReconciler::new(
                deps.shutdown_token(),
            ),
            durable_topology_recovery: Arc::new(
                Mutex::new(DurableTopologyRecoveryCache::default()),
            ),
        };
        // The provisional data is private until every local fallible step has completed.
        if let Some(forwarding_oplog) = downcast_oplog::<ForwardingOplog>(&worker.oplog) {
            forwarding_oplog
                .set_worker_event_service(worker.worker_event_service.clone())
                .await;
        }
        assert!(last_oplog_idx >= OplogIndex::INITIAL);

        // A crash may leave only Create. The initialization runs with the identity of the request
        // that created the agent, whichever request loads it now. The initialization reservation
        // excludes ordinary invocation admission; the status actor still performs the persisted
        // dedupe.
        if let Some(agent_id) = worker.owner_context.agent()
            && last_oplog_idx <= OplogIndex::from_u64(2)
            && !reconstructed_ephemeral
        {
            let create_request = &worker.initial_worker_metadata.create_request;
            let invocation_context = create_request.invocation_context();
            let principal = create_request.principal.clone();
            let idempotency_key = self.initialization_idempotency_key();
            let (_, entry) = self
                .pending_invocation_entry(
                    &worker.oplog,
                    AgentInvocation::AgentInitialization {
                        idempotency_key: idempotency_key.clone(),
                        input: agent_id.parameters.value().clone(),
                        invocation_context,
                        principal,
                    },
                    None,
                )
                .await?;
            let status = worker.state_actor.status().await;
            // Returned rather than ignored: an agent created at an epoch another executor has
            // already claimed is refused here with `OplogFenced`, which its caller retries on the
            // shard's owner. Nothing is lost by returning early, since the next load repeats this
            // check against the same short oplog.
            worker
                .state_actor
                .append_invocation_if_version(
                    entry,
                    idempotency_key,
                    status.invocation_results.change_generation(),
                    status.invocation_results.revert_generation(),
                    self.instance.clone().lock_owned().await,
                )
                .await?;
        }
        if worker.last_known_status.load().has_durable_stream_history
            && !self
                .durable_stream_producer_for(&worker)
                .await?
                .deletion_started()
                .await
        {
            let _ = self
                .reconcile_durable_stream_attachments_for(&worker)
                .await?;
        }
        Ok((worker, reconstructed_ephemeral))
    }
    /// `shard_epoch` is the epoch the opened oplog asserts, read by the caller with
    /// [`owned_shard_epoch`] before anything touches storage. See
    /// [`Self::get_or_create_worker_metadata`].
    pub(crate) async fn get_existing_worker_metadata<
        T: HasWorkerService + HasComponentService + HasOplogService + HasConfig + Sync,
    >(
        this: &T,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Option<GetOrCreateWorkerResult>, WorkerExecutorError> {
        let Some(metadata) = this.worker_service().get(owned_agent_id).await? else {
            return Ok(None);
        };
        if expected_fingerprint
            .is_some_and(|expected| metadata.initial_worker_metadata.fingerprint != expected)
        {
            return Ok(None);
        }
        Self::hydrate_existing_worker_metadata(
            this,
            lifecycle,
            owned_agent_id,
            metadata,
            shard_epoch,
        )
        .await
        .map(Some)
    }
    pub(in crate::worker) async fn hydrate_existing_worker_metadata<
        T: HasWorkerService + HasComponentService + HasOplogService + HasConfig + Sync,
    >(
        this: &T,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        metadata: GetWorkerMetadataResult,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<GetOrCreateWorkerResult, WorkerExecutorError> {
        let GetWorkerMetadataResult {
            initial_worker_metadata,
            last_known_status,
        } = metadata;
        if initial_worker_metadata.owner_kind == OwnerKind::OplogProcessor
            && (initial_worker_metadata.agent_mode != AgentMode::Durable
                || initial_worker_metadata.agent_type_name.is_some()
                || initial_worker_metadata.canonical_key.is_some()
                || initial_worker_metadata.original_phantom_id.is_some())
        {
            return Err(WorkerExecutorError::invalid_request(
                "oplog processor metadata must be durable and have no agent type, key or phantom identity",
            ));
        }
        let component_id = initial_worker_metadata.component_id;
        // This consistency check applies while IDs still encode the component and type.
        debug_assert_eq!(component_id, owned_agent_id.component_id());
        if let Some(agent_type_name) = &initial_worker_metadata.agent_type_name {
            debug_assert_eq!(
                ParsedAgentKey::parse_agent_type_name(&owned_agent_id.agent_id.agent_id).as_ref(),
                Ok(agent_type_name),
            );
        }
        initial_worker_metadata
            .owner_kind
            .validate_instance_name(&owned_agent_id.agent_id.agent_id)
            .map_err(WorkerExecutorError::runtime)?;
        let persisted_status = last_known_status.clone();
        let agent_mode = initial_worker_metadata.agent_mode;
        let current_status = calculate_last_known_status_with_checkpoint(
            this,
            owned_agent_id,
            initial_worker_metadata.fingerprint,
            agent_mode,
            last_known_status,
        )
        .await
        .map_err(WorkerExecutorError::from)?
        .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))?;

        // Reconstruction is pinned to the revision and mode recorded by the Create entry.
        let initial_component = this
            .component_service()
            .get_metadata(
                component_id,
                Some(initial_worker_metadata.last_known_status.component_revision),
            )
            .await?;
        let current_status = Arc::new(arc_swap::ArcSwap::from_pointee(current_status));
        let owner_context = match initial_worker_metadata.owner_kind {
            OwnerKind::ComponentAgent => {
                let agent = initial_worker_metadata
                    .parse_agent_id(&initial_component.metadata)
                    .map_err(WorkerExecutorError::invalid_request)?;
                ResolvedOwnerContext::Agent(Box::new(agent))
            }
            kind => ResolvedOwnerContext::from_authoritative_kind(
                kind,
                &owned_agent_id.agent_id.agent_id,
                &initial_component.metadata,
            )
            .map_err(WorkerExecutorError::invalid_request)?,
        };
        if matches!(owner_context, ResolvedOwnerContext::ComponentBaseline)
            && initial_worker_metadata.agent_mode == AgentMode::Durable
        {
            return Err(WorkerExecutorError::invalid_request(
                "An external tool owner cannot use durable mode",
            ));
        }
        let ResolvedAgentProperties {
            snapshot_policy, ..
        } = resolve_agent_properties(this, &owner_context, &initial_component.metadata);
        let execution_status = Arc::new(std::sync::RwLock::new(ExecutionStatus::Suspended {
            agent_mode,
            timestamp: Timestamp::now_utc(),
        }));
        let oplog = this
            .oplog_service()
            .open(
                lifecycle,
                owned_agent_id,
                agent_mode,
                None,
                initial_worker_metadata.clone(),
                read_only_lock::arc_swap::ReadOnlyView::new(current_status.clone()),
                read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                shard_epoch,
            )
            .await?;

        Ok(GetOrCreateWorkerResult {
            initial_worker_metadata,
            current_status,
            persisted_status,
            execution_status,
            owner_context,
            snapshot_policy,
            oplog,
            initial_component: Arc::new(initial_component),
            reconstructed_ephemeral: agent_mode == AgentMode::Ephemeral,
            newly_created: false,
        })
    }
    pub(in crate::worker) async fn get_or_create_worker_metadata<
        T: HasWorkerService
            + HasComponentService
            + HasConfig
            + HasOplogService
            + HasEnvironmentStateService
            + HasShardService
            + Sync,
    >(
        this: &T,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        component_revision: Option<ComponentRevision>,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        parent: Option<AgentId>,
        principal: &Principal,
        invocation_context: &InvocationContextStack,
        freshness_disposition: InvocationFreshnessDisposition,
        creation_mode: WorkerCreationMode,
        create_intent: &CreateIntent,
    ) -> Result<GetOrCreateWorkerResult, WorkerExecutorError> {
        // Captured once, here, and cached for the life of the oplog. One live oplog is one
        // ownership generation. Re-reading the epoch per write would only let a losing executor
        // talk itself back into ownership. A renewal never moves an epoch. A delivery that raises
        // the epoch of a shard this executor kept means the shard left and came back: the
        // assignment sweep gives the agent up and the open-oplog cache declines the old handle, so
        // the next open claims the new epoch. Read before anything else, so a worker whose shard
        // has already left the assignment is refused without touching storage.
        let shard_epoch = owned_shard_epoch(this, &owned_agent_id.agent_id)?;
        let component_id = owned_agent_id.component_id();

        let owner_kind = creation_mode.owner_kind();
        owner_kind
            .validate_instance_name(&owned_agent_id.agent_id.agent_id)
            .map_err(WorkerExecutorError::invalid_request)?;

        // KnownFresh has already been validated against the ephemeral agent type, phantom ID, and
        // idempotency key at invocation ingress. All other paths retain the checked lookup.
        let existing = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            None
        } else {
            Self::get_existing_worker_metadata(this, lifecycle, owned_agent_id, None, shard_epoch)
                .await?
        };

        match existing {
            Some(existing) => {
                if existing.initial_worker_metadata.owner_kind != owner_kind {
                    return Err(WorkerExecutorError::invalid_request(
                        "execution owner kind does not match the requested creation mode",
                    ));
                }
                Ok(existing)
            }
            None => {
                // Create and initialize a new worker.
                let component = this
                    .component_service()
                    .get_metadata(component_id, component_revision)
                    .await?;

                let owner_context = ResolvedOwnerContext::from_authoritative_kind(
                    owner_kind,
                    &owned_agent_id.agent_id.agent_id,
                    &component.metadata,
                )
                .map_err(WorkerExecutorError::invalid_request)?;
                if owner_kind != OwnerKind::ComponentAgent
                    && component.environment_id != owned_agent_id.environment_id
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "owner environment does not match the component environment",
                    ));
                }
                let agent_id = owner_context.agent().cloned();

                let ResolvedAgentProperties {
                    agent_mode,
                    snapshot_policy,
                } = resolve_agent_properties(this, &owner_context, &component.metadata);

                let execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };

                {
                    // The actual checks are performed in the DurableWorkerCtx on secret access.
                    // This is just to fail early with a nicer error.
                    let agent_secrets = this
                        .environment_state_service()
                        .get_agent_secrets(component.environment_id)
                        .await?;
                    ensure_required_agent_secrets_are_configured(
                        &agent_secrets,
                        agent_id.as_ref(),
                        &component,
                    )?
                };

                let initial_agent_config = parse_worker_creation_agent_config(
                    worker_agent_config,
                    agent_id.as_ref(),
                    &component,
                )?;
                // Store only the per-worker env overrides. Agent-type defaults are applied
                // at runtime in get_environment
                let worker_env: Vec<(String, String)> = worker_env.unwrap_or_default();
                let created_at = Timestamp::now_utc();
                let instance_id = Uuid::now_v7();

                // Note: Keep this in sync with the logic in crate::services::worker::WorkerService::get
                let initial_status = AgentStatusRecord {
                    component_revision: component.revision,
                    component_revision_for_replay: component.revision,
                    component_size: component.component_size,
                    total_linear_memory_size: component.metadata.initial_linear_memory_bytes(),
                    active_plugins: component
                        .metadata
                        .owner_plugins(owner_kind, agent_id.as_ref().map(|agent| &agent.agent_type))
                        .unwrap_or_default()
                        .iter()
                        .map(|i| i.environment_plugin_grant_id)
                        .collect(),
                    invocation_results: this.config().invocation_results.membership(),
                    export_fork_admissions: golem_common::model::ExportForkAdmissions {
                        owner_fingerprint: Some(AgentFingerprint(instance_id)),
                        ..Default::default()
                    },
                    agent_mode,
                    ..Default::default()
                };

                // Use the component's authoritative account_id and environment_id
                // rather than the caller-provided values. During cross-account or
                // cross-environment RPC the caller may pass its own account/environment,
                // but the worker must belong to the component's owning account and
                // environment for correct metric attribution and quota enforcement.

                let initial_worker_metadata = AgentMetadata {
                    agent_id: owned_agent_id.agent_id(),
                    component_id,
                    agent_type_name: agent_id.as_ref().map(|id| id.agent_type.clone()),
                    canonical_key: agent_id
                        .as_ref()
                        .filter(|id| agent_mode == AgentMode::Durable && id.phantom_id.is_none())
                        .map(ToString::to_string),
                    owner_kind,
                    env: worker_env,
                    config: initial_agent_config,
                    environment_id: component.environment_id,
                    created_by: component.account_id,
                    created_by_email: component.account_email.clone(),
                    created_at,
                    parent,
                    last_known_status: initial_status.clone(),
                    original_phantom_id: agent_id.as_ref().and_then(|id| id.phantom_id),
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                    create_request: create_intent.request(
                        agent_id.as_ref().and_then(|id| id.phantom_id),
                        principal.clone(),
                        &invocation_context
                            .clone()
                            .limit_depth(this.config().limits.max_invocation_context_stack_depth),
                    ),
                };

                // Alternatively, we could just write the oplog entry and recompute the initial_worker_metadata from it.
                // both options are equivalent here, this is just cheaper.

                // Strip the schema graph from the typed config entries to get
                // the raw (untyped) form persisted in the Create oplog entry.
                let local_agent_config: Vec<golem_common::model::agent::UntypedAgentConfigEntry> =
                    initial_worker_metadata
                        .config
                        .iter()
                        .cloned()
                        .map(golem_common::model::agent::UntypedAgentConfigEntry::try_from)
                        .collect::<Result<_, _>>()
                        .map_err(|err: String| WorkerExecutorError::runtime(err))?;

                let initial_oplog_entry =
                    OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
                        agent_id: initial_worker_metadata.agent_id.clone(),
                        component_id: initial_worker_metadata.component_id,
                        agent_type_name: initial_worker_metadata.agent_type_name.clone(),
                        canonical_key: initial_worker_metadata.canonical_key.clone(),
                        owner_kind: initial_worker_metadata.owner_kind,
                        agent_mode: initial_worker_metadata.agent_mode,
                        component_revision: initial_worker_metadata
                            .last_known_status
                            .component_revision,
                        env: initial_worker_metadata.env.clone(),
                        environment_id: initial_worker_metadata.environment_id,
                        created_by: initial_worker_metadata.created_by,
                        parent: initial_worker_metadata.parent.clone(),
                        component_size: initial_worker_metadata.last_known_status.component_size,
                        initial_total_linear_memory_size: initial_worker_metadata
                            .last_known_status
                            .total_linear_memory_size,
                        initial_active_plugins: initial_worker_metadata
                            .last_known_status
                            .active_plugins
                            .clone(),
                        local_agent_config,
                        original_phantom_id: initial_worker_metadata.original_phantom_id,
                        instance_id,
                        create_request: initial_worker_metadata.create_request.clone(),
                    }));

                let initial_status = Arc::new(arc_swap::ArcSwap::from_pointee(initial_status));
                let execution_status = Arc::new(std::sync::RwLock::new(execution_status));

                let oplog_service = this.oplog_service();
                let oplog = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                    oplog_service
                        .create_fresh(
                            lifecycle,
                            owned_agent_id,
                            agent_mode,
                            initial_oplog_entry,
                            initial_worker_metadata.clone(),
                            read_only_lock::arc_swap::ReadOnlyView::new(initial_status.clone()),
                            read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                            shard_epoch,
                        )
                        .await
                } else {
                    oplog_service
                        .create(
                            lifecycle,
                            owned_agent_id,
                            agent_mode,
                            initial_oplog_entry,
                            initial_worker_metadata.clone(),
                            read_only_lock::arc_swap::ReadOnlyView::new(initial_status.clone()),
                            read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                            shard_epoch,
                        )
                        .await
                }?;

                {
                    let mut status = initial_status.load_full().as_ref().clone();
                    status.oplog_idx = oplog.current_oplog_index().await;
                    initial_status.store(Arc::new(status));
                }

                Ok(GetOrCreateWorkerResult {
                    initial_worker_metadata,
                    current_status: initial_status,
                    persisted_status: None,
                    execution_status,
                    owner_context,
                    snapshot_policy,
                    oplog,
                    initial_component: Arc::new(component),
                    reconstructed_ephemeral: false,
                    newly_created: true,
                })
            }
        }
    }
}

pub(in crate::worker) fn resolve_agent_properties<T: HasConfig>(
    deps: &T,
    owner_context: &ResolvedOwnerContext,
    metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> ResolvedAgentProperties {
    match owner_context {
        ResolvedOwnerContext::Agent(agent_id) => {
            let agent_type = metadata
                .find_agent_type_by_name_ref(&agent_id.agent_type)
                .expect("Resolved agent type must exist in component metadata");
            let configured_policy = resolve_snapshot_policy(
                &deps.config().oplog.default_snapshotting,
                Some(&agent_type.snapshotting),
            );
            ResolvedAgentProperties {
                agent_mode: agent_type.mode,
                snapshot_policy: snapshot_policy_of_mode(agent_type.mode, configured_policy),
            }
        }
        ResolvedOwnerContext::ComponentBaseline => ResolvedAgentProperties {
            agent_mode: AgentMode::Ephemeral,
            snapshot_policy: SnapshotPolicy::Disabled,
        },
        ResolvedOwnerContext::OplogProcessor => ResolvedAgentProperties {
            agent_mode: AgentMode::Durable,
            snapshot_policy: if metadata.has_save_snapshot() && metadata.has_load_snapshot() {
                deps.config().oplog.oplog_processor_snapshotting.clone()
            } else {
                deps.config().oplog.default_snapshotting.clone()
            },
        },
    }
}

/// The snapshot policy of an agent in `mode` whose configuration gives `configured`. An ephemeral
/// agent takes no snapshot: a start from a snapshot record skips the initialization that the
/// replay of an ephemeral agent needs, and that replay then fails.
pub(in crate::worker) fn snapshot_policy_of_mode(
    mode: AgentMode,
    configured: SnapshotPolicy,
) -> SnapshotPolicy {
    match mode {
        AgentMode::Durable => configured,
        AgentMode::Ephemeral => SnapshotPolicy::Disabled,
    }
}

pub(in crate::worker) fn resolve_snapshot_policy(
    default_config: &SnapshotPolicy,
    agent_snapshotting: Option<&Snapshotting>,
) -> SnapshotPolicy {
    match agent_snapshotting {
        None | Some(Snapshotting::Enabled(SnapshottingConfig::Default(_))) => {
            default_config.clone()
        }
        Some(Snapshotting::Disabled(_)) => SnapshotPolicy::Disabled,
        Some(Snapshotting::Enabled(SnapshottingConfig::Periodic(p))) => {
            let period = Duration::from_nanos(p.duration_nanos);
            if period.is_zero() {
                warn!("Agent snapshot periodic duration is zero, disabling");
                SnapshotPolicy::Disabled
            } else {
                SnapshotPolicy::Periodic { period }
            }
        }
        Some(Snapshotting::Enabled(SnapshottingConfig::EveryNInvocation(n))) => {
            if n.count == 0 {
                warn!("Agent snapshot every-n-invocation count is zero, disabling");
                SnapshotPolicy::Disabled
            } else {
                SnapshotPolicy::EveryNInvocation { count: n.count }
            }
        }
    }
}

pub fn merge_agent_env_with_default_env(
    agent_env: Option<Vec<(String, String)>>,
    default_agent_env: BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut seen_keys = HashSet::new();
    let mut result = Vec::new();

    if let Some(worker_env) = agent_env {
        for (key, value) in worker_env {
            seen_keys.insert(key.clone());
            result.push((key, value));
        }
    }

    for (key, value) in default_agent_env {
        // Prioritise per worker environment variables all the time
        if !seen_keys.contains(&key) {
            result.push((key, value));
        }
    }

    result
}

/// Restores first-execution admission for an ephemeral target created before its scheduled work
/// is due. Reconstructing that target must not consume its first execution or admit a different
/// method in place of its accepted work. An execution-begun marker or terminal status keeps the
/// target observation-only; the startup gate separately enforces the deadline.
///
/// The admission a reconstructed ephemeral agent may still run, when it never began executing:
/// `Some(Some(key))` for one scheduled method, `Some(None)` when no method was accepted yet.
/// `None` keeps the agent observation-only. An agent with a constructor
/// (`initialization_key`) is admitted only while its constructor reservation is still pending, or
/// when a crash left only `Create`, which acquisition repairs by appending the reservation. A
/// withdrawn reservation is not repaired: nothing could initialize the agent before the method
/// runs.
pub fn never_started_ephemeral_admission(
    status: &AgentStatusRecord,
    initialization_key: Option<&IdempotencyKey>,
) -> Option<Option<IdempotencyKey>> {
    let is_initialization = |pending: &golem_common::model::agent::PendingInvocationRef| {
        initialization_key.is_some_and(|key| pending.has_idempotency_key(key))
    };
    if status.ephemeral_execution_begun
        || matches!(
            status.status,
            AgentStatus::Failed | AgentStatus::Exited | AgentStatus::Interrupted
        )
        || (initialization_key.is_some()
            && status.oplog_idx > OplogIndex::from_u64(2)
            && !status.pending_invocations.iter().any(is_initialization))
    {
        return None;
    }
    let mut methods = status
        .pending_invocations
        .iter()
        .filter(|pending| !pending.is_manual_update() && !is_initialization(pending));
    match (methods.next(), methods.next()) {
        (None, _) => Some(None),
        (Some(pending), None) if pending.not_before.is_some() => {
            Some(pending.idempotency_key().cloned())
        }
        _ => None,
    }
}
