use super::*;

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Finalizes a taken interrupt without a resident Store, retaining the caller's retry
    /// decision. Returns whether the loop exits.
    pub(super) async fn handle_unloaded_interrupt(
        &self,
        interrupt: PendingWorkerInterrupt,
        retry_was_live: bool,
        decision: RetryDecision,
    ) -> bool {
        let kind = interrupt.kind;
        debug!(
            ?decision,
            "Invocation queue loop interrupted while unloaded"
        );
        if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
            let status = self.parent.get_last_known_status().await;
            let marker = match kind {
                InterruptKind::EntityCancellation => {
                    unreachable!("entity cancellation cannot interrupt the owner")
                }
                InterruptKind::Suspend(_) => Some(OplogEntry::suspend()),
                InterruptKind::Interrupt(_) => Some(OplogEntry::interrupted()),
                InterruptKind::Restart | InterruptKind::Jump | InterruptKind::ShardLost => None,
            };
            let recorded = match marker {
                Some(entry) => self
                    .parent
                    .add_and_commit_oplog(entry)
                    .await
                    .map(|_| ())
                    .map_err(WorkerExecutorError::from),
                None => Ok(()),
            };
            // Refusal retires the shard; unfinished or lost writer progress stays failed
            // cleanup. Neither path invents an invocation result or restarts in place.
            if let Err(error) = recorded {
                if self.parent.retire_if_shard_lost(&error) {
                    self.stop_startup_retired().await;
                } else {
                    self.stop_cleanup_failed(error).await;
                }
                return true;
            }
            if matches!(kind, InterruptKind::Interrupt(_))
                && let Some(key) = status.current_idempotency_key.clone()
            {
                self.parent
                    .store_invocation_failure(&key, &TrapType::Interrupt(kind))
                    .await;
                self.parent.event_service().emit_invocation_finished(
                    "interrupted while unloaded",
                    &key,
                    retry_was_live,
                );
            } else if matches!(kind, InterruptKind::Interrupt(_))
                && retry_was_live
                && self.parent.agent_mode() == AgentMode::Ephemeral
            {
                // The idle-stop adapter must still notify an accepted ephemeral caller
                // waiting behind a completed constructor, without replacing that result
                // or recording an execution of the pending method.
                let status = self.parent.get_last_known_status().await;
                if let Some(key) = status
                    .pending_invocations
                    .iter()
                    .find_map(|pending| pending.idempotency_key().cloned())
                {
                    self.parent
                        .store_invocation_failure(&key, &TrapType::Interrupt(kind))
                        .await;
                }
            }
        }
        match decision {
            RetryDecision::Immediate => {
                self.parent.acknowledge_startup_interruption(kind).await;
                false
            }
            RetryDecision::None => {
                self.stop_closed(None, None, PendingLiveInvocationDisposition::Fail)
                    .await;
                true
            }
            RetryDecision::TryStop(timestamp) => {
                if timestamp < *self.parent.last_resume_request.lock().await {
                    self.release_terminal_interrupt().await;
                    false
                } else {
                    self.stop_closed(
                        None,
                        None,
                        resident_work_disposition(interrupt.unload_request.reason),
                    )
                    .await;
                    true
                }
            }
            RetryDecision::Delayed(_) | RetryDecision::ReacquirePermits => {
                unreachable!("queued interrupts do not delay or reacquire permits")
            }
        }
    }

    pub(super) async fn stop_unloaded(
        &self,
        startup_failure: Option<WorkerExecutorError>,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        if !self.parent.state_actor.lifecycle_is_closed()
            && let Err(error) = self.parent.commit_pending_terminal_stop().await
        {
            self.stop_cleanup_failed(error).await;
            return;
        }
        let owner_failure = startup_failure
            .clone()
            .map(OwnerFailureWinner::Infrastructure);
        let enforcement_failure = self.parent.take_monthly_policy_failure();
        // Report the outage to current waiters without storing it as a startup failure.
        let failure_for_waiters = startup_failure.clone().or(enforcement_failure);
        self.parent.complete_startup(
            self.start_attempt,
            Err(failure_for_waiters.clone().unwrap_or_else(|| {
                WorkerExecutorError::unknown("Worker stopped before startup completed")
            })),
        );
        if let Some(active_agent) = self.parent.active_agent_for_this_owner().await {
            let failure = if self.parent.retired_for_lost_shard() {
                Some(OwnerFailureWinner::Lifecycle(InterruptKind::ShardLost))
            } else {
                owner_failure
            };
            match failure {
                Some(failure) => active_agent.fence_entity_bodies(failure).await,
                None => active_agent.fence_entity_bodies_for_unload().await,
            }
        }
        self.parent
            .stop_internal(
                true,
                failure_for_waiters,
                UnloadRequest::ordinary(UnloadReason::ExplicitStop),
                FinalWorkerState::Unloaded { startup_failure },
                pending_live_invocations,
            )
            .await;
    }

    pub(super) async fn stop_cleanup_failed(&self, error: WorkerExecutorError) {
        self.parent
            .complete_startup(self.start_attempt, Err(error.clone()));
        if let Some(active_agent) = self.parent.active_agent_for_this_owner().await {
            active_agent
                .fence_entity_bodies(OwnerFailureWinner::Infrastructure(error.clone()))
                .await;
        }
        let pending_failure = error.clone();
        self.parent
            .stop_internal(
                true,
                Some(pending_failure),
                UnloadRequest::ordinary(UnloadReason::Failure),
                FinalWorkerState::CleanupFailed(error),
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
    }

    pub(super) async fn stop_closed(
        &self,
        cleanup_error: Option<WorkerExecutorError>,
        startup_failure: Option<WorkerExecutorError>,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        publish_unload_outcome(
            cleanup_error,
            startup_failure,
            |error| async move { self.stop_cleanup_failed(error).await },
            |startup_failure| async move {
                self.stop_unloaded(startup_failure, pending_live_invocations)
                    .await
            },
        )
        .await;
    }

    pub(super) fn unload_running_agent<Runtime, Adapter>(
        agent: RunningAgent<Runtime, Adapter>,
        reason: UnloadReason,
        deadline: Instant,
        permit_state: &mut ConcurrentAgentPermitState<
            crate::services::active_agents::ConcurrentAgentPermit,
        >,
        filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
        before_delete: Option<BoxFuture<'static, ()>>,
    ) -> UnloadObserver
    where
        Runtime: Send + 'static,
        Adapter: SandboxFilesystemAdapter,
    {
        let RunningAgent {
            runtime,
            filesystem,
        } = agent;
        unload_resident_agent_ownership(
            ResidentAgentOwnership {
                runtime,
                filesystem,
            },
            reason,
            deadline,
            permit_state,
            filesystem_activity,
            before_delete,
        )
    }

    pub(super) fn archive_ephemeral_oplog(&self) {
        let worker = self.parent.clone();
        let shutdown_worker = worker.clone();
        let invocation_loops = worker.active_agents().invocation_loops();
        invocation_loops.spawn(async move {
            let Some(archival) = worker
                .durable_stream_producer
                .wait_for_responses_and_fence()
                .await else { return; };
            let _lifecycle = worker
                .oplog_service()
                .lock_lifecycle(&worker.owned_agent_id.agent_id)
                .await;
            let forwarding = downcast_oplog::<ForwardingOplog>(&worker.oplog);
            let mut cleanup = worker.owner_cleanup.lock().await;
            if *cleanup != crate::worker::OwnerCleanupState::PreRemoval
                || !worker.is_current_cached_owner().await
                || worker.deletion_owns_retirement().await
            {
                archival.send_replace(Some(Err(
                    crate::durable_host::durable_stream::StreamStoreError::RecoveryRequired,
                )));
                return;
            }
            let result: Result<(), WorkerExecutorError> = async {
                worker
                    .quiesce_for_owner_retirement(None, forwarding.as_deref())
                    .await?;
                loop {
                    match EphemeralOplog::try_archive_blocking(&worker.oplog).await {
                        Ok(Some(true)) => {}
                        Ok(_) => break,
                        Err(error) => {
                            tracing::warn!(
                                agent_id = %worker.agent_id(),
                                error = %error,
                                "Failed to archive ephemeral oplog during retirement; the source remains available for a later sweep"
                            );
                            break;
                        }
                    }
                }
                worker.remove_from_active_agents().await;
                *cleanup = crate::worker::OwnerCleanupState::Retired;
                Ok(())
            }
            .await;
            archival.send_replace(Some(result.clone().map_err(|error| {
                crate::durable_host::durable_stream::StreamStoreError::Oplog(error.to_string())
            })));
            if let Err(error) = result {
                tracing::error!(agent_id = %worker.agent_id(), error = %error, "Failed to retire ephemeral worker before archival");
            }
        }, move || {
            let retirement = shutdown_worker.durable_stream_producer.shutdown();
            let agent_id = shutdown_worker.agent_id();
            Box::pin(async move {
                if let Err(error) = retirement.await {
                    tracing::error!(agent_id = %agent_id, error = %error, "Failed to drain ephemeral streams during executor shutdown");
                }
            })
        });
    }
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Suspends the worker after the invocation loop exited
    pub(super) async fn suspend_worker(&self, store: &Mutex<Store<Ctx>>) {
        async {
            // Marking the worker as suspended
            store.lock().await.data().set_suspended();
            if self.parent.state_actor.lifecycle_is_closed() {
                return;
            }

            // Making sure all pending commits are flushed
            // Make sure all pending commits are done
            let worker = store.lock().await.data().get_public_state().worker();
            // A failed commit is a refused one, and gives the agent up.
            let _ = worker
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            // The worker is going idle; persist its cached status synchronously now instead of leaving
            // it for the next background sweep, so reads of an idle worker see an up-to-date blob.
            worker.force_flush_status().await;

            // Idle is a structurally clean boundary (the invocation loop has exited and committed, so
            // no jumpable region is open): write a throttled clean status checkpoint so a later
            // jump-induced recompute can fold forward from here.
            worker
                .checkpoint_status(status_checkpointer::CheckpointReason::Idle)
                .await;
        }
        .instrument(agent_phase_span!(self, "suspend_worker"))
        .await
    }
}

pub(super) async fn publish_unload_outcome<T>(
    cleanup_error: Option<WorkerExecutorError>,
    startup_failure: Option<WorkerExecutorError>,
    cleanup_failed: impl AsyncFnOnce(WorkerExecutorError) -> T,
    unloaded: impl AsyncFnOnce(Option<WorkerExecutorError>) -> T,
) -> T {
    match cleanup_error {
        Some(error) => cleanup_failed(error).await,
        None => unloaded(startup_failure).await,
    }
}

pub(super) async fn finish_filesystem_limit_unload(
    suspend: bool,
    unload: impl Future<Output = Option<WorkerExecutorError>>,
    record_suspend: impl AsyncFnOnce(),
) -> Option<WorkerExecutorError> {
    let cleanup_failure = unload.await;
    if suspend && cleanup_failure.is_none() {
        record_suspend().await;
    }
    cleanup_failure
}

pub(super) fn unload_resident_agent_ownership<Runtime, Adapter>(
    ownership: ResidentAgentOwnership<Runtime, Adapter>,
    reason: UnloadReason,
    deadline: Instant,
    permit_state: &mut ConcurrentAgentPermitState<
        crate::services::active_agents::ConcurrentAgentPermit,
    >,
    filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    before_delete: Option<BoxFuture<'static, ()>>,
) -> UnloadObserver
where
    Runtime: Send + 'static,
    Adapter: SandboxFilesystemAdapter,
{
    let ResidentAgentOwnership {
        runtime,
        filesystem,
    } = ownership;
    unload_sealed_agent_ownership(
        SealedAgentOwnership {
            runtime,
            filesystem: seal(filesystem),
        },
        reason,
        deadline,
        permit_state,
        filesystem_activity,
        before_delete,
    )
}

pub(super) fn unload_sealed_agent_ownership<Runtime, Adapter>(
    ownership: SealedAgentOwnership<Runtime, Adapter>,
    reason: UnloadReason,
    deadline: Instant,
    permit_state: &mut ConcurrentAgentPermitState<
        crate::services::active_agents::ConcurrentAgentPermit,
    >,
    filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    before_delete: Option<BoxFuture<'static, ()>>,
) -> UnloadObserver
where
    Runtime: Send + 'static,
    Adapter: SandboxFilesystemAdapter,
{
    use crate::metrics::resource_release::{Failure, Stage};
    let scope = permit_state.release_scope.clone();
    let cleanup_receipt = scope.unload(reason.release_cause());
    ownership.filesystem.observe_release(&scope);
    ownership.filesystem.accept_deletion_observation();
    let drained = scope.receipt(Stage::FilesystemDrained);
    let window = permit_state.take_window();
    let mut permit = permit_state.take_permit();
    if let Some(permit) = &mut permit {
        permit.observe_release(&scope);
    }
    scope.seal();
    let permit_held = Arc::clone(&permit_state.held);
    filesystem_activity.lock().unwrap().take();
    let cleanup_scope = scope.clone();
    spawn_module_owned_unload_continuation(Some(cleanup_scope), move |completion| async move {
        let SealedAgentOwnership {
            runtime,
            filesystem,
        } = ownership;
        debug!(?reason, "Unloading resident agent");
        drop(runtime);

        let cleanup = async {
            drain_sealed_filesystem(&filesystem).await;
            drained.complete();
            let (window, close_error) = match window {
                Some(window) => {
                    let (window, error) = window.prepare_disposal().await;
                    (Some(window), error)
                }
                None => (None, None),
            };
            let settlement = match window {
                Some(window) => crate::services::resource_usage_metering::close_window(
                    window,
                    resource_usage_close_deadline(),
                )
                .await
                .map(|_| ())
                .inspect_err(|error| {
                    scope.fail(Stage::PermitReleased, error.release_failure());
                })
                .map_err(|error| WorkerExecutorError::runtime(error.to_string())),
                None => Ok(()),
            };
            drop(permit);
            permit_held.store(false, Ordering::Release);
            let close_error = close_error.or_else(|| settlement.err());
            if let Some(error) = &close_error {
                completion.complete(Some(error.clone()));
            }

            if let Some(before_delete) = before_delete {
                before_delete.await;
            }
            match delete_unloaded_filesystem(filesystem).await {
                Ok(()) => {
                    cleanup_receipt.complete();
                    close_error
                        .map_or(Ok(()), Err)
                        .map_err(UnloadCleanupFailure::Other)
                }
                Err(failure) => Err(UnloadCleanupFailure::Filesystem {
                    failure,
                    close_error,
                    release_receipt: Some(cleanup_receipt),
                }),
            }
        };
        tokio::pin!(cleanup);
        tokio::select! {
            result = &mut cleanup => result,
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                scope.fail(Stage::EndToEnd, Failure::Deadline);
                completion.complete(Some(unload_deadline_error(reason)));
                cleanup.await
            }
        }
    })
}

pub async fn cleanup_startup_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
    window: ExecutionWindow,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    let held = Arc::new(AtomicBool::new(true));
    let mut permits = ConcurrentAgentPermitState::new(None, held);
    permits.install_window(window);
    let observer = unload_sealed_agent_ownership(
        SealedAgentOwnership {
            runtime: (),
            filesystem,
        },
        UnloadReason::Failure,
        resource_usage_close_deadline(),
        &mut permits,
        &Arc::new(StdMutex::new(None)),
        None,
    );
    let cleanup = observer.cleanup.completion.clone().await.err();
    CreateWorkerInstanceError::Failed {
        error: combine_unload_errors(
            startup_error,
            cleanup.as_ref().map(UnloadCleanupFailure::error),
        ),
        filesystem_cleanup_failure: cleanup,
    }
}

pub(super) fn combine_unload_errors(
    primary: WorkerExecutorError,
    secondary: Option<WorkerExecutorError>,
) -> WorkerExecutorError {
    secondary.map_or(primary.clone(), |secondary| {
        WorkerExecutorError::runtime(format!("{primary}; {secondary}"))
    })
}

pub(super) fn unload_deadline_error(reason: UnloadReason) -> WorkerExecutorError {
    WorkerExecutorError::runtime(format!("agent unload deadline reached for {reason:?}"))
}

pub(super) async fn delete_unloaded_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
) -> Result<(), DeleteFailure> {
    crate::services::agent_filesystem::delete(filesystem)
        .await
        .inspect_err(|error| {
            error!(error = %error.source, "Failed to delete agent runtime filesystem");
        })
}

pub(super) async fn catch_invocation_loop_panic<T>(
    future: impl Future<Output = T>,
) -> Result<T, WorkerExecutorError> {
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|panic| {
            let message = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            WorkerExecutorError::runtime(format!("invocation loop panicked: {message}"))
        })
}

pub async fn run_invocation_loop_task<T>(
    future: impl Future<Output = T>,
    on_panic: impl AsyncFnOnce(WorkerExecutorError),
) {
    if let Err(error) = catch_invocation_loop_panic(future).await {
        on_panic(error).await;
    }
}

impl UnloadCleanupFailure {
    pub(super) fn error(&self) -> WorkerExecutorError {
        match self {
            Self::Other(error) => error.clone(),
            Self::Filesystem {
                failure,
                close_error,
                ..
            } => combine_unload_errors(
                WorkerExecutorError::runtime(failure.source.to_string()),
                close_error.clone(),
            ),
        }
    }
}

impl UnloadCleanup {
    pub(super) fn new(
        release_scope: Option<crate::metrics::resource_release::ReleaseScope>,
    ) -> (Self, Sender<Result<(), UnloadCleanupFailure>>) {
        let (sender, receiver) = oneshot::channel();
        let completion = async move {
            receiver.await.unwrap_or_else(|_| {
                Err(UnloadCleanupFailure::Other(WorkerExecutorError::runtime(
                    "module-owned agent cleanup stopped without final completion",
                )))
            })
        }
        .boxed()
        .shared();
        (
            Self {
                completion,
                release_scope,
            },
            sender,
        )
    }

    pub(super) fn with_failure(&self, error: WorkerExecutorError) -> Self {
        let completion = self.completion.clone();
        Self {
            release_scope: self.release_scope.clone(),
            completion: async move {
                Err(match completion.await {
                    Err(UnloadCleanupFailure::Filesystem {
                        failure,
                        close_error,
                        release_receipt,
                    }) => UnloadCleanupFailure::Filesystem {
                        failure,
                        close_error: Some(combine_unload_errors(error, close_error)),
                        release_receipt,
                    },
                    Err(UnloadCleanupFailure::Other(other)) => {
                        UnloadCleanupFailure::Other(combine_unload_errors(error, Some(other)))
                    }
                    Ok(()) => UnloadCleanupFailure::Other(error),
                })
            }
            .boxed()
            .shared(),
        }
    }

    pub async fn wait(&self) -> Result<(), WorkerExecutorError> {
        self.completion
            .clone()
            .await
            .map_err(|failure| failure.error())
    }

    /// Joins pending cleanup or retries its failed owning component, without cancelling it when
    /// this observer is dropped. The worker serializes explicit deletion attempts.
    pub fn retry(&self) -> Self {
        let previous = self.completion.clone();
        let (cleanup, sender) = Self::new(self.release_scope.clone());
        let scope = self.release_scope.clone();
        tokio::spawn(async move {
            let result = AssertUnwindSafe(async move {
                match previous.await {
                    Err(UnloadCleanupFailure::Filesystem {
                        failure,
                        close_error,
                        release_receipt,
                    }) => match failure.retry().await {
                        Ok(()) => {
                            if let Some(receipt) = release_receipt {
                                receipt.complete();
                            }
                            close_error
                                .map_or(Ok(()), Err)
                                .map_err(UnloadCleanupFailure::Other)
                        }
                        Err(failure) => Err(UnloadCleanupFailure::Filesystem {
                            failure,
                            close_error,
                            release_receipt,
                        }),
                    },
                    result => result,
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                if let Some(scope) = &scope {
                    scope.fail(
                        crate::metrics::resource_release::Stage::EndToEnd,
                        crate::metrics::resource_release::Failure::Panic,
                    );
                }
                Err(UnloadCleanupFailure::Other(WorkerExecutorError::runtime(
                    "module-owned agent cleanup retry panicked",
                )))
            });
            let _ = sender.send(result);
        });
        cleanup
    }
}
impl UnloadCompletion {
    pub(super) fn complete(&self, result: Option<WorkerExecutorError>) -> bool {
        let Some(sender) = self.sender.lock().unwrap().take() else {
            return false;
        };
        let _ = sender.send(result);
        true
    }
}

impl Future for UnloadObserver {
    type Output = Option<WorkerExecutorError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::pin::Pin::new(&mut self.receiver).poll(context) {
            std::task::Poll::Ready(Ok(result)) => std::task::Poll::Ready(result),
            std::task::Poll::Ready(Err(_)) => std::task::Poll::Ready(Some(
                WorkerExecutorError::runtime("module-owned agent unload task stopped unexpectedly"),
            )),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

#[cfg(test)]
pub(super) fn spawn_module_owned_unload(
    task: impl Future<Output = Option<WorkerExecutorError>> + Send + 'static,
) -> UnloadObserver {
    spawn_module_owned_unload_continuation(None, move |_completion| async move {
        task.await
            .map_or(Ok(()), |error| Err(UnloadCleanupFailure::Other(error)))
    })
}

pub(super) fn spawn_module_owned_unload_continuation<Task, TaskFuture>(
    scope: Option<crate::metrics::resource_release::ReleaseScope>,
    task: Task,
) -> UnloadObserver
where
    Task: FnOnce(UnloadCompletion) -> TaskFuture + Send + 'static,
    TaskFuture: Future<Output = Result<(), UnloadCleanupFailure>> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let completion = UnloadCompletion {
        sender: Arc::new(StdMutex::new(Some(sender))),
    };
    let completion_after_task = completion.clone();
    let (cleanup, final_sender) = UnloadCleanup::new(scope.clone());
    tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(async move { task(completion).await })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                if let Some(scope) = &scope {
                    scope.fail(
                        crate::metrics::resource_release::Stage::EndToEnd,
                        crate::metrics::resource_release::Failure::Panic,
                    );
                }
                let error = WorkerExecutorError::runtime("module-owned agent unload task panicked");
                error!(error = %error, "Module-owned agent unload task panicked");
                Err(UnloadCleanupFailure::Other(error))
            });
        completion_after_task.complete(result.as_ref().err().map(UnloadCleanupFailure::error));
        let _ = final_sender.send(result);
    });
    UnloadObserver { receiver, cleanup }
}
