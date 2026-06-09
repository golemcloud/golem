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
//! Currently a single host function (`monotonic_clock::now`) uses this path; every other call
//! site stays on the legacy [`crate::durable_host::durability::Durability`] path. Because the
//! ported host method still takes `&mut self`, two calls cannot truly overlap yet, so the
//! resolver's out-of-order behaviour is proven by synthetic unit tests rather than a concurrent
//! runtime test.

use std::collections::HashMap;
use std::marker::PhantomData;

use golem_common::model::Timestamp;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, PersistenceLevel,
};
use golem_service_base::error::worker_executor::{GolemSpecificWasmTrap, WorkerExecutorError};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durability::{DurabilityHost, InFunctionRetryHost, is_write_side_effect};
use crate::services::HasWorker;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;

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
    /// Used by the resolver's unit tests. The production replay path uses
    /// [`Self::resolve_if_pending`] so that resolutions for calls we are not tracking (e.g. legacy
    /// host calls still on the [`crate::durable_host::durability::Durability`] path) are dropped
    /// instead of accumulating in `buffered`.
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
    /// for a call nobody is awaiting (every legacy host call, which is consumed through the same
    /// cursor) is silently ignored rather than buffered forever.
    pub fn resolve_if_pending(&mut self, start_idx: OplogIndex, resolution: Resolution) -> bool {
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(resolution);
            true
        } else {
            false
        }
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
    function_type: DurableFunctionType,
    is_live: bool,
    /// `true` when a `Start` entry was actually appended. It is `false` while snapshotting (where
    /// the legacy path also persisted nothing) and for replay handles.
    persisted: bool,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    replay: Option<ReplayCallHandle>,
    finished: bool,
    /// The `current_retry_point` value to restore when the call finishes.
    saved_retry_point: OplogIndex,
    /// Optional sink used by unit tests to observe unfinished-drop behaviour. `None` in production.
    drop_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy> CallHandle<Pair, P> {
    /// Begins a durable call.
    ///
    /// Mirrors the ordering of the legacy `Durability::new`: observe the function call, apply the
    /// read-only side-effect guard before appending anything, apply any pending replay events, then
    /// (live) upload the request and append the eager `Start`, or (replay) claim the next `Start`
    /// and register a resolver receiver for it.
    pub async fn start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);

        // Central read-only side-effect guard, identical to `begin_durable_function`.
        if is_write_side_effect(&function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = ctx.check_read_only_allows(Pair::FQFN)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }

        ctx.process_pending_replay_events().await?;

        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let saved_retry_point = ctx.state.current_retry_point;

        if durable_execution_state.is_live {
            let snapshotting = durable_execution_state.snapshotting_mode.is_some();
            let (start_idx, persisted) = if snapshotting {
                // Snapshotting mode persists nothing (matching legacy `persist_raw`).
                (ctx.state.oplog.current_oplog_index().await, false)
            } else {
                let parent_start_index = ctx.state.current_parent_start_index();
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
                    durable_function_type: function_type.clone(),
                };
                let idx = ctx.state.oplog.add(start).await;
                (idx, true)
            };
            ctx.state.current_retry_point = start_idx;
            Ok(Self {
                start_idx,
                function_type,
                is_live: true,
                persisted,
                replay: None,
                finished: false,
                saved_retry_point,
                drop_sink: None,
                _phantom: PhantomData,
            })
        } else {
            // Defensive guard, mirroring legacy `read_persisted_durable_function_invocation`.
            if durable_execution_state.persistence_level == PersistenceLevel::PersistNothing {
                return Err(WorkerExecutorError::runtime(
                    "Trying to replay a durable invocation in a PersistNothing block",
                ));
            }
            let replay = ctx
                .state
                .replay_state
                .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, &function_type)
                .await?;
            let start_idx = replay.start_idx();
            ctx.state.current_retry_point = start_idx;
            Ok(Self {
                start_idx,
                function_type,
                is_live: false,
                persisted: false,
                replay: Some(replay),
                finished: false,
                saved_retry_point,
                drop_sink: None,
                _phantom: PhantomData,
            })
        }
    }

    pub fn is_live(&self) -> bool {
        self.is_live
    }

    pub fn start_index(&self) -> OplogIndex {
        self.start_idx
    }

    /// Completes a live call: upload the response and append the matching `End`.
    pub async fn complete<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(self.is_live, "complete() called on a replay handle");
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
            // Mirror legacy `end_durable_function` commit semantics: remote/batched/transactional
            // writes commit immediately; local reads do not.
            if matches!(
                self.function_type,
                DurableFunctionType::WriteRemote
                    | DurableFunctionType::WriteRemoteBatched(_)
                    | DurableFunctionType::WriteRemoteTransaction(_)
            ) {
                ctx.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }
        }
        ctx.state.current_retry_point = self.saved_retry_point;
        self.finished = true;
        Ok(response)
    }

    /// Replays a call: drive the cursor until the call resolves, then decode its response.
    pub async fn replay<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let replay = self
            .replay
            .take()
            .expect("replay() called on a live handle");
        let resolution = ctx.state.replay_state.await_resolution(replay).await?;
        let response = match resolution {
            Resolution::Completed { response, .. } => {
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
                host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?
            }
            Resolution::Cancelled { cancelled_idx, .. } => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End",
                    format!("Cancelled at {cancelled_idx}"),
                ));
            }
        };
        ctx.state.current_retry_point = self.saved_retry_point;
        self.finished = true;
        Ok(response)
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
        self.finished = true;
        Ok(())
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
        // No registration: resolve_if_pending must not buffer (this is the legacy-End case).
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
        CallHandle {
            start_idx,
            function_type: DurableFunctionType::ReadLocal,
            is_live: true,
            persisted: true,
            replay: None,
            finished: false,
            saved_retry_point: OplogIndex::INITIAL,
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
}
