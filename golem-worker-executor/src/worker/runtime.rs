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
    pub(crate) async fn create_entity_context(
        self: &Arc<Self>,
        runtime: OwnerRuntime,
        execution_mode: InvocationExecutionMode,
        filesystem: FilesystemCapability,
        executable: WorkerCtxExecutable,
        activation: Arc<golem_common::model::entity::EntityActivation>,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
        authority_wallet: Vec<StoredCard>,
        entity_start_index: Option<OplogIndex>,
    ) -> Result<Ctx, WorkerExecutorError> {
        if !matches!(runtime, OwnerRuntime::Entity(_)) {
            return Err(WorkerExecutorError::runtime(
                "Entity context construction requires an entity runtime",
            ));
        }
        let worker_metadata = self.get_latest_worker_metadata().await;
        let agent_effective_surface = match &self.owner_context {
            ResolvedOwnerContext::Agent(agent_id) => {
                agent_effective_surface_from_component_metadata(
                    &owner_component_metadata,
                    &self.owned_agent_id,
                    agent_id,
                )?
            }
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                crate::durable_host::owner_effective_surface_from_component_metadata(
                    &owner_component_metadata,
                    &self.owned_agent_id,
                    &self.owner_context,
                )?
            }
        };
        use golem_common::model::entity::EntityActivationSource;
        let executable_revision = match (&executable, activation.source()) {
            (
                WorkerCtxExecutable::Component(component),
                EntityActivationSource::Component { executable },
            ) if component.id == executable.component_id
                && component.revision == executable.component_revision =>
            {
                component.revision
            }
            (
                WorkerCtxExecutable::Native {
                    host_tool_id,
                    implementation_version,
                },
                EntityActivationSource::Host {
                    host_tool_id: expected_id,
                    implementation_version: expected_version,
                },
            ) if host_tool_id == expected_id && implementation_version == expected_version => {
                owner_component_metadata.revision
            }
            (WorkerCtxExecutable::Component(_), _) | (WorkerCtxExecutable::Native { .. }, _) => {
                return Err(WorkerExecutorError::runtime(
                    "Entity context executable does not match its validated activation source",
                ));
            }
        };
        let initial_agent_config = match &self.owner_context {
            ResolvedOwnerContext::Agent(agent_id) => {
                let component_config = owner_component_metadata
                    .metadata
                    .agent_type_config(&agent_id.agent_type)
                    .unwrap_or_default()
                    .to_vec();
                effective_agent_config(worker_metadata.config, component_config)?
                    .into_iter()
                    .map(|(path, value)| TypedAgentConfigEntry { path, value })
                    .collect()
            }
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                effective_agent_config(
                    worker_metadata.config,
                    owner_component_metadata
                        .metadata
                        .component_provision_config()
                        .config
                        .clone(),
                )?
                .into_iter()
                .map(|(path, value)| TypedAgentConfigEntry { path, value })
                .collect()
            }
        };
        let filesystem_generation = self
            .owner_runtime_resources
            .filesystem_generation_handle()?;
        if let Some(files) =
            Some(&activation.policy().provision().files).filter(|files| !files.is_empty())
        {
            provision_initial_files(
                &filesystem_generation,
                self.file_loader(),
                self.owned_agent_id.environment_id,
                files.clone(),
            )
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        }
        let filesystem_context = create_filesystem_context(filesystem_generation).await?;
        let initial_linear_memory = match &executable {
            WorkerCtxExecutable::Component(component) => {
                component.metadata.initial_linear_memory_bytes()
            }
            WorkerCtxExecutable::Native { .. } => 0,
        };
        if initial_linear_memory > self.resource_entry.max_memory_limit() as u64 {
            return Err(WorkerExecutorError::worker_creation_failed(
                self.agent_id(),
                format!(
                    "Linear memories require {initial_linear_memory} bytes, exceeding the per-agent limit of {} bytes",
                    self.resource_entry.max_memory_limit()
                ),
            ));
        }
        let retained_memory_grant = Arc::new(StdMutex::new(match execution_mode {
            InvocationExecutionMode::Live => {
                self.active_agents()
                    .acquire_memory(initial_linear_memory)
                    .await
            }
            InvocationExecutionMode::ReplayingCompleted
            | InvocationExecutionMode::ReplayingIncomplete => {
                MemoryGrant::inert(initial_linear_memory)
            }
        }));
        let admitted_startup_bytes = retained_memory_grant.lock().unwrap().bytes();
        let replaying = matches!(
            execution_mode,
            InvocationExecutionMode::ReplayingCompleted
                | InvocationExecutionMode::ReplayingIncomplete
        );
        let linear_memory = LinearMemoryTracker::new_with_metering(
            initial_linear_memory,
            admitted_startup_bytes,
            self.agent_mode(),
            replaying,
            Arc::clone(&self.resource_entry),
            retained_memory_grant,
            self.config().effective_resource_usage_metering().memory,
        );
        if let Some(meter) = linear_memory.meter_if_enabled() {
            self.owner_runtime_resources
                .register_resource_usage_flusher(meter.usage_flusher());
        }

        Ctx::create(
            worker_metadata.created_by,
            self.owned_agent_id.clone(),
            self.owner_context.clone(),
            self.promise_service(),
            self.worker_service(),
            self.worker_enumeration_service(),
            self.key_value_service(),
            self.blob_store_service(),
            self.rdbms_service(),
            self.quota_service(),
            self.worker_event_service.clone(),
            self.active_agents(),
            self.oplog_service(),
            self.oplog.clone(),
            Arc::downgrade(self),
            self.scheduler_service(),
            self.rpc(),
            self.worker_proxy(),
            self.card_service(),
            self.card_interest_index.clone(),
            self.component_service(),
            self.native_tool_catalog(),
            self.extra_deps(),
            self.config(),
            filesystem_context,
            linear_memory,
            AgentConfig::new(
                DeletedRegions::default(),
                0,
                executable_revision,
                worker_metadata.created_by,
                worker_metadata.created_by_email,
                initial_agent_config,
                filesystem_snapshots::ReplayBaseline::initial_files(),
                agent_effective_surface,
                Some(authority_wallet),
                entity_start_index,
                Some(owner_component_metadata),
            ),
            self.execution_status.clone(),
            self.file_loader(),
            self.worker_fork_service(),
            self.resource_limits(),
            self.agent_types(),
            self.environment_state_service(),
            self.agent_webhooks(),
            self.shard_service(),
            self.http_connection_pool(),
            self.websocket_connection_pool(),
            None,
            worker_metadata.original_phantom_id,
            runtime,
            Some(execution_mode),
            self.owner_execution(),
            self.owner_runtime_resources(),
            filesystem,
            executable,
            Some(activation),
        )
        .await
    }
}

impl RunningWorker {
    /// Gives the component revision of the agent just before the oplog index `before`: the target
    /// of the last successful update before it, or the revision of the `Create` entry.
    pub(in crate::worker) async fn revision_before<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        status: &AgentStatusRecord,
        before: OplogIndex,
    ) -> Result<ComponentRevision, UnreadableOplog> {
        Ok(
            match filesystem_snapshots::revision_before(&status.successful_updates, before) {
                Some(revision) => revision,
                None => match parent.oplog.read(OplogIndex::INITIAL).await? {
                    OplogEntry::Create { parameters, .. } => parameters.component_revision,
                    _ => status.component_revision_for_replay,
                },
            },
        )
    }
    /// Gives the initial files of the agent in the component revision `revision`.
    pub(in crate::worker) async fn initial_files_of<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        revision: ComponentRevision,
    ) -> Result<Box<[golem_common::model::component::InitialAgentFile]>, WorkerExecutorError> {
        let metadata = parent
            .component_service()
            .get_metadata(parent.owned_agent_id.component_id(), Some(revision))
            .await?;
        Ok(parent
            .parsed_agent_id
            .as_ref()
            .and_then(|agent_id| {
                metadata
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
            })
            .map(|config| config.files.iter().cloned().collect())
            .unwrap_or_default())
    }
    /// What a start does with the failure `error` of a start from `role` with the pending update
    /// `head`, as [`start_outcome::decide`] decides.
    pub(in crate::worker) fn start_action<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        role: &start_outcome::BaselineRole,
        head: Option<&PendingUpdateRef>,
        error: start_outcome::RawStartError<'_>,
    ) -> start_outcome::StartAction {
        start_outcome::decide(
            role,
            head,
            error,
            &parent.owned_agent_id.agent_id,
            parent.retired_for_lost_shard(),
        )
    }
    /// Performs `action` of a failed start. `Ok` means that the start runs again: a failed
    /// update is written, or a periodic record is skipped or rejected. `Err` is the error that
    /// ends the start. A write that fails ends the start with its error; a fenced write has
    /// already retired the agent, and the update stays pending for the new owner of the shard.
    pub(in crate::worker) async fn perform_start_action<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        action: start_outcome::StartAction,
    ) -> Result<(), WorkerExecutorError> {
        match action {
            start_outcome::StartAction::FailUpdate { entry, reject } => {
                warn!(?entry, "Pending update failed at the start");
                parent
                    .add_and_commit_oplog(entry)
                    .await
                    .map_err(WorkerExecutorError::from)?;
                if let Some(index) = reject {
                    parent.reject_periodic(index);
                }
                Ok(())
            }
            start_outcome::StartAction::SkipPeriodic(index) => {
                warn!(
                    snapshot_index = %index,
                    "The filesystem snapshot of an automatic snapshot record does not restore; the start uses the usable record before it"
                );
                parent.mark_periodic_unavailable(index);
                Ok(())
            }
            start_outcome::StartAction::RejectPeriodic(index) => {
                parent.reject_periodic(index);
                Ok(())
            }
            start_outcome::StartAction::Error(error) => Err(error),
            start_outcome::StartAction::ShardLost => Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::ShardLost,
            }),
            start_outcome::StartAction::Retry(_) => Err(WorkerExecutorError::runtime(
                "a failed start before the replay gave a replay outcome",
            )),
        }
    }
    /// Ends a start that failed with `error` while its filesystem was open, as
    /// [`start_outcome::decide`] decides for a start from `role` with the pending update `head`.
    /// `cleanup` closes the filesystem with the error that ends the start. An action that writes
    /// or changes something runs only after a clean cleanup, so a failed update is written only
    /// when the agent can start again on its source revision; the start then ends with `Restart`,
    /// an internal retry signal that records no interruption. After a failed cleanup the start
    /// ends with the error as it is, and nothing is written.
    pub(in crate::worker) async fn end_failed_start<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        role: &start_outcome::BaselineRole,
        head: Option<&PendingUpdateRef>,
        error: start_outcome::RawStartError<'_>,
        cleanup: impl AsyncFnOnce(WorkerExecutorError) -> CreateWorkerInstanceError,
    ) -> CreateWorkerInstanceError {
        let raw = error.passed_through();
        match Self::start_action(parent, role, head, error) {
            start_outcome::StartAction::Error(error) => cleanup(error).await,
            start_outcome::StartAction::ShardLost => {
                cleanup(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::ShardLost,
                })
                .await
            }
            action @ (start_outcome::StartAction::FailUpdate { .. }
            | start_outcome::StartAction::SkipPeriodic(_)
            | start_outcome::StartAction::RejectPeriodic(_)
            | start_outcome::StartAction::Retry(_)) => {
                let cleaned = cleanup(raw).await;
                if matches!(
                    cleaned,
                    CreateWorkerInstanceError::Failed {
                        filesystem_cleanup_failure: Some(_),
                        ..
                    }
                ) {
                    return cleaned;
                }
                CreateWorkerInstanceError::from(
                    match Self::perform_start_action(parent, action).await {
                        Ok(()) => WorkerExecutorError::Interrupted {
                            kind: InterruptKind::Restart,
                        },
                        Err(error) => error,
                    },
                )
            }
        }
    }
    pub(in crate::worker) async fn create_instance<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        start_attempt: Uuid,
        filesystem_snapshot_slot: &filesystem_snapshots::SnapshotSlot,
    ) -> Result<
        (
            WorkerRunningAgent<Ctx>,
            monthly_limits::ExecutionWindow,
            Option<RetryDecision>,
        ),
        CreateWorkerInstanceError,
    > {
        let component_id = parent.owned_agent_id.component_id();
        let entity_generation = parent
            .active_agents()
            .try_get_active_agent(&parent.owned_agent_id)
            .await
            .filter(|active_agent| Arc::ptr_eq(&active_agent.primary(), &parent))
            .map(|active_agent| {
                let generation = active_agent.entity_fence_generation();
                (active_agent, generation)
            });
        let Some(entity_generation) = entity_generation else {
            return Err(CreateWorkerInstanceError::Failed {
                error: WorkerExecutorError::Interrupted {
                    kind: InterruptKind::ShardLost,
                },
                filesystem_cleanup_failure: None,
            });
        };

        // Transport-owned bodies and custom begin coordinators can outlive their Stores. Seal
        // their old admission handle permanently before fixing the committed replay horizon.
        parent.runtime_append_tasks().seal_and_wait().await;
        let wallet_installation_guard = parent.card_event_boundary_lock.clone().lock_owned().await;

        // Drain buffered work from the previous runtime before constructing its replacement.
        parent
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await
            .map_err(WorkerExecutorError::from)?;
        parent.hydrated_invocation_results.write().await.clear();

        let mut worker_metadata = parent.get_latest_worker_metadata().await;
        let mut decision = parent
            .decide_start(&worker_metadata.last_known_status)
            .await?;
        if let StartDecision::PersistStrategy {
            description,
            admission_index,
        } = decision
        {
            parent
                .add_and_commit_oplog(OplogEntry::pending_update(
                    description,
                    Some(admission_index),
                ))
                .await
                .map_err(WorkerExecutorError::from)?;
            worker_metadata = parent.get_latest_worker_metadata().await;
            decision = parent.decide_start_in_memory(&worker_metadata.last_known_status);
        }
        let selection = match decision {
            StartDecision::PersistStrategy {
                description,
                admission_index,
            } => {
                parent
                    .add_and_commit_oplog(OplogEntry::pending_update(
                        description,
                        Some(admission_index),
                    ))
                    .await
                    .map_err(WorkerExecutorError::from)?;
                drop(wallet_installation_guard);
                return Box::pin(Self::create_instance(
                    parent,
                    concurrent_agent_permit,
                    start_attempt,
                    filesystem_snapshot_slot,
                ))
                .await;
            }
            StartDecision::FailHead { role, found } => {
                let action = Self::start_action(
                    &parent,
                    &role,
                    None,
                    start_outcome::RawStartError::StaleSource(&found),
                );
                Self::perform_start_action(&parent, action).await?;
                drop(wallet_installation_guard);
                return Box::pin(Self::create_instance(
                    parent,
                    concurrent_agent_permit,
                    start_attempt,
                    filesystem_snapshot_slot,
                ))
                .await;
            }
            StartDecision::Start(selection) => selection,
        };
        debug!("Creating instance with parent metadata {worker_metadata:?}");
        let role = selection.baseline.role();
        let pending_update_ref = worker_metadata
            .last_known_status
            .pending_updates
            .front()
            .cloned();

        let (pending_update, component, component_metadata) = {
            let component_revision = pending_update_ref.as_ref().map_or(
                worker_metadata.last_known_status.component_revision,
                |update| {
                    let target_revision = update.target_revision;
                    info!(
                        "Attempting {} update from {} to revision {target_revision}",
                        match update.kind {
                            PendingUpdateKind::Automatic => "automatic",
                            PendingUpdateKind::SnapshotAssistedAutomatic { .. } => {
                                "snapshot-assisted automatic"
                            }
                            PendingUpdateKind::SnapshotBased { .. } => "snapshot based",
                        },
                        worker_metadata.last_known_status.component_revision
                    );
                    target_revision
                },
            );

            if let Some(head) = &pending_update_ref {
                let metadata = parent
                    .component_service()
                    .get_metadata(component_id, Some(component_revision))
                    .await;
                if let Err(error) = &metadata
                    && matches!(
                        start_outcome::FetchProblem::of(error),
                        start_outcome::FetchProblem::Unavailable
                    )
                {
                    return Err(WorkerExecutorError::RecoveryRequired {
                        retry_from: None,
                        details: error.to_string(),
                    }
                    .into());
                }
                let problem = match &metadata {
                    Ok(metadata) => component_support_error(&metadata.metadata)
                        .map(start_outcome::RawStartError::TargetUnsupported),
                    Err(error) => Some(start_outcome::RawStartError::TargetFetch(error)),
                };
                if let Some(problem) = problem {
                    let action = Self::start_action(&parent, &role, Some(head), problem);
                    Self::perform_start_action(&parent, action).await?;
                    return Err(CreateWorkerInstanceError::ReacquireMemory);
                }
            }

            match parent
                .component_service()
                .get(&parent.engine(), component_id, component_revision)
                .await
            {
                Ok((component, component_metadata)) => {
                    if let Some(pending_update_ref) = &pending_update_ref
                        && let Some(agent_id) = &parent.parsed_agent_id
                        && let Some(target_agent_type) = component_metadata
                            .metadata
                            .find_agent_type_by_name_ref(&agent_id.agent_type)
                        && target_agent_type.mode != worker_metadata.agent_mode
                    {
                        let details = format!(
                            "Cannot update worker {} from {:?} to component revision {}: the agent type '{}' has mode {:?} in the target revision but the worker was created with mode {:?}. Changing an agent type's mode across revisions is not supported.",
                            parent.owned_agent_id,
                            worker_metadata.agent_mode,
                            component_revision,
                            agent_id.agent_type,
                            target_agent_type.mode,
                            worker_metadata.agent_mode,
                        );
                        let action = Self::start_action(
                            &parent,
                            &role,
                            Some(pending_update_ref),
                            start_outcome::RawStartError::ModeChange(&details),
                        );
                        Self::perform_start_action(&parent, action).await?;
                        drop(wallet_installation_guard);
                        return Box::pin(Self::create_instance(
                            parent,
                            concurrent_agent_permit,
                            start_attempt,
                            filesystem_snapshot_slot,
                        ))
                        .await;
                    }

                    // The status record only keeps a lightweight reference to the pending update;
                    // hydrate the full description (including any snapshot payload) from the oplog
                    // before handing it to the worker context.
                    let pending_update = match &pending_update_ref {
                        Some(pending_update_ref) => {
                            Some(parent.hydrate_pending_update(pending_update_ref).await?)
                        }
                        None => None,
                    };
                    (pending_update, component, component_metadata)
                }
                Err(error) => match &pending_update_ref {
                    Some(pending_update_ref) => {
                        warn!(
                            component_revision = %component_revision,
                            error = %error,
                            "Loading the pending-update target failed"
                        );
                        let action = Self::start_action(
                            &parent,
                            &role,
                            Some(pending_update_ref),
                            start_outcome::RawStartError::TargetLoad(&error),
                        );
                        Self::perform_start_action(&parent, action)
                            .await
                            .map_err(|error| match error {
                                WorkerExecutorError::Runtime { .. } => {
                                    WorkerExecutorError::RecoveryRequired {
                                        retry_from: None,
                                        details: error.to_string(),
                                    }
                                }
                                error => error,
                            })?;
                        drop(wallet_installation_guard);
                        return Box::pin(Self::create_instance(
                            parent,
                            concurrent_agent_permit,
                            start_attempt,
                            filesystem_snapshot_slot,
                        ))
                        .await;
                    }
                    None => return Err(error.into()),
                },
            }
        };

        if let Some(reason) = component_support_error(&component_metadata.metadata) {
            if let Some(head) = &pending_update_ref {
                let action = Self::start_action(
                    &parent,
                    &role,
                    Some(head),
                    start_outcome::RawStartError::TargetUnsupported(reason),
                );
                Self::perform_start_action(&parent, action).await?;
                return Err(CreateWorkerInstanceError::ReacquireMemory);
            }
            return Err(WorkerExecutorError::worker_creation_failed(
                parent.owned_agent_id.agent_id(),
                reason,
            )
            .into());
        }

        // Refresh the snapshot used by the read-only cache key. The component
        // metadata was already fetched above, so no extra fetch is incurred.
        parent
            .current_component
            .store(Arc::new(component_metadata.clone()));

        let start_filesystem = match StartFilesystem::load_and_plan(
            &parent,
            &worker_metadata.last_known_status,
            selection,
            pending_update.as_ref(),
        )
        .await
        {
            Ok(start_filesystem) => start_filesystem,
            Err(StartFilesystemError::Other(error)) => return Err(error.into()),
            Err(StartFilesystemError::Disabled) => {
                let action = Self::start_action(
                    &parent,
                    &role,
                    pending_update_ref.as_ref(),
                    start_outcome::RawStartError::Disabled,
                );
                Self::perform_start_action(&parent, action).await?;
                drop(wallet_installation_guard);
                return Box::pin(Self::create_instance(
                    parent,
                    concurrent_agent_permit,
                    start_attempt,
                    filesystem_snapshot_slot,
                ))
                .await;
            }
        };
        let component_version_for_replay = start_filesystem.replay_revision();

        let component_metadata_for_replay =
            if component_metadata.revision == component_version_for_replay {
                component_metadata.clone()
            } else {
                parent
                    .component_service()
                    .get_metadata(component_id, Some(component_version_for_replay))
                    .await?
            };

        let agent_effective_surface =
            crate::durable_host::owner_effective_surface_from_component_metadata(
                &component_metadata_for_replay,
                &parent.owned_agent_id,
                &parent.owner_context,
            )?;

        let skipped_regions = start_filesystem
            .replay()
            .skipped_regions(&worker_metadata.last_known_status.skipped_regions);

        let filesystems = parent.active_agents().agent_filesystems();
        let initial_files = parent
            .parsed_agent_id
            .as_ref()
            .and_then(|agent_id| {
                component_metadata_for_replay
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
            })
            .map(|config| config.files.clone())
            .unwrap_or_else(|| {
                if matches!(
                    parent.owner_context,
                    ResolvedOwnerContext::ComponentBaseline
                ) {
                    component_metadata_for_replay
                        .metadata
                        .component_provision_config()
                        .files
                        .clone()
                } else {
                    Vec::new()
                }
            });
        let limits = filesystems
            .resolved_limits(parent.resource_entry.max_disk_space_limit())
            .map_err(|error| CreateWorkerInstanceError::Failed {
                error: WorkerExecutorError::runtime(error.to_string()),
                filesystem_cleanup_failure: error.cleanup_failed().then(|| {
                    UnloadCleanupFailure::Other(WorkerExecutorError::runtime(error.to_string()))
                }),
            })?;
        let pressure = filesystems.pressure_policy();
        let pressure_recovery =
            crate::filesystem_pressure::FilesystemWriteRecovery::for_active_agents(
                filesystems.volume().clone(),
                Arc::downgrade(&parent.active_agents()),
                crate::filesystem_pressure::FilesystemWritePressurePolicy::from_config(pressure),
            );
        let created = filesystems
            .create_fresh_with_pressure_recovery(
                parent.owned_agent_id.clone(),
                limits,
                pressure_recovery,
            )
            .await
            .map_err(|failure| CreateWorkerInstanceError::Failed {
                error: WorkerExecutorError::runtime(failure.source.to_string()),
                filesystem_cleanup_failure: failure.source.cleanup_failed().then(|| {
                    UnloadCleanupFailure::Other(WorkerExecutorError::runtime(
                        failure.source.to_string(),
                    ))
                }),
            })?;
        created.observe_release(&parent.release_scope());
        let retained_memory_grant = parent.linear_memory_grant();
        let admitted_startup_bytes = retained_memory_grant.lock().unwrap().bytes();
        // The tracker starts in replay mode so the admitted startup reservation stays protected
        // and prepays the growth replayed from the oplog; `DurableWorkerCtx::create` switches it
        // to live once the replay state is known to be live.
        let linear_memory = LinearMemoryTracker::new_with_metering(
            parent.startup_linear_memory_bytes(),
            admitted_startup_bytes,
            parent.agent_mode(),
            true,
            Arc::clone(&parent.resource_entry),
            retained_memory_grant,
            parent.config().effective_resource_usage_metering().memory,
        );
        let account = ResourceUsageAccount::new(
            parent.agent_mode(),
            linear_memory.clone(),
            Arc::clone(&parent.resource_entry),
        );
        let metering = parent.config().effective_resource_usage_metering();
        let reconstructing = match bind_configured_resource_usage_metering(
            created, account, metering,
        ) {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                let startup_error = WorkerExecutorError::runtime(failure.source.to_string());
                return Err(match delete_created(failure.filesystem).await {
                    Ok(()) => startup_error.into(),
                    Err(cleanup_error) => CreateWorkerInstanceError::Failed {
                        error: WorkerExecutorError::runtime(format!(
                            "{startup_error}; additionally failed to clean up the created agent filesystem: {}",
                            cleanup_error.source
                        )),
                        filesystem_cleanup_failure: Some(UnloadCleanupFailure::Filesystem {
                            failure: cleanup_error,
                            close_error: None,
                            release_receipt: None,
                        }),
                    },
                });
            }
        };
        let window =
            match open_resource_usage_window(&reconstructing, concurrent_agent_permit).await {
                Ok(window) => window,
                Err(error) => {
                    let sealed = abort_reconstruction(reconstructing);
                    return Err(cleanup_typed_agent_filesystem(
                        sealed,
                        WorkerExecutorError::runtime(format!(
                            "Failed to open worker resource usage window: {error}"
                        )),
                        None,
                    )
                    .await);
                }
            };
        let window = monthly_limits::ExecutionWindow::new(&parent, window, start_attempt).await;
        let prepared = match prepare_initial_files(
            parent.file_loader(),
            parent.owned_agent_id.environment_id,
            &initial_files,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(cleanup_reconstructing_agent_filesystem(
                    reconstructing,
                    window,
                    WorkerExecutorError::runtime(error.to_string()),
                )
                .await);
            }
        };
        let (reconstructing, replay) = match start_filesystem
            .materialize(reconstructing, prepared, filesystem_snapshot_slot)
            .await
        {
            Ok(materialized) => materialized,
            Err(failure) => {
                return Err(Self::end_failed_start(
                    &parent,
                    &failure.replay.role,
                    pending_update_ref.as_ref(),
                    start_outcome::RawStartError::Filesystem(&failure.error),
                    async move |error| {
                        cleanup_open_agent_filesystem(failure.filesystem, window, error).await
                    },
                )
                .await);
            }
        };
        let replays_a_snapshot = replay.snapshot.is_some();
        let reconstruction_generation_handle =
            match reconstruction_generation_handle(&reconstructing) {
                Ok(generation_handle) => generation_handle,
                Err(error) => {
                    return Err(cleanup_reconstructing_agent_filesystem(
                        reconstructing,
                        window,
                        WorkerExecutorError::runtime(error.to_string()),
                    )
                    .await);
                }
            };
        let filesystem_publication = FilesystemGenerationPublication::new(
            Arc::clone(&parent.owner_runtime_resources),
            reconstruction_generation_handle.clone(),
        );
        let filesystem_context = match create_filesystem_context(reconstruction_generation_handle)
            .await
        {
            Ok(context) => context,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        let install_wallet = async {
            parent
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            let horizon = parent.oplog.current_oplog_index().await;
            let snapshot_index = replay.snapshot.map(|(index, _)| index);
            let baseline = owner_wallet::WalletBaseline::new(
                parent.owned_agent_id.clone(),
                parent.owner_context.clone(),
                parent.component_service(),
                &component_metadata_for_replay,
                parent.oplog.as_ref(),
                snapshot_index,
                worker_metadata.config.clone(),
            )
            .await?;
            parent
                .owner_execution
                .install_replay_generation_locked(
                    skipped_regions.clone(),
                    snapshot_index,
                    baseline,
                    horizon,
                    parent.tasks.clone(),
                    &wallet_installation_guard,
                )
                .await
        }
        .await;
        if let Err(error) = install_wallet {
            drop(filesystem_context);
            return Err(
                cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
            );
        }
        drop(wallet_installation_guard);
        {
            let mut lifecycle = parent.instance.lock().await;
            if let WorkerInstance::Running(running) = lifecycle.deletion_runtime_mut()
                && running.start_attempt == start_attempt
            {
                running.owner_generation_started = true;
            }
        }
        let mut context = match Ctx::create(
            worker_metadata.created_by,
            OwnedAgentId::new(worker_metadata.environment_id, &worker_metadata.agent_id),
            parent.owner_context.clone(),
            parent.promise_service(),
            parent.worker_service(),
            parent.worker_enumeration_service(),
            parent.key_value_service(),
            parent.blob_store_service(),
            parent.rdbms_service(),
            parent.quota_service(),
            parent.worker_event_service.clone(),
            parent.active_agents(),
            parent.oplog_service(),
            parent.oplog.clone(),
            Arc::downgrade(&parent),
            parent.scheduler_service(),
            parent.rpc(),
            parent.worker_proxy(),
            parent.card_service(),
            parent.card_interest_index.clone(),
            parent.component_service(),
            parent.native_tool_catalog(),
            parent.extra_deps(),
            parent.config(),
            filesystem_context,
            linear_memory,
            AgentConfig::new(
                skipped_regions,
                worker_metadata.last_known_status.total_linear_memory_size,
                component_version_for_replay,
                worker_metadata.created_by,
                worker_metadata.created_by_email,
                worker_metadata.config,
                replay,
                agent_effective_surface,
                None,
                None,
                None,
            ),
            parent.execution_status.clone(),
            parent.file_loader(),
            parent.worker_fork_service(),
            parent.resource_limits(),
            parent.agent_types(),
            parent.environment_state_service(),
            parent.agent_webhooks(),
            parent.shard_service(),
            parent.http_connection_pool(),
            parent.websocket_connection_pool(),
            pending_update,
            worker_metadata.original_phantom_id,
            OwnerRuntime::Agent,
            None,
            parent.owner_execution(),
            parent.owner_runtime_resources(),
            FilesystemCapability::Capable,
            WorkerCtxExecutable::Component(Box::new(component_metadata_for_replay)),
            None,
        )
        .await
        {
            Ok(context) => context,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };

        let instance_host = match instance::InstanceHost::new(
            &parent,
            OwnerRuntime::Agent,
            ExecutableTarget::new(component_id, component_metadata.revision),
        ) {
            Ok(instance_host) => instance_host,
            Err(error) => {
                drop(context);
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        if replays_a_snapshot {
            // Core initializers run before load-snapshot, but their recorded host calls are
            // already inside the skipped snapshot history. Recreate that runtime state with
            // the same durability suppression as snapshot loading, without consuming the tail.
            context.begin_call_snapshotting_function();
        }
        let runtime = async {
            let mut hosted = instance_host.instantiate(context, &component).await?;
            instance_host.reconcile_linear_memories(&mut hosted).await?;
            Ok::<_, WorkerExecutorError>(hosted.into_parts())
        }
        .await;
        let (instance, mut store) = match runtime {
            Ok(runtime) => runtime,
            Err(error) => {
                return Err(Self::end_failed_start(
                    &parent,
                    &role,
                    pending_update_ref.as_ref(),
                    start_outcome::RawStartError::Instantiation(&error),
                    async move |error| {
                        cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await
                    },
                )
                .await);
            }
        };
        if replays_a_snapshot {
            store.data_mut().end_call_snapshotting_function();
        }
        {
            let (active_agent, generation) = entity_generation;
            let establishment = parent.interrupt_establishment_tail.lock().unwrap().clone();
            establishment.wait().await;
            let reopening = if let Some(error) = parent.monthly_cleanup_failure() {
                Err(error)
            } else if parent
                .interrupt_signal
                .when_idle(|| {
                    Arc::ptr_eq(
                        &establishment,
                        &parent.interrupt_establishment_tail.lock().unwrap(),
                    ) && active_agent.reopen_entity_admission_if_generation(generation)
                })
                .await
                == Some(true)
            {
                Ok(())
            } else {
                Err(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                })
            };
            if let Err(error) = reopening {
                drop(store);
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        }
        let prepare_result =
            Ctx::prepare_instance(&parent.owned_agent_id.agent_id, &instance, &mut store).await;
        let decision = match prepare_result {
            Ok(decision) => decision,
            Err(error) => {
                drop(store);
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        let reconstructing = match finish_replay(reconstructing).await {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                drop(store);
                return Err(Self::end_failed_start(
                    &parent,
                    &role,
                    pending_update_ref.as_ref(),
                    start_outcome::RawStartError::Finish(&failure.source),
                    async move |error| {
                        cleanup_open_agent_filesystem(failure.filesystem, window, error).await
                    },
                )
                .await);
            }
        };
        let filesystem = match finish_reconstruction(reconstructing).await {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                drop(store);
                return Err(Self::end_failed_start(
                    &parent,
                    &role,
                    pending_update_ref.as_ref(),
                    start_outcome::RawStartError::Finish(&failure.source),
                    async move |error| {
                        cleanup_open_agent_filesystem(failure.filesystem, window, error).await
                    },
                )
                .await);
            }
        };
        let filesystem_generation = resident_generation_handle(&filesystem);
        store
            .data_mut()
            .durable_ctx_mut()
            .activate_resident_generation_handle(filesystem_generation.clone());
        filesystem_publication.commit(filesystem_generation);
        Ok((
            RunningAgent {
                runtime: RunningAgentRuntime {
                    instance,
                    store: store.map(async_lock::Mutex::new),
                },
                filesystem,
            },
            window,
            decision,
        ))
    }
}

impl<Ctx: 'static> ObservedPrimaryStore<Store<Ctx>> {
    pub(in crate::worker) fn new(
        engine: &wasmtime::Engine,
        context: Ctx,
        scope: Option<&crate::metrics::resource_release::ReleaseScope>,
    ) -> Self {
        use crate::metrics::resource_release::Stage;
        Self {
            store: Some(Store::new(engine, context)),
            receipts: scope.map(|scope| {
                (
                    scope.receipt(Stage::PrimaryExecutionStopped),
                    scope.receipt(Stage::PrimaryStoreDrop),
                )
            }),
        }
    }
}

impl<T> ObservedPrimaryStore<T> {
    pub(in crate::worker) fn map<U>(
        mut self,
        transform: impl FnOnce(T) -> U,
    ) -> ObservedPrimaryStore<U> {
        ObservedPrimaryStore {
            store: Some(transform(self.store.take().unwrap())),
            receipts: self.receipts.take(),
        }
    }

    pub(in crate::worker) fn mark_primary_execution_stopped(&self) {
        if let Some((stopped, _)) = &self.receipts {
            stopped.complete();
        }
    }
}

impl<T> std::ops::Deref for ObservedPrimaryStore<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.store.as_ref().unwrap()
    }
}

impl<T> std::ops::DerefMut for ObservedPrimaryStore<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.store.as_mut().unwrap()
    }
}

impl<T> Drop for ObservedPrimaryStore<T> {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            self.mark_primary_execution_stopped();
            drop(store);
            if let Some((_, released)) = &self.receipts {
                released.complete();
            }
        }
    }
}

impl<Ctx: WorkerCtx> LinearMemoryGrantRegistration<Ctx> {
    pub(in crate::worker) fn new(
        worker: Arc<Worker<Ctx>>,
        grant: Arc<StdMutex<MemoryGrant>>,
    ) -> Self {
        let previous = worker
            .linear_memory_grant
            .lock()
            .unwrap()
            .replace(grant.clone());
        assert!(
            previous.is_none(),
            "worker already has a linear memory grant"
        );
        Self { worker, grant }
    }
}

impl<Ctx: WorkerCtx> Drop for LinearMemoryGrantRegistration<Ctx> {
    fn drop(&mut self) {
        let mut registered = self.worker.linear_memory_grant.lock().unwrap();
        if registered
            .as_ref()
            .is_some_and(|grant| Arc::ptr_eq(grant, &self.grant))
        {
            registered.take();
        }
    }
}

impl FilesystemGenerationPublication {
    pub(in crate::worker) fn new(
        resources: Arc<OwnerRuntimeResources>,
        generation_handle: FilesystemGenerationHandle,
    ) -> Self {
        resources.activate_filesystem_generation(generation_handle);
        Self {
            resources,
            clear_on_drop: true,
        }
    }

    pub(in crate::worker) fn commit(mut self, generation_handle: FilesystemGenerationHandle) {
        self.resources
            .activate_filesystem_generation(generation_handle);
        self.clear_on_drop = false;
    }
}

impl Drop for FilesystemGenerationPublication {
    fn drop(&mut self) {
        if self.clear_on_drop {
            self.resources.fence_filesystem_generation();
        }
    }
}

pub(in crate::worker) async fn create_filesystem_context(
    generation_handle: FilesystemGenerationHandle,
) -> Result<WorkerFilesystemContext, WorkerExecutorError> {
    let target = PathTarget::at_root(&generation_handle, "")
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let preopen = open_agent_filesystem(
        &generation_handle,
        target,
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    .await
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    .node;
    Ok(WorkerFilesystemContext::new(generation_handle, preopen))
}

pub(in crate::worker) async fn cleanup_reconstructing_agent_filesystem(
    filesystem: ReconstructingFilesystem,
    window: monthly_limits::ExecutionWindow,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    cleanup_open_agent_filesystem(abort_reconstruction(filesystem), window, startup_error).await
}

pub(in crate::worker) async fn cleanup_open_agent_filesystem(
    filesystem: SealedFilesystem,
    window: monthly_limits::ExecutionWindow,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    invocation_loop::cleanup_startup_filesystem(filesystem, window, startup_error).await
}

pub(in crate::worker) async fn cleanup_typed_agent_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
    startup_error: WorkerExecutorError,
    close_error: Option<WorkerExecutorError>,
) -> CreateWorkerInstanceError {
    match delete_agent_filesystem(filesystem).await {
        Ok(()) => CreateWorkerInstanceError::Failed {
            error: startup_error,
            filesystem_cleanup_failure: close_error.map(UnloadCleanupFailure::Other),
        },
        Err(cleanup_error) => {
            warn!(error = %cleanup_error.source, "Failed to clean up filesystem after worker startup failure");
            CreateWorkerInstanceError::Failed {
                error: WorkerExecutorError::runtime(format!(
                    "{startup_error}; additionally failed to clean up the agent filesystem: {}",
                    cleanup_error.source
                )),
                filesystem_cleanup_failure: Some(UnloadCleanupFailure::Filesystem {
                    failure: cleanup_error,
                    close_error,
                    release_receipt: None,
                }),
            }
        }
    }
}

pub(in crate::worker) fn shared_linear_memory_error<Ctx: WorkerCtx>(
    parent: &Arc<Worker<Ctx>>,
) -> WorkerExecutorError {
    WorkerExecutorError::worker_creation_failed(
        parent.owned_agent_id.agent_id(),
        SHARED_LINEAR_MEMORY_ERROR,
    )
}

impl From<WorkerExecutorError> for CreateWorkerInstanceError {
    fn from(error: WorkerExecutorError) -> Self {
        Self::Failed {
            error,
            filesystem_cleanup_failure: None,
        }
    }
}
