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

//! Concurrent-replay core for durable host calls.
//!
//! A durable host call is identified by the [`OplogIndex`] of its `Start` entry. While live,
//! the call eagerly appends a `Start` (capturing its request) and later an `End` (its response)
//! or a `Cancelled`. During replay the [`ConcurrentReplayResolver`] matches each completed
//! `End`/`Cancelled` back to the awaiting [`CallHandle`] via a [`ReplayableOneshot`], so the two
//! halves of a call no longer have to be adjacent in the oplog — which is what lets us track
//! async, parallel host functions.
//!
//! Every durable host call runs through this path via [`CallHandle`]. Because the ported host
//! methods still take `&mut self`, two calls cannot truly overlap yet, so the resolver's
//! out-of-order behaviour is proven by synthetic unit tests rather than a concurrent runtime test.

use std::collections::HashMap;
use std::marker::PhantomData;

use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, PersistenceLevel,
};
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durability::{
    DurabilityHost, HostFailureKind, InFunctionRetryController, InFunctionRetryHost,
    InternalRetryResult,
};
use crate::services::oplog::OplogOps;
use crate::workerctx::WorkerCtx;
use std::fmt::Display;

/// Replayable single-shot channel used to deliver a call's [`Resolution`] from the replay cursor
/// to the awaiting [`CallHandle`].
///
/// `tokio::sync::oneshot` already supports send-before-await, which is all this currently needs.
/// The only "resolve happened before the awaiter registered" case is handled by the resolver's
/// `buffered` map, not by the channel. This is kept behind a type alias so it can later be swapped
/// for a dedicated replayable primitive.
pub type ReplayableOneshot<T> = oneshot::Sender<T>;
pub type ReplayableOneshotReceiver<T> = oneshot::Receiver<T>;

/// The outcome of a durable call as observed while replaying the oplog.
///
/// The entry index is carried purely for validation and diagnostics.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// The call completed successfully via an `End` entry.
    Completed {
        end_idx: OplogIndex,
        response: Option<OplogPayload<HostResponse>>,
        forced_commit: bool,
    },
    /// The call was cancelled (dropped before completion) via a `Cancelled` entry.
    Cancelled {
        cancelled_idx: OplogIndex,
        partial: Option<OplogPayload<HostResponse>>,
    },
}

/// The outcome of driving the replay cursor for a durable call.
///
/// With eager `Start` every durable call writes its `Start` before the side effect, so a forced
/// commit elsewhere can make a lone `Start` durable before its `End`. When replay reaches the end
/// of the oplog without ever seeing the matching `End`/`Cancelled`, the call is reported as
/// [`ResolutionOutcome::Incomplete`] so the caller can re-execute it live and complete the existing
/// `Start`, instead of failing the whole replay.
#[derive(Debug)]
pub enum ResolutionOutcome {
    /// The call's `End`/`Cancelled` was observed during replay.
    Resolved(Resolution),
    /// Replay reached the end of the oplog (now live) without the call's `End`/`Cancelled`.
    Incomplete,
}

/// The result of [`CallHandle::replay`].
pub enum CallReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    /// The call's `End` was replayed and decoded into its response.
    Replayed(Pair::Resp),
    /// The call's `Start` was committed but its `End` never was. The returned handle has been
    /// switched to live completion of that existing `Start`: the caller must re-run the side effect
    /// and call [`CallHandle::complete`] (which appends the missing `End`). Only produced for
    /// function types that are safe to re-execute.
    Incomplete(CallHandle<Pair, P>),
}

/// Matches replayed `End`/`Cancelled` entries back to the [`CallHandle`]s awaiting them, keyed by
/// the `OplogIndex` of the call's `Start`.
///
/// Lives inside the replay state behind its lock. It is fed **only** from the committed-consume
/// hook (see [`crate::durable_host::replay_state::ReplayState`]); speculative cursor reads that
/// roll back must never reach it.
#[derive(Debug, Default)]
pub struct ConcurrentReplayResolver {
    /// Awaiters that have registered but whose resolution has not been observed yet.
    pending: HashMap<OplogIndex, oneshot::Sender<Resolution>>,
    /// Resolutions observed before their awaiter registered. While durable host calls are
    /// serialized this is always empty (the await-resolution guard guarantees a call's `Start` is
    /// claimed before its `End`/`Cancelled` is consumed); it exists for the resolver's own unit
    /// tests and for once host calls can genuinely overlap and that order is no longer guaranteed.
    buffered: HashMap<OplogIndex, Resolution>,
}

impl ConcurrentReplayResolver {
    /// Registers an awaiter for the call started at `start_idx` and returns the receiver it should
    /// await on. If the resolution was already observed (buffered), the returned receiver is
    /// pre-resolved.
    pub fn register(&mut self, start_idx: OplogIndex) -> oneshot::Receiver<Resolution> {
        let (tx, rx) = oneshot::channel();
        if let Some(resolution) = self.buffered.remove(&start_idx) {
            let _ = tx.send(resolution);
        } else {
            // A `Start` index is claimed (and thus registered) exactly once: claiming advances the
            // positional cursor past that `Start`. A second registration for the same index would
            // mean two awaiters for one call, silently dropping the first.
            debug_assert!(
                !self.pending.contains_key(&start_idx),
                "duplicate awaiter registered for Start at {start_idx}"
            );
            self.pending.insert(start_idx, tx);
        }
        rx
    }

    /// Resolves a registered awaiter, or buffers the resolution if none is registered yet.
    ///
    /// Test-only seam exercising the buffered (resolve-before-register) branch directly. The
    /// production replay path uses [`Self::resolve_if_pending`] instead, so that resolutions for
    /// calls nobody is awaiting are dropped rather than accumulating in `buffered`.
    #[cfg(test)]
    pub fn resolve(&mut self, start_idx: OplogIndex, resolution: Resolution) {
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(resolution);
        } else {
            self.buffered.insert(start_idx, resolution);
        }
    }

    /// Resolves a registered awaiter if (and only if) one exists, returning whether it did.
    ///
    /// This is the only entry point used by the committed-consume replay hook: an `End`/`Cancelled`
    /// for a call nobody is awaiting — e.g. the guest-facing manual durability pair written by
    /// `persist_durable_function_invocation`, which is consumed through the same cursor but never
    /// registers an awaiter — is silently ignored rather than buffered forever.
    pub fn resolve_if_pending(&mut self, start_idx: OplogIndex, resolution: Resolution) -> bool {
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(resolution);
            true
        } else {
            false
        }
    }

    /// Removes a registered awaiter without resolving it. Used when a claimed call turns out to be
    /// incomplete on replay (its `Start` is committed but its `End` never was): the awaiter is
    /// switched to live completion, so its pending registration must not linger in the resolver.
    pub fn unregister(&mut self, start_idx: OplogIndex) {
        self.pending.remove(&start_idx);
    }

    /// Returns whether an awaiter is currently registered for `start_idx`.
    #[cfg(test)]
    pub fn is_pending(&self, start_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx)
    }
}

/// Replay-side state for a single in-flight call: the `Start` index it claimed and the receiver
/// that will deliver its [`Resolution`].
#[derive(Debug)]
pub struct ReplayCallHandle {
    start_idx: OplogIndex,
    receiver: oneshot::Receiver<Resolution>,
}

impl ReplayCallHandle {
    pub fn new(start_idx: OplogIndex, receiver: oneshot::Receiver<Resolution>) -> Self {
        Self {
            start_idx,
            receiver,
        }
    }

    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    /// Consumes the handle into its parts (used by the replay-state driver).
    pub fn into_parts(self) -> (OplogIndex, oneshot::Receiver<Resolution>) {
        (self.start_idx, self.receiver)
    }
}

/// Event emitted when a [`CallHandle`] is dropped without being finished or cancelled.
///
/// There is no recorder actor in production yet, so an unfinished drop only logs. The drop policy
/// is exercised in unit tests by attaching a sink that records these events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropEvent {
    /// A `Cancellable` handle was dropped unfinished; a real recorder would enqueue a `Cancelled`.
    UnfinishedCancellable { start_idx: OplogIndex },
    /// A `NotCancellable` handle was dropped unfinished; this is a programming error.
    UnfinishedNotCancellable { start_idx: OplogIndex },
}

/// Compile-time policy describing what happens when a [`CallHandle`] is dropped without being
/// explicitly finished or cancelled.
pub trait DropPolicy {
    fn unfinished_drop(start_idx: OplogIndex, sink: Option<&UnboundedSender<DropEvent>>);
}

/// Drop policy for calls that may legitimately be cancelled (dropped from a `select!`, etc.).
pub struct Cancellable;

/// Drop policy for calls that must always be finished or explicitly cancelled. Dropping one
/// unfinished is a bug (default-deny).
pub struct NotCancellable;

impl DropPolicy for Cancellable {
    fn unfinished_drop(start_idx: OplogIndex, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            let _ = sink.send(DropEvent::UnfinishedCancellable { start_idx });
        } else {
            tracing::warn!(
                "durable call {start_idx} dropped unfinished; no production cancellation recorder yet"
            );
        }
    }
}

impl DropPolicy for NotCancellable {
    fn unfinished_drop(start_idx: OplogIndex, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            let _ = sink.send(DropEvent::UnfinishedNotCancellable { start_idx });
        } else if cfg!(debug_assertions) && !std::thread::panicking() {
            panic!("non-cancellable durable call {start_idx} dropped without finish/cancel");
        } else {
            tracing::error!(
                "non-cancellable durable call {start_idx} dropped without finish/cancel"
            );
        }
    }
}

/// A handle to one durable host call.
///
/// Created by [`CallHandle::start`], which eagerly appends the call's `Start` (live) or claims it
/// from the oplog (replay). The call is finished with [`CallHandle::complete`] (live) /
/// [`CallHandle::replay`] (replay), or cancelled with [`CallHandle::cancel`]. All terminal methods
/// consume the handle so a call cannot be finished twice; an unfinished drop runs the `P` drop
/// policy.
pub struct CallHandle<Pair: HostPayloadPair, P: DropPolicy> {
    start_idx: OplogIndex,
    /// The index returned by `begin_durable_function` / `begin_function`. For a non-idempotent
    /// `WriteRemote` (or a `WriteRemoteBatched(None)`) this is the **durable scope** `Start` that
    /// must be closed via `end_durable_function`; for every other function type it is just the
    /// pre-call index and `end_durable_function` only uses it to commit at the right boundary.
    begin_index: OplogIndex,
    is_live: bool,
    /// `true` when a `Start` entry was actually appended. It is `false` while snapshotting (where
    /// nothing is persisted) and for replay handles.
    persisted: bool,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    replay: Option<ReplayCallHandle>,
    finished: bool,
    /// The `current_retry_point` value to restore when the call finishes.
    saved_retry_point: OplogIndex,
    /// In-function retry decision logic. Also the home of the call's `DurableFunctionType` and
    /// captured `DurableExecutionState`.
    retry: InFunctionRetryController,
    /// Optional sink used by unit tests to observe unfinished-drop behaviour. `None` in production.
    drop_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy> CallHandle<Pair, P> {
    /// Begins a durable call.
    ///
    /// Observes the function call, then runs `begin_durable_function` — which applies the read-only
    /// side-effect guard, drains pending replay events, and (for a non-idempotent `WriteRemote` /
    /// `WriteRemoteBatched(None)`) opens the durable scope and runs the replay-side "operation was
    /// not completed" recovery. Then, (live) upload the request and append the eager host-call
    /// `Start`, or (replay) claim the next host-call `Start` and register a resolver receiver for it.
    ///
    /// Reusing `begin_durable_function`/`end_durable_function` (rather than re-deriving scope logic
    /// here) keeps the scope semantics consistent by construction: the same scope `Start`/`End`,
    /// the same `parent_start_index` nesting, the same commit/checkpoint boundaries.
    pub async fn start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        let begun = Self::begin(ctx, function_type).await?;
        if begun.is_live() {
            begun.start_live(ctx, request).await
        } else {
            begun.start_replay(ctx).await
        }
    }

    /// First phase of a durable call: run `begin_durable_function` (read-only guard, pending replay
    /// events, durable-scope open + recovery) and capture the retry state, *without* yet writing
    /// the host-call `Start` or claiming it on replay.
    ///
    /// This is the explicit two-phase entry point for the rare "two-step" calls whose request
    /// payload depends on the durable-scope begin index (e.g. an RPC scheduled invocation embeds an
    /// idempotency key derived from it). Such calls cannot use [`Self::start`] because the request
    /// is not yet known when the scope is opened. The common case stays on [`Self::start`], which is
    /// just `begin` + `start_live`/`start_replay`.
    pub async fn begin<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
    ) -> Result<BegunCall<Pair, P>, WorkerExecutorError> {
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);

        // Read-only guard, pending replay events and durable-scope open all happen inside
        // `begin_durable_function`.
        let begin_index = ctx
            .begin_durable_function(&function_type, Pair::FQFN)
            .await?;
        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let saved_retry_point = ctx.state.current_retry_point;
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN);

        Ok(BegunCall {
            begin_index,
            saved_retry_point,
            retry,
            _phantom: PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.is_live
    }

    pub fn start_index(&self) -> OplogIndex {
        self.start_idx
    }

    /// The index returned by `begin_durable_function`: the durable scope `Start` for a
    /// non-idempotent `WriteRemote` / `WriteRemoteBatched(None)`, or the pre-call index otherwise.
    /// Used by call sites that derive a stable identifier from that index (e.g. the idempotency-key
    /// derivation).
    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    /// Marks the call as finished without writing anything to the oplog, leaving its host-call
    /// `Start` incomplete on disk. This is the terminal used when a host call traps (fall-back to
    /// oplog replay) or is interrupted: a trap is **not** a cancellation, so it must never write a
    /// `Cancelled`. The incomplete `Start` is resolved on the next replay/retry (see
    /// [`CallReplayOutcome::Incomplete`]).
    ///
    /// It deliberately does **not** restore `current_retry_point`, so the failure stays grouped
    /// against this call's `Start` (or the active durable scope, via `effective_retry_point`) for
    /// the trap-recovery decision.
    pub fn abandon_for_trap(&mut self) {
        self.finished = true;
    }

    /// Retry wrapper around [`InFunctionRetryController::try_trigger_retry`]. On the `Err` branch
    /// (a trap is being raised to trigger an oplog-level retry) it automatically
    /// [`abandon_for_trap`](Self::abandon_for_trap)s, so `?`-style call sites stay correct without
    /// hitting the `NotCancellable` unfinished-drop guard.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &mut self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<()> {
        let outcome = self.retry.try_trigger_retry(ctx, result, classify).await;
        if outcome.is_err() {
            self.abandon_for_trap();
        }
        outcome
    }

    pub async fn try_trigger_retry_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        let outcome = self
            .retry
            .try_trigger_retry_with_properties(ctx, result, classify, properties)
            .await;
        if outcome.is_err() {
            self.abandon_for_trap();
        }
        outcome
    }

    pub async fn try_trigger_retry_or_loop<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        let outcome = self
            .retry
            .try_trigger_retry_or_loop(ctx, result, classify)
            .await;
        if outcome.is_err() {
            self.abandon_for_trap();
        }
        outcome
    }

    pub async fn try_trigger_retry_or_loop_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<InternalRetryResult> {
        let outcome = self
            .retry
            .try_trigger_retry_or_loop_with_properties(ctx, result, classify, properties)
            .await;
        if outcome.is_err() {
            self.abandon_for_trap();
        }
        outcome
    }

    /// Drives the full live / replay / incomplete-replay flow for a **re-executable** durable call
    /// (reads and idempotent writes), inverting control: the actual side effect is supplied as
    /// `live_action` and run only when needed. It runs on the live path, and again only if replay
    /// finds the eager `Start` committed without its `End` (re-executing to complete that `Start`);
    /// when the call replays from a committed `End`, `live_action` is not run at all.
    ///
    /// On a `live_action` error the handle is abandoned for trap (its `Start` left incomplete, never
    /// a `Cancelled`) and the error is returned, so the propagating `?` cannot trip the
    /// unfinished-drop policy. `E` is the call site's native error (e.g. `wasmtime::Error`); it only
    /// needs to absorb a `WorkerExecutorError` from the durability machinery.
    ///
    /// This is the optional Shape-A combinator that removes the live/replay/incomplete boilerplate.
    /// Sites with retry loops or bespoke control flow keep the explicit `start` / `complete` /
    /// `replay` form.
    pub async fn run<Ctx, A, E>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: From<WorkerExecutorError>,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        debug_assert!(
            self.retry.can_reexecute_on_incomplete_replay(),
            "CallHandle::run is only valid for re-executable calls (reads / idempotent writes); \
             use start/complete/replay explicitly for non-idempotent / batched / transaction writes"
        );
        if self.is_live() {
            self.run_live_action(ctx, live_action).await
        } else {
            match self.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => Ok(response),
                CallReplayOutcome::Incomplete(handle) => {
                    handle.run_live_action(ctx, live_action).await
                }
            }
        }
    }

    /// Runs `live_action` and either completes the call with its response or, on error, abandons the
    /// call for trap and propagates the error. Shared by both [`Self::run`] paths.
    async fn run_live_action<Ctx, A, E>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: From<WorkerExecutorError>,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        match live_action(ctx).await {
            Ok(response) => Ok(self.complete(ctx, response).await?),
            Err(err) => {
                self.abandon_for_trap();
                Err(err)
            }
        }
    }

    /// Completes a live call: upload the response, append the matching host-call `End`, then close
    /// the durable scope / commit / checkpoint via `end_durable_function`.
    pub async fn complete<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(self.is_live, "complete() called on a replay handle");
        // This is the call's legitimate terminal; mark it finished up front so that a failure of the
        // downstream commit / scope close (`?` below) does not drop the handle "unfinished" and trip
        // the drop policy. The host-call `End` is what makes the call durable, not these follow-ups.
        self.finished = true;
        if self.persisted {
            let host_response: HostResponse = response.clone().into();
            let response_payload = ctx
                .state
                .oplog
                .upload_payload(&host_response)
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize and store durable call response: {err}"
                    ))
                })?;
            ctx.state.mark_atomic_region_has_side_effects();
            let end = OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: self.start_idx,
                response: Some(response_payload),
                forced_commit: false,
            };
            ctx.state.oplog.add(end).await;
            // Close the durable scope (if one was opened), commit at the right boundary, and run the
            // mid-invocation checkpoint, all via `end_durable_function`.
            ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                .await?;
        }
        ctx.state.current_retry_point = self.saved_retry_point;
        Ok(response)
    }

    /// Replays a call: drive the cursor until the call resolves, decode its response, then close the
    /// durable scope / commit via `end_durable_function`.
    ///
    /// If replay reaches the end of the oplog without ever seeing the matching `End`/`Cancelled` —
    /// a lone committed host-call `Start`, now possible for any write because `Start` is eager —
    /// the call is returned as [`CallReplayOutcome::Incomplete`] (for function types that are safe
    /// to re-execute) so the caller can re-run the side effect and `complete` the existing `Start`.
    /// For non-idempotent / batched / transaction writes re-execution is unsafe, so the same hard
    /// error as before is returned: those are protected by the surrounding durable-scope recovery
    /// run in [`Self::start`] (`begin_durable_function`), not by silent re-execution here.
    pub async fn replay<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        let replay = self
            .replay
            .take()
            .expect("replay() called on a live handle");
        let start_idx = self.start_idx;
        let outcome = ctx
            .state
            .replay_state
            .await_resolution_outcome(replay)
            .await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                // Terminal: mark finished up front so a decode / scope-close failure below does not
                // drop the (replay) handle as "unfinished".
                self.finished = true;
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let host_response =
                    ctx.state
                        .oplog
                        .download_payload(payload)
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "End payload cannot be downloaded: {err}"
                            ))
                        })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
                ctx.state.current_retry_point = self.saved_retry_point;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. }) => {
                self.finished = true;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End",
                    format!("Cancelled at {cancelled_idx}"),
                ))
            }
            ResolutionOutcome::Incomplete => {
                if self.retry.can_reexecute_on_incomplete_replay() {
                    // Switch the handle to live completion of the existing, committed `Start`: the
                    // caller re-runs the side effect and `complete`s, appending the missing `End`.
                    // `current_retry_point` is intentionally left at this `Start` (set in `start`),
                    // so a failure during re-execution stays grouped here.
                    self.is_live = true;
                    self.persisted = true;
                    Ok(CallReplayOutcome::Incomplete(self))
                } else {
                    // Re-executing a non-idempotent / batched / transaction write could duplicate an
                    // external side effect. Reaching here means the surrounding scope recovery did
                    // not already resolve it; fail hard, as before.
                    self.finished = true;
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled",
                        format!(
                            "incomplete non-idempotent durable call Start at {start_idx} cannot be safely re-executed"
                        ),
                    ))
                }
            }
        }
    }

    /// Replays a **non-re-executable** call (batched / transaction writes), where an incomplete
    /// `Start` cannot be safely re-run. For these function types [`Self::replay`] never yields
    /// [`CallReplayOutcome::Incomplete`] — it hard-errors first — so this optional combinator
    /// collapses the outcome to the replayed response, removing the dead `Incomplete` arm from the
    /// call sites. Re-executable sites must use [`Self::run`] or the explicit [`Self::replay`] match
    /// instead.
    pub async fn replay_expecting_completion<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(
            !self.retry.can_reexecute_on_incomplete_replay(),
            "replay_expecting_completion is only valid for non-re-executable calls (batched / \
             transaction writes); re-executable calls must use run() or replay() explicitly"
        );
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

    /// Cancels a call.
    ///
    /// Live: append a `Cancelled` entry. Replay: expect the call to resolve as `Cancelled`. The
    /// retry point is intentionally left pointing at this call's `Start` on the live path, so a
    /// host error propagating after cancellation is grouped against this call.
    pub async fn cancel<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError> {
        // Terminal: mark finished up front so a fallible step below does not drop the handle as
        // "unfinished".
        self.finished = true;
        if self.is_live {
            if self.persisted {
                let partial_payload = match partial {
                    Some(partial) => {
                        let host_response: HostResponse = partial.into();
                        Some(ctx.state.oplog.upload_payload(&host_response).await.map_err(
                            |err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to serialize and store partial durable call response: {err}"
                                ))
                            },
                        )?)
                    }
                    None => None,
                };
                let cancelled = OplogEntry::Cancelled {
                    timestamp: Timestamp::now_utc(),
                    start_index: self.start_idx,
                    partial: partial_payload,
                };
                ctx.state.oplog.add(cancelled).await;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            if let Resolution::Completed { end_idx, .. } = resolution {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "Cancelled",
                    format!("End at {end_idx}"),
                ));
            }
            ctx.state.current_retry_point = self.saved_retry_point;
        }
        Ok(())
    }
}

/// The first phase of a two-phase durable call, produced by [`CallHandle::begin`]. The durable
/// scope is already open and the begin index is known; the host-call `Start` has not yet been
/// written (live) nor claimed (replay). Finalised into a [`CallHandle`] with [`Self::start_live`]
/// (after the request has been built) or [`Self::start_replay`].
pub struct BegunCall<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    saved_retry_point: OplogIndex,
    retry: InFunctionRetryController,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy> BegunCall<Pair, P> {
    pub fn is_live(&self) -> bool {
        self.retry.durable_execution_state().is_live
    }

    /// The index returned by `begin_durable_function` — see [`CallHandle::begin_index`]. Available
    /// before the request is finalised, so a two-step call can derive its request payload from it.
    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    /// Second phase on the live path: upload the request and append the eager host-call `Start`
    /// (or, while snapshotting, persist nothing).
    pub async fn start_live<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
    ) -> Result<CallHandle<Pair, P>, WorkerExecutorError> {
        debug_assert!(self.is_live(), "start_live() called on a replay handle");
        let snapshotting = self
            .retry
            .durable_execution_state()
            .snapshotting_mode
            .is_some();
        // The host-call `Start` nests inside the enclosing durable scope (its own opened scope, or
        // the scope encoded in the function type), derived explicitly — never from the set of
        // temporally-open sibling scopes. `None` for a top-level unscoped call.
        let parent_start_index = ctx
            .state
            .child_parent_start_index(self.retry.function_type(), self.begin_index);
        let (start_idx, persisted) = if snapshotting {
            // Snapshotting mode persists nothing.
            (ctx.state.oplog.current_oplog_index().await, false)
        } else {
            let request: HostRequest = request.into();
            let request_payload =
                ctx.state
                    .oplog
                    .upload_payload(&request)
                    .await
                    .map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "failed to serialize and store durable call request: {err}"
                        ))
                    })?;
            let start = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index,
                function_name: Pair::HOST_FUNCTION_NAME,
                request: Some(request_payload),
                durable_function_type: self.retry.function_type().clone(),
            };
            let idx = ctx.state.oplog.add(start).await;
            (idx, true)
        };
        // Group a trap during this call against the enclosing scope `Start` if there is one, so the
        // failure is retried from the scope rather than from inside it; otherwise from this call's
        // own `Start`.
        ctx.state.current_retry_point = parent_start_index.unwrap_or(start_idx);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: true,
            persisted,
            replay: None,
            finished: false,
            saved_retry_point: self.saved_retry_point,
            retry: self.retry,
            drop_sink: None,
            _phantom: PhantomData,
        })
    }

    /// Second phase on the replay path: claim the next host-call `Start` from the oplog and register
    /// a resolver receiver for it.
    pub async fn start_replay<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallHandle<Pair, P>, WorkerExecutorError> {
        debug_assert!(!self.is_live(), "start_replay() called on a live handle");
        // Defensive guard, mirroring `read_persisted_durable_function_invocation`.
        if self.retry.durable_execution_state().persistence_level
            == PersistenceLevel::PersistNothing
        {
            return Err(WorkerExecutorError::runtime(
                "Trying to replay a durable invocation in a PersistNothing block",
            ));
        }
        let replay = ctx
            .state
            .replay_state
            .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, self.retry.function_type())
            .await?;
        let start_idx = replay.start_idx();
        // Mirror the live path: group a trap during this call against the enclosing scope `Start`
        // if there is one, otherwise against this call's own `Start`.
        let parent_start_index = ctx
            .state
            .child_parent_start_index(self.retry.function_type(), self.begin_index);
        ctx.state.current_retry_point = parent_start_index.unwrap_or(start_idx);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: false,
            persisted: false,
            replay: Some(replay),
            finished: false,
            saved_retry_point: self.saved_retry_point,
            retry: self.retry,
            drop_sink: None,
            _phantom: PhantomData,
        })
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> Drop for CallHandle<Pair, P> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.is_live {
            if self.persisted {
                // A live call dropped without finish/cancel: run the compile-time drop policy.
                P::unfinished_drop(self.start_idx, self.drop_sink.as_ref());
            }
            // Not persisted (snapshotting): there is nothing on disk to reconcile.
        } else {
            // A replay handle must never enqueue a live cancellation; just note the anomaly.
            tracing::warn!(
                "replay durable call handle for Start {} dropped without finishing",
                self.start_idx
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::host_functions;
    use test_r::test;
    use tokio::sync::mpsc;

    fn idx(n: u64) -> OplogIndex {
        OplogIndex::from_u64(n)
    }

    fn completed(end_idx: u64) -> Resolution {
        Resolution::Completed {
            end_idx: idx(end_idx),
            response: None,
            forced_commit: false,
        }
    }

    // ---- ConcurrentReplayResolver ----

    #[test]
    fn resolver_out_of_order_completion() {
        // [S1, S2, E2, E1]: claim both, then resolve E2 before E1.
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx1 = resolver.register(idx(1));
        let mut rx2 = resolver.register(idx(2));
        assert!(rx1.try_recv().is_err());
        assert!(rx2.try_recv().is_err());

        assert!(resolver.resolve_if_pending(idx(2), completed(3)));
        match rx2.try_recv() {
            Ok(Resolution::Completed { end_idx, .. }) => assert_eq!(end_idx, idx(3)),
            other => panic!("unexpected resolution for h2: {other:?}"),
        }
        assert!(rx1.try_recv().is_err());

        assert!(resolver.resolve_if_pending(idx(1), completed(4)));
        match rx1.try_recv() {
            Ok(Resolution::Completed { end_idx, .. }) => assert_eq!(end_idx, idx(4)),
            other => panic!("unexpected resolution for h1: {other:?}"),
        }
    }

    #[test]
    fn resolver_cancelled() {
        // [S1, Cancelled1]
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx = resolver.register(idx(1));
        assert!(resolver.resolve_if_pending(
            idx(1),
            Resolution::Cancelled {
                cancelled_idx: idx(2),
                partial: None,
            },
        ));
        match rx.try_recv() {
            Ok(Resolution::Cancelled { cancelled_idx, .. }) => assert_eq!(cancelled_idx, idx(2)),
            other => panic!("unexpected resolution: {other:?}"),
        }
    }

    #[test]
    fn resolver_resolve_before_register_buffers() {
        let mut resolver = ConcurrentReplayResolver::default();
        resolver.resolve(idx(1), completed(2));
        assert!(!resolver.is_pending(idx(1)));
        let mut rx = resolver.register(idx(1));
        match rx.try_recv() {
            Ok(Resolution::Completed { end_idx, .. }) => assert_eq!(end_idx, idx(2)),
            other => panic!("expected pre-resolved receiver, got {other:?}"),
        }
    }

    #[test]
    fn resolver_missing_pending_is_dropped_not_buffered() {
        let mut resolver = ConcurrentReplayResolver::default();
        // No registration: resolve_if_pending must not buffer (this is the unregistered-End case,
        // e.g. the guest-facing manual durability pair).
        assert!(!resolver.resolve_if_pending(idx(1), completed(2)));
        let mut rx = resolver.register(idx(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn resolver_duplicate_resolution_is_ignored() {
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx = resolver.register(idx(1));
        assert!(resolver.resolve_if_pending(idx(1), completed(2)));
        // Second resolution: no longer pending.
        assert!(!resolver.resolve_if_pending(idx(1), completed(3)));
        match rx.try_recv() {
            Ok(Resolution::Completed { end_idx, .. }) => assert_eq!(end_idx, idx(2)),
            other => panic!("unexpected resolution: {other:?}"),
        }
    }

    // ---- CallHandle drop policy ----

    fn live_unfinished_handle<P: DropPolicy>(
        start_idx: OplogIndex,
        sink: mpsc::UnboundedSender<DropEvent>,
    ) -> CallHandle<host_functions::MonotonicClockNow, P> {
        use crate::durable_host::durability::DurableExecutionState;
        use std::time::Duration;
        let durable_execution_state = DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: false,
            max_in_function_retry_delay: Duration::ZERO,
        };
        CallHandle {
            start_idx,
            begin_index: start_idx,
            is_live: true,
            persisted: true,
            replay: None,
            finished: false,
            saved_retry_point: OplogIndex::INITIAL,
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                durable_execution_state,
                "test:monotonic_clock::now",
            ),
            drop_sink: Some(sink),
            _phantom: PhantomData,
        }
    }

    #[test]
    fn drop_cancellable_unfinished_enqueues_exactly_one_cancelled() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { start_idx }) => assert_eq!(start_idx, idx(5)),
            other => panic!("expected one UnfinishedCancellable, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "expected exactly one drop event");
    }

    #[test]
    fn drop_not_cancellable_unfinished_signals_policy_violation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<NotCancellable>(idx(7), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedNotCancellable { start_idx }) => assert_eq!(start_idx, idx(7)),
            other => panic!("expected UnfinishedNotCancellable, got {other:?}"),
        }
    }

    #[test]
    fn drop_after_finish_is_noop() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut handle = live_unfinished_handle::<Cancellable>(idx(9), tx);
            handle.finished = true;
        }
        assert!(
            rx.try_recv().is_err(),
            "finished handle must not emit a drop event"
        );
    }

    #[test]
    fn abandon_for_trap_suppresses_unfinished_drop() {
        // A trap-abandoned NotCancellable handle must NOT trip the unfinished-drop policy: the
        // host-call Start is intentionally left incomplete for replay/retry, not cancelled.
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut handle = live_unfinished_handle::<NotCancellable>(idx(11), tx);
            handle.abandon_for_trap();
        }
        assert!(
            rx.try_recv().is_err(),
            "abandon_for_trap must not emit a drop event"
        );
    }

    // ---- function-type re-execution policy ----

    #[test]
    fn can_reexecute_matches_internal_retry_eligibility() {
        use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};
        use std::time::Duration;

        fn controller(
            ft: DurableFunctionType,
            assume_idempotence: bool,
        ) -> InFunctionRetryController {
            InFunctionRetryController::new(
                ft,
                DurableExecutionState {
                    is_live: true,
                    persistence_level: PersistenceLevel::Smart,
                    snapshotting_mode: None,
                    assume_idempotence,
                    max_in_function_retry_delay: Duration::ZERO,
                },
                "test:fn",
            )
        }

        // Reads and local/idempotent writes are safe to re-execute on an incomplete Start.
        assert!(
            controller(DurableFunctionType::ReadLocal, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::ReadRemote, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::WriteLocal, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::WriteRemote, true).can_reexecute_on_incomplete_replay()
        );

        // Non-idempotent / batched / transaction writes are not — neither the `None` (scope-opening)
        // nor the `Some(begin_index)` (in-scope host call) variants. The `Some(..)` variants are the
        // ones a migrated batched/transaction host call carries, so a lone committed host-call
        // `Start` for them must hard-error on incomplete replay rather than re-execute and risk
        // duplicating an external write (`CallHandle::replay` Incomplete arm).
        assert!(
            !controller(DurableFunctionType::WriteRemote, false)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteBatched(None), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteBatched(Some(idx(7))), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteTransaction(None), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(
                DurableFunctionType::WriteRemoteTransaction(Some(idx(9))),
                true
            )
            .can_reexecute_on_incomplete_replay()
        );
    }
}
