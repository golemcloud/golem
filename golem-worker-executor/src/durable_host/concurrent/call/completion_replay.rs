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
    /// Replays a call: drive the cursor until the call resolves, decode its response, then close the
    /// durable scope / commit via `end_durable_function`.
    ///
    /// If replay reaches the end of the oplog without ever seeing the matching `End`/`Cancelled` —
    /// a lone committed host-call `Start`, now possible for any write because `Start` is eager —
    /// the call is returned as [`CallReplayOutcome::Incomplete`] (for function types that are safe
    /// to re-execute) so the caller can re-run the side effect and `complete` the existing `Start`.
    /// A child call requires explicit repair authorization from its protocol owner.
    pub async fn replay<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.replay_context(ctx).await
    }

    /// Replays a direct host-return call. Markerless Ends return immediately; delivery and
    /// discard markers remain divergence errors, not deferred guest completion boundaries.
    pub(crate) async fn replay_context<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.reproduce_refusal_for_claimed_start(access).await?;
        let replay = self
            .replay
            .take()
            .expect("replay() called on a live handle");
        let (replay_state, oplog) = access.with_ctx(|ctx| {
            if let Some(hook) = ctx.replay_admission_hook() {
                hook.before_direct_replay_wait(Pair::FQFN, self.start_idx);
            }
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                if let ReplayDeliveryDisposition::AtMarker(marker_idx) = disposition {
                    self.finished = true;
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "a completion-delivery marker for an accessor durable call",
                        format!(
                            "CompletionDelivered at {marker_idx} references non-accessor durable call Start at {}",
                            self.start_idx
                        ),
                    ));
                }
                // `Immediate` and `AtReplayTail` both deliver at the host return: the direct
                // path's guest task is synchronously blocked inside the host call, so delivery
                // coincides with the host return and has no pre-delivery divergence window.
                //
                // Terminal: mark finished up front so a decode / scope-close failure below does not
                // drop the (replay) handle as "unfinished".
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access
                    .finish_scope(self.retry.function_type().clone())
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ReplayedResolution::Undelivered(terminal) => {
                // Replay divergence: undelivered terminals cannot legally occur on the direct
                // path (see [`UndeliveredTerminal::direct_divergence_error`]).
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(access).await?;
                if let Err(error) = access.publish_direct_replay_tail(self.start_idx).await {
                    self.finished = true;
                    return Err(error);
                }
                if let Some((response, span_finished)) =
                    self.recorded_denied_response(access).await?
                {
                    let response = self
                        .complete_drained(access, response, span_finished)
                        .await
                        .map_err(|error| error.source)?;
                    return Ok(CallReplayOutcome::Replayed(response));
                }
                if self.requires_agent_authority {
                    match access.agent_auth_ctx().await {
                        Ok(auth_ctx) => self.agent_auth_ctx = Some(auth_ctx),
                        Err(error) => {
                            self.abandon_for_trap();
                            return Err(error);
                        }
                    }
                }
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Accessor-window replay for non-scope-opening p3 durable calls.
    pub async fn replay_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.replay_access_to(store, get_ctx, AccessResultDestination::Guest, false, None)
            .await
    }

    /// Replays an effectful destructor from a background task. Its End is host-internal;
    /// cancellation or a guest delivery marker is a divergence. Missing terminals repair only
    /// after the natural replay tail, never by moving past intervening guest history.
    pub(crate) async fn replay_drop_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        activity: &crate::durable_host::tail_work::TailActivity,
    ) -> Result<CallReplayOutcome<Pair, P>, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        self.replay_access_to(
            store,
            get_ctx,
            AccessResultDestination::Host,
            false,
            Some(activity),
        )
        .await
        .map_err(|source| TerminalCallError::new(source, context))
    }

    pub(super) async fn replay_access_to<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        destination: AccessResultDestination,
        require_completed: bool,
        drop_activity: Option<&crate::durable_host::tail_work::TailActivity>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access")?;
        self.reproduce_refusal_for_claimed_start(&mut AccessorContext::new(store, get_ctx))
            .await?;
        let function_type = self.retry.function_type().clone();
        let (replay_state, oplog, completion_marker_recorder) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state
                    .completion_marker_recorder()
                    .for_store(ctx.runtime_suspension.clone()),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access() called on a live handle");
        let outcome = replay_state
            .await_resolution_outcome_for_drop(replay, drop_activity)
            .await?;
        if drop_activity.is_some()
            && matches!(
                outcome,
                ResolutionOutcome::Resolved(Resolution::Cancelled { .. })
            )
        {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "End for an effectful resource drop",
                format!("Cancelled for Start {}", self.start_idx),
            ));
        }
        if require_completed
            && !matches!(
                outcome,
                ResolutionOutcome::Resolved(Resolution::Completed { .. })
            )
        {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "completed host-internal child",
                "completed parent has an unfinished host-internal child",
            ));
        }
        if destination == AccessResultDestination::Host
            && matches!(
                outcome,
                ResolutionOutcome::Resolved(
                    Resolution::Completed {
                        delivery_marker: Some(_),
                        ..
                    } | Resolution::CompletedButDiscarded { .. }
                )
            )
        {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "host-internal terminal without a guest delivery marker",
                format!("delivery marker for host-internal Start {}", self.start_idx),
            ));
        }
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                end_durable_function_access(store, get_ctx, function_type, false).await?;
                if destination == AccessResultDestination::Guest {
                    CompletionDelivery::replay_delivered(
                        disposition,
                        self.start_idx,
                        completion_marker_recorder,
                        self.trap_context(),
                        self.cleanup_sink.clone(),
                        self.teardown_probe(),
                    )
                    .deliver_at_accessor_terminal(store)
                    .await?;
                }
                Ok(CallReplayOutcome::Replayed(response))
            }
            ReplayedResolution::Undelivered(terminal) => {
                // The recorded run never delivered this terminal to the guest (either the call
                // was cancelled mid-flight without a partial result, or its completion was
                // discarded before delivery). Mirror that exactly: never complete, so the
                // deterministic guest drops this future at the same point it did live. Resolving
                // to an error here instead would race the guest's own drop — the resolver
                // delivers the recorded terminal as soon as the cursor crosses it, which may be
                // before the winning branch has been polled and had a chance to drop the loser.
                //
                // Durable cleanup (closing the durable scope for scope-opening calls) is
                // deferred to this handle's `Drop`, which enqueues the idempotent
                // `CloseDurableScope` event. Awaiting `end_durable_function_access` *here*,
                // before the park, would open a cancellation window: for scope-opening calls
                // it takes the scope's replay handle before its first await, so the guest
                // dropping this future mid-close would strand the open scope (with `finished`
                // already set, `Drop` would not clean it up either).
                terminal.log_parking();
                self.parked_undelivered_replay = true;
                // Release the terminal before suspending: a discarded completion carries the
                // persisted response payload, which must not be kept alive for as long as the
                // guest keeps this parked future around.
                drop(terminal);
                park_undelivered_forever().await
            }
            ReplayedResolution::Incomplete => {
                if let Some(activity) = drop_activity {
                    if store.with(|mut access| {
                        get_ctx(access.data_mut()).rejects_live_continuation_at_replay_tail()
                    }) {
                        self.finished = true;
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "End for an effectful resource drop",
                            format!(
                                "incomplete Start {} during completed entity replay",
                                self.start_idx
                            ),
                        ));
                    }
                    // Do not acquire a live-call permit while later invocations must still
                    // replay; that permit would hold their settlement open.
                    replay_state.await_natural_tail_end(Some(activity)).await?;
                }
                self.prepare_incomplete_live_repair_at(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                let transition = if let Some(activity) = drop_activity {
                    continue_live_at_natural_tail_end_access(
                        store,
                        get_ctx,
                        &replay_state,
                        activity,
                        "effectful resource drop",
                        Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                            index: self.start_idx.into(),
                        }),
                    )
                    .await
                } else {
                    publish_incomplete_replay_tail_at(
                        &mut AccessorContext::new(store, get_ctx),
                        Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                            index: self.start_idx.into(),
                        }),
                    )
                    .await
                    .and_then(FinishReplayToLive::require_live)
                };
                if let Err(error) = transition {
                    self.finished = true;
                    return Err(error);
                }
                let synchronization = if self.requires_agent_authority {
                    match crate::durable_host::call_coordinator::capture_live_agent_authority_access(store, get_ctx, |ctx| DurableWorkerCtx::agent_auth_ctx(ctx)).await {
                        Ok(Some(auth_ctx)) => {
                            self.agent_auth_ctx = Some(auth_ctx);
                            Ok(())
                        }
                        Ok(None) => Err(WorkerExecutorError::runtime("incomplete durable call did not switch to live execution")),
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = synchronization {
                    self.abandon_for_trap();
                    return Err(error);
                }
                let denied = self
                    .recorded_denied_response(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                if let Some((response, span_finished)) = denied {
                    if destination != AccessResultDestination::Guest {
                        self.abandon_for_trap();
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "host-internal Start without a denial",
                            format!("denied host-internal Start {}", self.start_idx),
                        ));
                    }
                    let response = match span_finished {
                        Some(span_finished) => {
                            self.complete_access_with_span(store, get_ctx, response, span_finished)
                                .await
                        }
                        None => self.complete_access(store, get_ctx, response).await,
                    }
                    .map_err(|error| error.source)?;
                    return Ok(CallReplayOutcome::Replayed(response));
                }
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Replay terminal handling for entity-body reconstruction. Unlike the guest-future accessor
    /// path, a recorded cancellation is returned to the reconstruction coordinator instead of
    /// parking forever: the coordinator owns the transient entity task and aborts it at that
    /// durable boundary. Completed responses are decoded normally and incomplete Starts become
    /// live-repair handles under the original Start index.
    pub async fn replay_reconstruction_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<ReconstructionReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_reconstruction_access")?;
        let function_type = self.retry.function_type().clone();
        let (replay_state, oplog, completion_marker_recorder) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state
                    .completion_marker_recorder()
                    .for_store(ctx.runtime_suspension.clone()),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_reconstruction_access() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let cancelled = matches!(payload, ReplayedPayload::CancelledPartial(_));
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                end_durable_function_access(store, get_ctx, function_type, false).await?;
                CompletionDelivery::replay_delivered(
                    disposition,
                    self.start_idx,
                    completion_marker_recorder,
                    self.trap_context(),
                    self.cleanup_sink.clone(),
                    self.teardown_probe(),
                )
                .deliver_at_accessor_terminal(store)
                .await?;
                if cancelled {
                    Ok(ReconstructionReplayOutcome::Cancelled(response))
                } else {
                    Ok(ReconstructionReplayOutcome::Replayed(response))
                }
            }
            ReplayedResolution::Undelivered(UndeliveredTerminal::CancelledWithoutPartial {
                cancelled_idx,
            }) => {
                self.finished = true;
                end_durable_function_access(store, get_ctx, function_type, false).await?;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "entity cancellation with partial terminal provenance",
                    format!("Cancelled without partial at {cancelled_idx}"),
                ))
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                let transition = match publish_incomplete_replay_tail_access(
                    store,
                    get_ctx,
                    SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                        index: self.start_idx.into(),
                    },
                )
                .await
                {
                    Ok(transition) => transition,
                    Err(error) => {
                        self.finished = true;
                        return Err(error);
                    }
                };
                match transition {
                    FinishReplayToLive::Cancelled => {
                        Ok(ReconstructionReplayOutcome::LiveAdmissionCancelled(self))
                    }
                    FinishReplayToLive::Live | FinishReplayToLive::AttachmentAdmissionRejected => {
                        Ok(ReconstructionReplayOutcome::Incomplete(self))
                    }
                }
            }
        }
    }

    async fn resolve_completed_reconstruction_to(
        mut self,
        replay_state: ReplayState,
        oplog: Arc<dyn Oplog>,
    ) -> Result<(Pair::Resp, bool), WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("resolve_completed_reconstruction")?;
        let replay = self
            .replay
            .take()
            .expect("resolve_completed_reconstruction() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let cancelled = matches!(payload, ReplayedPayload::CancelledPartial(_));
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                if matches!(disposition, ReplayDeliveryDisposition::AtMarker(_)) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "internal entity terminal without delivery marker",
                        "CompletionDelivered for internal entity invocation",
                    ));
                }
                Ok((response, cancelled))
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.finished = true;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "completed entity invocation terminal",
                    format!(
                        "end of replay after entity Start at {}; the completed reconstruction was unresolved",
                        self.start_idx
                    ),
                ))
            }
        }
    }

    /// Like [`Self::replay_access`], but for deferred-delivery call sites (see
    /// [`Self::complete_access_deferred`]): a replayed terminal is returned together with a
    /// [`CompletionDelivery`] token instead of parking here when the recorded run discarded the
    /// completion. For a recorded `CompletionDiscarded` marker this decodes the persisted
    /// response and closes the durable scope — mirroring exactly what live did before its token
    /// was armed — and the *caller* parks at its own delivery boundary after performing its
    /// deterministic post-`End` continuation (e.g. restoring resource state).
    pub async fn replay_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.replay_deferred(&mut AccessorContext::new(store, get_ctx))
            .await
    }

    /// Resolves the recorded terminal while retaining the actual caller-delivery boundary.
    /// Callers map errors with the session's trap context, as on the accessor path.
    pub(crate) async fn replay_deferred<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("replay_access_deferred")?;
        self.reproduce_refusal_for_claimed_start(access).await?;
        let function_type = self.retry.function_type().clone();
        let (replay_state, oplog, completion_marker_recorder) = access.with_ctx(|ctx| {
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state
                    .completion_marker_recorder()
                    .for_store(ctx.runtime_suspension.clone()),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access_deferred() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access.finish_scope(function_type).await?;
                // The delivery token is constructed only after the fallible decode / scope close
                // succeeded: a token dropped on the error path would log a spurious
                // unconsumed-token warning.
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_delivered(
                        disposition,
                        self.start_idx,
                        completion_marker_recorder,
                        self.trap_context(),
                        self.cleanup_sink.clone(),
                        self.teardown_probe(),
                    ),
                ))
            }
            ReplayedResolution::Undelivered(UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                response,
            }) => {
                // The recorded run persisted a successful `End` but the guest discarded the
                // completion before final delivery (marker at `marker_idx`). Decode the
                // persisted response and close the durable scope — the exact state live was in
                // when its delivery token was armed — and let the caller run its deterministic
                // post-`End` continuation before parking at the delivery boundary.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded \
                     by the guest (marker at {marker_idx}); replaying its post-End \
                     continuation before parking at the delivery boundary"
                );
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    ReplayedPayload::Completed(response),
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access.finish_scope(function_type).await?;
                // As above, the token is constructed only after the fallible operations.
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_discarded(),
                ))
            }
            ReplayedResolution::Undelivered(
                terminal @ UndeliveredTerminal::CancelledWithoutPartial { .. },
            ) => {
                // Cancelled with no partial: in the recorded run this call never returned a
                // value to the guest. Mirror exactly — park here so the deterministic guest
                // drops this future at the same point it did live; durable cleanup is
                // deferred to this handle's `Drop`. See [`Self::replay_access`] for why the
                // scope close must not happen before the park.
                terminal.log_parking();
                self.parked_undelivered_replay = true;
                park_undelivered_forever().await
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(access).await?;
                let transition = publish_incomplete_replay_tail_at(
                    access,
                    Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                        index: self.start_idx.into(),
                    }),
                )
                .await;
                if let Err(error) = transition.and_then(FinishReplayToLive::require_live) {
                    self.finished = true;
                    return Err(error);
                }
                let synchronization = if self.requires_agent_authority {
                    match access.agent_auth_ctx().await {
                        Ok(auth_ctx) => {
                            self.agent_auth_ctx = Some(auth_ctx);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = synchronization {
                    self.abandon_for_trap();
                    return Err(error);
                }
                if let Some((response, span_finished)) =
                    self.recorded_denied_response(access).await?
                {
                    let (response, delivery) = self
                        .complete_deferred_drained(access, response, span_finished)
                        .await
                        .map_err(|error| error.source)?;
                    return Ok(DeferredCallReplayOutcome::Replayed(response, delivery));
                }
                Ok(DeferredCallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Finalizes a response that has already been produced by adapter-specific logic, converging
    /// live completion, completed replay, and incomplete-replay repair in one transition.
    pub(crate) async fn finish_access<T, D, Ctx>(
        self,
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
        if self.is_live() {
            self.complete_access(store, get_ctx, response).await
        } else {
            match self
                .replay_access(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                CallReplayOutcome::Replayed(recorded) => Ok(recorded),
                CallReplayOutcome::Incomplete(call) => {
                    call.complete_access(store, get_ctx, response).await
                }
            }
        }
    }

    /// Deferred-delivery form of [`Self::finish_access`]. The returned token always describes the
    /// recorded delivery state, regardless of whether this invocation completed the item live,
    /// repaired an incomplete `Start`, or replayed an existing terminal. The fallback response
    /// is built only after incomplete-call readiness has selected the owner's recovery outcome.
    pub(crate) async fn finish_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: impl FnOnce() -> Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if self.is_live() {
            self.complete_access_deferred(store, get_ctx, response())
                .await
        } else {
            match self
                .replay_access_deferred(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                DeferredCallReplayOutcome::Replayed(recorded, delivery) => Ok((recorded, delivery)),
                DeferredCallReplayOutcome::Incomplete(call) => {
                    call.complete_access_deferred(store, get_ctx, response())
                        .await
                }
            }
        }
    }

    pub(crate) async fn finish_access_deferred_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: impl FnOnce() -> (Pair::Resp, golem_common::model::oplog::SpanFinished),
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if self.is_live() {
            let (response, span_finished) = response();
            self.complete_access_deferred_with_span(store, get_ctx, response, span_finished)
                .await
        } else {
            match self
                .replay_access_deferred(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                DeferredCallReplayOutcome::Replayed(recorded, delivery) => Ok((recorded, delivery)),
                DeferredCallReplayOutcome::Incomplete(call) => {
                    let (response, span_finished) = response();
                    call.complete_access_deferred_with_span(store, get_ctx, response, span_finished)
                        .await
                }
            }
        }
    }

    /// Replays a call whose protocol requires a recorded terminal rather than live repair.
    pub async fn replay_expecting_completion<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        match self.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => Ok(response),
            CallReplayOutcome::Incomplete(mut handle) => {
                handle.abandon_for_trap();
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End or Cancelled",
                    "an incomplete non-re-executable durable call".to_string(),
                ))
            }
        }
    }

    /// Replays a recorded response or completes the original incomplete call with a typed
    /// failure. This grants no permission to repeat the external effect.
    pub async fn replay_or_persist_failure<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        failure: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        self.reproduce_refusal_for_claimed_start(ctx).await?;
        let replay = self
            .replay
            .take()
            .expect("response-only repair requires replay");
        let outcome = ctx
            .state
            .replay_state
            .await_resolution_outcome(replay)
            .await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                if matches!(disposition, ReplayDeliveryDisposition::AtMarker(_)) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "a direct host-return terminal",
                        "a completion-delivery marker".to_string(),
                    ));
                }
                let response = decode_replayed_payload::<Pair>(
                    &ctx.state.oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                ctx.finish_scope(self.retry.function_type().clone()).await?;
                Ok(response)
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(ctx).await?;
                if let Err(error) = ctx.publish_direct_replay_tail(self.start_idx).await {
                    self.finished = true;
                    return Err(error);
                }
                if let Some((response, span_finished)) = self.recorded_denied_response(ctx).await? {
                    return self
                        .complete_context(ctx, response, span_finished, None, true)
                        .await;
                }
                self.complete_context(ctx, failure, None, None, true).await
            }
        }
    }

    /// The response of an incomplete call whose `Start` records a denial: the host denied the
    /// call before it ran, so the repair completes the `Start` with the denied response and does
    /// not run the operation. A `Start` that opens a span gets the span transition that closes it
    /// as denied, as the live denial does. `None` when the `Start` records no denial.
    async fn recorded_denied_response<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<
        Option<(Pair::Resp, Option<golem_common::model::oplog::SpanFinished>)>,
        WorkerExecutorError,
    > {
        let oplog = access.with_ctx(|ctx| ctx.state.oplog.clone());
        let start_idx = self.start_idx;
        let result = async {
            let (request, denial, span_started) = match oplog.read(start_idx).await? {
                OplogEntry::Start { denial: None, .. } => return Ok(None),
                OplogEntry::Start {
                    request: Some(request),
                    denial: Some(denial),
                    span_started,
                    ..
                } => (request, denial, span_started),
                entry => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("denied {} Start with a request", Pair::FQFN),
                        format!("{entry:?}"),
                    ));
                }
            };
            let request = oplog.download_payload(request).await.map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to load the request of the denied {} call at {}: {error}",
                    Pair::FQFN,
                    start_idx
                ))
            })?;
            let request = Pair::Req::try_from(request).map_err(|actual| {
                WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN.to_string(), actual)
            })?;
            let span_finished =
                span_started.map(|started| golem_common::model::oplog::SpanFinished {
                    span_id: started.span_id,
                    finished_at: Timestamp::now_utc(),
                    outcome: golem_common::model::oplog::SpanOutcome::Denied,
                });
            Pair::denied_response(&request, &denial)
                .map(|response| Some((response, span_finished)))
                .ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        format!("{} Start without a denial", Pair::FQFN),
                        format!("denial at {}: {denial}", start_idx),
                    )
                })
        }
        .await;
        if result.is_err() {
            self.abandon_for_trap();
        }
        result
    }
}

impl
    DurableCallSession<
        golem_common::model::oplog::host_functions::GolemEntityInvoke,
        LeaveIncompleteOnDrop,
    >
{
    pub(crate) fn accept_entity_execution_ownership(&mut self) {
        self.entity_admission = false;
    }

    fn ensure_owned_entity_scope(&self) -> Result<(), WorkerExecutorError> {
        // Entity sessions are WriteLocal. Non-forced scope finish is therefore a no-op, just
        // as in end_durable_function_access; owned settlement must never commit a remote scope.
        if self.retry.function_type() != &DurableFunctionType::WriteLocal {
            return Err(WorkerExecutorError::runtime(
                "owned entity settlement requires WriteLocal",
            ));
        }
        Ok(())
    }

    pub(crate) async fn complete_entity_owned<Ctx: WorkerCtx>(
        mut self,
        context: &EntitySettlementContext<Ctx>,
        response: golem_common::model::oplog::HostResponseEntityInvocation,
        span: golem_common::model::oplog::SpanFinished,
    ) -> Result<golem_common::model::oplog::HostResponseEntityInvocation, WorkerExecutorError> {
        self.ensure_owned_entity_scope()?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "owned completion requires a live entity session",
            ));
        }
        let (response, mut guard) = self
            .persist_guarded_terminal(context.oplog.clone(), None, response, Some(span))
            .await?;
        guard.disarm();
        Ok(response)
    }

    pub(crate) async fn cancel_entity_owned<Ctx: WorkerCtx>(
        mut self,
        context: &EntitySettlementContext<Ctx>,
        response: golem_common::model::oplog::HostResponseEntityInvocation,
    ) -> Result<(), WorkerExecutorError> {
        self.ensure_owned_entity_scope()?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "owned cancellation requires a live entity session",
            ));
        }
        let mut guard = self.terminal_guard();
        let result = if self.persisted {
            Self::persist_cancellation(context.oplog.clone(), &mut guard, Some(response)).await
        } else {
            Ok(())
        };
        guard.disarm();
        result
    }

    pub(crate) async fn replay_entity_owned<Ctx: WorkerCtx>(
        mut self,
        context: &EntitySettlementContext<Ctx>,
        completed: bool,
    ) -> Result<
        ReconstructionReplayOutcome<
            golem_common::model::oplog::host_functions::GolemEntityInvoke,
            LeaveIncompleteOnDrop,
        >,
        WorkerExecutorError,
    > {
        self.ensure_owned_entity_scope()?;
        if completed {
            let (response, cancelled) = self
                .resolve_completed_reconstruction_to(context.replay.clone(), context.oplog.clone())
                .await?;
            return Ok(if cancelled {
                ReconstructionReplayOutcome::Cancelled(response)
            } else {
                ReconstructionReplayOutcome::Replayed(response)
            });
        }
        let replay = self
            .replay
            .take()
            .expect("owned replay requires replay session");
        let outcome = context.replay.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                if matches!(disposition, ReplayDeliveryDisposition::AtMarker(_)) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "internal entity terminal without delivery marker",
                        "CompletionDelivered for internal entity invocation",
                    ));
                }
                let cancelled = matches!(payload, ReplayedPayload::CancelledPartial(_));
                let response = decode_replayed_payload::<
                    golem_common::model::oplog::host_functions::GolemEntityInvoke,
                >(
                    &context.oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                Ok(if cancelled {
                    ReconstructionReplayOutcome::Cancelled(response)
                } else {
                    ReconstructionReplayOutcome::Replayed(response)
                })
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                if let Err(error) = self
                    .prepare_incomplete_live_repair(|| context.counter.clone())
                    .await
                {
                    context.replay.abandon_replay_for_failure().await?;
                    context.authoritative_failure.store(true, Ordering::Release);
                    return Err(error);
                }
                let transition = match context.publish_live(self.start_idx).await {
                    Ok(transition) => transition,
                    Err(error) => {
                        self.abandon_for_trap();
                        return Err(error);
                    }
                };
                Ok(match transition {
                    FinishReplayToLive::Cancelled => {
                        ReconstructionReplayOutcome::LiveAdmissionCancelled(self)
                    }
                    FinishReplayToLive::Live | FinishReplayToLive::AttachmentAdmissionRejected => {
                        ReconstructionReplayOutcome::Incomplete(self)
                    }
                })
            }
        }
    }
}

/// Parks a replaying call forever, after the caller marked its replay as undelivered: the
/// recorded run never delivered this terminal to the guest, which will therefore drop this
/// future at the same point it did live. This future never completes; the return type exists
/// only so a `match` arm can produce whatever type its siblings do.
async fn park_undelivered_forever<T>() -> T {
    let never: std::convert::Infallible = std::future::pending().await;
    match never {}
}

impl UndeliveredTerminal {
    fn log_parking(&self) {
        match self {
            UndeliveredTerminal::CancelledWithoutPartial { cancelled_idx } => {
                tracing::debug!(
                    "durable call cancelled without partial at {cancelled_idx} during replay; \
                     parking until the guest drops it"
                );
            }
            UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                ..
            } => {
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); parking until the guest drops it"
                );
            }
        }
    }

    /// The replay-divergence error the direct (p2) path reports for an undelivered terminal.
    /// The direct path is serialized by the store borrow and delivers at the resolution point,
    /// so a terminal that was never delivered to the guest cannot legally occur on it: an
    /// undelivered cancellation means the future was dropped mid-flight (accessor-only), and
    /// discarded completions are recorded only on the accessor completion path — calls replay
    /// through the same path they recorded on.
    pub(in crate::durable_host::concurrent) fn direct_divergence_error(
        &self,
    ) -> WorkerExecutorError {
        match self {
            UndeliveredTerminal::CancelledWithoutPartial { cancelled_idx } => {
                WorkerExecutorError::unexpected_oplog_entry(
                    "End or Cancelled { partial: Some(..) }",
                    format!("Cancelled without partial at {cancelled_idx}"),
                )
            }
            UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                ..
            } => WorkerExecutorError::unexpected_oplog_entry(
                "End delivered to the guest",
                format!(
                    "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a non-accessor durable call"
                ),
            ),
        }
    }
}

/// Pure classification of a replayed durable-call resolution outcome, shared by the direct,
/// accessor, and deferred-accessor replay paths; the per-path policies (divergence vs park vs
/// deferred re-delivery, store access, scope close, park bookkeeping) stay in the respective
/// methods.
pub(in crate::durable_host::concurrent) fn classify_replay_resolution(
    outcome: ResolutionOutcome,
) -> ReplayedResolution {
    match outcome {
        ResolutionOutcome::Incomplete => ReplayedResolution::Incomplete,
        ResolutionOutcome::Resolved(Resolution::Completed {
            response,
            delivery_marker,
            ..
        }) => ReplayedResolution::Delivered(
            ReplayedPayload::Completed(response),
            match delivery_marker {
                Some(marker_idx) => ReplayDeliveryDisposition::AtMarker(marker_idx),
                // A completed `End` without a delivery marker: the recorded run crashed after
                // the `End` became durable but before the completion crossed to the guest.
                None => ReplayDeliveryDisposition::AtReplayTail,
            },
        ),
        ResolutionOutcome::Resolved(Resolution::Cancelled {
            cancelled_idx,
            partial,
        }) => match partial {
            Some(payload) => ReplayedResolution::Delivered(
                ReplayedPayload::CancelledPartial(payload),
                // A cancellation's partial result is delivered at the guest-initiated (and
                // therefore deterministic) cancellation point: never marker-bearing, no gating.
                ReplayDeliveryDisposition::Immediate,
            ),
            // `Cancelled` with no partial result: in the recorded run this call never returned a
            // value to the guest — its future was dropped mid-flight.
            None => ReplayedResolution::Undelivered(UndeliveredTerminal::CancelledWithoutPartial {
                cancelled_idx,
            }),
        },
        // The recorded run persisted a successful `End`, but a `CompletionDiscarded` marker
        // records that the guest dropped the completion future before the response was
        // delivered.
        ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            response,
        }) => ReplayedResolution::Undelivered(UndeliveredTerminal::CompletionDiscarded {
            end_idx,
            marker_idx,
            response,
        }),
    }
}

/// Decodes a replayed delivery's recorded payload into the call's typed response.
async fn decode_replayed_payload<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    payload: ReplayedPayload,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    match payload {
        ReplayedPayload::Completed(response) => {
            decode_completed_response::<Pair>(oplog, response, retry_from).await
        }
        ReplayedPayload::CancelledPartial(payload) => {
            download_and_decode_response::<Pair>(
                oplog,
                payload,
                "Cancelled partial payload",
                retry_from,
            )
            .await
        }
    }
}

/// Downloads a recorded response payload and decodes it into the call's typed response,
/// preserving the canonical error classification: backend unavailability requires reconstruction,
/// corrupt or missing data and type mismatches are unexpected oplog entries.
async fn download_and_decode_response<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    payload: OplogPayload<HostResponse>,
    payload_kind: &str,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    let host_response = oplog
        .download_payload_classified(payload)
        .await
        .map_err(|error| match error {
            crate::services::oplog::OplogPayloadDownloadError::Backend(error) => {
                WorkerExecutorError::recovery_required_from(
                    format!("{payload_kind} cannot be downloaded: {error}"),
                    retry_from,
                )
            }
            crate::services::oplog::OplogPayloadDownloadError::Corrupt(error) => {
                WorkerExecutorError::unexpected_oplog_entry(
                    "valid durable call response payload",
                    format!("{payload_kind} is corrupt: {error}"),
                )
            }
        })?;
    host_response
        .try_into()
        .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))
}

/// Decodes a replayed successful completion's response: requires the recorded `End` to carry a
/// response payload, then downloads and decodes it via [`download_and_decode_response`].
pub(super) async fn decode_completed_response<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    response: Option<OplogPayload<HostResponse>>,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    let payload = response.ok_or_else(|| {
        WorkerExecutorError::unexpected_oplog_entry(
            "End { response: Some(..) }",
            "End { response: None }".to_string(),
        )
    })?;
    download_and_decode_response::<Pair>(oplog, payload, "End payload", retry_from).await
}

/// Validates a replay-side cancellation: the recorded resolution must be `Cancelled`. A recorded
/// completion (delivered or discarded) means the replayed guest cancelled a call that the
/// recorded run completed — a replay divergence.
pub(super) fn expect_cancelled_resolution(
    resolution: &Resolution,
) -> Result<(), WorkerExecutorError> {
    match resolution {
        Resolution::Completed { end_idx, .. }
        | Resolution::CompletedButDiscarded { end_idx, .. } => Err(
            WorkerExecutorError::unexpected_oplog_entry("Cancelled", format!("End at {end_idx}")),
        ),
        Resolution::Cancelled { .. } => Ok(()),
    }
}
