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
    pub(crate) async fn deletion_owns_retirement(&self) -> bool {
        self.instance.lock().await.deletion_owns_retirement()
    }
    pub(in crate::worker) async fn deletion_stage_completed(
        &self,
        stage: WorkerDeletionStage,
    ) -> bool {
        matches!(&*self.instance.lock().await, WorkerInstance::Deleting(deleting) if deleting.completed_stage >= stage)
    }
    pub(in crate::worker) async fn complete_deletion_stage(&self, stage: WorkerDeletionStage) {
        let mut instance = self.instance.lock().await;
        if let WorkerInstance::Deleting(deleting) = &mut *instance {
            deleting.completed_stage = deleting.completed_stage.max(stage);
        }
    }
    /// Atomically claims worker-owned deletion and starts an executor-owned attempt. Overlapping
    /// callers share the retained result; after failure, the next explicit call claims one retry.
    pub(crate) async fn start_deletion(
        self: &Arc<Self>,
    ) -> Result<Option<DeleteOutcome>, WorkerExecutorError> {
        if let Some(hook) = Ctx::worker_deletion_hook(&self.extra_deps()) {
            hook.before_claim(&self.owned_agent_id).await;
        }
        let cleanup = self.owner_cleanup.lock().await;
        if *cleanup == OwnerCleanupState::Retired {
            return Ok(None);
        }
        // The lifecycle-to-cache lock order serializes the claim with ordinary retirement. The
        // cache lookup itself never takes a worker lifecycle lock.
        let mut instance = self.instance.lock().await;
        if self.cache_retirement_in_progress() {
            return Ok(None);
        }
        if !self.active_agents().contains_worker_generation(self).await {
            return Ok(None);
        }
        let retry_cleanup = matches!(
            instance.deletion_runtime(),
            WorkerInstance::CleanupFailed(_)
        );
        let (outcome, sender) = match &mut *instance {
            WorkerInstance::Deleting(deleting) => match deleting.handle.result() {
                None | Some(Ok(())) => (
                    DeleteOutcome::AlreadyDeleting(deleting.handle.clone()),
                    None,
                ),
                Some(Err(_)) => {
                    let (handle, sender) = DeletionHandle::new();
                    deleting.handle = handle.clone();
                    (DeleteOutcome::Started(handle), Some(sender))
                }
            },
            _ => {
                let (handle, sender) = DeletionHandle::new();
                let runtime = std::mem::replace(
                    &mut *instance,
                    WorkerInstance::Unloaded {
                        startup_failure: None,
                    },
                );
                *instance = WorkerInstance::Deleting(DeletingWorker {
                    runtime: Box::new(runtime),
                    handle: handle.clone(),
                    completed_stage: WorkerDeletionStage::Claimed,
                });
                (DeleteOutcome::Started(handle), Some(sender))
            }
        };
        let started = matches!(outcome, DeleteOutcome::Started(_));
        drop(instance);
        drop(cleanup);
        if let Some(hook) = Ctx::worker_deletion_hook(&self.extra_deps()) {
            let handle = match &outcome {
                DeleteOutcome::Started(handle) | DeleteOutcome::AlreadyDeleting(handle) => {
                    handle.clone()
                }
            };
            hook.observe_result(
                &self.owned_agent_id,
                async move { handle.wait().await }.boxed(),
            );
            hook.claimed(&self.owned_agent_id, started);
        }
        match &outcome {
            DeleteOutcome::Started(_) => {}
            DeleteOutcome::AlreadyDeleting(_) => return Ok(Some(outcome)),
        }

        let worker = self.clone();
        let sender = sender.expect("started deletion has a completion sender");
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(worker.run_deletion_attempt(retry_cleanup))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(WorkerExecutorError::runtime(
                        "Worker deletion task panicked",
                    ))
                });
            let _ = sender.send(result);
        });

        Ok(Some(outcome))
    }
    /// Runs monotonic cleanup. A retry skips every stage already completed by an earlier attempt.
    pub(in crate::worker) async fn run_deletion_attempt(
        self: &Arc<Self>,
        retry_cleanup: bool,
    ) -> Result<(), WorkerExecutorError> {
        let interrupt_kind = InterruptKind::Interrupt(Timestamp::now_utc());
        if !self
            .deletion_stage_completed(WorkerDeletionStage::ExecutionFenced)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::ExecutionFenced)
                .await?;
            info!(agent_id = %self.owned_agent_id, "Fencing worker execution for deletion");
            self.set_interrupting_internal(interrupt_kind, false, UnloadReason::Deleting, true)
                .await?;
            self.complete_deletion_stage(WorkerDeletionStage::ExecutionFenced)
                .await;
        }

        // Stop any future background flush or clean-checkpoint write from resurrecting the cached
        // status after the upcoming `WorkerService::remove`/`remove_cached_status` deletes it (the
        // latter clears both the live cache and the checkpoint). Each awaits any in-flight write so
        // none can land after the delete.
        if !self
            .deletion_stage_completed(WorkerDeletionStage::BarriersClosed)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::BarriersClosed)
                .await?;
            self.status_flusher.begin_delete().await;
            self.status_checkpointer.begin_delete().await;
            self.complete_deletion_stage(WorkerDeletionStage::BarriersClosed)
                .await;
        }

        if !self
            .deletion_stage_completed(WorkerDeletionStage::RuntimeStopped)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::RuntimeStopped)
                .await?;
            let retirement = self.retire_durable_stream_producer();
            let error = WorkerExecutorError::invalid_request("Worker is being deleted");
            self.stop_internal(
                false,
                Some(error),
                UnloadRequest::ordinary(UnloadReason::Deleting),
                FinalWorkerState::Deleting,
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
            self.tasks.stop_roots_and_wait().await;
            self.durable_stream_attachment_reconciler.stop().await;
            retirement.await?;
            let cleanup_error = {
                let instance = self.instance.lock().await;
                match instance.deletion_runtime() {
                    WorkerInstance::CleanupFailed(error) => Some(error.clone()),
                    _ => None,
                }
            };
            if let Some(error) = cleanup_error {
                // A shutdown failure observed by this attempt belongs to its immutable result.
                // Only an explicit claim made after the failure may join or retry final cleanup.
                if !retry_cleanup {
                    return Err(error);
                }
                let cleanup = self.unload_cleanup.lock().unwrap().clone().ok_or(error)?;
                let retry = cleanup.retry();
                *self.unload_cleanup.lock().unwrap() = Some(retry.clone());
                retry.wait().await?;
                *self.instance.lock().await.deletion_runtime_mut() =
                    WorkerInstance::StoppedForDeletion;
            }
            self.wait_for_unload_cleanup().await?;
            self.complete_deletion_stage(WorkerDeletionStage::RuntimeStopped)
                .await;
        }

        if !self
            .deletion_stage_completed(WorkerDeletionStage::StreamsCleaned)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::StreamsCleaned)
                .await?;
            // The steps below reach other agents, and each skips its fenced record when an earlier
            // attempt, here or in another process, already wrote it. Test the fence first, without
            // writing, so an executor that has lost the shard sends nothing.
            if let Some(epoch) = self.oplog.shard_epoch()
                && let Err(error) = self
                    .oplog_service()
                    .assert_owning_epoch(
                        &self.owned_agent_id,
                        self.initial_worker_metadata.agent_mode,
                        epoch,
                    )
                    .await
            {
                return Err(self.deletion_step_failed(error.into()).await);
            }
            let producer = match DurableStreamStore::load_indexed_with_commit(
                self.oplog.clone(),
                self.owned_agent_id.clone(),
                self.initial_worker_metadata.fingerprint,
                Some(
                    self.deps
                        .config()
                        .limits
                        .live_stream_event_broadcast_capacity
                        .get(),
                ),
                self.durable_stream_commit(),
                self.worker_service(),
                self.agent_mode(),
            )
            .await
            {
                Ok(producer) => producer,
                Err(error) => {
                    let error = error.into_worker_executor_error(WorkerExecutorError::runtime);
                    return Err(self.deletion_step_failed(error).await);
                }
            };
            let activity = crate::services::activity::ActivityGate::new();
            let guard = activity.try_enter().unwrap();
            let maintenance = std::panic::AssertUnwindSafe(guard.scope(async {
                self.finalize_durable_stream_consumer_dependencies(&producer)
                    .await?;
                let config = &self.deps.config().durable_stream;
                producer
                    .reconcile_attachments_configured(
                        Timestamp::now_utc().to_millis(),
                        config.reconciliation_batch_size,
                        &DbDirectStreamAttachmentConsumerProbe::new(
                            self.worker_service(),
                            self.oplog_service(),
                        ),
                    )
                    .await
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?;
                let probe = DbDirectStreamAttachmentConsumerProbe::new_routed(
                    self.worker_service(),
                    self.oplog_service(),
                    self.rpc(),
                );
                producer
                .cascade_deletion(Timestamp::now_utc().to_millis(), &probe)
                .await
                .map_err(|error| {
                    if let Some(evidence) = error.deletion_blocked_evidence() {
                        WorkerExecutorError::invalid_request(format!(
                            "Worker deletion is blocked by durable stream dependents: {evidence}"
                        ))
                    } else {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    }
                })?;
                let diagnostics = producer.deletion_diagnostics().await.map_err(|error| {
                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                })?;
                debug!(
                    agent_id = %self.owned_agent_id,
                    deleting = diagnostics.deleting,
                    attachments = ?diagnostics.attachments,
                    cascade_completed = ?diagnostics.cascade_completed,
                    "Durable stream deletion cascade completed"
                );
                Ok::<(), WorkerExecutorError>(())
            }))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                Err(WorkerExecutorError::runtime(
                    "Worker deletion maintenance panicked",
                ))
            });
            producer.poison();
            activity.close();
            activity.wait_drained().await;
            producer.wait_durable_drained().await;
            self.state_actor.drain_lifecycle().await?;
            self.durable_stream_commit()(None).await;
            if let Err(error) = maintenance {
                return Err(self.deletion_step_failed(error).await);
            }
            self.complete_deletion_stage(WorkerDeletionStage::StreamsCleaned)
                .await;
        }

        if !self
            .deletion_stage_completed(WorkerDeletionStage::DurableStateRemoved)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::DurableStateRemoved)
                .await?;
            let mut lifecycle = self
                .oplog_service()
                .lock_lifecycle(&self.owned_agent_id.agent_id)
                .await;
            if !self
                .deletion_stage_completed(WorkerDeletionStage::OwnedWorkJoined)
                .await
            {
                self.before_deletion_stage(WorkerDeletionStage::OwnedWorkJoined)
                    .await?;
                let result = self.oplog.stop_and_wait().await;
                // An error is returned only after every owner has finished. Preserve it on
                // this attempt, but do not make its cached result poison explicit retries.
                self.complete_deletion_stage(WorkerDeletionStage::OwnedWorkJoined)
                    .await;
                result.map_err(WorkerExecutorError::runtime)?;
            }
            // Attempted even when the shard was lost while the deletion ran: a revoke alone
            // hands the shard to nobody, and the fenced delete below is what tells the two cases
            // apart. It succeeds while the key is still this executor's, and is refused once
            // another executor has taken it.
            let removed = self
                .worker_service()
                .remove(
                    &mut lifecycle,
                    &self.owned_agent_id,
                    self.initial_worker_metadata.agent_mode,
                    self.initial_worker_metadata.fingerprint,
                    self.oplog.shard_epoch(),
                    &|fingerprint| {
                        self.agent_filesystem_snapshots().delete_all_snapshots(
                            &crate::filesystem_snapshot::AgentSnapshots::agent(
                                &self.owned_agent_id,
                                fingerprint,
                            ),
                            self.initial_worker_metadata.agent_mode,
                        )
                    },
                )
                .await;
            if let Err(error) = removed {
                drop(lifecycle);
                return Err(self.deletion_step_failed(error).await);
            }
            self.complete_deletion_stage(WorkerDeletionStage::DurableStateRemoved)
                .await;
        }

        if !self
            .deletion_stage_completed(WorkerDeletionStage::CacheRemoved)
            .await
        {
            self.before_deletion_stage(WorkerDeletionStage::CacheRemoved)
                .await?;
            if !self.active_agents().remove_worker(self, true).await {
                return Err(WorkerExecutorError::runtime(
                    "Deleting worker lost active-cache authority before retirement",
                ));
            }
            self.complete_deletion_stage(WorkerDeletionStage::CacheRemoved)
                .await;
        }
        Ok(())
    }
    /// The error a failed deletion step ends the attempt with. A step refused because the shard
    /// has a new owner retires the worker for a lost shard and answers with the routing miss that
    /// sends the delete there. The generation stays cached in `Deleting` with the stages it
    /// completed, as any failed attempt does, so a retry here runs no stage again and repeats no
    /// remote effect; it leaves the cache once the shard has left this executor. Any other failure
    /// is returned unchanged.
    pub(in crate::worker) async fn deletion_step_failed(
        self: &Arc<Self>,
        error: WorkerExecutorError,
    ) -> WorkerExecutorError {
        if !self.retire_if_shard_lost(&error) {
            return error;
        }
        if self
            .shard_service()
            .check_worker(&self.owned_agent_id.agent_id)
            .is_err()
        {
            self.evict_failed_deletion().await;
        }
        WorkerExecutorError::ShardingNotReady
    }
    /// Whether this generation holds a deletion whose last attempt failed, with nothing running.
    pub(in crate::worker) async fn deletion_failed(&self) -> bool {
        matches!(
            &*self.instance.lock().await,
            WorkerInstance::Deleting(deleting) if matches!(deleting.handle.result(), Some(Err(_)))
        )
    }
    /// Drops a lost-shard generation whose deletion can no longer finish here. A retry cannot
    /// rebuild it, because the delete is refused at the door once the shard has left.
    pub(in crate::worker) async fn evict_failed_deletion(self: &Arc<Self>) {
        if !self.active_agents().remove_worker(self, true).await {
            debug!(
                agent_id = %self.owned_agent_id,
                "A newer generation replaced this one before its failed deletion was evicted"
            );
        }
    }
    pub(in crate::worker) async fn before_deletion_stage(
        &self,
        stage: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError> {
        if let Some(hook) = Ctx::worker_deletion_hook(&self.extra_deps()) {
            hook.before_stage(&self.owned_agent_id, stage).await?;
        }
        Ok(())
    }
    pub(in crate::worker) async fn finalize_durable_stream_consumer_dependencies(
        &self,
        producer: &DurableStreamStore,
    ) -> Result<(), WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        let mut deleting_recorded = false;
        let mut dependencies = HashMap::new();
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_exact(OplogIndex::INITIAL, current.as_u64())
                .await?
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = self
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                validate_stream_session_record(&record)?;
                match record {
                    StreamSessionRecord::ConsumerDeleting(record)
                        if record.consumer_environment_id == self.owned_agent_id.environment_id
                            && record.consumer == self.owned_agent_id.agent_id
                            && record.consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        deleting_recorded = true;
                    }
                    StreamSessionRecord::TopologyPrepared(record)
                        if record.attachment.consumer_environment_id
                            == self.owned_agent_id.environment_id
                            && record.attachment.consumer == self.owned_agent_id.agent_id
                            && record.attachment.expected_consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        let slot = (
                            record.attachment.producer_environment_id,
                            record.attachment.producer.clone(),
                            record.attachment.attachment_id,
                            record.attachment.stream_id,
                        );
                        if dependencies.get(&slot).is_none_or(
                            |(key, _): &(StreamAttachmentKey, StreamSessionMappingRecord)| {
                                key.epoch <= record.attachment.epoch
                            },
                        ) {
                            dependencies.insert(slot, (record.attachment, record.mapping));
                        }
                    }
                    StreamSessionRecord::TopologyActivated(record)
                        if record.attachment.consumer_environment_id
                            == self.owned_agent_id.environment_id
                            && record.attachment.consumer == self.owned_agent_id.agent_id
                            && record.attachment.expected_consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        let slot = (
                            record.attachment.producer_environment_id,
                            record.attachment.producer.clone(),
                            record.attachment.attachment_id,
                            record.attachment.stream_id,
                        );
                        if dependencies.get(&slot).is_none_or(
                            |(key, _): &(StreamAttachmentKey, StreamSessionMappingRecord)| {
                                key.epoch <= record.attachment.epoch
                            },
                        ) {
                            dependencies.insert(slot, (record.attachment, record.mapping));
                        }
                    }
                    _ => {}
                }
            }
        }
        if !deleting_recorded {
            producer
                .append_session_record(
                    None,
                    StreamSessionRecord::ConsumerDeleting(StreamConsumerDeletingRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        consumer_environment_id: self.owned_agent_id.environment_id,
                        consumer: self.owned_agent_id.agent_id.clone(),
                        consumer_fingerprint: self.initial_worker_metadata.fingerprint,
                        deleting_at_millis: Timestamp::now_utc().to_millis(),
                    }),
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        if dependencies.is_empty() {
            return Ok(());
        }
        let auth_ctx = self.durable_stream_consumer_auth_ctx()?;
        for (key, mapping) in dependencies.into_values() {
            let producer = OwnedAgentId::new(key.producer_environment_id, &key.producer);
            let control = RoutedStreamAttachmentControl::new(self.rpc(), mapping, auth_ctx.clone());
            tokio::time::timeout(
                CONSUMER_DELETION_FINALIZATION_TIMEOUT,
                control.finalize_deleted_consumer(key, Timestamp::now_utc().to_millis()),
            )
            .await
            .map_err(|_| {
                WorkerExecutorError::runtime(format!(
                    "durable stream producer {producer} stayed unavailable while finalizing a deleted consumer's attachment"
                ))
            })?
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        Ok(())
    }
}

impl DeletionHandle {
    pub(in crate::worker) fn new() -> (Self, oneshot::Sender<Result<(), WorkerExecutorError>>) {
        let (sender, receiver) = oneshot::channel();
        let completion = async move {
            receiver.await.unwrap_or_else(|_| {
                Err(WorkerExecutorError::runtime(
                    "Worker deletion task ended before publishing a terminal result",
                ))
            })
        }
        .boxed()
        .shared();
        (Self { completion }, sender)
    }

    pub(in crate::worker) fn result(&self) -> Option<Result<(), WorkerExecutorError>> {
        self.completion.clone().now_or_never()
    }

    pub(crate) async fn wait(&self) -> Result<(), WorkerExecutorError> {
        self.completion.clone().await
    }
}

impl DeleteOutcome {
    pub(crate) fn handle(self) -> DeletionHandle {
        match self {
            Self::Started(handle) | Self::AlreadyDeleting(handle) => handle,
        }
    }
}

impl WorkerInstance {
    pub(in crate::worker) fn ensure_not_deleting(&self) -> Result<(), WorkerExecutorError> {
        if matches!(self, Self::Deleting(_) | Self::StoppedForDeletion) {
            Err(WorkerExecutorError::invalid_request(
                "Worker is being deleted",
            ))
        } else {
            Ok(())
        }
    }

    pub(in crate::worker) fn deletion_owns_retirement(&self) -> bool {
        matches!(self, Self::Deleting(_) | Self::StoppedForDeletion)
    }

    pub(in crate::worker) fn deletion_runtime(&self) -> &Self {
        match self {
            Self::Deleting(deleting) => &deleting.runtime,
            instance => instance,
        }
    }

    pub(in crate::worker) fn deletion_runtime_mut(&mut self) -> &mut Self {
        match self {
            Self::Deleting(deleting) => &mut deleting.runtime,
            instance => instance,
        }
    }

    pub(in crate::worker) fn retained_cleanup_failure(&self) -> Option<&WorkerExecutorError> {
        match self.deletion_runtime() {
            Self::CleanupFailed(error) => Some(error),
            Self::Stopping(stopping) => match &stopping.final_state {
                FinalWorkerState::CleanupFailed(error) => Some(error),
                _ => None,
            },
            _ => None,
        }
    }
}
