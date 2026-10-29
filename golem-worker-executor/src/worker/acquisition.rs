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
        let component = deps
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await?;
        let parsed_agent_id =
            match ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata) {
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

        let agent_mode = if freshness_disposition == InvocationFreshnessDisposition::MayExist {
            deps.worker_service()
                .get(owned_agent_id)
                .await?
                .map(|metadata| metadata.initial_worker_metadata.agent_mode)
                .unwrap_or(agent_type.mode)
        } else {
            agent_type.mode
        };

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
            instance: Arc::new(Mutex::new(WorkerInstance::Unresolved)),
            unload_cleanup: StdMutex::new(None),
            resolved: OnceCell::new(),
            initialization_complete: AtomicBool::new(false),
            initialization_request: StdMutex::new(None),
        })
    }
    /// Joins final cleanup after the caller has fenced and stopped the runtime. Unlike the
    /// bounded unload notification, success proves the owned filesystem deletion was verified.
    /// Must be called without lifecycle, cache, or metadata locks held.
    pub async fn wait_for_unload_cleanup(&self) -> Result<(), WorkerExecutorError> {
        let instance = self.instance.lock().await;
        let cleanup = self.unload_cleanup.lock().unwrap().clone();
        let missing_cleanup_error = match instance.deletion_runtime() {
            WorkerInstance::CleanupFailed(error) if cleanup.is_none() => Some(error.clone()),
            WorkerInstance::Running(_)
            | WorkerInstance::Stopping(_)
            | WorkerInstance::WaitingForPermit(_) => Some(WorkerExecutorError::runtime(
                "worker runtime has not stopped before cleanup wait",
            )),
            _ => None,
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
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let worker = self.clone();
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
                let freshness_disposition =
                    if worker.initialization_request.lock().unwrap().is_some() {
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
                    freshness_disposition,
                    creation_mode,
                )
                .await;
                worker
                    .finish_construction(
                        start,
                        metadata,
                        Some((invocation_context_stack, principal)),
                    )
                    .await
            }
            .boxed();
            let (started, result) = self.initialize_with(build).await;
            match result {
                Err(WorkerExecutorError::AgentNotFound { .. }) if !started => continue,
                result => return result,
            }
        }
    }
    pub(crate) async fn ensure_existing_with_fingerprint(
        self: &Arc<Self>,
        principal: Principal,
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
            worker
                .finish_construction(
                    start,
                    metadata,
                    Some((InvocationContextStack::fresh(), principal)),
                )
                .await
        }
        .boxed();
        self.initialize_with(build).await.1
    }
    pub(in crate::worker) async fn initialize_with(
        self: &Arc<Self>,
        build: BoxFuture<'static, Result<WorkerInstance, WorkerExecutorError>>,
    ) -> (bool, Result<(), WorkerExecutorError>) {
        let (started, completion) = {
            let mut instance = self.instance.lock().await;
            match &*instance {
                WorkerInstance::Unresolved => {
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
                    *instance = WorkerInstance::Initializing(completion.clone());
                    let worker = self.clone();
                    let invocation_loops = self.active_agents().invocation_loops();
                    let construction = async move {
                        let result = crate::worker::invocation::with_invocation_stack(build).await;
                        let published = result.as_ref().map(|_| ()).map_err(Clone::clone);
                        let mut instance = worker.instance.lock().await;
                        *instance = match result {
                            Ok(instance) => {
                                worker.initialization_request.lock().unwrap().take();
                                worker
                                    .initialization_complete
                                    .store(true, Ordering::Release);
                                instance
                            }
                            Err(_) => WorkerInstance::Unresolved,
                        };
                        if published.is_ok() {
                            Self::start_durable_stream_attachment_reconciler(&worker);
                        }
                        drop(instance);
                        let resolved = published.is_ok();
                        let _ = sender.send(published);
                        if resolved {
                            worker.retire_if_shard_left_during_construction().await;
                        }
                    };
                    if !invocation_loops.spawn_construction(construction) {
                        *instance = WorkerInstance::Unresolved;
                        return (
                            false,
                            Err(WorkerExecutorError::runtime(
                                "Worker initialization rejected because its executor is stopping",
                            )),
                        );
                    }
                    (true, completion)
                }
                WorkerInstance::Initializing(completion) => (false, completion.clone()),
                WorkerInstance::CleanupFailed(error) => return (false, Err(error.clone())),
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
        initialization: Option<(InvocationContextStack, Principal)>,
    ) -> Result<WorkerInstance, WorkerExecutorError> {
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
        let (worker, reconstructed_ephemeral) =
            match self.prepare_worker_data(metadata, initialization).await {
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
        assert!(
            self.resolved.set(worker).is_ok(),
            "Worker initialized twice"
        );
        crate::metrics::wasm::record_create_worker(start.elapsed());

        Ok(WorkerInstance::Unloaded {
            startup_failure: reconstructed_ephemeral.then(inactive_ephemeral_agent_error),
        })
    }
    pub(in crate::worker) async fn prepare_worker_data(
        self: &Arc<Self>,
        metadata: GetOrCreateWorkerResult,
        initialization: Option<(InvocationContextStack, Principal)>,
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
            reconstructed_ephemeral && self.initialization_request.lock().unwrap().is_none();
        let initialization = {
            let mut request = self.initialization_request.lock().unwrap();
            if newly_created {
                // Retain provenance only after this request actually created the incarnation.
                *request = initialization;
                request.clone()
            } else {
                request.clone().or(initialization)
            }
        };
        if newly_created {
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
                EphemeralInvocationState::Available
            }),
            cache_retirement_in_progress: AtomicBool::new(false),
            startup_attempt: StartupAttemptTracker::default(),
            linear_memory_grant: StdMutex::new(None),
            #[cfg(feature = "test-utils")]
            resident_wakeup_gate: StdMutex::new(None),
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

        // A crash may leave only Create. The initialization reservation excludes ordinary
        // invocation admission; the status actor still performs the persisted dedupe.
        if let (Some(agent_id), Some((invocation_context, principal))) =
            (worker.owner_context.agent(), initialization)
            && last_oplog_idx <= OplogIndex::from_u64(2)
            && !reconstructed_ephemeral
        {
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
        let component_id = owned_agent_id.component_id();
        let GetWorkerMetadataResult {
            initial_worker_metadata,
            last_known_status,
        } = metadata;
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
        let owner_context = ResolvedOwnerContext::from_authoritative_kind(
            initial_worker_metadata.owner_kind,
            &owned_agent_id.agent_id.agent_id,
            &initial_component.metadata,
        )
        .map_err(WorkerExecutorError::invalid_request)?;
        if matches!(owner_context, ResolvedOwnerContext::ComponentBaseline)
            && initial_worker_metadata.agent_mode == AgentMode::Durable
        {
            return Err(WorkerExecutorError::invalid_request(
                "An external tool owner cannot use durable mode",
            ));
        }
        let agent_id = owner_context.agent().cloned();
        let ResolvedAgentProperties {
            snapshot_policy, ..
        } = resolve_agent_properties(this, agent_id.as_ref(), &initial_component.metadata);
        let snapshot_policy =
            if initial_worker_metadata.owner_kind == OwnerKind::EphemeralExternalTool {
                SnapshotPolicy::Disabled
            } else {
                snapshot_policy
            };
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
        freshness_disposition: InvocationFreshnessDisposition,
        creation_mode: WorkerCreationMode,
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

        if creation_mode == WorkerCreationMode::ComponentAgent {
            OwnerKind::ComponentAgent
                .validate_instance_name(&owned_agent_id.agent_id.agent_id)
                .map_err(WorkerExecutorError::invalid_request)?;
        }

        // KnownFresh has already been validated against the ephemeral agent type, phantom ID, and
        // idempotency key at invocation ingress. All other paths retain the checked lookup.
        let existing = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            None
        } else {
            Self::get_existing_worker_metadata(this, lifecycle, owned_agent_id, None, shard_epoch)
                .await?
        };

        match existing {
            Some(existing) => Ok(existing),
            None => {
                // Create and initialize a new worker.
                let component = this
                    .component_service()
                    .get_metadata(component_id, component_revision)
                    .await?;

                let virtual_owner = creation_mode == WorkerCreationMode::EphemeralExternalTool;
                let owner_kind = if virtual_owner {
                    OwnerKind::EphemeralExternalTool
                } else {
                    OwnerKind::ComponentAgent
                };
                let owner_context = ResolvedOwnerContext::from_authoritative_kind(
                    owner_kind,
                    &owned_agent_id.agent_id.agent_id,
                    &component.metadata,
                )
                .map_err(WorkerExecutorError::invalid_request)?;
                if virtual_owner && component.environment_id != owned_agent_id.environment_id {
                    return Err(WorkerExecutorError::invalid_request(
                        "owner environment does not match the component environment",
                    ));
                }
                let agent_id = owner_context.agent().cloned();

                let ResolvedAgentProperties {
                    agent_mode,
                    snapshot_policy,
                } = resolve_agent_properties(this, agent_id.as_ref(), &component.metadata);
                let (agent_mode, snapshot_policy) = if virtual_owner {
                    (AgentMode::Ephemeral, SnapshotPolicy::Disabled)
                } else {
                    (agent_mode, snapshot_policy)
                };

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
    agent_id: Option<&ParsedAgentId>,
    metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> ResolvedAgentProperties {
    let resolved_agent_type =
        agent_id.and_then(|id| metadata.find_agent_type_by_name_ref(&id.agent_type));

    let agent_mode = resolved_agent_type.map_or(AgentMode::Durable, |at| at.mode);

    let configured_policy = if let Some(agent_type) = resolved_agent_type {
        // Agent with explicit metadata — use agent-level snapshotting config
        resolve_snapshot_policy(
            &deps.config().oplog.default_snapshotting,
            Some(&agent_type.snapshotting),
        )
    } else if is_snapshot_capable_oplog_processor(metadata) {
        // Oplog processor that exports save-snapshot/load-snapshot — use the
        // oplog-processor-specific global config
        deps.config().oplog.oplog_processor_snapshotting.clone()
    } else {
        // Non-agent, non-snapshot-capable-oplog-processor — use default
        resolve_snapshot_policy(&deps.config().oplog.default_snapshotting, None)
    };

    ResolvedAgentProperties {
        agent_mode,
        snapshot_policy: snapshot_policy_of_mode(agent_mode, configured_policy),
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

/// Returns true if the component is an oplog processor that also exports
/// save-snapshot and load-snapshot functions, making it eligible for
/// automatic snapshot-based recovery.
pub(in crate::worker) fn is_snapshot_capable_oplog_processor(
    metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> bool {
    metadata.has_oplog_processor() && metadata.has_save_snapshot() && metadata.has_load_snapshot()
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
