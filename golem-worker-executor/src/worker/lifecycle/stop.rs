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
    /// Stops this worker through this handle, whichever generation it is. Nothing public reaches
    /// the stop through an arbitrary handle, and a handle kept past its generation is exactly what
    /// the tests using this need.
    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_stop(&self) {
        self.stop_internal(
            false,
            None,
            UnloadRequest::ordinary(UnloadReason::ExplicitStop),
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            PendingLiveInvocationDisposition::Fail,
        )
        .await;
    }
    pub(in crate::worker) async fn stop_internal(
        &self,
        called_from_invocation_loop: bool,
        fail_pending_invocations: Option<WorkerExecutorError>,
        unload_request: UnloadRequest,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        let startup_error = fail_pending_invocations.clone().unwrap_or_else(|| {
            WorkerExecutorError::unknown("Worker stopped before startup completed")
        });
        let mut instance_guard = self.instance.lock().await;
        let startup_attempt = self.startup_attempt.pending();

        let stop_result = self
            .stop_internal_locked(
                &mut instance_guard,
                called_from_invocation_loop,
                fail_pending_invocations,
                unload_request,
                final_state,
                pending_live_invocations,
            )
            .await;

        // Startup can stop before a prepared Store exists to acknowledge interruption.
        // Keep the lifecycle lock until the retiring execution's receipt is delivered.
        if called_from_invocation_loop {
            acknowledge_no_store_interruption(&mut self.execution_status.write().unwrap(), None);
        }

        // IMPORTANT: drop the lock here as the invocation loop might reenter this method after we drop a running worker.
        drop(instance_guard);

        self.handle_stop_result(stop_result).await;

        // A lost shard found inside the loop - a fence refused in a host call traps with
        // `ShardLost` - is only recorded there; its retirement starts here, once the loop has gone,
        // so the new owner cannot recover the agent while it still runs on this executor. The
        // retirement fails the waiters while this generation still holds the cache entry, then
        // drops it. It is shared, so the passes its own stop and other stops make here join it,
        // and it is spawned because its stop comes back through here.
        if let Some(reason) = self.lost_shard_reason()
            && let Some(worker) = self
                .deps
                .active_agents()
                .try_get_cached(&self.owned_agent_id)
                .await
            && std::ptr::eq(worker.as_ref(), self)
        {
            worker.spawn_lost_shard_retirement(reason);
        }

        if !called_from_invocation_loop && let Some(startup_attempt) = startup_attempt {
            self.complete_startup(startup_attempt, Err(startup_error));
        }
    }
    pub(in crate::worker) async fn acknowledge_startup_interruption(&self, kind: InterruptKind) {
        let _lifecycle = self.instance.lock().await;
        acknowledge_no_store_interruption(&mut self.execution_status.write().unwrap(), Some(kind));
    }
    pub(in crate::worker) async fn stop_internal_locked(
        &self,
        instance_guard: &mut MutexGuard<'_, WorkerInstance>,
        called_from_invocation_loop: bool,
        // Only respected when this is the call that triggered the stop
        fail_pending_invocations: Option<WorkerExecutorError>,
        unload_request: UnloadRequest,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) -> StopResult {
        let runtime = match &mut **instance_guard {
            WorkerInstance::Deleting(deleting) => &mut *deleting.runtime,
            instance => instance,
        };
        let actor_loss_cleanup = self.state_actor.lifecycle_is_closed()
            && (matches!(&final_state, FinalWorkerState::CleanupFailed(_))
                || runtime.retained_cleanup_failure().is_some());
        // Preserve cleanup health even if a later finalization step unwinds. Physical
        // ownership remains with the invocation loop or the existing stopping task.
        let fallback = match (&*runtime, &final_state) {
            (WorkerInstance::CleanupFailed(error), _)
            | (
                WorkerInstance::Stopping(StoppingWorker {
                    final_state: FinalWorkerState::CleanupFailed(error),
                    ..
                }),
                _,
            ) => WorkerInstance::CleanupFailed(error.clone()),
            (_, FinalWorkerState::CleanupFailed(error)) if called_from_invocation_loop => {
                WorkerInstance::CleanupFailed(error.clone())
            }
            _ => WorkerInstance::Unloaded {
                startup_failure: None,
            },
        };
        let previous_instance_state = std::mem::replace(runtime, fallback);

        match previous_instance_state {
            WorkerInstance::Unloaded { .. } => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(error.clone(), actor_loss_cleanup)
                        .await;
                }
                *runtime = final_state.into_instance();
                if let WorkerInstance::Unloaded { startup_failure } = &*runtime {
                    self.resolve_pending_queue_on_unload(
                        startup_failure.as_ref(),
                        pending_live_invocations,
                    )
                    .await;
                }
                StopResult::Stopped
            }
            WorkerInstance::CleanupFailed(error) => {
                if let Some(ref pending_error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(
                        pending_error.clone(),
                        actor_loss_cleanup,
                    )
                    .await;
                }
                *runtime = WorkerInstance::CleanupFailed(error);
                StopResult::Stopped
            }
            WorkerInstance::WaitingForPermit(_) => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(error.clone(), actor_loss_cleanup)
                        .await;
                }
                crate::metrics::workers::dec_worker_waiting_for_memory();
                *runtime = final_state.into_instance();
                match &*runtime {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations,
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                StopResult::Stopped
            }
            WorkerInstance::Deleting(_) | WorkerInstance::StoppedForDeletion => {
                *runtime = previous_instance_state;
                // Should we return an error here?
                StopResult::Stopped
            }
            WorkerInstance::Unresolved | WorkerInstance::Initializing(_) => {
                *runtime = previous_instance_state;
                StopResult::Stopped
            }
            WorkerInstance::Stopping(stopping) if called_from_invocation_loop => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(error.clone(), actor_loss_cleanup)
                        .await;
                }
                let pending_live_invocations = stopping.pending_live_invocations;
                let (instance, notify) = complete_stopping_worker(stopping, final_state);
                *runtime = instance;
                match &*runtime {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations,
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                notify.set();
                StopResult::Stopped
            }
            WorkerInstance::Stopping(mut stopping) => {
                let deleting = matches!(&final_state, FinalWorkerState::Deleting);
                stopping.final_state = merge_final_worker_state(stopping.final_state, final_state);
                if pending_live_invocations == PendingLiveInvocationDisposition::Fail {
                    stopping.pending_live_invocations = PendingLiveInvocationDisposition::Fail;
                }
                if deleting && let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(error.clone(), actor_loss_cleanup)
                        .await;
                }
                let notify = stopping.notify.clone();
                *runtime = WorkerInstance::Stopping(stopping);
                StopResult::AlreadyStopping { notify }
            }
            WorkerInstance::Running(running) => {
                self.owner_runtime_resources.fence_filesystem_generation();
                debug!(
                    "Stopping running worker ({called_from_invocation_loop}) ({})",
                    fail_pending_invocations.is_some()
                );

                // Make sure the oplog is committed. Best-effort: a stop must finish.
                match self.oplog.commit(CommitLevel::Always).await {
                    Ok(_) => {}
                    Err(OplogError::Fenced(fence)) => {
                        // The shard has a new owner. Recording the retirement is synchronous and
                        // takes no lock, so it is safe under the worker lifecycle lock this arm
                        // holds.
                        self.record_retirement(
                            InterruptKind::ShardLost,
                            RetirementReason::Fenced(Some(fence)),
                        );
                    }
                    Err(error) => {
                        warn!(%error, "Committing the oplog while stopping failed");
                    }
                }

                // After the commit: it can be the first write to find that the shard has a new
                // owner, and the waiters are then told to retry there rather than handed the
                // stop's own error, which would be cached as a failure the new owner's run of the
                // invocation could never correct.
                //
                // TODO: fail pending invocations should be factored out of here and be guaranteed to run
                // even if there are multiple concurrent stop attempts.
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations_for_stop(error.clone(), actor_loss_cleanup)
                        .await;
                };

                // Persist any pending cached-status changes synchronously before the worker leaves
                // memory, so a subsequent cold load does not have to re-fold oplog entries that were
                // only reflected in the (deferred) in-memory status. Best-effort: a failure is
                // logged/metered inside `flush` and re-queued; the blob is reconstructable from the
                // oplog, so it must not block the stop.
                //
                // For a lost shard the flusher refuses this itself: it is a key-value write, which
                // is NOT fenced, so it would overwrite the new owner's newer status blob with our
                // stale one.
                if let Err(err) = self
                    .status_flusher
                    .flush(status_flusher::FlushReason::Forced)
                    .await
                {
                    debug!("Forced status flush on stop failed (will retry in background): {err}");
                }

                // when stopping via the invocation loop we can stop immediately, no need to go via the stopping status
                if called_from_invocation_loop {
                    crate::metrics::workers::dec_worker_memory_resident();
                    // The invocation-loop task retains the shared grant cell until it
                    // exits. Release its reservation now so permit reacquisition can
                    // register and admit the replacement generation without overlapping
                    // the old generation's grant.
                    self.release_linear_memory_grant();
                    *runtime = final_state.into_instance();
                    match &*runtime {
                        WorkerInstance::Unloaded { startup_failure } => {
                            self.resolve_pending_queue_on_unload(
                                startup_failure.as_ref(),
                                pending_live_invocations,
                            )
                            .await;
                        }
                        WorkerInstance::CleanupFailed(error) => {
                            self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                                .await;
                        }
                        _ => {}
                    }
                    StopResult::Stopped
                } else {
                    // drop the running worker, this signals to the invocation loop to start exiting.
                    // `stop()` consumes the RunningWorker and drops everything but
                    // its join handle, releasing its memory grant back to the gate.
                    let run_loop_handle = running.stop(unload_request);
                    let notify = OneShotEvent::new();
                    crate::metrics::workers::dec_worker_memory_resident();
                    *runtime = WorkerInstance::Stopping(StoppingWorker {
                        notify: notify.clone(),
                        final_state,
                        pending_live_invocations,
                    });
                    StopResult::NeedsWaitForLoopExit {
                        run_loop_handle,
                        notify,
                    }
                }
            }
        }
    }
    // IMPORTANT: must not be called while holding the worker lifecycle lock
    pub(in crate::worker) async fn handle_stop_result(&self, stop_result: StopResult) {
        match stop_result {
            StopResult::Stopped => {}
            StopResult::AlreadyStopping { notify } => notify.wait().await,
            StopResult::NeedsWaitForLoopExit {
                run_loop_handle,
                notify,
            } => {
                let run_loop_failure = run_loop_handle.await.err().map(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invocation loop task stopped unexpectedly: {error}"
                    ))
                });

                let mut instance_guard = self.instance.lock().await;
                if let Some(error) = run_loop_failure.as_ref() {
                    merge_run_loop_failure(instance_guard.deletion_runtime_mut(), error.clone());
                }
                let actor_loss_cleanup = self.state_actor.lifecycle_is_closed()
                    && instance_guard.retained_cleanup_failure().is_some();
                let is_deleting = match &*instance_guard {
                    WorkerInstance::Stopping(stopping) => {
                        matches!(stopping.final_state, FinalWorkerState::Deleting)
                    }
                    WorkerInstance::Deleting(_) => true,
                    _ => false,
                };

                // After the invocation loop has fully exited, fail any remaining
                // unresolved invocations (e.g. the currently running one that was
                // in progress when deletion was requested).
                if is_deleting {
                    drop(instance_guard);
                    self.fail_pending_invocations_for_stop(
                        WorkerExecutorError::invalid_request("Worker is being deleted"),
                        actor_loss_cleanup,
                    )
                    .await;
                    instance_guard = self.instance.lock().await;
                }

                if let Some(error) = run_loop_failure {
                    drop(instance_guard);
                    self.fail_pending_invocations_for_stop(error, actor_loss_cleanup)
                        .await;
                    instance_guard = self.instance.lock().await;
                }

                let runtime = instance_guard.deletion_runtime_mut();
                let pending_live_invocations = if matches!(&*runtime, WorkerInstance::Stopping(_)) {
                    match std::mem::replace(
                        runtime,
                        WorkerInstance::Unloaded {
                            startup_failure: None,
                        },
                    ) {
                        WorkerInstance::Stopping(stopping) => {
                            let pending_live_invocations = stopping.pending_live_invocations;
                            *runtime = stopping.final_state.into_instance();
                            Some(pending_live_invocations)
                        }
                        _ => unreachable!(),
                    }
                } else {
                    None
                };
                match &*runtime {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations
                                .unwrap_or(PendingLiveInvocationDisposition::Fail),
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                drop(instance_guard);

                notify.set();
            }
        }
    }
    /// Resolves all queued `AwaitReadyToProcessCommands` markers when the worker reaches the
    /// `Unloaded` state without the invocation loop having drained them — for example when the
    /// worker suspends itself mid-invocation. Waiters observe the startup failure if there is
    /// one, otherwise a successful stop. Inspection waiters survive only a recoverable unload;
    /// startup and cleanup failure or a terminal stop must not leave requests waiting for a
    /// loop that will not return.
    pub(in crate::worker) async fn resolve_pending_queue_on_unload(
        &self,
        startup_failure: Option<&WorkerExecutorError>,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        self.resolve_pending_readiness_awaiters_on_stop(startup_failure)
            .await;
        let mut queue = self.queue.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        if pending_live_invocations == PendingLiveInvocationDisposition::Fail && !queue.is_empty() {
            let status = self.get_last_known_status().await;
            let error = Self::ensure_not_failed(
                &self.deps,
                &self.owned_agent_id,
                self.agent_mode(),
                &status,
            )
            .await
            .err()
            .unwrap_or_else(|| {
                WorkerExecutorError::runtime("Worker stopped with queued resident work")
            });
            for invocation in queue.drain(..) {
                invocation.fail(&error);
            }
        }
    }
    pub(in crate::worker) async fn resolve_pending_readiness_awaiters_on_stop(
        &self,
        startup_failure: Option<&WorkerExecutorError>,
    ) {
        let mut queue = self.queue.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        if let Some(error) = startup_failure {
            for invocation in queue.drain(..) {
                invocation.fail(error);
            }
        }
        let items = queue.drain(..).collect::<Vec<_>>();
        for item in items {
            match item {
                QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                    let _ = sender.send(match startup_failure {
                        Some(err) => Err(err.clone()),
                        None => Ok(()),
                    });
                }
                other => queue.push_back(other),
            }
        }
    }
    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_fail_pending_invocations(&self) {
        self.fail_pending_invocations(WorkerExecutorError::runtime("test stop failure"))
            .await;
    }
    pub(in crate::worker) async fn fail_pending_invocations_for_stop(
        &self,
        error: WorkerExecutorError,
        actor_loss_cleanup: bool,
    ) {
        if actor_loss_cleanup {
            let error = if self.retired_for_lost_shard() {
                WorkerExecutorError::ShardingNotReady
            } else {
                error
            };
            for item in self.queue.write().await.drain(..) {
                item.fail(&error);
            }
            // Actor loss cannot supply a fresh FIFO status receipt. Fail resident
            // observers only, leaving persisted outcomes and the result cache untouched.
            let status = self.last_known_status.load_full();
            let mut keys = invocation_keys_to_fail(&status, None, true);
            let mut origins = self.external_invocation_origins.write().await;
            for (key, _) in origins.drain() {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            let results = self.hydrated_invocation_results.read().await;
            for key in keys {
                if !results.is_complete(&key, &status) {
                    self.publish_completion(&key, Err(error.clone()));
                }
            }
            return;
        }
        self.fail_pending_invocations(error).await;
    }
    /// Settles the pending invocations after the terminal entry of an interrupt was not written.
    ///
    /// Refused by a fence, the shard moved during this retirement: nothing is cached for the
    /// invocation the new owner resumes, and its waiters are sent there. Any other failure, such as
    /// an archive layer of an ephemeral oplog that did not store the entries, is this agent's own:
    /// the waiters get the error with its cause, and the agent is not retired for a lost shard.
    pub(in crate::worker) async fn terminal_write_failed(&self, error: OplogError) {
        let error = match terminal_write_failure(error) {
            Err(fence) => {
                self.record_retirement(
                    InterruptKind::ShardLost,
                    RetirementReason::Fenced(Some(fence)),
                );
                WorkerExecutorError::runtime("Worker ownership has retired")
            }
            Ok(error) => error,
        };
        self.fail_pending_invocations(error).await;
    }
    pub(in crate::worker) async fn fail_pending_invocations(&self, error: WorkerExecutorError) {
        // The early completion receipt does not join its fold. Wait before taking locks also
        // needed by success publication, so cleanup cannot turn that receipt into a fold barrier.
        let status = self
            .state_actor
            .observe_status()
            .await
            .unwrap_or_else(|error| panic!("Worker status observation failed: {error}"));
        assert!(
            status.is_some() || self.tasks.actors_stopping(),
            "Worker status actor terminated unexpectedly"
        );
        // A lost shard's pending invocations are not failed, they move: the shard's new owner
        // runs them. So their waiters are told to retry there, whatever stopped this generation,
        // and no result is cached. A cached failure would outlive the stop: a later lookup would
        // answer the key with an `InvocationFailed` the caller does not retry, and the invocation
        // loop, finding the key complete, would cancel the pending invocation - waiting on this
        // very stop to do it, or, with nothing fenced yet, cancelling work the new owner still has
        // to run. This is the one place the retirement's kind maps to the waiters' error.
        let lost_shard = self.retired_for_lost_shard();
        let error = if lost_shard {
            WorkerExecutorError::ShardingNotReady
        } else {
            error
        };
        let queued_items = self.queue.write().await.drain(..).collect::<VecDeque<_>>();

        // Publishing the provided initialization error to all queued internal operations
        for item in queued_items {
            item.fail(&error);
        }

        // A stop continuation can resume after deletion has already drained and closed actors.
        // Its resident waiters still need cleanup, but durable outcome publication is finished.
        let Some(status) = status else {
            return;
        };
        let mut origins = self.external_invocation_origins.write().await;
        let keys_to_fail = invocation_keys_to_fail(&status, None, true);

        let mut invocation_results = self.hydrated_invocation_results.write().await;
        for idempotency_key in &keys_to_fail {
            if invocation_results.is_complete(idempotency_key, &status) {
                continue;
            }
            if lost_shard {
                self.publish_completion(idempotency_key, Err(error.clone()));
                origins.remove(idempotency_key);
                continue;
            }
            invocation_results.insert(
                idempotency_key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: TrapType::Error {
                            error: golem_common::model::oplog::AgentError::Unknown(
                                error.to_string(),
                            ),
                            retry_from: OplogIndex::INITIAL,
                            semantic_trap_retry_override: None,
                        },
                        stderr: String::new(),
                    }),
                },
                status.invocation_results.revert_generation(),
                status.oplog_idx,
            );
            self.publish_completion(idempotency_key, Err(error.clone()));
            origins.remove(idempotency_key);
        }
    }
    // Lock a worker not in stopping state.
    pub(in crate::worker) async fn lock_non_stopping_worker(
        &self,
    ) -> MutexGuard<'_, WorkerInstance> {
        loop {
            let instance_guard = self.instance.lock().await;

            match &*instance_guard {
                WorkerInstance::Stopping(stopping) => {
                    let notify = stopping.notify.clone();
                    drop(instance_guard);
                    notify.wait().await;
                }
                _ => return instance_guard,
            }
        }
    }
    pub(in crate::worker) async fn lock_non_stopping_worker_owned(
        &self,
    ) -> OwnedMutexGuard<WorkerInstance> {
        loop {
            let instance_guard = self.instance.clone().lock_owned().await;

            match &*instance_guard {
                WorkerInstance::Stopping(stopping) => {
                    let notify = stopping.notify.clone();
                    drop(instance_guard);
                    notify.wait().await;
                }
                _ => return instance_guard,
            }
        }
    }
    pub(in crate::worker) async fn restart_with_memory_admission(
        this: Arc<Worker<Ctx>>,
        called_from_invocation_loop: bool,
        delay: Option<Duration>,
        oom_retry_count: u32,
        start_attempt: Option<Uuid>,
        reason: UnloadReason,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        this.stop_internal(
            called_from_invocation_loop,
            None,
            UnloadRequest::ordinary(reason),
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            PendingLiveInvocationDisposition::Preserve,
        )
        .await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Self::start_if_needed_internal(this, oom_retry_count, start_attempt).await
    }
}

/// What a failed terminal entry of an interrupt means for the pending invocations: the fence of a
/// lost shard as `Err`, or the error, with its cause, that the waiters get.
pub(in crate::worker) fn terminal_write_failure(
    error: OplogError,
) -> Result<WorkerExecutorError, OplogFence> {
    match error {
        OplogError::Fenced(fence) => Err(fence),
        error => Ok(WorkerExecutorError::from(error)),
    }
}

pub(in crate::worker) fn acknowledge_no_store_interruption(
    status: &mut ExecutionStatus,
    expected_kind: Option<InterruptKind>,
) {
    if let ExecutionStatus::Interrupting {
        interrupt_kind,
        agent_mode,
        await_interruption,
        ..
    } = status
        && expected_kind.is_none_or(|kind| kind == *interrupt_kind)
    {
        let receipt = await_interruption.clone();
        *status = ExecutionStatus::Suspended {
            agent_mode: *agent_mode,
            timestamp: Timestamp::now_utc(),
        };
        let _ = receipt.send(());
    }
}

pub(in crate::worker) fn merge_final_worker_state(
    current: FinalWorkerState,
    requested: FinalWorkerState,
) -> FinalWorkerState {
    match (&current, &requested) {
        (FinalWorkerState::CleanupFailed(_), _) => current,
        (_, FinalWorkerState::CleanupFailed(_)) => requested,
        (FinalWorkerState::Deleting, _) => current,
        (_, FinalWorkerState::Deleting) => requested,
        (
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            FinalWorkerState::Unloaded {
                startup_failure: Some(_),
            },
        ) => requested,
        _ => current,
    }
}

pub(in crate::worker) fn merge_run_loop_failure(
    instance: &mut WorkerInstance,
    error: WorkerExecutorError,
) {
    match instance {
        WorkerInstance::Stopping(stopping) => {
            stopping.final_state = merge_final_worker_state(
                std::mem::replace(
                    &mut stopping.final_state,
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                ),
                FinalWorkerState::CleanupFailed(error),
            );
        }
        WorkerInstance::CleanupFailed(_) => {}
        _ => *instance = WorkerInstance::CleanupFailed(error),
    }
}

pub(in crate::worker) fn complete_stopping_worker(
    mut stopping: StoppingWorker,
    final_state: FinalWorkerState,
) -> (WorkerInstance, OneShotEvent) {
    stopping.final_state = merge_final_worker_state(stopping.final_state, final_state);
    let notify = stopping.notify;
    (stopping.final_state.into_instance(), notify)
}

pub(in crate::worker) fn invocation_keys_to_fail(
    status: &AgentStatusRecord,
    first_key: Option<&IdempotencyKey>,
    include_pending_and_current: bool,
) -> Vec<IdempotencyKey> {
    let mut keys = Vec::new();

    if let Some(key) = first_key {
        keys.push(key.clone());
    }

    if !include_pending_and_current {
        return keys;
    }

    for pending_key in status
        .pending_invocations
        .iter()
        .filter_map(|entry| entry.idempotency_key())
    {
        if !keys.contains(pending_key) {
            keys.push(pending_key.clone());
        }
    }

    if let Some(current_key) = &status.current_idempotency_key
        && !keys.contains(current_key)
    {
        keys.push(current_key.clone());
    }

    keys
}

impl FinalWorkerState {
    pub(in crate::worker) fn into_instance(self) -> WorkerInstance {
        match self {
            FinalWorkerState::Unloaded { startup_failure } => {
                WorkerInstance::Unloaded { startup_failure }
            }
            FinalWorkerState::CleanupFailed(error) => WorkerInstance::CleanupFailed(error),
            FinalWorkerState::Deleting => WorkerInstance::StoppedForDeletion,
        }
    }
}

impl UnloadReason {
    pub(in crate::worker) fn release_cause(self) -> crate::metrics::resource_release::Cause {
        use crate::metrics::resource_release::Cause;
        match self {
            Self::Deleting => Cause::Deleting,
            Self::ExplicitStop => Cause::ExplicitStop,
            Self::Failure => Cause::Failure,
            Self::FilesystemLimit => Cause::FilesystemLimit,
            Self::FilesystemPressure => Cause::FilesystemPressure,
            Self::Idle => Cause::Idle,
            Self::Interrupt => Cause::Interrupt,
            Self::MemoryLimit => Cause::MemoryLimit,
            Self::MemoryPressure => Cause::MemoryPressure,
            Self::OutOfMemory => Cause::OutOfMemory,
            Self::Panic => Cause::Panic,
            Self::Restart => Cause::Restart,
            Self::ShardLost => Cause::ShardLost,
            Self::Suspend => Cause::Suspend,
        }
    }

    pub(in crate::worker) fn from_interrupt(kind: InterruptKind) -> Self {
        match kind {
            InterruptKind::Restart | InterruptKind::Jump => Self::Restart,
            InterruptKind::Suspend(_) => Self::Suspend,
            InterruptKind::Interrupt(_) => Self::Interrupt,
            InterruptKind::ShardLost => Self::ShardLost,
        }
    }
}

impl UnloadRequest {
    pub(crate) fn new(reason: UnloadReason, deadline: Instant) -> Self {
        Self { reason, deadline }
    }

    pub(in crate::worker) fn ordinary(reason: UnloadReason) -> Self {
        Self::new(reason, Instant::now() + Duration::from_secs(30))
    }
}
