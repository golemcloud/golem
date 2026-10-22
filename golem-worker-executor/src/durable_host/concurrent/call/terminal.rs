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
impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// Completes a live call: upload the response, append the matching host-call `End`, then close
    /// the durable scope / commit / checkpoint via `end_durable_function`.
    ///
    /// A terminal failure is returned as a [`TerminalCallError`] carrying this call's own
    /// [`DurableCallTrapContext`], so the post-trap retry grouping stays owned by the call rather
    /// than falling back to ambient worker state a sibling could have clobbered once durable calls
    /// overlap.
    pub(crate) async fn complete<Ctx: WorkerCtx>(
        self,
        ctx: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError> {
        self.complete_drained(ctx, response, None).await
    }

    pub(super) async fn complete_drained<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = ctx.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_context(ctx, response, span_finished, None, false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    pub async fn complete_with_span<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response, Some(span_finished), None, false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    pub async fn complete_with_span_attributes<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_attributes: golem_common::model::oplog::SpanAttributes,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response, None, Some(span_attributes), false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_impl<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
        forced_commit: bool,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        self.complete_context(ctx, response, span_finished, span_attributes, forced_commit)
            .await
    }

    /// Records a direct host-return terminal without a completion delivery token or marker.
    pub(crate) async fn complete_context<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
        forced_commit: bool,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(self.is_live, "complete() called on a replay handle");
        // This is the call's legitimate terminal; mark it finished up front so that a failure of the
        // downstream commit / scope close (`?` below) does not drop the handle "unfinished" and trip
        // the drop policy. The host-call `End` is what makes the call durable, not these follow-ups.
        self.finished = true;
        let response = if self.persisted {
            let oplog = access.with_ctx(|ctx| ctx.state.oplog.clone());
            let (response, end) = prepare_end_entry::<Pair>(
                &oplog,
                &self.request_upload,
                self.start_idx,
                response,
                span_finished,
                span_attributes,
            )
            .await?;
            oplog.add(end).await?;
            access
                .finish_scope_with_commit(
                    self.retry.function_type().clone(),
                    self.begin_index(),
                    forced_commit,
                )
                .await?;
            response
        } else {
            response
        };
        Ok(response)
    }

    /// Accessor-window completion for non-scope-opening p3 durable calls.
    ///
    /// As with [`Self::complete`], a terminal failure carries this call's own
    /// [`DurableCallTrapContext`] via [`TerminalCallError`].
    pub async fn complete_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError>
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
        let (response, delivery) = self
            .complete_access_impl(store, get_ctx, response)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        // Non-deferred boundary: no fallible guest-facing work follows the host return, so hand
        // the token to Wasmtime's terminal observer synchronously (no tear window exists between
        // the impl returning and this call) — the observer settles it when the guest actually
        // consumes (or discards) the lowered result.
        delivery
            .deliver_at_accessor_terminal(store)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok(response)
    }

    /// Completes at the ordinary accessor boundary with a span transition on the terminal.
    pub(crate) async fn complete_access_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<Pair::Resp, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        let (response, delivery) = self
            .complete_access_deferred_with_span(store, get_ctx, response, span_finished)
            .await?;
        delivery
            .deliver_at_accessor_terminal(store)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok(response)
    }

    /// Like [`Self::complete_access`], but for call sites whose response crosses one more
    /// fallible/cancellable boundary after the durable terminal (a second-stage channel send, a
    /// span finish before the host method returns, a wire conversion). Returns the response
    /// together with an armed [`CompletionDelivery`] token; the caller must consume the token at
    /// the real guest-facing boundary (`delivered` / `suppress` / `discarded`), and a torn future
    /// in between records the `CompletionDiscarded` marker via the token's drop.
    pub async fn complete_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_deferred(&mut AccessorContext::new(store, get_ctx), response)
            .await
    }

    /// Completes a call without publishing its response to the caller. Native callers prepare
    /// the delivery token and consume it at the Rust return, with no intervening await.
    pub(crate) async fn complete_deferred<Ctx: WorkerCtx>(
        self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError> {
        self.complete_deferred_drained(access, response, None).await
    }

    pub(super) async fn complete_deferred_drained<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = access.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        let (response, mut guard) = self
            .complete_terminal(
                access,
                response,
                span_finished,
                AccessResultDestination::Guest,
            )
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok((response, guard.take_completion_delivery()))
    }

    /// Prepares the End before calling `accept`. The caller must hold its local effect lock
    /// while reserving the unpublished effect and invoking `enqueue(reservation)`. Returning
    /// without invoking `enqueue` rejects the candidate. Rejection returns the
    /// unfinished call and response, so the caller can select another terminal or retry.
    /// Publication owns the reservation until the End append succeeds and, for a `WriteRemote`
    /// call, its DurableOnly commit; other calls leave the End to the next owner-oplog commit,
    /// as [`end_durable_function_access`] does. Publication must be infallible after acceptance.
    /// `failed` must synchronously fence observers before returning the owner cleanup future.
    /// The owned coordinator resolves the terminal receipt before polling that future, so
    /// cleanup may join the receipt without depending on itself. Runtime replacement and
    /// worker retirement retain the coordinator until cleanup completes.
    /// Replay uses the ordinary deferred replay interface; this callback is live-only.
    pub async fn complete_access_deferred_with_effect<T, D, Ctx, R, A, F, E, EF>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        accept: A,
        publish: F,
        failed: E,
    ) -> Result<Result<(Pair::Resp, CompletionDelivery), (Self, Pair::Resp)>, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        R: Send + 'static,
        A: FnOnce(&Pair::Resp, Box<dyn FnOnce(R) + '_>),
        F: FnOnce(R, OplogIndex) + Send + 'static,
        E: FnOnce(WorkerExecutorError) -> EF + Send + 'static,
        EF: Future<Output = ()> + Send + 'static,
    {
        self.complete_deferred_with_effect(
            &mut AccessorContext::new(store, get_ctx),
            response,
            accept,
            publish,
            failed,
        )
        .await
    }

    /// Shared append-before-publication protocol; see [`Self::complete_access_deferred_with_effect`].
    pub(crate) async fn complete_deferred_with_effect<Ctx, R, A, F, E, EF>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        accept: A,
        publish: F,
        failed: E,
    ) -> Result<Result<(Pair::Resp, CompletionDelivery), (Self, Pair::Resp)>, TerminalCallError>
    where
        Ctx: WorkerCtx,
        R: Send + 'static,
        A: FnOnce(&Pair::Resp, Box<dyn FnOnce(R) + '_>),
        F: FnOnce(R, OplogIndex) + Send + 'static,
        E: FnOnce(WorkerExecutorError) -> EF + Send + 'static,
        EF: Future<Output = ()> + Send + 'static,
    {
        let context = self.trap_context();
        if let Err(err) = access.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.ensure_accessor_terminal_supported("complete_access_deferred_with_effect")
            .map_err(|err| TerminalCallError::new(err, context))?;
        if !self.is_live || !self.persisted {
            return Err(TerminalCallError::new(
                WorkerExecutorError::runtime(
                    "local effect completion requires a persisted live call",
                ),
                context,
            ));
        }
        let (oplog, recorder, worker) = access.with_ctx(|ctx| {
            (
                ctx.state.oplog.clone(),
                ctx.state
                    .completion_marker_recorder()
                    .for_store(ctx.runtime_suspension.clone()),
                ctx.public_state.worker(),
            )
        });
        let prepared = prepare_end_entry::<Pair>(
            &oplog,
            &self.request_upload,
            self.start_idx,
            response,
            None,
            None,
        )
        .await;
        let (response, end) = match prepared {
            Ok(prepared) => prepared,
            Err(err) => {
                self.abandon_for_trap();
                return Err(TerminalCallError::new(err, context));
            }
        };
        let Some(recording) = worker.runtime_append_tasks().register() else {
            self.abandon_for_trap();
            return Err(TerminalCallError::new(
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                },
                context,
            ));
        };
        let tasks = worker.tasks.clone();
        let commits = *self.retry.function_type() == DurableFunctionType::WriteRemote;
        let coordinator = prepare_access_terminal_effect(
            &tasks,
            recording,
            async move {
                if commits {
                    worker
                        .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                        .await?;
                }
                Ok(())
            },
            publish,
            failed,
        );
        let (handoff, terminal) = match coordinator {
            Ok(coordinator) => coordinator,
            Err(err) => {
                self.abandon_for_trap();
                return Err(TerminalCallError::new(err, context));
            }
        };
        let mut guard = AccessTerminalGuard::<P>::new(
            self.dropped_call_snapshot(self.live_call_permit.clone()),
            self.drop_sink.clone(),
            self.cleanup_sink.clone(),
            self.teardown_probe(),
        )
        .with_tool_trap_disposal(self.tool_trap_disposal.clone());
        let accepted = accept_access_terminal_effect(&response, &oplog, end, accept);
        let (reservation, append) = match accepted {
            Some(accepted) => accepted,
            None => {
                guard.disarm();
                return Ok(Err((self, response)));
            }
        };
        // Admission preceded acceptance; the coordinator cannot be cancelled by retirement.
        self.finished = true;
        assert!(handoff.send((reservation, append)).is_ok());
        guard.cleanup_after_terminal(
            terminal,
            Some(CompletionMarkerRecord {
                start_idx: self.start_idx,
                recorder,
            }),
        );
        if let Err(err) = guard.wait_terminal().await {
            guard.disarm();
            return Err(TerminalCallError::new(err, context));
        }
        self.finish_terminal_scope(access, &mut guard)
            .await
            .map_err(|err| TerminalCallError::new(err, context))?;
        Ok(Ok((response, guard.take_completion_delivery())))
    }

    pub async fn complete_access_deferred_with_span<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
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
        self.complete_access_impl_with_span(store, get_ctx, response, Some(span_finished))
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_access_impl<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_access_impl_with_span(store, get_ctx, response, None)
            .await
    }

    async fn complete_access_impl_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let (response, mut guard) = self
            .complete_access_terminal(
                store,
                get_ctx,
                response,
                span_finished,
                AccessResultDestination::Guest,
            )
            .await?;
        Ok((response, guard.take_completion_delivery()))
    }

    pub(super) async fn complete_access_terminal<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        destination: AccessResultDestination,
    ) -> Result<(Pair::Resp, AccessTerminalGuard<P>), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_terminal(
            &mut AccessorContext::new(store, get_ctx),
            response,
            span_finished,
            destination,
        )
        .await
    }

    async fn complete_terminal<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        destination: AccessResultDestination,
    ) -> Result<(Pair::Resp, AccessTerminalGuard<P>), WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("complete_access")?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "complete_access called on a replay handle; use replay_access first",
            ));
        }
        {
            let (oplog, completion_marker_recorder) = access.with_ctx(|ctx| {
                (
                    ctx.state.oplog.clone(),
                    (destination == AccessResultDestination::Guest).then(|| {
                        ctx.state
                            .completion_marker_recorder()
                            .for_store(ctx.runtime_suspension.clone())
                    }),
                )
            });
            let (response, mut guard) = self
                .persist_guarded_terminal(
                    oplog,
                    completion_marker_recorder,
                    response,
                    span_finished,
                )
                .await?;
            self.finish_terminal_scope(access, &mut guard).await?;
            Ok((response, guard))
        }
    }

    /// Common terminal guard ownership for Store-bound and owned internal completions.
    pub(super) async fn persist_guarded_terminal(
        &mut self,
        oplog: Arc<dyn Oplog>,
        recorder: Option<CompletionMarkerRecorder>,
        response: Pair::Resp,
        span: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(Pair::Resp, AccessTerminalGuard<P>), WorkerExecutorError> {
        let mut guard = self.terminal_guard();
        let result = if self.persisted {
            Self::persist_access_terminal(
                oplog,
                recorder,
                &mut guard,
                self.start_idx,
                response,
                span,
            )
            .await
        } else {
            Ok(response)
        };
        match result {
            Ok(response) => Ok((response, guard)),
            Err(error) => {
                guard.disarm();
                Err(error)
            }
        }
    }

    pub(super) fn terminal_guard(&mut self) -> AccessTerminalGuard<P> {
        let guard = AccessTerminalGuard::<P>::new(
            self.dropped_call_snapshot(self.live_call_permit.clone()),
            self.drop_sink.clone(),
            self.cleanup_sink.clone(),
            self.teardown_probe(),
        )
        .with_tool_trap_disposal(self.tool_trap_disposal.clone());
        self.finished = true;
        guard
    }

    async fn finish_terminal_scope<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
        guard: &mut AccessTerminalGuard<P>,
    ) -> Result<(), WorkerExecutorError> {
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let result = access.finish_scope(function_type, begin_index).await;
        if result.is_err() {
            // The caller observes this failure; it is not a guest-discarded completion.
            guard.suppress_discard_marker();
        }
        result
    }

    /// Persistence stage of [`Self::complete_access_impl`]: wait for the (possibly deferred)
    /// request upload, upload the response payload, hand the terminal `End` append to an owned
    /// task via [`AccessTerminalGuard::cleanup_after_terminal`], and join it before returning.
    ///
    /// Deliberately store-free — it sees only the oplog and the armed guard, never the `Accessor`
    /// — so nothing on this path can release the live-call permit or emit the guard's cleanup
    /// event before the terminal entry is appended: the guard still owns both when this returns,
    /// and they are only released downstream (`disarm` after `end_durable_function_access`), or
    /// handed to the drain queue if the completion future is torn mid-await. The focused ordering
    /// test `access_terminal_end_is_appended_before_cleanup_and_permit_release` drives this exact
    /// function against a gated oplog to keep that invariant observable.
    pub(in crate::durable_host::concurrent) async fn persist_access_terminal(
        oplog: Arc<dyn Oplog>,
        completion_marker_recorder: Option<CompletionMarkerRecorder>,
        guard: &mut AccessTerminalGuard<P>,
        start_idx: OplogIndex,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let (response, end) = prepare_end_entry::<Pair>(
            &oplog,
            &guard
                .call()
                .expect("terminal guard is armed")
                .request_upload,
            start_idx,
            response,
            span_finished,
            None,
        )
        .await?;
        // Reserve the terminal synchronously in oplog order before arming the terminal guard. A
        // tear may queue a completion marker immediately, and that marker must never overtake an
        // unpolled spawned append task.
        if let OplogEntry::End {
            span_finished: Some(finished),
            ..
        } = &end
            && let Some(observer) = &guard.call().expect("terminal guard is armed").span_observer
        {
            observer.closed(finished);
        }
        let end_append = oplog.enqueue_add(end);
        // Arm the discard marker together with the owned `End` append: from this point a torn
        // completion future means the guest discarded a persisted successful completion (a tear
        // *during* `wait_terminal` still counts — the owned task appends the `End` regardless).
        guard.cleanup_after_terminal(
            tokio::spawn(async move {
                end_append
                    .await
                    .map(|_| ())
                    .map_err(WorkerExecutorError::from)
            }),
            completion_marker_recorder.map(|recorder| CompletionMarkerRecord {
                start_idx,
                recorder,
            }),
        );
        guard.wait_terminal().await?;
        Ok(response)
    }
}

/// Store-free persistence preparation shared by the direct ([`DurableCallSession::complete`]) and
/// accessor ([`DurableCallSession::persist_access_terminal`]) completion paths: waits for the
/// (possibly deferred) request upload, encodes and uploads the response payload, and constructs
/// the terminal `End` entry — without appending it or closing
/// scopes, which the two paths deliberately order differently (direct appends inline; accessor
/// hands the append to an owned task via its terminal guard).
async fn prepare_end_entry<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    request_upload: &PendingUpload,
    start_idx: OplogIndex,
    response: Pair::Resp,
    span_finished: Option<golem_common::model::oplog::SpanFinished>,
    span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
) -> Result<(Pair::Resp, OplogEntry), WorkerExecutorError> {
    // Surface a deferred request-upload failure here, at the call site, before recording the
    // `End` that references the request. The leaf oplog's commit barrier is the backstop, but
    // awaiting here turns an upload failure into a graceful error instead of a commit-time
    // panic. A no-op when the request was inline or eagerly uploaded.
    request_upload.wait().await.map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to serialize and store durable call request: {err}"
        ))
    })?;

    // Flat responses have input-independent encoding depth and need no thread-pool handoff.
    // Recursive or unaudited response families still use a fresh stack: this can be called inside
    // a Wasmtime fiber, whose stack is independent of the native runtime thread's stack.
    let response = Box::new(response);
    let prepared = if response_has_bounded_encoding_stack::<Pair::Resp>() {
        encode_response(response)
    } else {
        join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
            encode_response(response)
        }))
        .await
        .map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "durable call response encoding task failed: {err}"
            ))
        })?
    }
    .map_err(|err| {
        WorkerExecutorError::runtime(format!("failed to serialize durable call response: {err}"))
    })?;
    let (response, bytes) = *prepared;
    let raw_payload = oplog.upload_raw_payload(bytes).await.map_err(|err| {
        WorkerExecutorError::runtime(format!("failed to store durable call response: {err}"))
    })?;
    let response_payload = raw_payload.into_payload().map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to prepare durable call response payload: {err}"
        ))
    })?;
    Ok((
        *response,
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: start_idx,
            response: Some(response_payload),
            forced_commit: false,
            span_finished,
            span_attributes,
        },
    ))
}

pub(super) fn response_has_bounded_encoding_stack<Resp: 'static>() -> bool {
    use golem_common::model::oplog::payload::*;
    use std::any::TypeId;

    // Only audited scalar/flat collection families are admitted. In particular, a small value of
    // a recursive schema-bearing type is not safe to classify by its current size or variant.
    [
        TypeId::of::<HostResponseBlobStoreUnit>(),
        TypeId::of::<HostResponseBlobStoreGetData>(),
        TypeId::of::<HostResponseBlobStoreListObjects>(),
        TypeId::of::<HostResponseBlobStoreContains>(),
        TypeId::of::<HostResponseBlobStoreTimestamp>(),
        TypeId::of::<HostResponseBlobStoreOptionalTimestamp>(),
        TypeId::of::<HostResponseMonotonicClockTimestamp>(),
        TypeId::of::<HostResponseWallClock>(),
        TypeId::of::<HostResponseGolemAgentWebhookUrl>(),
        TypeId::of::<HostResponseGolemApiIdempotencyKey>(),
        TypeId::of::<HostResponseGolemApiPromiseCompletion>(),
        TypeId::of::<HostResponseGolemApiPromiseResult>(),
        TypeId::of::<HostResponseGolemApiUnit>(),
        TypeId::of::<HostResponseGolemRpcUnit>(),
        TypeId::of::<HostResponseKVGet>(),
        TypeId::of::<HostResponseKVGetMany>(),
        TypeId::of::<HostResponseKVDelete>(),
        TypeId::of::<HostResponseKVKeys>(),
        TypeId::of::<HostResponseKVUnit>(),
        TypeId::of::<HostResponsePollReady>(),
        TypeId::of::<HostResponsePollResult>(),
        TypeId::of::<HostResponseRandomBytes>(),
        TypeId::of::<HostResponseRandomU64>(),
        TypeId::of::<HostResponseRandomSeed>(),
        TypeId::of::<HostResponseP3MonotonicClockUnit>(),
        TypeId::of::<HostResponseStreamCheckWrite>(),
        TypeId::of::<HostResponseStreamSkip>(),
        TypeId::of::<HostResponseQuotaTokenAcquired>(),
        TypeId::of::<HostResponseQuotaCommitResult>(),
        TypeId::of::<HostResponsePermissionCardDerived>(),
        TypeId::of::<HostResponsePermissionCardLiveness>(),
        TypeId::of::<HostResponsePermissionCardDecision>(),
        TypeId::of::<HostResponsePermissionCardTransferComplete>(),
        TypeId::of::<HostResponsePermissionWallet>(),
        TypeId::of::<HostResponsePermissionWalletVersion>(),
        TypeId::of::<HostResponseP3KeyvalueIncomingValueStream>(),
        TypeId::of::<HostResponseP3BlobstoreIncomingValueStream>(),
        TypeId::of::<HostResponseP3HttpClientConsumeBodyChunk>(),
        TypeId::of::<HostResponseP3SocketsTcpReceiveChunk>(),
        TypeId::of::<HostResponseGolemApiCard>(),
        TypeId::of::<HostResponseGolemApiOplogEntries>(),
        TypeId::of::<HostResponseToolAttachmentCreated>(),
        TypeId::of::<HostResponseToolAttachmentWrite>(),
        TypeId::of::<HostResponseToolAttachmentTerminalResult>(),
        TypeId::of::<HostResponseToolAttachmentClosed>(),
        TypeId::of::<HostResponseConfigGetResponse>(),
        TypeId::of::<HostResponseConfigGetAllResponse>(),
        TypeId::of::<HostResponseCliEnvironmentGetEnvironment>(),
    ]
    .contains(&TypeId::of::<Resp>())
}

// Keep the large HostResponse enum's construction/serialization frame out of the async caller.
// Box the input and output so the blocking-task envelope also has a bounded stack footprint.
#[inline(never)]
#[allow(
    clippy::boxed_local,
    reason = "the caller must not move the generic response onto its small Wasmtime fiber stack"
)]
pub(super) fn encode_response<Resp: Into<HostResponse> + TryFrom<HostResponse, Error = String>>(
    response: Box<Resp>,
) -> Result<EncodedResponse<Resp>, String> {
    let host_response: HostResponse = (*response).into();
    let bytes = golem_common::serialization::serialize(&host_response)?;
    let response = Box::new(Resp::try_from(host_response)?);
    Ok(Box::new((response, bytes)))
}

pub(crate) async fn end_durable_function_access<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    forced_commit: bool,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    end_durable_function_access_with_span(
        store,
        get_ctx,
        function_type,
        begin_index,
        forced_commit,
        None,
    )
    .await
}

pub(crate) async fn end_durable_function_access_with_span<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    _begin_index: OplogIndex,
    forced_commit: bool,
    _span_finished: Option<golem_common::model::oplog::SpanFinished>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (snapshotting_mode, public_state) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (ctx.state.snapshotting_mode, ctx.public_state.clone())
    });

    if snapshotting_mode {
        return Ok(());
    }

    if function_type == DurableFunctionType::WriteRemote || forced_commit {
        public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await?;
        if store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.at_clean_checkpoint_boundary()
        }) {
            public_state
                .worker()
                .checkpoint_status_mid_invocation()
                .await;
        }
    }

    Ok(())
}

/// Finishes a span in the in-memory invocation context only, without writing or consuming any
/// oplog entry: pops the current-span pointer if it points at this span, then marks the span
/// finished. The owning durable operation records the span transition separately.
pub(crate) fn finish_span_in_memory<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError> {
    if &ctx.state.current_span_id == span_id {
        let span = ctx.state.invocation_context.get(span_id).map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "span {span_id} missing during finish_span replay: {err}"
            ))
        })?;
        ctx.state.current_span_id = span
            .parent()
            .map(|p| p.span_id().clone())
            .unwrap_or_else(|| ctx.state.invocation_context.root.span_id().clone());
    }
    let _ = ctx
        .state
        .invocation_context
        .finish_span(span_id)
        .map_err(WorkerExecutorError::runtime);
    Ok(())
}
