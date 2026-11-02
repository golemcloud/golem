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
use crate::durable_host::tail_work::TailActivity;
use wasmtime::component::AccessorTask;

impl DropPolicy for Cancellable {
    const CANCEL_ON_TOOL_TRAP: bool = true;

    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            call.notify_span_closed();
            let _ = sink.send(DropEvent::UnfinishedCancellable { call });
        } else {
            // Production always attaches the worker's dropped-call event sink; a missing sink
            // means a test fixture without one, and the drop is only logged.
            let start_idx = call.start_idx;
            tracing::warn!(
                "durable call {start_idx} dropped unfinished with no drop-event sink attached; no Cancelled entry will be recorded"
            );
        }
    }
}

impl DropPolicy for LeaveIncompleteOnDrop {
    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, _sink: Option<&UnboundedSender<DropEvent>>) {
        tracing::debug!(
            start_idx = %call.start_idx(),
            "durable call dropped unfinished; leaving committed Start incomplete for replay"
        );
    }
}

impl DropPolicy for NotCancellable {
    const CANCEL_ON_TOOL_TRAP: bool = true;

    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            let _ = sink.send(DropEvent::UnfinishedNotCancellable { call });
        } else if cfg!(debug_assertions) && !std::thread::panicking() {
            let start_idx = call.start_idx;
            panic!("non-cancellable durable call {start_idx} dropped without finish/cancel");
        } else {
            let start_idx = call.start_idx;
            tracing::error!(
                "non-cancellable durable call {start_idx} dropped without finish/cancel"
            );
        }
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// Drops a session whose owned start finished after the guest dropped the call. A live
    /// session runs its drop policy. A replayed session is dropped at the point the guest
    /// dropped the call in the recorded run, which is expected.
    pub(super) fn discard_after_guest_drop(mut self) {
        if !self.is_live {
            self.parked_undelivered_replay = true;
        }
    }

    /// Snapshots everything a cancellation recorder / terminal guard needs from this live
    /// persisted call into a self-contained [`DroppedCall`]. `Drop` cannot await, use an
    /// `Accessor`, or borrow worker state, so the snapshot must carry every call-owned fact
    /// needed for the deferred oplog/state work. `live_call_permit` is passed explicitly
    /// because ownership differs by site: terminal paths clone it (the handle stays alive with
    /// its own permit until the terminal completes), while `Drop` moves it into the drop event
    /// so the call stays counted as in-flight until the event is drained.
    /// Snapshot of the two teardown signals (process-equivalent executor shutdown and the
    /// owning runtime's store teardown) for guards and delivery tokens that may outlive the
    /// session.
    pub(super) fn teardown_probe(&self) -> TeardownProbe {
        TeardownProbe::new(
            self.executor_shutdown.clone(),
            self.runtime_teardown.clone(),
        )
    }

    pub(super) fn dropped_call_snapshot(
        &self,
        live_call_permit: Option<LiveCallPermit>,
    ) -> DroppedCall {
        DroppedCall {
            start_idx: self.start_idx,
            function_type: self.retry.function_type().clone(),
            request_upload: self.request_upload.clone(),
            entity_admission: self.entity_admission,
            trap_context: self.trap_context(),
            live_call_permit,
            span_observer: self.span_observer.clone(),
            span_finished: self.cancellation_span_id.as_ref().map(|span_id| {
                golem_common::model::oplog::SpanFinished {
                    span_id: span_id.clone(),
                    finished_at: Timestamp::now_utc(),
                    outcome: golem_common::model::oplog::SpanOutcome::Cancelled,
                }
            }),
        }
    }

    /// Cancels a call.
    ///
    /// Live: append a `Cancelled` entry. Replay: expect the call to resolve as `Cancelled`. This
    /// call's own retry grouping stays with its call-owned `execution_scope` (and the semantic-trap
    /// error marker); the cancel path no longer touches the ambient/global retry-point fallback.
    pub async fn cancel<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.cancel_impl(ctx, partial)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn cancel_impl<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError> {
        // Terminal: mark finished up front so a fallible step below does not drop the handle as
        // "unfinished".
        self.finished = true;
        if self.is_live {
            if self.persisted {
                let oplog = ctx.state.oplog.clone();
                let dropped_call = self.dropped_call_snapshot(self.live_call_permit.clone());
                // As in `complete`: surface a deferred request-upload failure at the call site before
                // recording the `Cancelled` that references the request. A no-op when the request was
                // inline or eagerly uploaded.
                dropped_call.wait_request_upload().await?;
                let partial_payload = upload_partial_response(&oplog, partial).await?;
                dropped_call
                    .append_cancelled_with_oplog(oplog, partial_payload)
                    .await?;
                DurableCallCoordinator::new(ctx)
                    .finish(self.retry.function_type(), false)
                    .await?;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            expect_cancelled_resolution(&resolution)?;
            DurableCallCoordinator::new(ctx)
                .finish(self.retry.function_type(), false)
                .await?;
        }
        Ok(())
    }

    /// Accessor-window cancellation for p3 durable calls.
    pub async fn cancel_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.cancel_access_impl(store, get_ctx, partial)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn cancel_access_impl<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("cancel_access")?;
        self.finished = true;
        let function_type = self.retry.function_type().clone();
        if self.is_live {
            if self.persisted {
                let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
                let mut guard = self.terminal_guard();
                let result = Self::persist_cancellation(oplog, &mut guard, partial).await;
                if let Err(err) = result {
                    guard.disarm();
                    return Err(err);
                }
                end_durable_function_access(store, get_ctx, function_type, false).await?;
                guard.disarm();
            }
        } else {
            let replay_state =
                store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
            let replay = self
                .replay
                .take()
                .expect("cancel_access() in replay called on a live handle");
            let resolution = replay_state.await_resolution(replay).await?;
            expect_cancelled_resolution(&resolution)?;
            end_durable_function_access(store, get_ctx, function_type, false).await?;
        }
        Ok(())
    }

    /// Replay counterpart of [`Self::cancel_access`] for a guest drop whose position is not part
    /// of the recorded history: a garbage-collected guest, or a heap restored from a snapshot, may
    /// drop the handle before or after the point where the recorded run dropped it. Awaiting the
    /// recorded `Cancelled` on the guest thread would park the guest when the drop runs earlier
    /// than recorded and an entry only the guest can consume lies between them, or when the
    /// recorded `Cancelled` belongs to a later invocation. A spawned task awaits the resolution
    /// instead and fails the Store if the call did not end as `Cancelled`. When the recorded run
    /// never cancelled the call, the task waits for the end of replay and records the
    /// cancellation live, as a live drop does.
    pub async fn cancel_access_deferred<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        debug_assert!(
            !self.is_live,
            "cancel_access_deferred() called on a live handle"
        );
        let context = self.trap_context();
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.ensure_accessor_terminal_supported("cancel_access_deferred")
            .map_err(|source| TerminalCallError::new(source, context))?;
        self.finished = true;
        let replay = self
            .replay
            .take()
            .expect("cancel_access_deferred() called on a live handle");
        let (replay_state, activity) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.tail_work_tracker().activity(),
            )
        });
        store.spawn(ReplayCancelWaiter {
            replay_state,
            replay,
            call: self.dropped_call_snapshot(None),
            get_ctx,
            activity,
        });
        Ok(())
    }

    pub(super) async fn persist_cancellation(
        oplog: Arc<dyn Oplog>,
        guard: &mut AccessTerminalGuard<P>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError> {
        guard
            .call()
            .expect("terminal guard is armed")
            .wait_request_upload()
            .await?;
        let partial = upload_partial_response(&oplog, partial).await?;
        let call = guard.call().expect("terminal guard is armed");
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: call.start_idx,
            partial,
            span_finished: call.span_finished.clone(),
        };
        call.notify_span_closed();
        // Reserve in oplog order before spawning the waiter; the guard retains the permit.
        let append = oplog.enqueue_add(cancelled);
        guard.cleanup_after_terminal(
            tokio::spawn(async move {
                append.await?;
                Ok(())
            }),
            None,
        );
        guard.wait_terminal().await
    }
}

/// Awaits the recorded `Cancelled` of a replayed call that the guest dropped, off the guest
/// thread. See [`DurableCallSession::cancel_access_deferred`].
struct ReplayCancelWaiter<T: 'static, Ctx: WorkerCtx> {
    replay_state: ReplayState,
    replay: ReplayCallHandle,
    /// Records the cancellation live when the recorded run never cancelled the call.
    call: DroppedCall,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    activity: TailActivity,
}

impl<T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx> AccessorTask<T, D>
    for ReplayCancelWaiter<T, Ctx>
{
    async fn run(self, store: &Accessor<T, D>) -> wasmtime::Result<()> {
        let context = self.call.trap_context;
        self.finish(store)
            .await
            .map_err(|source| TerminalCallError::new(source, context).into())
    }
}

impl<T: 'static, Ctx: WorkerCtx> ReplayCancelWaiter<T, Ctx> {
    async fn finish<D: HasData + ?Sized>(
        self,
        store: &Accessor<T, D>,
    ) -> Result<(), WorkerExecutorError> {
        let Self {
            replay_state,
            replay,
            mut call,
            get_ctx,
            activity,
        } = self;
        let outcome = replay_state
            .await_resolution_outcome_for_drop(replay, Some(&activity))
            .await?;
        match outcome {
            ResolutionOutcome::Resolved(resolution) => {
                expect_cancelled_resolution(&resolution)?;
            }
            ResolutionOutcome::Incomplete => {
                let (live, rejected, oplog) = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    (
                        ctx.is_live(),
                        ctx.rejects_live_continuation_at_replay_tail(),
                        ctx.state.oplog.clone(),
                    )
                });
                if !live {
                    if rejected {
                        // This check runs only after the shared cursor reaches its tail. A
                        // completed entity can dispose its Store earlier, cancelling this
                        // parked waiter; it is not a fence that keeps that Store alive.
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "Cancelled for a dropped future",
                            format!(
                                "no recorded cancellation for Start {} during completed entity replay",
                                call.start_idx()
                            ),
                        ));
                    }
                    continue_live_at_natural_tail_end_access(
                        store,
                        get_ctx,
                        &replay_state,
                        &activity,
                        "dropped future cancellation",
                        None,
                    )
                    .await?;
                }
                call.live_call_permit = Some(store.with(|mut access| {
                    LiveCallPermit::new(get_ctx(access.data_mut()).state.live_host_call_counter())
                }));
                call.wait_request_upload().await?;
                call.append_cancelled_with_oplog(oplog, None).await?;
            }
        }
        end_durable_function_access(store, get_ctx, call.function_type.clone(), false).await?;
        drop(activity);
        Ok(())
    }
}

/// Serializes and uploads a cancellation's optional partial response payload, shared by the
/// direct and accessor cancellation paths. The caller is responsible for site-specific failure
/// cleanup through its terminal guard.
async fn upload_partial_response<Resp: Into<HostResponse> + Send + 'static>(
    oplog: &Arc<dyn Oplog>,
    partial: Option<Resp>,
) -> Result<Option<OplogPayload<HostResponse>>, WorkerExecutorError> {
    match partial {
        Some(partial) => {
            let partial = Box::new(partial);
            let prepared =
                join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
                    let host_response: HostResponse = (*partial).into();
                    let bytes = golem_common::serialization::serialize(&host_response)?;
                    Ok::<_, String>(Box::new((bytes, Arc::new(host_response))))
                }))
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "partial durable call response encoding task failed: {err}"
                    ))
                })?
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize partial durable call response: {err}"
                    ))
                })?;
            let (bytes, cached) = *prepared;
            let raw_payload = oplog.upload_raw_payload(bytes).await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to store partial durable call response: {err}"
                ))
            })?;
            Ok(Some(raw_payload.into_payload_with_cache(cached).map_err(
                |err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to prepare partial durable call response payload: {err}"
                    ))
                },
            )?))
        }
        None => Ok(None),
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> Drop for DurableCallSession<Pair, P> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.is_live
            && self.persisted
            && (P::CANCEL_ON_TOOL_TRAP || self.entity_admission)
            && self.tool_trap_disposal.load(Ordering::Acquire)
        {
            let permit = self.live_call_permit.take();
            Cancellable::unfinished_drop(
                self.dropped_call_snapshot(permit),
                self.cleanup_sink.as_ref(),
            );
            return;
        }
        if self.teardown_probe().is_tearing_down() {
            tracing::debug!(
                start_idx = %self.start_idx,
                "durable call abandoned during executor shutdown"
            );
            return;
        }
        if self.is_live {
            if self.persisted {
                let live_call_permit = self.live_call_permit.take();
                // A live call dropped without finish/cancel: run the compile-time drop policy.
                P::unfinished_drop(
                    self.dropped_call_snapshot(live_call_permit),
                    self.drop_sink.as_ref(),
                );
            }
            // Not persisted: there is nothing on disk to reconcile.
        } else {
            if self.parked_undelivered_replay {
                // Expected: the recorded terminal was never delivered to the guest live
                // (`Cancelled { partial: None }` or an `End` marked `CompletionDiscarded`) and
                // the deterministic guest dropped this future at the same point it did live.
                tracing::debug!(
                    "parked undelivered durable call replay handle for Start {} dropped by the guest",
                    self.start_idx
                );
            } else {
                // A replay handle must never enqueue a live cancellation; just note the anomaly.
                tracing::warn!(
                    "replay durable call handle for Start {} dropped without finishing",
                    self.start_idx
                );
            }
        }
    }
}
