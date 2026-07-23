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
//! Every durable host call runs through this path via [`CallHandle`]. Calls made through the
//! p3 `Accessor` entry points ([`CallHandle::start_access_with`] and friends) run concurrently;
//! host methods still taking `&mut self` remain serialized by the store borrow.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::{InvocationContextSpan, SpanId};
use golem_common::model::oplog::UpdateDescription;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, PersistenceLevel, ScopeScanState, host_functions::HostFunctionName,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, HasData, TerminalConsumption};

use crate::durable_host::durability::{
    ClassifiedHostError, DurabilityHost, DurableCallTrapContext, DurableCallTrapError,
    DurableExecutionState, HostFailureKind, InFunctionRetryController, InFunctionRetryHost,
    InternalRetryResult, TaskRetryContext, TerminalCallError, mark_durable_call_trap_context,
    try_trigger_host_trap_retry,
};
use crate::durable_host::replay_state::{OplogEntryLookupResult, ReplayState};
use crate::durable_host::{
    AtomicRegionLease, DurableScopeKind, DurableWorkerCtx, IFSWorkerFile, PublicDurableWorkerState,
};
use crate::services::HasWorker;
use crate::services::component::ComponentService;
use crate::services::file_loader::FileLoader;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, PendingUpload};
use crate::worker::agent_config::{effective_agent_config, validate_agent_config};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use std::fmt::Display;

fn ambient_trap_context<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> DurableCallTrapContext {
    DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
        in_atomic_region: InFunctionRetryHost::in_atomic_region(ctx),
    }
}

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
        #[expect(
            dead_code,
            reason = "preserved for the concurrent-durability replay model"
        )]
        forced_commit: bool,
    },
    /// The call was cancelled (dropped before completion) via a `Cancelled` entry.
    Cancelled {
        cancelled_idx: OplogIndex,
        partial: Option<OplogPayload<HostResponse>>,
    },
    /// The call completed successfully via an `End` entry, but a `CompletionDiscarded` marker
    /// records that the response was never delivered to the guest: the guest dropped the accessor
    /// completion future (e.g. the losing branch of a `select!`) after the `End` was persisted.
    /// Replay must not deliver the response to the *guest* either — the replaying guest parks
    /// (at the recorded delivery boundary) until it drops the future at the same point it did
    /// live. The recorded response payload is still carried: deferred-delivery replay sites
    /// ([`CallHandle::replay_access_deferred`]) must decode it to reconstruct deterministic
    /// host-side state (span finishes, terminal-child bookkeeping) executed between the `End`
    /// and the point where delivery would have happened.
    CompletedButDiscarded {
        end_idx: OplogIndex,
        marker_idx: OplogIndex,
        response: Option<OplogPayload<HostResponse>>,
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
///
/// Transient: callers destructure it immediately, so the size difference between the variants
/// never lives beyond the replay call itself.
#[allow(clippy::large_enum_variant)]
pub enum CallReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    /// The call's `End` was replayed and decoded into its response.
    Replayed(Pair::Resp),
    /// The call's `Start` was committed but its `End` never was. The returned handle has been
    /// switched to live completion of that existing `Start`: the caller must re-run the side effect
    /// and call [`CallHandle::complete`] (which appends the missing `End`). Only produced for
    /// function types that are safe to re-execute.
    Incomplete(CallHandle<Pair, P>),
}

/// The result of [`CallHandle::replay_access_deferred`]: like [`CallReplayOutcome`], but each
/// replayed response carries the [`CompletionDelivery`] token describing the recorded delivery
/// status the caller must mirror.
#[allow(clippy::large_enum_variant)]
pub enum DeferredCallReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    /// The call's terminal was replayed and decoded. If the token reports
    /// [`CompletionDelivery::is_replay_discarded`], the recorded run discarded this completion:
    /// the caller must not deliver the response and instead parks at the delivery boundary after
    /// its deterministic post-`End` continuation.
    Replayed(Pair::Resp, CompletionDelivery),
    /// See [`CallReplayOutcome::Incomplete`]; the caller re-runs the side effect and completes
    /// via [`CallHandle::complete_access_deferred`].
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
    pending: HashMap<OplogIndex, ReplayableOneshot<ResolutionOutcome>>,
    /// Resolutions observed before their awaiter registered. The await-resolution guard
    /// guarantees a call's `Start` is claimed before its `End`/`Cancelled` is consumed, so on the
    /// replay path this stays empty; it covers the resolver's own unit tests and any future entry
    /// point that resolves without that ordering guarantee.
    buffered: HashMap<OplogIndex, ResolutionOutcome>,
}

impl ConcurrentReplayResolver {
    /// Registers an awaiter for the call started at `start_idx` and returns the receiver it should
    /// await on. If the resolution was already observed (buffered), the returned receiver is
    /// pre-resolved.
    pub fn register(
        &mut self,
        start_idx: OplogIndex,
    ) -> ReplayableOneshotReceiver<ResolutionOutcome> {
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
        let outcome = ResolutionOutcome::Resolved(resolution);
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(outcome);
        } else {
            self.buffered.insert(start_idx, outcome);
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
            let _ = tx.send(ResolutionOutcome::Resolved(resolution));
            true
        } else {
            false
        }
    }

    /// Resolves every still-registered awaiter as [`ResolutionOutcome::Incomplete`].
    ///
    /// Called when replay reaches the end of the oplog ([`crate::durable_host::replay_state::ReplayState::switch_to_live`]):
    /// any call whose `Start` was committed but whose `End`/`Cancelled` never was is, by definition,
    /// incomplete. Waking the awaiters here (rather than relying on each to notice end-of-replay
    /// itself) is what lets a call that is *suspended* waiting for the cursor to advance — because a
    /// concurrently-replaying sibling call owns the cursor head — make progress once replay finishes
    /// instead of hanging forever.
    pub fn fail_all_pending_incomplete(&mut self) {
        for (_start_idx, tx) in self.pending.drain() {
            let _ = tx.send(ResolutionOutcome::Incomplete);
        }
    }

    /// Removes a registered awaiter without resolving it. Used when a claimed call turns out to be
    /// incomplete on replay (its `Start` is committed but its `End` never was): the awaiter is
    /// switched to live completion, so its pending registration must not linger in the resolver.
    pub fn unregister(&mut self, start_idx: OplogIndex) {
        self.pending.remove(&start_idx);
    }

    /// Returns whether an awaiter is currently registered for `start_idx`.
    ///
    /// The replay cursor uses this to decide which `End`/`Cancelled` entries are *awaited
    /// terminals* it may auto-drain (and route back to their awaiter) versus the ones it must leave
    /// for their own positional consumer: scope `End`s, unclaimed `Start`s, and deterministic
    /// markers.
    pub fn is_pending(&self, start_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx)
    }
}

/// Replay-side state for a single in-flight call: the `Start` index it claimed and the receiver
/// that will deliver its [`Resolution`].
#[derive(Debug)]
pub struct ReplayCallHandle {
    start_idx: OplogIndex,
    receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
}

impl ReplayCallHandle {
    pub fn new(
        start_idx: OplogIndex,
        receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
    ) -> Self {
        Self {
            start_idx,
            receiver,
        }
    }

    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    /// Consumes the handle into its parts (used by the replay-state driver).
    pub fn into_parts(self) -> (OplogIndex, ReplayableOneshotReceiver<ResolutionOutcome>) {
        (self.start_idx, self.receiver)
    }
}

/// Call-owned facts available to a cancellation recorder when a live persisted handle is dropped.
///
/// `Drop` cannot use wasmtime's `Accessor`, cannot borrow worker state, and cannot `.await`; any
/// production recorder must receive everything it needs from the handle itself and do the async
/// oplog/state work later.
#[derive(Debug, Clone)]
pub struct DroppedCall {
    start_idx: OplogIndex,
    begin_index: OplogIndex,
    function_type: DurableFunctionType,
    request_upload: PendingUpload,
    /// The dropped call's atomic-region ownership lease, shared with every other holder (the
    /// originating handle, terminal guards). Released — store-free and idempotently — once the
    /// call's terminal is recorded.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
    /// The dropped call's own trap classification, captured from its execution scope at drop time.
    /// A cancellation-drain failure (deferred request upload / terminal recorder join) traps with
    /// this context so the retry grouping belongs to the dropped call, not to whichever later host
    /// call happens to drive the drain.
    trap_context: DurableCallTrapContext,
    /// Keeps the dropped call counted as an in-flight live host call until the drop event is
    /// actually processed (its `Cancelled`/terminal recorded at a drain point). Without this, a
    /// handle dropped between a drain and a subsequent in-flight check (e.g. the
    /// `set_oplog_persistence_level` boundary guard) would release its permit before its terminal
    /// entry is recorded, letting the terminal land on the far side of a positional replay
    /// boundary. `None` for call sites that only use the snapshot locally while the handle (and
    /// its own permit) is still alive.
    live_call_permit: Option<LiveCallPermit>,
}

impl DroppedCall {
    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn request_upload(&self) -> &PendingUpload {
        &self.request_upload
    }

    /// The atomic region currently owning the dropped call, read through its lease.
    pub fn atomic_region(&self) -> Option<OplogIndex> {
        self.atomic_lease.as_ref().and_then(|lease| lease.owner())
    }

    /// Releases the dropped call's atomic-region membership (idempotent, store-free).
    fn release_atomic_lease(&self) {
        if let Some(lease) = &self.atomic_lease {
            lease.release();
        }
    }

    pub fn trap_context(&self) -> DurableCallTrapContext {
        self.trap_context
    }

    async fn wait_request_upload(&self) -> Result<(), WorkerExecutorError> {
        self.request_upload.wait().await.map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "failed to serialize and store durable call request: {err}"
            ))
        })
    }

    async fn append_cancelled<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<OplogPayload<HostResponse>>,
    ) -> Result<(), WorkerExecutorError> {
        self.append_cancelled_with_oplog(ctx.state.oplog.clone(), partial)
            .await?;
        self.release_atomic_lease();
        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;
        Ok(())
    }

    async fn append_cancelled_with_oplog(
        &self,
        oplog: std::sync::Arc<dyn crate::services::oplog::Oplog>,
        partial: Option<OplogPayload<HostResponse>>,
    ) -> Result<(), WorkerExecutorError> {
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: self.start_idx,
            partial,
        };
        oplog.add(cancelled).await;
        Ok(())
    }
}

/// Deferred work emitted from `Drop` impls that cannot touch the worker store themselves.
///
/// Each worker owns a `dropped_call_events` channel (see `PrivateDurableWorkerState`); `Drop`
/// impls ([`CallHandle`], [`AccessTerminalGuard`], resource wrappers) enqueue these events, and
/// they are drained from the next safe worker-access window: [`drain_queued_dropped_call_events`]
/// at the start of every `&mut ctx` durable call and [`drain_dropped_call_events_access`] on the
/// accessor path (call start and terminals). The drain records durable effects (`Cancelled`
/// entries, scope closes, span finishes) via [`record_dropped_call_event`] or the accessor-window
/// equivalent. Unit tests attach their own sink to observe the enqueued events directly.
#[derive(Debug)]
pub enum DropEvent {
    /// A `Cancellable` handle was dropped unfinished; the next drain records `Cancelled` from this
    /// call-owned snapshot and closes the matching durable-function scope.
    UnfinishedCancellable { call: DroppedCall },
    /// A `NotCancellable` handle was dropped unfinished; this is a programming error.
    UnfinishedNotCancellable { call: DroppedCall },
    /// A terminal append was handed to an owned task; wait for it before releasing the call's
    /// atomic-region lease. A join failure traps with the dropped call's own `trap_context`
    /// rather than ambient state.
    CleanupAfterTerminal {
        atomic_lease: Option<Arc<AtomicRegionLease>>,
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Keeps the call counted as in flight until this event is fully consumed (the owned
        /// terminal task joined and the durable-function scope closed), so a positional boundary
        /// (e.g. a persistence-level change) cannot be placed before the delayed terminal append.
        /// Never read — held purely for its `Drop` effect on the shared counter.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A deferred guest-delivery token ([`CompletionDelivery`]) was dropped while still armed:
    /// its call's terminal `End` is already recorded and the durable scope already closed, so the
    /// only remaining work is joining the owned marker (or trailing ordered-append) task while
    /// keeping the call counted as in flight — no scope close and no atomic-region cleanup.
    AwaitDiscardMarker {
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Held so invocation settlement waits until the marker append is joined; released when
        /// the event is finished or dropped. Never read.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A guest-cancelled accessor future may leave a caller-managed durable scope with no code path
    /// back into the resource's `drop`. Close that parent scope from the next safe store-access
    /// window. The close is idempotent because the resource may be dropped before this event drains.
    CloseDurableScope {
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        span_id: Option<SpanId>,
    },
    /// An invocation-context span whose owning resource was dropped from a synchronous host
    /// context (e.g. a p3 HTTP response dropped before its body was consumed). Finish it from the
    /// next drain point. When `durable` (the span was replayed from a legacy positional
    /// `StartSpan` entry), live writes the `FinishSpan` there and replay consumes it at the same
    /// point, since drain points are deterministic replay points; otherwise the span is derived
    /// (no span oplog entries exist) and is finished in memory only.
    FinishSpan { span_id: SpanId, durable: bool },
}

struct AccessDropEventDrainGuard {
    sink: UnboundedSender<DropEvent>,
    pending: VecDeque<DropEvent>,
    current: Option<DropEvent>,
    disarmed: bool,
}

impl AccessDropEventDrainGuard {
    fn new(sink: UnboundedSender<DropEvent>, events: Vec<DropEvent>) -> Self {
        Self {
            sink,
            pending: events.into(),
            current: None,
            disarmed: false,
        }
    }

    fn start_next(&mut self) -> bool {
        debug_assert!(self.current.is_none());
        self.current = self.pending.pop_front();
        self.current.is_some()
    }

    fn current_mut(&mut self) -> &mut DropEvent {
        self.current
            .as_mut()
            .expect("access dropped-call drain has an active event")
    }

    fn replace_current(&mut self, event: DropEvent) {
        self.current = Some(event);
    }

    fn finish_current(&mut self) {
        self.current = None;
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for AccessDropEventDrainGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        if let Some(event) = self.current.take() {
            let _ = self.sink.send(event);
        }
        while let Some(event) = self.pending.pop_front() {
            let _ = self.sink.send(event);
        }
    }
}

/// Everything needed to append a `CompletionDiscarded` marker from an owned task when an armed
/// [`AccessTerminalGuard`] is dropped: the guest tore the accessor completion future *after* the
/// successful `End` append was handed to its owned task, so the response was persisted but never
/// delivered. Armed only on the `End` path ([`CallHandle::persist_access_terminal`]) — never for
/// cancellations — and explicitly suppressed on internal-error returns the caller observes
/// ([`AccessTerminalGuard::suppress_discard_marker`]), so a marker is recorded exactly when the
/// completion was silently discarded by the guest.
struct DiscardMarker {
    start_idx: OplogIndex,
    oplog: Arc<dyn Oplog>,
    replay_state: ReplayState,
}

impl DiscardMarker {
    /// Appends the `CompletionDiscarded` marker entry and records it in the replay state. The
    /// terminal `End` append (and any ordered post-`End` append, e.g. a `FinishSpan`) must
    /// already be durable when this runs.
    async fn append(self) {
        let marker_idx = self
            .oplog
            .add(OplogEntry::CompletionDiscarded {
                timestamp: Timestamp::now_utc(),
                start_index: self.start_idx,
            })
            .await;
        self.replay_state
            .record_discarded_completion(self.start_idx, marker_idx);
    }

    /// Spawns the owned marker-recording task, chained after the (possibly still pending) owned
    /// terminal `End` append. Returns the chained handle, which replaces the terminal handle in
    /// the emitted [`DropEvent::CleanupAfterTerminal`], so every existing drain that joins the
    /// terminal also awaits the marker append — invocation completion cannot overtake it. The
    /// task is detached (`tokio::spawn`): even if the drain future itself is torn and the event
    /// is lost, the marker append still runs to completion.
    fn spawn_chained(
        self,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
    ) -> tokio::task::JoinHandle<Result<(), WorkerExecutorError>> {
        tokio::spawn(async move {
            if let Some(handle) = terminal {
                handle.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                })??;
            }
            self.append().await;
            Ok(())
        })
    }
}

/// A deferred guest-delivery token returned by [`CallHandle::complete_access_deferred`] /
/// [`CallHandle::replay_access_deferred`] for call sites whose result crosses one more fallible
/// boundary *after* the durable terminal is recorded — a second-stage channel send to the guest
/// task, a span finish plus resource-state transition before the host method returns, or a wire
/// conversion. The plain `complete_access` boundary (the accessor terminal itself) is too early
/// for those sites: the guest can silently discard the persisted completion between the `End`
/// and the real delivery, which replay would otherwise deliver.
///
/// Live, the token stays armed after the `End` is persisted and the durable scope is closed:
/// - [`Self::delivered`] — the final guest-facing transfer succeeded; no marker.
/// - [`Self::suppress`] — a post-`End` error is observed by the caller (the worker traps); no
///   marker.
/// - [`Self::discarded`] — the caller detected a silent discard (e.g. the guest dropped the
///   receiving end of the delivery channel); appends exactly one `CompletionDiscarded` marker
///   inline and returns once it is durable.
/// - `Drop` while armed — the delivering future itself was torn; spawns exactly one owned marker
///   append (ordered after any pending [`Self::append_ordered`] entry) and hands its join plus
///   the in-flight [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitDiscardMarker`],
///   so invocation settlement cannot overtake the append.
///
/// On replay the token mirrors the recorded delivery status: [`Self::is_replay_discarded`]
/// reports whether the recorded run discarded the completion, in which case the caller must not
/// deliver — it performs its deterministic post-`End` continuation (span consumption, terminal
/// bookkeeping) and parks at the exact point where live delivery would have happened. All token
/// operations are no-ops on replay.
pub struct CompletionDelivery {
    state: CompletionDeliveryState,
}

enum CompletionDeliveryState {
    /// Live, armed: the `End` is persisted and a torn/failed delivery must record a marker.
    Live(Box<LiveDelivery>),
    /// Live, but the call was not persisted (snapshotting): nothing to reconcile.
    Unarmed,
    /// Replay of a normally delivered completion.
    ReplayDelivered,
    /// Replay of a recorded discarded completion: the caller must not deliver and parks at the
    /// delivery boundary.
    ReplayDiscarded,
    /// Consumed (`delivered`/`suppress`/`discarded`).
    Done,
}

struct LiveDelivery {
    marker: DiscardMarker,
    trap_context: DurableCallTrapContext,
    /// Keeps the call counted as in flight (for positional-boundary and snapshot checks) until
    /// the token is consumed or its drain event is processed. Settlement itself waits for the
    /// marker because both invocation exit paths drain the drop-event queue — joining any
    /// [`DropEvent::AwaitDiscardMarker`] — before writing their final oplog state.
    live_call_permit: Option<LiveCallPermit>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// An owned oplog append (e.g. a durable `FinishSpan`) that must land *before* any marker
    /// append, preserving the recorded `End → FinishSpan → CompletionDiscarded` order replay
    /// consumes positionally. See [`CompletionDelivery::append_ordered`].
    pending_append: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
}

impl CompletionDelivery {
    fn unarmed() -> Self {
        Self {
            state: CompletionDeliveryState::Unarmed,
        }
    }

    fn replay_delivered() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered,
        }
    }

    fn replay_discarded() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDiscarded,
        }
    }

    /// Whether the recorded run discarded this completion: the caller must not deliver the
    /// response to the guest and instead parks at the delivery boundary after finishing its
    /// deterministic post-`End` continuation.
    pub fn is_replay_discarded(&self) -> bool {
        matches!(self.state, CompletionDeliveryState::ReplayDiscarded)
    }

    /// Whether the token is live and armed (a torn delivery would record a marker). Callers use
    /// this to route ordered post-`End` appends through [`Self::append_ordered`] instead of a
    /// direct oplog append that would race the torn-drop marker.
    pub fn is_live_armed(&self) -> bool {
        matches!(self.state, CompletionDeliveryState::Live(_))
    }

    /// Hands an oplog entry append (e.g. a durable `FinishSpan`) to an owned task ordered
    /// *before* any later marker append by this token. Must be called with no `await` between
    /// the token's creation (or previous [`Self::wait_appends`]) and this call when the entry is
    /// mandatory — a tear cannot happen between synchronous statements, so the obligation is
    /// transferred atomically. No-op unless live and armed.
    pub fn append_ordered(&mut self, entry: OplogEntry) {
        if let CompletionDeliveryState::Live(live) = &mut self.state {
            let oplog = live.marker.oplog.clone();
            let previous = live.pending_append.take();
            live.pending_append = Some(tokio::spawn(async move {
                if let Some(handle) = previous {
                    handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "ordered post-End append task failed: {err}"
                        ))
                    })??;
                }
                oplog.add(entry).await;
                Ok(())
            }));
        }
    }

    /// Joins the pending ordered append(s). Cancellation-safe: a tear mid-join leaves the join
    /// handle owned by the token, so the torn-drop marker append still chains after it.
    pub async fn wait_appends(&mut self) -> Result<(), WorkerExecutorError> {
        if let CompletionDeliveryState::Live(live) = &mut self.state
            && let Some(handle) = &mut live.pending_append
        {
            let result = handle.await.map_err(|err| {
                WorkerExecutorError::runtime(format!("ordered post-End append task failed: {err}"))
            });
            live.pending_append = None;
            result??;
        }
        Ok(())
    }

    /// The final guest-facing delivery succeeded: no marker. Consumes the token; any pending
    /// ordered append keeps running as an owned task and its join (plus the in-flight permit) is
    /// handed to the drain queue so settlement still waits for it.
    pub fn delivered(mut self) {
        self.settle();
    }

    /// A post-`End` error is returned to (observed by) the caller — the worker traps — so the
    /// completion was not *silently* discarded: no marker.
    pub fn suppress(mut self) {
        self.settle();
    }

    /// Consumes the token by arming Wasmtime's terminal-consumption observer for the host
    /// subtask `store` belongs to: the *actual* guest-delivery boundary of a direct accessor
    /// host call. The host method returning its result is not that boundary — Wasmtime still
    /// lowers the result and queues the subtask's `Returned` event afterwards, and the guest can
    /// consume that event via `subtask.cancel` (or abandon it through a post-`End` cancellation)
    /// without ever observing the response.
    ///
    /// The observer maps Wasmtime's verdict onto the token:
    /// - `Delivered` (the guest received the successful terminal) → [`Self::delivered`].
    /// - `Discarded` / `Cancelled` (the guest consumed or abandoned the completion without
    ///   observing the result after the `End` was persisted) → the armed token is dropped, which
    ///   spawns the owned cancellation-safe `CompletionDiscarded` marker append and hands its
    ///   join to the drain queue, so invocation settlement waits for it.
    /// - Dropped without being invoked (a trap, a lowering failure, or store teardown — all of
    ///   which abandon the whole execution rather than silently discarding this one completion;
    ///   replay re-executes the guest to the same point and redelivers) → [`Self::suppress`].
    ///
    /// Registering a newer observer for the same host subtask (a later durable call in the same
    /// host function) supersedes this one, suppressing its token: once a later durable event is
    /// recorded, replay re-executes the host code past this `End` deterministically and
    /// re-consumes the response internally, so no marker is needed.
    ///
    /// Non-live tokens (replay, unpersisted snapshotting calls) settle immediately; if the
    /// accessor has no guest-visible host subtask (e.g. a spawned background task), the token
    /// settles without a marker, matching the pre-observer behavior of consuming it at the host
    /// return.
    pub fn deliver_at_accessor_terminal<T, D>(self, store: &Accessor<T, D>)
    where
        T: 'static,
        D: HasData + ?Sized,
    {
        if !self.is_live_armed() {
            self.delivered();
            return;
        }
        let guard = AccessorDeliveryGuard {
            delivery: Some(self),
        };
        if let Err(error) = store
            .register_terminal_observer(Box::new(move |consumption| guard.consume(consumption)))
        {
            // No guest-visible host subtask to observe (the guard is dropped by the failed
            // registration, suppressing the token — no marker).
            tracing::debug!(
                "durable call completion has no guest-visible host subtask to observe: {error}"
            );
        }
    }

    fn settle(&mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                if let Some(pending) = live.pending_append {
                    // The ordered append is still in flight: keep it settlement-accounted via
                    // the drain queue, without a marker.
                    Self::emit_await_event(
                        live.cleanup_sink,
                        pending,
                        live.trap_context,
                        live.live_call_permit,
                    );
                }
            }
            CompletionDeliveryState::ReplayDelivered
            | CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => {}
        }
    }

    /// The caller detected a silent discard of the persisted completion (e.g. the guest dropped
    /// the receiving end of the delivery channel): appends exactly one `CompletionDiscarded`
    /// marker — ordered after any pending [`Self::append_ordered`] entry — and returns once it
    /// is durable. Cancellation-safe: marker persistence moves to an owned task *before* the
    /// first await, and a tear mid-wait hands the join plus the in-flight permit to the drain
    /// queue exactly like a torn armed drop, so the marker still lands and settlement still
    /// waits for it. No-op on replay.
    pub async fn discarded(mut self) -> Result<(), WorkerExecutorError> {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                let LiveDelivery {
                    marker,
                    trap_context,
                    live_call_permit,
                    cleanup_sink,
                    pending_append,
                } = *live;
                let guard = MarkerAwaitGuard {
                    join: Some(marker.spawn_chained(pending_append)),
                    trap_context,
                    live_call_permit,
                    cleanup_sink,
                };
                guard.wait().await
            }
            CompletionDeliveryState::ReplayDelivered => {
                // Live delivered this completion (no marker on record), but replay could not: a
                // nondeterministic guest tore the receiving end at a different point. Nothing
                // durable to reconcile.
                tracing::warn!(
                    "replayed durable call completion could not be delivered although the recorded run delivered it"
                );
                Ok(())
            }
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => Ok(()),
        }
    }

    fn emit_await_event(
        sink: Option<UnboundedSender<DropEvent>>,
        terminal: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
        trap_context: DurableCallTrapContext,
        live_call_permit: Option<LiveCallPermit>,
    ) {
        if let Some(sink) = &sink {
            let _ = sink.send(DropEvent::AwaitDiscardMarker {
                terminal: Some(terminal),
                trap_context,
                live_call_permit,
            });
        }
    }
}

/// Test-only [`CompletionDelivery`] factories for delivery-boundary unit tests outside this
/// module (e.g. the consume-body chunk transfer helper). They build real tokens — the live one
/// appends a real `CompletionDiscarded` marker to the given oplog — without exposing the token
/// internals.
#[cfg(test)]
impl CompletionDelivery {
    /// A live-armed token over `oplog` whose torn/failed delivery appends a
    /// `CompletionDiscarded` marker for `start_idx`, exactly as
    /// [`CallHandle::complete_access_deferred`] arms one for a persisted live call whose `End`
    /// is already durable. `oplog` must already contain the call's `Start`/`End` entries (the
    /// token's replay state is built over its current contents).
    pub(crate) async fn test_live_armed(
        oplog: Arc<dyn Oplog>,
        start_idx: OplogIndex,
    ) -> Result<Self, WorkerExecutorError> {
        let replay_state = ReplayState::new(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "completion-delivery-test".to_string(),
                },
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
        )
        .await?;
        Ok(Self {
            state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                marker: DiscardMarker {
                    start_idx,
                    oplog,
                    replay_state,
                },
                trap_context: DurableCallTrapContext {
                    retry_from: start_idx,
                    in_atomic_region: false,
                },
                live_call_permit: None,
                cleanup_sink: None,
                pending_append: None,
            })),
        })
    }

    /// A replay token for a recorded discarded completion, as
    /// [`CallHandle::replay_access_deferred`] returns when the recorded run persisted the `End`
    /// but never delivered it.
    pub(crate) fn test_replay_discarded() -> Self {
        Self::replay_discarded()
    }
}

impl Drop for CompletionDelivery {
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                // The delivering future was torn while the token was still armed: the guest
                // silently discarded a persisted successful completion. Chain the owned marker
                // append after any pending ordered append (preserving the recorded
                // `End → FinishSpan → CompletionDiscarded` order) and hand the join plus the
                // in-flight permit to the drain queue so invocation settlement waits for it. The
                // task is spawned unconditionally — marker recording must not depend on the
                // event surviving the drain.
                let terminal = live.marker.spawn_chained(live.pending_append);
                Self::emit_await_event(
                    live.cleanup_sink,
                    terminal,
                    live.trap_context,
                    live.live_call_permit,
                );
            }
            CompletionDeliveryState::ReplayDelivered => {
                // Anomalous: replay delivered a completion the recorded run delivered too, but
                // the site dropped the token without consuming it (or the future was torn at a
                // point live was not). Nothing durable to reconcile.
                tracing::warn!("replay completion-delivery token dropped without being consumed");
            }
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => {}
        }
    }
}

/// Adapts a live armed [`CompletionDelivery`] token to a Wasmtime terminal observer (see
/// [`CompletionDelivery::deliver_at_accessor_terminal`]). The observer runs inside Wasmtime's
/// event loop and must not access the store: every branch below only consumes the token, which
/// touches Golem-owned channels and owned tasks.
struct AccessorDeliveryGuard {
    delivery: Option<CompletionDelivery>,
}

impl AccessorDeliveryGuard {
    fn consume(mut self, consumption: TerminalConsumption) {
        let delivery = self
            .delivery
            .take()
            .expect("terminal observers are invoked at most once");
        match consumption {
            // The guest received the successful terminal: no marker.
            TerminalConsumption::Delivered => delivery.delivered(),
            // The guest consumed the pending terminal via `subtask.cancel` after the successful
            // lowering (`Discarded`), or cancelled the call after the `End` was persisted
            // (`Cancelled`): either way the guest never observes the persisted completion.
            // Dropping the armed token spawns the owned cancellation-safe marker append and
            // hands its join to the drain queue, so invocation settlement waits for it.
            TerminalConsumption::Discarded | TerminalConsumption::Cancelled => drop(delivery),
        }
    }
}

impl Drop for AccessorDeliveryGuard {
    fn drop(&mut self) {
        // Dropped without being invoked: the terminal was never consumed by the guest — a trap,
        // a lowering failure, or store teardown (or a later durable call in the same host
        // function superseding this observer). None of these silently discard the completion,
        // so no marker.
        if let Some(delivery) = self.delivery.take() {
            delivery.suppress();
        }
    }
}

/// RAII guard for awaiting an owned `CompletionDiscarded` marker task inline
/// ([`CompletionDelivery::discarded`]). Marker persistence already lives in the owned task; the
/// guard makes the *wait* cancellation-safe: a tear mid-wait hands the join plus the in-flight
/// [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitDiscardMarker`], so invocation
/// settlement still waits for the marker append.
struct MarkerAwaitGuard {
    /// `Some` until the join completes (successfully or not); a `Drop` with the join still
    /// pending emits the drain event.
    join: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
    trap_context: DurableCallTrapContext,
    live_call_permit: Option<LiveCallPermit>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
}

impl MarkerAwaitGuard {
    async fn wait(mut self) -> Result<(), WorkerExecutorError> {
        let joined = self
            .join
            .as_mut()
            .expect("MarkerAwaitGuard is always constructed with a join handle")
            .await;
        self.join = None;
        joined.map_err(|err| {
            WorkerExecutorError::runtime(format!("durable call discard-marker task failed: {err}"))
        })?
    }
}

impl Drop for MarkerAwaitGuard {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            CompletionDelivery::emit_await_event(
                self.cleanup_sink.take(),
                join,
                self.trap_context,
                self.live_call_permit.take(),
            );
        }
    }
}

enum AccessTerminalGuardState {
    BeforeTerminal {
        call: DroppedCall,
    },
    CleanupAfterTerminal {
        atomic_lease: Option<Arc<AtomicRegionLease>>,
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Moved out of the armed [`DroppedCall`] when the terminal is handed to an owned task,
        /// so the call stays counted as in flight while the append is still pending.
        live_call_permit: Option<LiveCallPermit>,
        /// `Some` while a torn drop must record a `CompletionDiscarded` marker (successful `End`
        /// path, no error observed by the caller yet); see [`DiscardMarker`].
        discard_marker: Option<DiscardMarker>,
    },
    Disarmed,
}

struct AccessTerminalGuard<P: DropPolicy> {
    state: AccessTerminalGuardState,
    /// Policy-controlled sink for unfinished drops (`BeforeTerminal`): `None` for policies (e.g.
    /// `NotCancellable`) that treat an unfinished drop as a programming error instead of queueing
    /// a cancellation.
    sink: Option<UnboundedSender<DropEvent>>,
    /// Unconditional sink for `CleanupAfterTerminal`: once a terminal task has been spawned, its
    /// join + scope close (and the in-flight permit it carries) must be handed to the next drain
    /// regardless of the drop policy.
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<P>,
}

impl<P: DropPolicy> AccessTerminalGuard<P> {
    fn new(
        call: DroppedCall,
        sink: Option<UnboundedSender<DropEvent>>,
        cleanup_sink: Option<UnboundedSender<DropEvent>>,
    ) -> Self {
        Self {
            state: AccessTerminalGuardState::BeforeTerminal { call },
            sink,
            cleanup_sink,
            _phantom: PhantomData,
        }
    }

    /// Releases the armed call's atomic-region lease (idempotent, store-free). A no-op once the
    /// guard is disarmed.
    fn release_atomic_lease(&self) {
        let lease = match &self.state {
            AccessTerminalGuardState::BeforeTerminal { call } => call.atomic_lease.as_ref(),
            AccessTerminalGuardState::CleanupAfterTerminal { atomic_lease, .. } => {
                atomic_lease.as_ref()
            }
            AccessTerminalGuardState::Disarmed => None,
        };
        if let Some(lease) = lease {
            lease.release();
        }
    }

    fn call(&self) -> Option<&DroppedCall> {
        match &self.state {
            AccessTerminalGuardState::BeforeTerminal { call } => Some(call),
            _ => None,
        }
    }

    /// Hands the terminal append to an owned task. `discard_marker` must be `Some` only on the
    /// successful-`End` path (a torn drop from that state means the guest silently discarded the
    /// persisted completion); cancellation terminals pass `None`.
    fn cleanup_after_terminal(
        &mut self,
        terminal: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
        discard_marker: Option<DiscardMarker>,
    ) {
        let call = match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::BeforeTerminal { call } => call,
            _ => panic!("cleanup_after_terminal called before the terminal is armed"),
        };
        self.state = AccessTerminalGuardState::CleanupAfterTerminal {
            atomic_lease: call.atomic_lease.clone(),
            function_type: call.function_type().clone(),
            durable_begin_index: call.begin_index(),
            terminal: Some(terminal),
            trap_context: call.trap_context(),
            // The permit moves with the state transition: the call remains counted as in flight
            // while the owned terminal append is pending.
            live_call_permit: call.live_call_permit,
            discard_marker,
        };
    }

    /// Disarms only the `CompletionDiscarded` marker while keeping the terminal cleanup armed.
    /// Called on internal-error returns *after* the `End` was persisted (atomic-region close
    /// failure, durable-scope close failure): the caller observes the error and the worker traps,
    /// so the completion was not silently discarded by the guest and no marker may be recorded —
    /// but the pending terminal join / permit release must still reach the drain.
    fn suppress_discard_marker(&mut self) {
        if let AccessTerminalGuardState::CleanupAfterTerminal { discard_marker, .. } =
            &mut self.state
        {
            *discard_marker = None;
        }
    }

    async fn wait_terminal(&mut self) -> Result<(), WorkerExecutorError> {
        if let AccessTerminalGuardState::CleanupAfterTerminal { terminal, .. } = &mut self.state {
            if let Some(handle) = terminal {
                handle.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                })??;
            }
            *terminal = None;
        }
        Ok(())
    }

    fn disarm(&mut self) {
        self.state = AccessTerminalGuardState::Disarmed;
    }

    /// Converts a fully completed terminal guard (terminal joined, scope closed) into a deferred
    /// guest-delivery token: the armed discard marker, the in-flight permit and the trap context
    /// move to the [`CompletionDelivery`], which owns the silent-discard reconciliation from here
    /// to the final guest-facing boundary. Falls back to an unarmed token when the call was not
    /// persisted (no marker to reconcile).
    fn take_completion_delivery(&mut self) -> CompletionDelivery {
        match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::CleanupAfterTerminal {
                terminal,
                trap_context,
                live_call_permit,
                discard_marker: Some(marker),
                ..
            } => CompletionDelivery {
                state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                    marker,
                    trap_context,
                    live_call_permit,
                    cleanup_sink: self.cleanup_sink.clone(),
                    // Normally already joined (`wait_terminal`) by the time the guard is
                    // converted; chained defensively so a still-pending terminal append can never
                    // be overtaken by a marker append.
                    pending_append: terminal,
                })),
            },
            _ => CompletionDelivery::unarmed(),
        }
    }
}

impl<P: DropPolicy> Drop for AccessTerminalGuard<P> {
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::BeforeTerminal { call } => {
                P::unfinished_drop(call, self.sink.as_ref());
            }
            AccessTerminalGuardState::CleanupAfterTerminal {
                atomic_lease,
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
                live_call_permit,
                discard_marker,
            } => {
                // A torn drop with the marker still armed: the guest discarded a persisted
                // successful completion. Chain the owned marker append after the terminal task
                // and let the chained handle take the terminal's place in the drop event, so
                // drains (and thus invocation completion) await the marker append too. The task
                // is spawned unconditionally — marker recording must not depend on the event
                // surviving the drain.
                let terminal = match discard_marker {
                    Some(marker) => Some(marker.spawn_chained(terminal)),
                    None => terminal,
                };
                if let Some(sink) = &self.cleanup_sink {
                    let _ = sink.send(DropEvent::CleanupAfterTerminal {
                        atomic_lease,
                        function_type,
                        durable_begin_index,
                        terminal,
                        trap_context,
                        live_call_permit,
                    });
                }
            }
            AccessTerminalGuardState::Disarmed => {}
        }
    }
}

/// Drains the worker's currently queued dropped-call events and records their durable effects
/// (cancellable drops as `Cancelled`, deferred terminal joins, scope closes, span finishes).
///
/// Called from the next safe worker-access window after the events were enqueued — the start of
/// every `&mut ctx` durable call ([`CallHandle::begin`]). The helper deliberately drains only
/// currently available events; callers decide where to wait for more work.
pub async fn drain_queued_dropped_call_events<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError> {
    let mut recorded = 0;
    while let Ok(event) = ctx.state.dropped_call_events.1.try_recv() {
        record_dropped_call_event(ctx, event).await?;
        recorded += 1;
    }
    Ok(recorded)
}

async fn record_dropped_call_event<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    event: DropEvent,
) -> Result<(), TerminalCallError> {
    match event {
        DropEvent::UnfinishedCancellable { call } => {
            let context = call.trap_context();
            call.wait_request_upload()
                .await
                .map_err(|err| TerminalCallError::new(err, context))?;
            call.append_cancelled(ctx, None)
                .await
                .map_err(|err| TerminalCallError::new(err, context))?;
        }
        DropEvent::UnfinishedNotCancellable { call } => {
            return Err(TerminalCallError::new(
                WorkerExecutorError::runtime(format!(
                    "non-cancellable durable call {} dropped without finish/cancel",
                    call.start_idx()
                )),
                call.trap_context(),
            ));
        }
        DropEvent::CleanupAfterTerminal {
            atomic_lease,
            function_type,
            durable_begin_index,
            terminal,
            trap_context,
            // Bound (not `_`) so the permit is released only at the end of this arm, after the
            // terminal join and scope close.
            live_call_permit: _live_call_permit,
        } => {
            if let Some(terminal) = terminal {
                let joined = terminal.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                });
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) | Err(err) => {
                        return Err(TerminalCallError::new(err, trap_context));
                    }
                }
            }
            if let Some(lease) = &atomic_lease {
                lease.release();
            }
            ctx.end_durable_function(&function_type, durable_begin_index, false)
                .await
                .map_err(|err| TerminalCallError::new(err, trap_context))?;
        }
        DropEvent::AwaitDiscardMarker {
            terminal,
            trap_context,
            // Bound (not `_`) so the permit is released only at the end of this arm, after the
            // marker append is joined.
            live_call_permit: _live_call_permit,
        } => {
            if let Some(terminal) = terminal {
                let joined = terminal.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "completion-discarded marker recorder task failed: {err}"
                    ))
                });
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) | Err(err) => {
                        return Err(TerminalCallError::new(err, trap_context));
                    }
                }
            }
        }
        DropEvent::CloseDurableScope {
            function_type,
            begin_index,
            span_id,
        } => {
            if ctx.state.is_durable_scope_open(begin_index) {
                ctx.end_durable_function(&function_type, begin_index, false)
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                if let Some(span_id) = span_id {
                    ctx.finish_span(&span_id)
                        .await
                        .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                }
            }
        }
        DropEvent::FinishSpan { span_id, durable } => {
            if durable {
                ctx.finish_span(&span_id)
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
            } else {
                finish_span_in_memory(ctx, &span_id)
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
            }
        }
    }
    Ok(())
}

/// Accessor-window variant of [`drain_queued_dropped_call_events`]. It drains the queue from a short
/// worker-state window, records `Cancelled` entries using owned oplog handles outside the window,
/// then re-enters only to unregister atomic-region membership.
pub async fn drain_dropped_call_events_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (oplog, sink, events) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.oplog.clone(),
            ctx.state
                .dropped_call_event_sender()
                .expect("dropped-call event sender is always available"),
            ctx.state.take_dropped_call_events(),
        )
    });
    let mut drain = AccessDropEventDrainGuard::new(sink, events);
    let mut recorded = 0;
    let mut first_error = None;
    while drain.start_next() {
        match drain.current_mut() {
            DropEvent::UnfinishedCancellable { call } => {
                let context = call.trap_context();
                let function_type = call.function_type().clone();
                let durable_begin_index = call.begin_index();
                let result = async {
                    call.wait_request_upload().await?;
                    call.append_cancelled_with_oplog(oplog.clone(), None).await
                }
                .await;
                match result {
                    Ok(()) => {
                        // The `Cancelled` entry is durable and the lease release is synchronous
                        // (store-free), so neutralize the event before the remaining fallible
                        // work: a torn drain from here re-queues only a permit-holding no-op
                        // instead of re-appending a second `Cancelled`.
                        call.release_atomic_lease();
                        let live_call_permit = call.live_call_permit.take();
                        drain.replace_current(DropEvent::AwaitDiscardMarker {
                            terminal: None,
                            trap_context: context,
                            live_call_permit,
                        });
                        if let Err(err) = end_durable_function_access(
                            store,
                            get_ctx,
                            function_type,
                            durable_begin_index,
                            false,
                        )
                        .await
                            && first_error.is_none()
                        {
                            first_error = Some(TerminalCallError::new(err, context));
                        }
                        recorded += 1;
                    }
                    Err(err) => {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(err, context));
                        }
                    }
                }
            }
            DropEvent::UnfinishedNotCancellable { call } => {
                let context = call.trap_context();
                let start_idx = call.start_idx();
                first_error.get_or_insert_with(|| {
                    TerminalCallError::new(
                        WorkerExecutorError::runtime(format!(
                            "non-cancellable durable call {start_idx} dropped without finish/cancel"
                        )),
                        context,
                    )
                });
            }
            DropEvent::CleanupAfterTerminal {
                atomic_lease,
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let atomic_lease = atomic_lease.clone();
                let function_type = function_type.clone();
                let durable_begin_index = *durable_begin_index;
                let trap_context = *trap_context;
                let mut terminal_recorded = true;
                if let Some(handle) = terminal {
                    match handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "durable call terminal recorder task failed: {err}"
                        ))
                    }) {
                        Ok(Ok(())) => {
                            *terminal = None;
                        }
                        Ok(Err(err)) | Err(err) => {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, trap_context));
                            }
                            terminal_recorded = false;
                        }
                    }
                }
                if terminal_recorded {
                    if let Some(lease) = &atomic_lease {
                        lease.release();
                    }
                    if let Err(err) = end_durable_function_access(
                        store,
                        get_ctx,
                        function_type,
                        durable_begin_index,
                        false,
                    )
                    .await
                    {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(err, trap_context));
                        }
                    } else {
                        recorded += 1;
                    }
                }
            }
            DropEvent::AwaitDiscardMarker {
                terminal,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let trap_context = *trap_context;
                if let Some(handle) = terminal {
                    match handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "completion-discarded marker recorder task failed: {err}"
                        ))
                    }) {
                        Ok(Ok(())) => {
                            *terminal = None;
                            recorded += 1;
                        }
                        Ok(Err(err)) | Err(err) => {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, trap_context));
                            }
                        }
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::CloseDurableScope {
                function_type,
                begin_index,
                span_id,
            } => {
                let function_type = function_type.clone();
                let begin_index = *begin_index;
                let span_id = span_id.clone();
                match end_durable_function_access_if_open(
                    store,
                    get_ctx,
                    function_type,
                    begin_index,
                    false,
                )
                .await
                {
                    Ok(true) => {
                        if let Some(span_id) = span_id
                            && let Err(err) = finish_span_access(store, get_ctx, &span_id).await
                            && first_error.is_none()
                        {
                            first_error = Some(TerminalCallError::new(
                                err,
                                store.with(|mut access| {
                                    let ctx = get_ctx(access.data_mut());
                                    ambient_trap_context(ctx)
                                }),
                            ));
                        }
                        recorded += 1;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(
                                err,
                                store.with(|mut access| {
                                    let ctx = get_ctx(access.data_mut());
                                    ambient_trap_context(ctx)
                                }),
                            ));
                        }
                    }
                }
            }
            DropEvent::FinishSpan { span_id, durable } => {
                let span_id = span_id.clone();
                let durable = *durable;
                let finish_result = if durable {
                    finish_span_access(store, get_ctx, &span_id).await
                } else {
                    store.with(|mut access| {
                        let ctx = get_ctx(access.data_mut());
                        finish_span_in_memory(ctx, &span_id)
                    })
                };
                if let Err(err) = finish_result {
                    if first_error.is_none() {
                        first_error = Some(TerminalCallError::new(
                            err,
                            store.with(|mut access| {
                                let ctx = get_ctx(access.data_mut());
                                ambient_trap_context(ctx)
                            }),
                        ));
                    }
                } else {
                    recorded += 1;
                }
            }
        }
        drain.finish_current();
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    drain.disarm();
    Ok(recorded)
}

/// Compile-time policy describing what happens when a [`CallHandle`] is dropped without being
/// explicitly finished or cancelled.
pub trait DropPolicy {
    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>);

    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        let _ = sink;
        None
    }
}

/// Drop policy for calls that may legitimately be cancelled (dropped from a `select!`, etc.).
pub struct Cancellable;

/// Drop policy for idempotent calls whose unfinished live execution should be retried from the
/// committed `Start` during replay instead of recorded as a terminal cancellation.
pub struct LeaveIncompleteOnDrop;

/// Drop policy for calls that must always be finished or explicitly cancelled. Dropping one
/// unfinished is a bug (default-deny).
pub struct NotCancellable;

impl DropPolicy for Cancellable {
    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
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
        // No terminal will ever be recorded for this call (its committed `Start` is left
        // incomplete for replay), so its atomic-region membership ends here. The release is
        // store-free, so it is safe directly from the drop path.
        call.release_atomic_lease();
        tracing::debug!(
            start_idx = %call.start_idx(),
            "durable call dropped unfinished; leaving committed Start incomplete for replay"
        );
    }
}

impl DropPolicy for NotCancellable {
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
    /// Tracks the (possibly deferred) blob upload of this call's request payload, started when the
    /// `Start` was reserved. Awaited before the matching `End` / `Cancelled` is appended so an
    /// upload failure surfaces at the call site rather than only at the leaf oplog's commit barrier.
    /// `PendingUpload::already_durable()` (a no-op) for replay handles, snapshotting, and inline
    /// requests.
    request_upload: PendingUpload,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    replay: Option<ReplayCallHandle>,
    finished: bool,
    /// `true` when this replay handle parked on a recorded terminal that was never delivered to
    /// the guest — a `Cancelled { partial: None }` or an `End` marked `CompletionDiscarded` —
    /// waiting for the deterministic guest to drop it at the same point it did live. Makes that
    /// drop an expected state (debug-logged, scope close enqueued) rather than an anomaly.
    parked_undelivered_replay: bool,
    /// Initiation-time execution metadata owned by this call, including its atomic-region
    /// ownership lease (see [`CallExecutionScope::atomic_lease`]). Later phases still mirror
    /// selected fields into `PrivateDurableWorkerState` for compatibility, but the call-owned copy
    /// is the source we can move retry/atomic decisions onto as the Accessor reshape proceeds.
    execution_scope: CallExecutionScope,
    /// In-function retry decision logic. Also the home of the call's `DurableFunctionType` and
    /// captured `DurableExecutionState`.
    retry: InFunctionRetryController,
    /// Policy-controlled sink for unfinished drops, from [`DropPolicy::production_drop_sink`]:
    /// the worker's dropped-call event sender for `Cancellable`/`LeaveIncompleteOnDrop`, `None`
    /// for `NotCancellable`. Unit tests attach their own sink to observe drop events.
    drop_sink: Option<UnboundedSender<DropEvent>>,
    /// Policy-independent sink for `CleanupAfterTerminal` events: a torn terminal (the completion
    /// future dropped while the spawned terminal append is still pending) must hand its join +
    /// in-flight permit to the next drain even for policies whose `production_drop_sink` is `None`
    /// (e.g. `NotCancellable`).
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// Counts this handle as an in-flight live host call while present.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

#[derive(Debug)]
pub struct LiveCallPermit(Arc<AtomicUsize>);

impl LiveCallPermit {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Clone for LiveCallPermit {
    /// Cloning takes an additional permit on the same counter, so the call stays counted as
    /// in-flight until *every* holder (the handle itself, a queued drop event, a terminal guard)
    /// has been dropped.
    fn clone(&self) -> Self {
        Self::new(self.0.clone())
    }
}

impl Drop for LiveCallPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone)]
struct BegunCallExecutionScope {
    /// The durable scope this host-call `Start` will be nested under, if any. This is derived from
    /// the call's own function type / begin index, never from temporally-open sibling scopes.
    parent_start_index: Option<OplogIndex>,
    /// Atomic region active when the durable call was initiated, captured so the call's membership
    /// lease is registered against the region it was actually started in.
    atomic_region: Option<OplogIndex>,
    /// Persistence level active when the call was initiated. Kept with the call so p3 Accessor
    /// windows can snapshot all execution facts before async work resumes elsewhere.
    #[allow(dead_code)]
    persistence_level: PersistenceLevel,
}

impl BegunCallExecutionScope {
    fn finish(
        self,
        start_idx: OplogIndex,
        atomic_lease: Option<Arc<AtomicRegionLease>>,
    ) -> CallExecutionScope {
        CallExecutionScope {
            retry_from: self.parent_start_index.unwrap_or(start_idx),
            durable_scope: self.parent_start_index,
            atomic_lease,
            persistence_level: self.persistence_level,
        }
    }
}

#[derive(Debug, Clone)]
struct CallExecutionScope {
    /// The retry point owned by this in-flight call: the enclosing durable scope `Start` if present,
    /// otherwise the host-call `Start` itself.
    retry_from: OplogIndex,
    /// The enclosing durable scope, if this call belongs to one.
    #[allow(dead_code)]
    durable_scope: Option<OplogIndex>,
    /// The call's atomic-region ownership lease, registered when the call was initiated inside an
    /// open atomic region. The lease's *current* owner — not the initiation-time region — drives
    /// trap/retry classification: region close transfers pending members to the enclosing region
    /// or detaches them, and a detached call must group at its own `retry_from` instead of
    /// retrying into the committed region.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
    /// Persistence level active when this call was initiated.
    #[allow(dead_code)]
    persistence_level: PersistenceLevel,
}

impl CallExecutionScope {
    /// The atomic region *currently* owning this call, if any (read through the lease, so it
    /// reflects transfers and detachments performed by region close).
    fn atomic_region(&self) -> Option<OplogIndex> {
        self.atomic_lease.as_ref().and_then(|lease| lease.owner())
    }

    /// Releases the call's atomic-region membership (idempotent, store-free).
    fn release_atomic_lease(&self) {
        if let Some(lease) = &self.atomic_lease {
            lease.release();
        }
    }

    /// The retry point to attach to a trap raised by this call: the call's *currently owning*
    /// atomic region (whole region retried from its begin index) if it still has one, otherwise
    /// its enclosing durable scope `Start` or its own `Start`. This mirrors
    /// [`ScopedRetryHost::retry_point`] so a hard (non-semantic) trap groups exactly like an
    /// inline/semantic retry would.
    fn trap_retry_point(&self) -> OplogIndex {
        self.atomic_region().unwrap_or(self.retry_from)
    }
}

/// Builds an *unregistered* atomic-region lease: it preserves the call's initiation-time region
/// for trap/retry classification (matching the immutable capture used before leases existed) but
/// is not a member of any region registry, so it never transfers or detaches on region close.
/// Used for replay and snapshotting handles, which do not participate in the live in-flight
/// member guard.
fn unregistered_atomic_lease(
    atomic_region: Option<OplogIndex>,
    repairable_when_incomplete: bool,
) -> Option<Arc<AtomicRegionLease>> {
    atomic_region.map(|begin_index| {
        Arc::new(AtomicRegionLease::new(
            begin_index,
            repairable_when_incomplete,
        ))
    })
}

struct PreparedAccessStart<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    is_live: bool,
    snapshotting: bool,
    oplog: Arc<dyn Oplog>,
    public_state: PublicDurableWorkerState<Ctx>,
    replay_state: crate::durable_host::replay_state::ReplayState,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    /// The registered atomic-region membership lease for a live persisted call initiated inside an
    /// open atomic region; `None` otherwise.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    claim_options: AccessClaimOptions,
    /// The worker's in-flight live host call counter, kept so `execute_access_start` can take a
    /// permit when a replayed scope switches to live mid-start.
    live_host_calls: Arc<AtomicUsize>,
    /// Taken at prepare time — synchronously with worker state, *before* any oplog entry of this
    /// call is appended — so an in-flight check (e.g. the `set_oplog_persistence_level` boundary
    /// guard) can never observe zero calls between this call's `Start` append and its handle
    /// construction in `finish_access_start`. `Some` for live calls, `None` on replay.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

/// Options that make the durable records of a concurrent accessor call claim-safe on replay.
///
/// Accessor host calls run concurrently, so their oplog `Start` entries are appended in
/// scheduling order, which replay does not reproduce. Calls whose identity (function name +
/// durable function type) is shared by concurrent siblings need extra identity to be paired with
/// their own records instead of a sibling's:
///
/// - `scope_discriminator` is appended to the batched-write scope `Start`'s synthetic function
///   name (`<scope:batched-write:DISCRIMINATOR>`), so the replayed call claims exactly its own
///   scope. It must be a deterministic function of state that is identical on the live and replay
///   paths (e.g. a hash of the recorded request, or the `Start` index of an already-claimed
///   related call). Replay matches the discriminated name exactly — there is no plain-name
///   fallback (P3 deploys on a clean database, so no oplog predates the discriminators).
/// - `request_identity` is the [`HostRequest`] value the live path persists in the call's
///   `Start` entry; when set, the replay claim also requires the recorded request payload to
///   match it (by value, never by serialized bytes — payloads can contain `HashMap`s whose byte
///   order is process-random), disambiguating concurrent top-level calls that differ only in
///   their request.
#[derive(Default)]
pub(crate) struct AccessClaimOptions {
    pub(crate) scope_discriminator: Option<String>,
    pub(crate) request_identity: Option<HostRequest>,
}

struct ExecutedAccessStart<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    start_idx: OplogIndex,
    persisted: bool,
    request_upload: PendingUpload,
    replay: Option<ReplayCallHandle>,
    execution_scope: CallExecutionScope,
    retry: InFunctionRetryController,
    opened_scope: Option<AccessOpenedScope>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// The in-flight permit taken at prepare time (or when a replayed scope switched to live),
    /// handed on to the [`CallHandle`] so the call stays counted continuously from before its
    /// first oplog append until its terminal is recorded.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

struct AccessOpenedScope {
    begin_index: OplogIndex,
    replay_handle: Option<ReplayCallHandle>,
    switched_to_live: bool,
}

struct AccessStartCleanup {
    /// The lease to release when the start failed before a handle was constructed. Release is
    /// idempotent and store-free.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
}

/// Context handed to the request builder of [`CallHandle::start_access_with`], available after the
/// durable scope (if any) has been opened but before the host-call `Start` is written or claimed.
pub struct AccessStartContext {
    /// The begin index of the call, mirroring `begin_durable_function`: the durable-scope `Start`
    /// index when the call opens a scope, otherwise the pre-call oplog index. Stable across
    /// live/replay for scope-opening calls; approximately stable otherwise (see
    /// [`CallHandle::start_access_with`]).
    pub begin_index: OplogIndex,
    /// Whether the call executes live (the built request will be persisted) or replays (the built
    /// request is discarded; the builder still runs for its positional replay side effects).
    pub is_live: bool,
}

/// Releases a just-registered atomic-region lease when the accessor start path is torn (the
/// future dropped between registration and handle construction). Release is store-free, so the
/// `Drop` impl performs it directly.
struct AccessStartAtomicGuard {
    atomic_lease: Option<Arc<AtomicRegionLease>>,
}

impl AccessStartAtomicGuard {
    fn new(atomic_lease: Option<Arc<AtomicRegionLease>>) -> Self {
        Self { atomic_lease }
    }

    fn disarm(&mut self) {
        self.atomic_lease = None;
    }
}

impl Drop for AccessStartAtomicGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.atomic_lease.take() {
            lease.release();
        }
    }
}

fn is_write_side_effect_for_access(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::WriteRemote
            | DurableFunctionType::WriteRemoteBatched(_)
            | DurableFunctionType::WriteRemoteTransaction(_)
    )
}

struct ScopedRetryHost<'a, H> {
    inner: &'a mut H,
    execution_scope: &'a CallExecutionScope,
}

impl<'a, H> ScopedRetryHost<'a, H> {
    fn new(inner: &'a mut H, execution_scope: &'a CallExecutionScope) -> Self {
        Self {
            inner,
            execution_scope,
        }
    }

    fn retry_point(&self) -> OplogIndex {
        self.execution_scope.trap_retry_point()
    }
}

#[async_trait]
impl<H: InFunctionRetryHost + Send + Sync> InFunctionRetryHost for ScopedRetryHost<'_, H> {
    fn in_atomic_region(&self) -> bool {
        self.execution_scope.atomic_region().is_some()
    }

    fn current_retry_point(&self) -> OplogIndex {
        self.retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
        self.inner.named_retry_policies().await
    }

    async fn current_retry_state_for(
        &self,
        retry_from: OplogIndex,
    ) -> Option<golem_common::model::RetryPolicyState> {
        self.inner.current_retry_state_for(retry_from).await
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        let mut state = self.inner.durable_execution_state();
        state.persistence_level = self.execution_scope.persistence_level;
        state
    }

    fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
        self.inner.atomic_region_has_side_effects_for(begin_index)
    }

    fn retry_context_atomic_region_had_side_effects(&self) -> bool {
        // Membership-precise: classify against the region *currently owning* this call (via its
        // lease — the initiation region, a parent it was transferred to on nested close, or none
        // once detached). A call started outside any atomic region never writes
        // `inside_atomic_region = true`, even if a sibling later opened one.
        self.execution_scope
            .atomic_region()
            .is_some_and(|begin_index| self.inner.atomic_region_has_side_effects_for(begin_index))
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        inside_atomic_region: bool,
        retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) {
        self.inner
            .append_retry_error_entry(retry_from, inside_atomic_region, retry_policy_state)
            .await;
    }
}

#[async_trait]
impl<H: DurabilityHost + Send + Sync> DurabilityHost for ScopedRetryHost<'_, H> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        self.inner.observe_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.inner
            .begin_durable_function(function_type, host_function)
            .await
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.inner
            .end_durable_function(function_type, begin_index, forced_commit)
            .await
    }

    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) {
        self.inner
            .persist_durable_function_invocation(function_name, request, response, function_type)
            .await;
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<
        crate::durable_host::durability::PersistedDurableFunctionInvocation,
        WorkerExecutorError,
    > {
        self.inner
            .read_persisted_durable_function_invocation()
            .await
    }

    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        try_trigger_host_trap_retry(self, failure, properties).await
    }

    fn mark_atomic_region_side_effect(&mut self) {
        self.inner.mark_atomic_region_side_effect();
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.inner.create_interrupt_signal()
    }

    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        self.inner.check_read_only_allows(host_function)
    }
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

    /// Accessor-window entry point for p3 host functions. The method uses short synchronous store
    /// windows only for worker-state snapshots and in-memory bookkeeping; oplog/replay awaits run on
    /// owned handles outside those windows.
    ///
    /// The accessor path supports read calls and remote writes. Scope-opening writes open their
    /// durable scope through owned oplog/replay handles and only re-enter the store for in-memory
    /// bookkeeping.
    pub async fn start_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        Self::start_access_with(store, get_ctx, function_type, async move |_| Ok(request)).await
    }

    /// Like [`Self::start_access`], but the request payload is built by `build_request` *between*
    /// the durable-scope open and the host-call `Start` write/claim. This is the accessor
    /// counterpart of the two-step [`Self::begin`] + [`BegunCall::start_live`] flow: it exists for
    /// calls whose persisted request depends on the begin index (e.g. a derived idempotency key) or
    /// that must interleave other positional oplog entries (e.g. a `StartSpan`) between the scope
    /// `Start` and the host-call `Start`.
    ///
    /// The builder runs on **both** the live and the replay path (so positional side entries it
    /// replays, like `StartSpan`, are consumed in the same order they were written), but its
    /// returned request is only persisted on the live path. It receives an [`AccessStartContext`]
    /// with the begin index — the durable-scope `Start` index when the call opens a scope, or the
    /// pre-call oplog index otherwise, mirroring `begin_durable_function` — and the liveness flag.
    /// For non-scope-opening calls the pre-call index is not perfectly stable between a live run
    /// and an incomplete-replay re-execution under concurrent siblings; callers deriving identity
    /// from it accept the same tradeoff as the RPC `async-invoke-and-await` path.
    ///
    /// A builder error aborts the call like any other start failure: the atomic-region
    /// registration is cleaned up and any already-written durable-scope `Start` is left incomplete,
    /// to be recovered by scope recovery on the next replay.
    pub async fn start_access_with<T, D, Ctx, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        Self::start_access_with_options(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            build_request,
        )
        .await
    }

    /// [`Self::start_access_with`] with explicit [`AccessClaimOptions`], for calls whose durable
    /// records need extra identity to be claim-safe under concurrent siblings on replay.
    pub(crate) async fn start_access_with_options<T, D, Ctx, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path currently supports only ReadLocal/ReadRemote/WriteRemote/WriteRemoteBatched, got {function_type:?}"
            )));
        }
        let prepared = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::prepare_access_start(ctx, function_type, claim_options)
        })?;
        let mut start_guard = AccessStartAtomicGuard::new(prepared.atomic_lease.clone());

        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            return Err(err.source);
        }

        match Self::execute_access_start(prepared, build_request).await {
            Ok(executed) => {
                process_pending_replay_events_access(store, get_ctx).await?;
                let result = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::finish_access_start(ctx, executed)
                });
                if result.is_ok() {
                    start_guard.disarm();
                }
                let mut handle = result?;
                handle
                    .supersede_prior_completion_delivery(store, get_ctx)
                    .await?;
                Ok(handle)
            }
            Err((err, cleanup)) => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::cleanup_access_start(ctx, cleanup);
                });
                start_guard.disarm();
                Err(err)
            }
        }
    }

    /// Supersedes any completion-delivery observer still armed on this accessor's host subtask
    /// once this call owns a persisted live `Start`.
    ///
    /// Such an observer belongs to an *earlier* durable call in the same host function whose
    /// response has already been consumed internally by host code (see
    /// [`CompletionDelivery::deliver_at_accessor_terminal`]): the guest can no longer discard
    /// that completion individually — only the whole host call's terminal — and replay
    /// re-executes the host code past its `End` deterministically, re-consuming the response
    /// internally. So no `CompletionDiscarded` marker is needed, and the observer's
    /// [`LiveCallPermit`] must not keep blocking suspension (e.g. for the whole duration of a
    /// subsequent suspendable wait such as `monotonic-clock.wait-for`). Clearing the observer
    /// drops its guard, which suppresses the token and releases the permit; the follow-up drain
    /// joins any ordered append the superseded token parked in the drain queue, so its oplog
    /// ordering and settlement accounting survive.
    ///
    /// This handoff is only performed once *this* call is a persisted live barrier (never for
    /// replay or snapshotting handles): until then the prior observer must stay armed so a guest
    /// cancellation landing before this call's `Start` still records its `CompletionDiscarded`
    /// marker and parks replay at the prior call.
    ///
    /// Invariant required of host functions: after this supersession, a failure of this call
    /// must either arm a newer observer (via its own completion) or escape as a
    /// trap/cancellation — host code must not swallow the failure and return a successful outer
    /// result derived from the superseded completion with no observer armed.
    async fn supersede_prior_completion_delivery<T, D, Ctx>(
        &mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        if !(self.is_live && self.persisted) {
            return Ok(());
        }
        if let Err(error) = store.clear_terminal_observer() {
            self.abandon_for_trap();
            return Err(WorkerExecutorError::runtime(format!(
                "failed to supersede prior accessor completion observer: {error}"
            )));
        }
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err.source);
        }
        Ok(())
    }

    fn prepare_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
    ) -> Result<PreparedAccessStart<Pair, P, Ctx>, WorkerExecutorError> {
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path currently supports only ReadLocal/ReadRemote/WriteRemote/WriteRemoteBatched, got {function_type:?}"
            )));
        }
        if is_write_side_effect_for_access(&function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = DurableWorkerCtx::check_read_only_allows(ctx, Pair::FQFN)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }

        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let atomic_region = ctx
            .state
            .active_atomic_regions
            .last()
            .map(|region| region.begin_index);

        let parent_start_index = ctx
            .state
            .child_parent_start_index(&function_type, OplogIndex::INITIAL);
        let execution_scope = BegunCallExecutionScope {
            parent_start_index,
            atomic_region,
            persistence_level: ctx.state.persistence_level,
        };
        let is_live = durable_execution_state.is_live;
        let snapshotting = durable_execution_state.snapshotting_mode.is_some();
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN);
        // A live persisted call initiated inside an open atomic region joins the region's member
        // registry: its lease starts owned by that region and follows the region's close
        // transitions (transfer to the enclosing region, or detachment at the outermost close).
        let atomic_lease = if is_live && !snapshotting {
            match atomic_region {
                Some(begin_index) => Some(
                    ctx.state
                        .register_atomic_region_call(
                            begin_index,
                            retry.can_reexecute_on_incomplete_replay(),
                        )
                        .ok_or_else(|| {
                            WorkerExecutorError::runtime(format!(
                                "durable call started in atomic region {begin_index}, but the region is not open"
                            ))
                        })?,
                ),
                None => None,
            }
        } else {
            None
        };
        let cleanup_sink = ctx.state.dropped_call_event_sender();
        let live_host_calls = ctx.state.live_host_call_counter();
        Ok(PreparedAccessStart {
            is_live,
            snapshotting,
            oplog: ctx.state.oplog.clone(),
            public_state: ctx.public_state.clone(),
            replay_state: ctx.state.replay_state.clone(),
            execution_scope,
            retry,
            atomic_lease,
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink,
            claim_options,
            live_call_permit: if is_live {
                Some(LiveCallPermit::new(live_host_calls.clone()))
            } else {
                None
            },
            live_host_calls,
            _phantom: PhantomData,
        })
    }

    /// Persistence-suppression model: only **snapshotting** is handled here (the live
    /// `persisted: false` branch). `PersistenceLevel::PersistNothing` deliberately is *not* — a
    /// live call inside a persist-nothing zone still appends its `Start`/`End`, exactly like the
    /// legacy P2 path (`persist_durable_function_invocation`), because the PersistNothing contract
    /// is enforced elsewhere: `PrimaryOplog::commit` suppresses non-`Always` commits while the
    /// zone is open (so the call's own `DurableOnly` commits flush nothing), zone contents that do
    /// reach storage (via the zone-closing `Always` commit) are observability-only, and the replay
    /// cursor skips whole persist-nothing zones without claiming the entries inside them. The
    /// replay branch below guards against ever *replaying* a durable call inside a PersistNothing
    /// block, mirroring `read_persisted_durable_function_invocation`.
    async fn execute_access_start<Ctx: WorkerCtx, F>(
        mut prepared: PreparedAccessStart<Pair, P, Ctx>,
        build_request: F,
    ) -> Result<ExecutedAccessStart<Pair, P>, (WorkerExecutorError, AccessStartCleanup)>
    where
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        let mut live_call_permit = prepared.live_call_permit.take();
        let starts_scope = opens_accessor_scope(
            prepared.retry.function_type(),
            prepared.retry.durable_execution_state().assume_idempotence,
            prepared.snapshotting,
        );
        let scope_start = if starts_scope {
            Some(Self::execute_access_scope_start(&prepared).await?)
        } else {
            None
        };

        let mut execution_scope = prepared.execution_scope;
        let mut retry = prepared.retry;
        let mut is_live = prepared.is_live;
        if let Some(scope_start) = &scope_start {
            execution_scope.parent_start_index = Some(scope_start.begin_index);
            if scope_start.switched_to_live {
                let previous = retry.durable_execution_state();
                retry = InFunctionRetryController::new(
                    retry.function_type().clone(),
                    DurableExecutionState {
                        is_live: true,
                        persistence_level: previous.persistence_level,
                        snapshotting_mode: previous.snapshotting_mode,
                        assume_idempotence: previous.assume_idempotence,
                        max_in_function_retry_delay: previous.max_in_function_retry_delay,
                    },
                    Pair::FQFN,
                );
                is_live = true;
                if live_call_permit.is_none() {
                    live_call_permit = Some(LiveCallPermit::new(prepared.live_host_calls.clone()));
                }
            }
        }

        // Build the request between the scope open and the host-call `Start` write/claim. The
        // builder's begin index mirrors `begin_durable_function`: the scope `Start` index when a
        // scope was opened, otherwise the pre-call index (last added / last replayed non-hint
        // entry). It runs on the replay path too so any positional side entries it wrote live are
        // consumed here in the same order.
        let builder_begin_index = match &scope_start {
            Some(scope) => scope.begin_index,
            None => {
                if is_live {
                    prepared.oplog.current_oplog_index().await
                } else {
                    prepared.replay_state.last_replayed_non_hint_index()
                }
            }
        };
        let request = build_request(AccessStartContext {
            begin_index: builder_begin_index,
            is_live,
        })
        .await
        .map_err(|err| {
            (
                err,
                AccessStartCleanup {
                    atomic_lease: prepared.atomic_lease.clone(),
                },
            )
        })?;

        if is_live {
            if prepared.snapshotting {
                let start_idx = prepared.oplog.current_oplog_index().await;
                let atomic_lease = unregistered_atomic_lease(
                    execution_scope.atomic_region,
                    retry.can_reexecute_on_incomplete_replay(),
                );
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: false,
                    request_upload: PendingUpload::already_durable(),
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx, atomic_lease),
                    retry,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            } else {
                let request: HostRequest = request.into();
                let function_type = retry.function_type().clone();
                let parent_start_index = execution_scope.parent_start_index;
                let (start_idx, request_upload) = prepared
                    .oplog
                    .add_start_with_reserved_payload(request, move |request_payload| {
                        OplogEntry::Start {
                            timestamp: Timestamp::now_utc(),
                            parent_start_index,
                            function_name: Pair::HOST_FUNCTION_NAME,
                            request: Some(request_payload),
                            durable_function_type: function_type,
                        }
                    })
                    .await
                    .map_err(|err| {
                        (
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store durable call request: {err}"
                            )),
                            AccessStartCleanup {
                                atomic_lease: prepared.atomic_lease.clone(),
                            },
                        )
                    })?;
                // The registered lease from prepare time when this call started live inside an
                // open region; a replayed scope that switched to live mid-start falls back to an
                // unregistered lease preserving its initiation-time classification.
                let atomic_lease = prepared.atomic_lease.clone().or_else(|| {
                    unregistered_atomic_lease(
                        execution_scope.atomic_region,
                        retry.can_reexecute_on_incomplete_replay(),
                    )
                });
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: true,
                    request_upload,
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx, atomic_lease),
                    retry,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            }
        } else {
            if retry.durable_execution_state().persistence_level == PersistenceLevel::PersistNothing
            {
                return Err((
                    WorkerExecutorError::runtime(
                        "Trying to replay a durable invocation in a PersistNothing block",
                    ),
                    AccessStartCleanup {
                        atomic_lease: prepared.atomic_lease.clone(),
                    },
                ));
            }
            // A call owned by another durable record (its own opened scope, or the parent
            // encoded in the function type) is claimed by identity: accessor host calls run
            // concurrently, so owned `Start`s (e.g. per-chunk children of overlapping
            // consume-body scopes, or call `Start`s under sibling batched-write scopes) are not
            // appended in deterministic guest-initiation order and cannot be claimed
            // positionally. When the caller supplied a request identity, the claim additionally
            // requires the recorded request payload to match, disambiguating concurrent calls
            // whose durable identity is otherwise shared (e.g. parallel P3 HTTP sends).
            let claim_result = match (
                execution_scope.parent_start_index,
                prepared.claim_options.request_identity.as_ref(),
            ) {
                (Some(parent_start_index), Some(expected_request)) => {
                    prepared
                        .replay_state
                        .claim_owned_concurrent_start_matching_request(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            parent_start_index,
                            expected_request,
                        )
                        .await
                }
                (Some(parent_start_index), None) => {
                    prepared
                        .replay_state
                        .claim_owned_concurrent_start(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            parent_start_index,
                        )
                        .await
                }
                (None, Some(expected_request)) => {
                    prepared
                        .replay_state
                        .claim_concurrent_start_matching_request(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            expected_request,
                        )
                        .await
                }
                (None, None) => {
                    prepared
                        .replay_state
                        .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, retry.function_type())
                        .await
                }
            };
            let replay = claim_result.map_err(|err| {
                (
                    err,
                    AccessStartCleanup {
                        atomic_lease: prepared.atomic_lease.clone(),
                    },
                )
            })?;
            let start_idx = replay.start_idx();
            // Replay handles never participate in the live in-flight member guard, but keep
            // their initiation-time region for trap/retry classification.
            let atomic_lease = unregistered_atomic_lease(
                execution_scope.atomic_region,
                retry.can_reexecute_on_incomplete_replay(),
            );
            Ok(ExecutedAccessStart {
                begin_index: scope_start
                    .as_ref()
                    .map(|scope| scope.begin_index)
                    .unwrap_or(start_idx),
                start_idx,
                persisted: false,
                request_upload: PendingUpload::already_durable(),
                replay: Some(replay),
                execution_scope: execution_scope.finish(start_idx, atomic_lease),
                retry,
                opened_scope: scope_start,
                drop_sink: prepared.drop_sink,
                cleanup_sink: prepared.cleanup_sink,
                live_call_permit: None,
                _phantom: PhantomData,
            })
        }
    }

    async fn execute_access_scope_start<Ctx: WorkerCtx>(
        prepared: &PreparedAccessStart<Pair, P, Ctx>,
    ) -> Result<AccessOpenedScope, (WorkerExecutorError, AccessStartCleanup)> {
        let function_type = prepared.retry.function_type().clone();
        // A caller-supplied discriminator makes the synthetic scope name unique among concurrent
        // siblings, so the replay claim below pairs the call with exactly its own recorded scope
        // (and with it the correct incomplete-scope detection). Without one, concurrent scopes of
        // the same durable function type are interchangeable at claim time.
        let scope_name = match &prepared.claim_options.scope_discriminator {
            Some(discriminator) => {
                HostFunctionName::Custom(format!("<scope:batched-write:{discriminator}>"))
            }
            None => HostFunctionName::Custom("<scope:batched-write>".to_string()),
        };
        if prepared.is_live {
            let entry = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: scope_name,
                request: None,
                durable_function_type: function_type,
            };
            let begin_index = prepared
                .public_state
                .worker()
                .add_and_commit_oplog(entry)
                .await;
            Ok(AccessOpenedScope {
                begin_index,
                replay_handle: None,
                switched_to_live: false,
            })
        } else {
            // Replay requires exactly the name the live path recorded (including the
            // discriminator suffix). There is no plain-name fallback: a discriminated claim must
            // never steal a plain sibling scope, and P3 deploys on a clean database so every
            // replayed oplog was recorded with the discriminators already in place.
            let (begin_index, replay_handle) = prepared
                .replay_state
                .claim_scope_start(&scope_name, &function_type)
                .await
                .map_err(|err| {
                    (
                        err,
                        AccessStartCleanup {
                            atomic_lease: prepared.atomic_lease.clone(),
                        },
                    )
                })?;

            if function_type == DurableFunctionType::WriteRemote
                && !prepared.retry.durable_execution_state().assume_idempotence
            {
                if prepared
                    .replay_state
                    .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                    .await
                    .is_none()
                {
                    prepared.replay_state.switch_to_live().await;
                    return Err((
                        WorkerExecutorError::runtime(
                            "Non-idempotent remote write operation was not completed, cannot retry",
                        ),
                        AccessStartCleanup {
                            atomic_lease: prepared.atomic_lease.clone(),
                        },
                    ));
                }

                Ok(AccessOpenedScope {
                    begin_index,
                    replay_handle: Some(replay_handle),
                    switched_to_live: false,
                })
            } else if matches!(function_type, DurableFunctionType::WriteRemoteBatched(None)) {
                let lookup_result = prepared
                    .replay_state
                    .lookup_oplog_entry_with_condition_and_state(
                        begin_index,
                        OplogEntry::is_end_remote_write_s::<ScopeScanState>,
                        OplogEntry::no_concurrent_side_effect,
                        ScopeScanState::new(
                            begin_index,
                            prepared.execution_scope.persistence_level,
                        ),
                        OplogEntry::track_scope_membership,
                    )
                    .await;

                match lookup_result {
                    OplogEntryLookupResult::Found { .. } => Ok(AccessOpenedScope {
                        begin_index,
                        replay_handle: Some(replay_handle),
                        switched_to_live: false,
                    }),
                    OplogEntryLookupResult::NotFound {
                        violates_for_all: false,
                    } if prepared.retry.durable_execution_state().assume_idempotence => {
                        prepared.replay_state.switch_to_live().await;
                        let deleted_region = OplogRegion {
                            start: begin_index.next(),
                            end: prepared.replay_state.replay_target().next(),
                        };
                        prepared
                            .public_state
                            .worker()
                            .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                            .await;
                        prepared
                            .public_state
                            .worker()
                            .reattach_worker_status()
                            .await;
                        Ok(AccessOpenedScope {
                            begin_index,
                            replay_handle: None,
                            switched_to_live: true,
                        })
                    }
                    OplogEntryLookupResult::NotFound { .. } => {
                        prepared.replay_state.switch_to_live().await;
                        Err((
                            WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ),
                            AccessStartCleanup {
                                atomic_lease: prepared.atomic_lease.clone(),
                            },
                        ))
                    }
                }
            } else {
                Ok(AccessOpenedScope {
                    begin_index,
                    replay_handle: Some(replay_handle),
                    switched_to_live: false,
                })
            }
        }
    }

    fn finish_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        executed: ExecutedAccessStart<Pair, P>,
    ) -> Result<Self, WorkerExecutorError> {
        if let Some(scope) = executed.opened_scope {
            let kind = if matches!(
                executed.retry.function_type(),
                DurableFunctionType::WriteRemoteBatched(None)
            ) {
                DurableScopeKind::BatchedWrite
            } else {
                DurableScopeKind::NonIdempotentWrite
            };
            ctx.state
                .push_durable_scope(scope.begin_index, kind, scope.replay_handle);
            ctx.state.current_retry_point = scope.begin_index;
        }
        let is_live = executed.replay.is_none();
        Ok(CallHandle {
            start_idx: executed.start_idx,
            begin_index: executed.begin_index,
            is_live,
            persisted: executed.persisted,
            request_upload: executed.request_upload,
            replay: executed.replay,
            finished: false,
            parked_undelivered_replay: false,
            execution_scope: executed.execution_scope,
            retry: executed.retry,
            drop_sink: executed.drop_sink,
            cleanup_sink: executed.cleanup_sink,
            // Taken at prepare time (or at the replay→live switch), before this call's first
            // oplog append, so the in-flight count never dips to zero mid-call.
            live_call_permit: executed.live_call_permit,
            _phantom: PhantomData,
        })
    }

    fn cleanup_access_start<Ctx: WorkerCtx>(
        _ctx: &mut DurableWorkerCtx<Ctx>,
        cleanup: AccessStartCleanup,
    ) {
        if let Some(lease) = cleanup.atomic_lease {
            lease.release();
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
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(|err| err.source)?;
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);

        // Read-only guard, pending replay events and durable-scope open all happen inside
        // `begin_durable_function`.
        let begin_index = ctx
            .begin_durable_function(&function_type, Pair::FQFN)
            .await?;
        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let execution_scope = BegunCallExecutionScope {
            parent_start_index: ctx
                .state
                .child_parent_start_index(&function_type, begin_index),
            atomic_region: ctx
                .state
                .active_atomic_regions
                .last()
                .map(|region| region.begin_index),
            persistence_level: ctx.state.persistence_level,
        };
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN);

        Ok(BegunCall {
            begin_index,
            execution_scope,
            retry,
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink: ctx.state.dropped_call_event_sender(),
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

    /// Low-level abandon: marks the call as finished without writing anything to the oplog, leaving
    /// its host-call `Start` incomplete on disk. This is the terminal used when a host call traps
    /// (fall-back to oplog replay) or is interrupted: a trap is **not** a cancellation, so it must
    /// never write a `Cancelled`. The incomplete `Start` is resolved on the next replay/retry (see
    /// [`CallReplayOutcome::Incomplete`]).
    ///
    /// This does **not** attach a [`DurableCallTrapContext`]. Every *hard error* that escapes as a
    /// `TrapType::Error` must instead go through [`Self::trap`], so the post-trap retry grouping is
    /// owned by this call's scope rather than ambient worker state a sibling call could clobber once
    /// durable calls overlap. Raw `abandon_for_trap` is only for non-`TrapType::Error` control flow
    /// (interrupts / sleep-suspend), where `TrapType::from_error` ignores the marker anyway, and for
    /// tests.
    pub(crate) fn abandon_for_trap(&mut self) {
        self.finished = true;
    }

    /// The call-owned trap classification for a hard error escaping this call: its own retry point
    /// and atomic-region membership, derived purely from the call's execution scope (the region it
    /// was *initiated* in), never from ambient worker state. Being pure, it needs no `ctx` and is
    /// stable across the call's body, so it can be built at the trap egress point or snapshotted
    /// earlier when the handle is moved before egress (see `io::poll`).
    pub(crate) fn trap_context(&self) -> DurableCallTrapContext {
        DurableCallTrapContext {
            retry_from: self.execution_scope.trap_retry_point(),
            in_atomic_region: self.execution_scope.atomic_region().is_some(),
        }
    }

    /// Abandons this call for a hard trap and wraps the escaping error with this call's
    /// [`DurableCallTrapContext`], so `TrapType::from_error` groups the failure against the call's
    /// own scope. Use this at every `TrapType::Error` egress after the call has started; see
    /// [`Self::abandon_for_trap`] for the non-error control-flow cases.
    pub fn trap(&mut self, err: impl Into<anyhow::Error>) -> anyhow::Error {
        let err = err.into();
        // Invariant lock: a marked host-call trap must never be a deterministic wasm trap. The
        // marker carries no call-owned atomic-region side-effect bit (`TrapType::from_error` sources
        // that from ambient state), which is only sound because the side-effect bit is consulted
        // solely for `AgentError::DeterministicTrap` and a host error is never one. Guest wasm traps
        // are classified on the invocation path without a `CallHandle`, so they never reach here.
        debug_assert!(
            err.root_cause().downcast_ref::<wasmtime::Trap>().is_none(),
            "CallHandle::trap must not wrap a deterministic wasm trap (root cause was a wasmtime::Trap)"
        );
        let context = self.trap_context();
        self.abandon_for_trap();
        mark_durable_call_trap_context(err, context)
    }

    /// Retry wrapper around [`InFunctionRetryController::try_trigger_retry`]. On the `Err` branch
    /// (a trap is being raised to trigger an oplog-level retry) it automatically
    /// [`abandon_for_trap`](Self::abandon_for_trap)s, so `?`-style call sites stay correct without
    /// hitting the `NotCancellable` unfinished-drop guard.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<()> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry(&mut retry_host, result, classify)
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    pub async fn try_trigger_retry_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_with_properties(&mut retry_host, result, classify, properties)
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    pub async fn try_trigger_retry_or_loop<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_or_loop(&mut retry_host, result, classify)
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    pub async fn try_trigger_retry_or_loop_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<InternalRetryResult> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_or_loop_with_properties(
                    &mut retry_host,
                    result,
                    classify,
                    properties,
                )
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    /// Accessor-path counterpart of [`Self::try_trigger_retry_with_properties`] for p3 host
    /// wrappers: classifies a host error *value* that is about to be recorded and returned to the
    /// guest and, when it is transient and the worker's retry policy allows another attempt,
    /// raises a retry trap (`Err`) instead — routing the failure through the worker-level retry
    /// machinery (worker goes to `Retrying`) rather than letting the guest observe it. On the
    /// `Err` branch the handle is [`trap`](Self::trap)ped (abandoned with the call's trap
    /// context), so `?`-style call sites stay correct; its host-call `Start` is left incomplete
    /// and the call re-executes on the retry's replay. `Ok(())` means the error is permanent or
    /// the retry budget is exhausted: the caller must persist the failed result and return it to
    /// the guest, exactly as before.
    ///
    /// Only meaningful on the live path (recorded failures replay deterministically to the guest
    /// without re-classification, mirroring the `&mut self` path).
    ///
    /// The accessor cannot hold the store across the async policy resolution, so the retry
    /// decision runs against a [`TaskRetryContext`] snapshot taken in one short store window: the
    /// same policy tiers as `PrivateDurableWorkerState::named_retry_policies` and the current
    /// retry state of this call's own retry point (its atomic region or enclosing durable scope
    /// `Start`, matching [`ScopedRetryHost::retry_point`]). `properties` are enriched with the
    /// worker-local context (`agent-type`, `is-idempotent`) before resolution.
    pub async fn try_trigger_retry_access<T, D, Ctx, Ok, Err>(
        &mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        Err: Display,
    {
        debug_assert!(
            self.is_live(),
            "try_trigger_retry_access is only valid on the live path"
        );
        let Err(err) = result else {
            return Ok(());
        };
        if classify(err) != HostFailureKind::Transient {
            return Ok(());
        }

        let message = err.to_string();
        let mut properties = properties;
        properties.set(
            "error-type",
            golem_common::model::PredicateValue::Text("transient".to_string()),
        );

        let retry_point = self.execution_scope.trap_retry_point();
        let (
            environment_state_service,
            environment_id,
            default_retry_policy,
            agent_config_retry_policies,
            runtime_retry_policy_mutations,
            worker,
        ) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.enrich_retry_properties(&mut properties);
            (
                ctx.state.environment_state_service.clone(),
                ctx.state.owned_agent_id.environment_id,
                golem_common::model::NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
                ctx.state.agent_config_retry_policies(),
                ctx.state.runtime_retry_policy_mutations.clone(),
                ctx.public_state.worker(),
            )
        });

        let current_retry_policy_state = worker
            .get_non_detached_last_known_status()
            .await
            .current_retry_state
            .get(&retry_point)
            .cloned();

        let mut retry_host = TaskRetryContext {
            retry_point,
            environment_state_service,
            environment_id,
            default_retry_policy,
            agent_config_retry_policies,
            runtime_retry_policy_mutations,
            max_in_function_retry_delay: self
                .retry
                .durable_execution_state()
                .max_in_function_retry_delay,
            current_retry_policy_state,
            retry_properties: properties.clone(),
            worker,
        };

        let failure = Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message,
        });
        try_trigger_host_trap_retry(&mut retry_host, failure, properties)
            .await
            .map_err(|err| self.trap(err))
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
        E: DurableCallTrapError,
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
    /// call for trap and propagates the error wrapped with this call's [`DurableCallTrapContext`].
    /// Shared by both [`Self::run`] paths.
    async fn run_live_action<Ctx, A, E>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        match live_action(ctx).await {
            Ok(response) => self
                .complete(ctx, response)
                .await
                .map_err(|err| E::from_durable_call_trap(err.into_marked_anyhow())),
            Err(err) => Err(E::from_durable_call_trap(self.trap(err))),
        }
    }

    /// Completes a live call: upload the response, append the matching host-call `End`, then close
    /// the durable scope / commit / checkpoint via `end_durable_function`.
    ///
    /// A terminal failure is returned as a [`TerminalCallError`] carrying this call's own
    /// [`DurableCallTrapContext`], so the post-trap retry grouping stays owned by the call rather
    /// than falling back to ambient worker state a sibling could have clobbered once durable calls
    /// overlap.
    pub async fn complete<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_impl<Ctx: WorkerCtx>(
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
            // Surface a deferred request-upload failure here, at the call site, before recording the
            // `End` that references the request. The leaf oplog's commit barrier is the backstop, but
            // awaiting here turns an upload failure into a graceful error instead of a commit-time
            // panic. A no-op when the request was inline or eagerly uploaded.
            let oplog = ctx.state.oplog.clone();
            if let Err(err) = self.request_upload.wait().await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize and store durable call request: {err}"
                ))
            }) {
                self.execution_scope.release_atomic_lease();
                return Err(err);
            }
            let host_response: HostResponse = response.clone().into();
            let response_payload = match oplog.upload_payload(&host_response).await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize and store durable call response: {err}"
                ))
            }) {
                Ok(payload) => payload,
                Err(err) => {
                    self.execution_scope.release_atomic_lease();
                    return Err(err);
                }
            };
            if let Some(begin_index) = self.execution_scope.atomic_region()
                && !ctx
                    .state
                    .mark_atomic_region_has_side_effects_for(begin_index)
            {
                self.execution_scope.release_atomic_lease();
                return Err(WorkerExecutorError::runtime(format!(
                    "durable call {} completed after its atomic region {begin_index} was closed",
                    self.start_idx
                )));
            }
            let end = OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: self.start_idx,
                response: Some(response_payload),
                forced_commit: false,
            };
            oplog.add(end).await;
            self.execution_scope.release_atomic_lease();
            // Close the durable scope (if one was opened), commit at the right boundary, and run the
            // mid-invocation checkpoint, all via `end_durable_function`.
            ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                .await?;
        }
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
            .complete_access_impl(store, get_ctx, response, None)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        // Non-deferred boundary: no fallible guest-facing work follows the host return, so hand
        // the token to Wasmtime's terminal observer synchronously (no tear window exists between
        // the impl returning and this call) — the observer settles it when the guest actually
        // consumes (or discards) the lowered result.
        delivery.deliver_at_accessor_terminal(store);
        Ok(response)
    }

    /// Like [`Self::complete_access`], but for call sites whose response crosses one more
    /// fallible/cancellable boundary after the durable terminal (a second-stage channel send, a
    /// span finish before the host method returns, a wire conversion). Returns the response
    /// together with an armed [`CompletionDelivery`] token; the caller must consume the token at
    /// the real guest-facing boundary (`delivered` / `suppress` / `discarded`), and a torn future
    /// in between records the `CompletionDiscarded` marker via the token's drop.
    ///
    /// `post_end_entry` (e.g. the call's durable `FinishSpan`) is appended by the same owned task
    /// as the terminal `End`, so it is recorded even when this future is torn mid-completion:
    /// replay can then rely on the entry unconditionally following the `End` — the recorded
    /// discard marker (a hint entry) always sorts after both. Callers passing it must *not*
    /// append the entry themselves and only perform its in-memory effect.
    pub async fn complete_access_deferred<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        post_end_entry: Option<OplogEntry>,
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
        self.complete_access_impl(store, get_ctx, response, post_end_entry)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_access_impl<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        post_end_entry: Option<OplogEntry>,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("complete_access")?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "complete_access called on a replay handle; use replay_access first",
            ));
        }
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        {
            let (oplog, replay_state) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (ctx.state.oplog.clone(), ctx.state.replay_state.clone())
            });
            let trap_context = self.trap_context();
            let mut guard = AccessTerminalGuard::<P>::new(
                DroppedCall {
                    start_idx: self.start_idx,
                    begin_index: self.begin_index,
                    function_type: self.retry.function_type().clone(),
                    request_upload: self.request_upload.clone(),
                    atomic_lease: self.execution_scope.atomic_lease.clone(),
                    trap_context,
                    live_call_permit: self.live_call_permit.clone(),
                },
                self.drop_sink.clone(),
                self.cleanup_sink.clone(),
            );
            self.finished = true;
            let persist_result: Result<(), WorkerExecutorError> = if self.persisted {
                Self::persist_access_terminal(
                    oplog,
                    replay_state,
                    &mut guard,
                    self.start_idx,
                    &response,
                    post_end_entry,
                )
                .await
            } else {
                Ok(())
            };

            // Read the current owner through the lease *after* the terminal is persisted, so a
            // region close that transferred this call mid-flight marks side effects against the
            // region that now owns it.
            let atomic_region = self.execution_scope.atomic_region();
            let finish_result = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                let mut result = Ok(());
                if let Some(begin_index) = atomic_region
                    && persist_result.is_ok()
                    && !ctx.state.mark_atomic_region_has_side_effects_for(begin_index)
                {
                        result = Err(WorkerExecutorError::runtime(format!(
                            "durable call {} completed after its atomic region {begin_index} was closed",
                            self.start_idx
                        )));
                }
                result
            });
            guard.release_atomic_lease();

            if let Err(err) = persist_result {
                guard.disarm();
                return Err(err);
            }
            // From here on the `End` is persisted, but these error returns are *observed* by the
            // caller (the worker traps): the completion was not silently discarded by the guest,
            // so the marker must not be recorded — while the terminal join / permit release must
            // still reach the drain through the guard's drop event.
            if let Err(err) = finish_result {
                guard.suppress_discard_marker();
                return Err(err);
            }
            if let Err(err) =
                end_durable_function_access(store, get_ctx, function_type, begin_index, false).await
            {
                guard.suppress_discard_marker();
                return Err(err);
            }
            Ok((response, guard.take_completion_delivery()))
        }
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
    async fn persist_access_terminal(
        oplog: Arc<dyn Oplog>,
        replay_state: ReplayState,
        guard: &mut AccessTerminalGuard<P>,
        start_idx: OplogIndex,
        response: &Pair::Resp,
        post_end_entry: Option<OplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        guard
            .call()
            .expect("terminal guard is armed")
            .wait_request_upload()
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize and store durable call request: {err}"
                ))
            })?;
        let host_response: HostResponse = response.clone().into();
        let response_payload = oplog.upload_payload(&host_response).await.map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "failed to serialize and store durable call response: {err}"
            ))
        })?;
        let end = OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: start_idx,
            response: Some(response_payload),
            forced_commit: false,
        };
        let terminal_oplog = oplog.clone();
        let terminal = tokio::spawn(async move {
            terminal_oplog.add(end).await;
            // A deferred-delivery call's mandatory post-`End` entry (e.g. its durable
            // `FinishSpan`) is appended by the same owned task: it is recorded even when the
            // completing future is torn right after the `End`, so replay can rely on it
            // unconditionally following the `End` (any discard marker chains after this task).
            if let Some(entry) = post_end_entry {
                terminal_oplog.add(entry).await;
            }
            Ok(())
        });
        // Arm the discard marker together with the owned `End` append: from this point a torn
        // completion future means the guest discarded a persisted successful completion (a tear
        // *during* `wait_terminal` still counts — the owned task appends the `End` regardless).
        guard.cleanup_after_terminal(
            terminal,
            Some(DiscardMarker {
                start_idx,
                oplog,
                replay_state,
            }),
        );
        guard.wait_terminal().await
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
                let oplog = ctx.state.oplog.clone();
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                self.finished = true;
                if let Some(payload) = partial {
                    let oplog = ctx.state.oplog.clone();
                    let host_response = oplog.download_payload(payload).await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Cancelled partial payload cannot be downloaded: {err}"
                        ))
                    })?;
                    let response: Pair::Resp = host_response.try_into().map_err(|err| {
                        WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err)
                    })?;
                    ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                        .await?;
                    Ok(CallReplayOutcome::Replayed(response))
                } else {
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled { partial: Some(..) }",
                        format!("Cancelled without partial at {cancelled_idx}"),
                    ))
                }
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                ..
            }) => {
                // Discarded completions are recorded only on the accessor completion path, and
                // calls replay through the same path they recorded on — a marker resolving on
                // this non-accessor path means the oplog does not match this code path.
                self.finished = true;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End delivered to the guest",
                    format!(
                        "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a non-accessor durable call"
                    ),
                ))
            }
            ResolutionOutcome::Incomplete => {
                if !Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS {
                    // Debug sessions must never re-execute side effects; refuse instead of
                    // switching to live repair.
                    self.finished = true;
                    Err(WorkerExecutorError::invalid_request(format!(
                        "the replay target lies inside an in-flight durable call (Start at {start_idx} has no End/Cancelled before the replay target); live re-execution of incomplete durable calls is disabled in debug sessions"
                    )))
                } else if self.retry.can_reexecute_on_incomplete_replay() {
                    // Switch the handle to live completion of the existing, committed `Start`: the
                    // caller re-runs the side effect and `complete`s, appending the missing `End`.
                    // A failure during re-execution stays grouped at this call's own retry point via
                    // the call-owned `execution_scope` (and the semantic-trap error marker).
                    self.is_live = true;
                    self.persisted = true;
                    self.live_call_permit =
                        Some(LiveCallPermit::new(ctx.state.live_host_call_counter()));
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

    /// Accessor-window replay for non-scope-opening p3 durable calls.
    pub async fn replay_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        let (replay_state, oplog) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access() called on a live handle");
        let start_idx = self.start_idx;
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                self.finished = true;
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                if let Some(payload) = partial {
                    self.finished = true;
                    let host_response = oplog.download_payload(payload).await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Cancelled partial payload cannot be downloaded: {err}"
                        ))
                    })?;
                    let response: Pair::Resp = host_response.try_into().map_err(|err| {
                        WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err)
                    })?;
                    end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                        .await?;
                    Ok(CallReplayOutcome::Replayed(response))
                } else {
                    // `Cancelled` with no partial result: in the recorded run this call never
                    // returned a value to the guest — its future was dropped mid-flight (e.g. the
                    // loser of a guest `race`/`select!`). Mirror that exactly: never complete, so
                    // the deterministic guest drops this future at the same point it did live.
                    // Resolving to an error here instead would race the guest's own drop — the
                    // resolver delivers the recorded terminal as soon as the cursor crosses it,
                    // which may be before the winning branch has been polled and had a chance to
                    // drop the loser.
                    //
                    // Durable cleanup (closing the durable scope for scope-opening calls) is
                    // deferred to this handle's `Drop`, which enqueues the idempotent
                    // `CloseDurableScope` event. Awaiting `end_durable_function_access` *here*,
                    // before the park, would open a cancellation window: for scope-opening calls
                    // it takes the scope's replay handle before its first await, so the guest
                    // dropping this future mid-close would strand the open scope (with `finished`
                    // already set, `Drop` would not clean it up either).
                    tracing::debug!(
                        "durable call cancelled without partial at {cancelled_idx} during replay; \
                         parking until the guest drops it"
                    );
                    self.parked_undelivered_replay = true;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                ..
            }) => {
                // The recorded run persisted a successful `End`, but the guest dropped the
                // completion future before the response was delivered (the `CompletionDiscarded`
                // marker at `marker_idx` records this). Mirror live exactly, like the
                // cancelled-without-partial park above: never complete, so the deterministic
                // guest drops this future at the same point it did live; durable cleanup is
                // deferred to this handle's `Drop`.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); parking until the guest drops it"
                );
                self.parked_undelivered_replay = true;
                std::future::pending::<()>().await;
                unreachable!("std::future::pending never completes")
            }
            ResolutionOutcome::Incomplete => {
                if !Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS {
                    // Debug sessions must never re-execute side effects; refuse instead of
                    // switching to live repair.
                    self.finished = true;
                    Err(WorkerExecutorError::invalid_request(format!(
                        "the replay target lies inside an in-flight durable call (Start at {start_idx} has no End/Cancelled before the replay target); live re-execution of incomplete durable calls is disabled in debug sessions"
                    )))
                } else if self.retry.can_reexecute_on_incomplete_replay() {
                    self.is_live = true;
                    self.persisted = true;
                    self.live_call_permit = Some(LiveCallPermit::new(store.with(|mut access| {
                        get_ctx(access.data_mut()).state.live_host_call_counter()
                    })));
                    Ok(CallReplayOutcome::Incomplete(self))
                } else {
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

    /// Like [`Self::replay_access`], but for deferred-delivery call sites (see
    /// [`Self::complete_access_deferred`]): a replayed terminal is returned together with a
    /// [`CompletionDelivery`] token instead of parking here when the recorded run discarded the
    /// completion. For a recorded `CompletionDiscarded` marker this decodes the persisted
    /// response and closes the durable scope — mirroring exactly what live did before its token
    /// was armed — and the *caller* parks at its own delivery boundary after performing its
    /// deterministic post-`End` continuation (e.g. consuming a positional `FinishSpan`).
    pub async fn replay_access_deferred<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access_deferred")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        let (replay_state, oplog) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access_deferred() called on a live handle");
        let start_idx = self.start_idx;
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                self.finished = true;
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_delivered(),
                ))
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                response,
            }) => {
                // The recorded run persisted a successful `End` but the guest discarded the
                // completion before final delivery (marker at `marker_idx`). Decode the persisted
                // response and close the durable scope — the exact state live was in when its
                // delivery token was armed — and let the caller run its deterministic post-`End`
                // continuation before parking at the delivery boundary.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); replaying its post-End continuation before \
                     parking at the delivery boundary"
                );
                self.finished = true;
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_discarded(),
                ))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                if let Some(payload) = partial {
                    self.finished = true;
                    let host_response = oplog.download_payload(payload).await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Cancelled partial payload cannot be downloaded: {err}"
                        ))
                    })?;
                    let response: Pair::Resp = host_response.try_into().map_err(|err| {
                        WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err)
                    })?;
                    end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                        .await?;
                    Ok(DeferredCallReplayOutcome::Replayed(
                        response,
                        CompletionDelivery::replay_delivered(),
                    ))
                } else {
                    // Cancelled with no partial: in the recorded run this call never returned a
                    // value to the guest. Mirror exactly — park here so the deterministic guest
                    // drops this future at the same point it did live; durable cleanup is
                    // deferred to this handle's `Drop`. See [`Self::replay_access`] for why the
                    // scope close must not happen before the park.
                    tracing::debug!(
                        "durable call cancelled without partial at {cancelled_idx} during replay; \
                         parking until the guest drops it"
                    );
                    self.parked_undelivered_replay = true;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }
            }
            ResolutionOutcome::Incomplete => {
                if !Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS {
                    // Debug sessions must never re-execute side effects; refuse instead of
                    // switching to live repair.
                    self.finished = true;
                    Err(WorkerExecutorError::invalid_request(format!(
                        "the replay target lies inside an in-flight durable call (Start at {start_idx} has no End/Cancelled before the replay target); live re-execution of incomplete durable calls is disabled in debug sessions"
                    )))
                } else if self.retry.can_reexecute_on_incomplete_replay() {
                    self.is_live = true;
                    self.persisted = true;
                    self.live_call_permit = Some(LiveCallPermit::new(store.with(|mut access| {
                        get_ctx(access.data_mut()).state.live_host_call_counter()
                    })));
                    Ok(DeferredCallReplayOutcome::Incomplete(self))
                } else {
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
                let trap_context = self.trap_context();
                let dropped_call = DroppedCall {
                    start_idx: self.start_idx,
                    begin_index: self.begin_index,
                    function_type: self.retry.function_type().clone(),
                    request_upload: self.request_upload.clone(),
                    atomic_lease: self.execution_scope.atomic_lease.clone(),
                    trap_context,
                    live_call_permit: self.live_call_permit.clone(),
                };
                // As in `complete`: surface a deferred request-upload failure at the call site before
                // recording the `Cancelled` that references the request. A no-op when the request was
                // inline or eagerly uploaded.
                if let Err(err) = dropped_call.wait_request_upload().await {
                    dropped_call.release_atomic_lease();
                    return Err(err);
                }
                let partial_payload = match partial {
                    Some(partial) => {
                        let host_response: HostResponse = partial.into();
                        match oplog.upload_payload(&host_response).await.map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store partial durable call response: {err}"
                            ))
                        }) {
                            Ok(payload) => Some(payload),
                            Err(err) => {
                                dropped_call.release_atomic_lease();
                                return Err(err);
                            }
                        }
                    }
                    None => None,
                };
                dropped_call
                    .append_cancelled_with_oplog(oplog, partial_payload)
                    .await?;
                dropped_call.release_atomic_lease();
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            match resolution {
                Resolution::Completed { end_idx, .. }
                | Resolution::CompletedButDiscarded { end_idx, .. } => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "Cancelled",
                        format!("End at {end_idx}"),
                    ));
                }
                Resolution::Cancelled { .. } => {}
            }
            ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
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
        let begin_index = self.begin_index;
        if self.is_live {
            if self.persisted {
                let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
                let trap_context = self.trap_context();
                let mut guard = AccessTerminalGuard::<P>::new(
                    DroppedCall {
                        start_idx: self.start_idx,
                        begin_index: self.begin_index,
                        function_type: self.retry.function_type().clone(),
                        request_upload: self.request_upload.clone(),
                        atomic_lease: self.execution_scope.atomic_lease.clone(),
                        trap_context,
                        live_call_permit: self.live_call_permit.clone(),
                    },
                    self.drop_sink.clone(),
                    self.cleanup_sink.clone(),
                );
                let result = async {
                    guard.call().expect("terminal guard is armed").wait_request_upload().await?;
                    let partial_payload = match partial {
                        Some(partial) => {
                            let host_response: HostResponse = partial.into();
                            Some(oplog.upload_payload(&host_response).await.map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to serialize and store partial durable call response: {err}"
                                ))
                            })?)
                        }
                        None => None,
                    };
                    let call = guard.call().expect("terminal guard is armed").clone();
                    let terminal = tokio::spawn(async move {
                        call.append_cancelled_with_oplog(oplog, partial_payload)
                            .await
                    });
                    // Cancellation terminal: never a discarded completion, no marker.
                    guard.cleanup_after_terminal(terminal, None);
                    guard.wait_terminal().await
                }
                .await;
                guard.release_atomic_lease();
                if let Err(err) = result {
                    guard.disarm();
                    return Err(err);
                }
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
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
            match resolution {
                Resolution::Completed { end_idx, .. }
                | Resolution::CompletedButDiscarded { end_idx, .. } => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "Cancelled",
                        format!("End at {end_idx}"),
                    ));
                }
                Resolution::Cancelled { .. } => {}
            }
            end_durable_function_access(store, get_ctx, function_type, begin_index, false).await?;
        }
        Ok(())
    }

    fn ensure_accessor_terminal_supported(
        &self,
        operation: &str,
    ) -> Result<(), WorkerExecutorError> {
        if is_accessor_terminal_supported_function_type(self.retry.function_type()) {
            Ok(())
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "{operation} currently supports only ReadLocal/ReadRemote/WriteRemoteBatched durable calls, got {:?}",
                self.retry.function_type()
            )))
        }
    }
}

pub(crate) async fn end_durable_function_access<T, D, Ctx>(
    store: &Accessor<T, D>,
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
    let (opens_scope, is_live, replay_handle, replay_state, oplog, public_state) =
        store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            let opens_scope = ctx.state.opens_durable_scope(&function_type);
            let is_live = ctx.state.is_live();
            let replay_handle = if opens_scope && !is_live {
                ctx.state.take_durable_scope_replay_handle(begin_index)
            } else {
                None
            };
            (
                opens_scope,
                is_live,
                replay_handle,
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.public_state.clone(),
            )
        });

    if opens_scope {
        if is_live {
            oplog
                .add(OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index: begin_index,
                    response: None,
                    forced_commit: true,
                })
                .await;
        } else if let Some(handle) = replay_handle {
            match replay_state.await_resolution_outcome(handle).await? {
                ResolutionOutcome::Resolved(Resolution::Completed { .. }) => {}
                ResolutionOutcome::Resolved(Resolution::Cancelled { .. }) => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("End {{ start_index: {begin_index} }}"),
                        format!("Cancelled {{ start_index: {begin_index} }}"),
                    ));
                }
                ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                    end_idx,
                    marker_idx,
                    ..
                }) => {
                    // Discarded completions are recorded only for accessor completion futures; a
                    // marker referencing a durable scope `Start` means the oplog does not match
                    // this code path.
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("End {{ start_index: {begin_index} }}"),
                        format!(
                            "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a durable scope"
                        ),
                    ));
                }
                ResolutionOutcome::Incomplete => {
                    oplog
                        .add(OplogEntry::End {
                            timestamp: Timestamp::now_utc(),
                            start_index: begin_index,
                            response: None,
                            forced_commit: true,
                        })
                        .await;
                }
            }
        }

        store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.remove_durable_scope(begin_index)
        })?;
    }

    if function_type == DurableFunctionType::WriteRemote
        || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
        || matches!(
            function_type,
            DurableFunctionType::WriteRemoteTransaction(_)
        )
        || forced_commit
    {
        public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        if let Some(min_exposed_marker) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            if ctx.state.at_clean_checkpoint_boundary() {
                Some(ctx.state.min_exposed_marker)
            } else {
                None
            }
        }) {
            public_state
                .worker()
                .checkpoint_status_mid_invocation(min_exposed_marker)
                .await;
        }
    }

    Ok(())
}

pub(crate) async fn end_durable_function_access_if_open<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    forced_commit: bool,
) -> Result<bool, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let is_open = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        ctx.state.is_durable_scope_open(begin_index)
    });
    if is_open {
        end_durable_function_access(store, get_ctx, function_type, begin_index, forced_commit)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Legacy-oplog fallback for the p3 HTTP send span: consumes a positional `StartSpan` entry at
/// the replay cursor head, if one is there, and reconstructs the recorded span (same span id and
/// start timestamp, recorded attributes) in the in-memory invocation context with the current
/// span as parent. Returns `None` — consuming nothing — when the head is any other entry.
///
/// Oplogs written since the send span became *derived* (computed from the send's own host-call
/// `Start` index, with no separate span entries) never contain a `StartSpan` at this position, so
/// this only fires for oplogs recorded by older executors. For those, positional consumption is
/// exactly how the entry was consumed before — best-effort for old *concurrent* recordings, which
/// never carried owner identity in the first place.
pub(crate) async fn try_replay_recorded_start_span_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<SpanId>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let replay_state = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        ctx.state.replay_state.clone()
    });

    let consumed = replay_state
        .try_get_oplog_entry_owned(|entry| matches!(entry, OplogEntry::StartSpan { .. }))
        .await?;
    let Some((
        _,
        OplogEntry::StartSpan {
            timestamp,
            span_id,
            attributes,
            ..
        },
    )) = consumed
    else {
        return Ok(None);
    };

    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let parent = ctx.state.current_span_id.clone();
        let parent_span = ctx.state.invocation_context.get(&parent).map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "parent span {parent} missing during StartSpan replay: {err}"
            ))
        })?;
        let span = InvocationContextSpan::local()
            .with_span_id(span_id.clone())
            .with_start(timestamp)
            .with_parent(parent_span)
            .with_attributes(attributes.0.clone())
            .build();
        ctx.state.invocation_context.add_span(span);
        Ok::<_, WorkerExecutorError>(())
    })?;
    Ok(Some(span_id))
}

/// Finishes a span in the in-memory invocation context only, without writing or consuming any
/// oplog entry: pops the current-span pointer if it points at this span, then marks the span
/// finished. This is the non-durable half of [`finish_span_access`], used directly for spans
/// whose identity is derived from durable records (no `StartSpan`/`FinishSpan` entries exist).
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

pub(crate) async fn finish_span_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (is_live, worker, replay_state) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.is_live(),
            ctx.public_state.worker(),
            ctx.state.replay_state.clone(),
        )
    });

    if is_live {
        worker
            .add_to_oplog(OplogEntry::finish_span(span_id.clone()))
            .await;
    } else {
        crate::get_oplog_entry_owned!(replay_state, OplogEntry::FinishSpan)?;
    }

    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        finish_span_in_memory(ctx, span_id)
    })
}

fn is_accessor_supported_function_type(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::ReadLocal
            | DurableFunctionType::ReadRemote
            | DurableFunctionType::WriteRemote
            | DurableFunctionType::WriteRemoteBatched(_)
    )
}

fn opens_accessor_scope(
    function_type: &DurableFunctionType,
    assume_idempotence: bool,
    snapshotting: bool,
) -> bool {
    !snapshotting
        && ((*function_type == DurableFunctionType::WriteRemote && !assume_idempotence)
            || matches!(function_type, DurableFunctionType::WriteRemoteBatched(None)))
}

fn opens_replay_durable_scope(
    function_type: &DurableFunctionType,
    assume_idempotence: bool,
) -> bool {
    (*function_type == DurableFunctionType::WriteRemote && !assume_idempotence)
        || matches!(
            *function_type,
            DurableFunctionType::WriteRemoteBatched(None)
        )
}

fn is_accessor_terminal_supported_function_type(function_type: &DurableFunctionType) -> bool {
    is_accessor_supported_function_type(function_type)
        || matches!(function_type, DurableFunctionType::WriteRemote)
}

async fn process_pending_replay_events_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let replay_state =
        store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
    let replay_events = replay_state.take_new_replay_events();
    for event in replay_events {
        match event {
            crate::durable_host::replay_state::ReplayEvent::ForkReplayed { new_phantom_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state.current_phantom_id = Some(new_phantom_id);
                });
            }
            crate::durable_host::replay_state::ReplayEvent::UpdateReplayed { new_revision } => {
                tracing::debug!(
                    "Updating worker state to component metadata revision {new_revision}"
                );
                update_state_to_new_component_revision_access(store, get_ctx, new_revision).await?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardInstalled { card } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    let card_id = card.card_id();
                    tracing::debug!(card_id = %card_id, "Applying replayed card installation");
                    ctx.state.agent_wallet_cards.insert(card_id, card);
                    ctx.rederive_agent_effective_surface_from_wallet();
                });
            }
            crate::durable_host::replay_state::ReplayEvent::CardRevoked { card_id }
            | crate::durable_host::replay_state::ReplayEvent::CardExpired { card_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    tracing::debug!(card_id = %card_id, "Applying replayed card removal");
                    if ctx.state.agent_wallet_cards.remove(&card_id).is_some() {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                });
            }
            crate::durable_host::replay_state::ReplayEvent::ReplayFinished => {
                tracing::debug!("Replaying oplog finished");
                finalize_pending_automatic_update_access(store, get_ctx).await?;
            }
        }
    }
    Ok(())
}

struct AccessRevisionUpdateInputs {
    component_service: Arc<dyn ComponentService>,
    file_loader: Arc<FileLoader>,
    owned_agent_id: golem_common::model::OwnedAgentId,
    agent_id: Option<ParsedAgentId>,
    initial_agent_config: Vec<golem_common::model::worker::TypedAgentConfigEntry>,
    worker_dir: PathBuf,
    current_revision: ComponentRevision,
}

type AccessRevisionUpdateAgentState = (
    HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
    golem_common::model::card::EffectiveSurface,
    BTreeMap<golem_common::model::card::CardId, golem_common::model::card::StoredCard>,
);

struct AccessRevisionUpdate {
    metadata: Component,
    agent_state: Option<AccessRevisionUpdateAgentState>,
    files: HashMap<PathBuf, IFSWorkerFile>,
}

async fn finalize_pending_automatic_update_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let pending_update = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let pending_update = ctx
            .state
            .pending_update
            .try_lock()
            .map_err(|_| {
                WorkerExecutorError::runtime(
                    "p3 accessor durable call path cannot inspect pending component update state",
                )
            })?
            .take();
        Ok::<_, WorkerExecutorError>(pending_update)
    });

    let pending_update = if let Some(pending_update) = pending_update? {
        pending_update
    } else {
        return Ok(());
    };

    match pending_update.description {
        UpdateDescription::Automatic { target_revision } => {
            tracing::debug!("Finalizing pending automatic update");
            if let Err(error) =
                update_state_to_new_component_revision_access(store, get_ctx, target_revision).await
            {
                let stringified_error = format!("Applying worker update failed: {error}");
                record_worker_update_failed_access(
                    store,
                    get_ctx,
                    target_revision,
                    stringified_error,
                )
                .await?;
                return Err(error);
            }

            let (component_size, active_plugins) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (
                    ctx.state.component_metadata.component_size,
                    HashSet::from_iter({
                        ctx.agent_type_provision_config()
                            .map(|c| c.plugins.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .map(|installation| installation.environment_plugin_grant_id)
                    }),
                )
            });
            record_worker_update_succeeded_access(
                store,
                get_ctx,
                target_revision,
                component_size,
                active_plugins,
            )
            .await?;
            tracing::debug!("Finalizing automatic update to revision {target_revision}");
            Ok(())
        }
        _ => Err(WorkerExecutorError::runtime(
            "pending replay event finalization expected an automatic update description",
        )),
    }
}

async fn update_state_to_new_component_revision_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    new_revision: ComponentRevision,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let inputs = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        AccessRevisionUpdateInputs {
            component_service: ctx.state.component_service.clone(),
            file_loader: ctx.state.file_loader.clone(),
            owned_agent_id: ctx.owned_agent_id.clone(),
            agent_id: ctx.state.agent_id.clone(),
            initial_agent_config: ctx.state.initial_agent_config.clone(),
            worker_dir: ctx.worker_dir.path().to_path_buf(),
            current_revision: ctx.state.component_metadata.revision,
        }
    });

    if new_revision <= inputs.current_revision {
        tracing::debug!("Update {new_revision} was already applied, skipping");
        return Ok(());
    }

    let update = prepare_revision_update_access(store, get_ctx, &inputs, new_revision).await?;
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        apply_revision_update_access(ctx, update);
    });
    Ok(())
}

async fn prepare_revision_update_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    inputs: &AccessRevisionUpdateInputs,
    new_revision: ComponentRevision,
) -> Result<AccessRevisionUpdate, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let metadata = inputs
        .component_service
        .get_metadata(inputs.owned_agent_id.component_id(), Some(new_revision))
        .await?;

    let provision_config = inputs.agent_id.as_ref().and_then(|agent_id| {
        metadata
            .metadata
            .agent_type_provision_configs()
            .get(&agent_id.agent_type)
            .cloned()
    });

    let agent_state = if let Some(agent_id) = &inputs.agent_id {
        let agent_type = metadata
            .metadata
            .find_agent_type_by_name_ref(&agent_id.agent_type)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type {} not found in updated agent metadata",
                    agent_id.agent_type
                ))
            })?;

        let updated_agent_config = effective_agent_config(
            inputs.initial_agent_config.clone(),
            provision_config
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or_default(),
        )?;
        validate_agent_config(&updated_agent_config, agent_type)?;

        let initial_card = super::agent_initial_card_from_component_metadata(&metadata, agent_id)?;
        let initial_wallet_cards = BTreeMap::from([(initial_card.card_id(), initial_card)]);
        let context =
            super::agent_monomorphization_context(&metadata, &inputs.owned_agent_id, agent_id);
        let effective_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            initial_wallet_cards.values(),
        );

        Some((
            updated_agent_config,
            effective_surface,
            initial_wallet_cards,
        ))
    } else {
        None
    };

    let mut files = take_initial_files_access(store, get_ctx)?;
    let update_result = super::update_filesystem(
        &mut files,
        &inputs.file_loader,
        inputs.owned_agent_id.environment_id,
        &inputs.worker_dir,
        provision_config
            .as_ref()
            .map(|c| c.files.as_slice())
            .unwrap_or_default(),
    )
    .await;

    if let Err(error) = update_result {
        restore_initial_files_access(store, get_ctx, files)?;
        return Err(error);
    }

    Ok(AccessRevisionUpdate {
        metadata,
        agent_state,
        files,
    })
}

fn take_initial_files_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<HashMap<PathBuf, IFSWorkerFile>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let mut files = ctx.state.files.try_write().map_err(|_| {
            WorkerExecutorError::runtime(
                "p3 accessor durable call path cannot acquire initial-files lock",
            )
        })?;
        Ok(std::mem::take(&mut *files))
    })
}

fn restore_initial_files_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    restored: HashMap<PathBuf, IFSWorkerFile>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let mut files = ctx.state.files.try_write().map_err(|_| {
            WorkerExecutorError::runtime(
                "p3 accessor durable call path cannot restore initial-files state",
            )
        })?;
        *files = restored;
        Ok(())
    })
}

fn apply_revision_update_access<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    update: AccessRevisionUpdate,
) {
    let read_only_paths = super::compute_read_only_paths(&update.files);
    {
        let mut files = ctx
            .state
            .files
            .try_write()
            .expect("initial-files state was taken by this update path");
        *files = update.files;
    }
    {
        let mut read_only = ctx.state.read_only_paths.write().unwrap();
        *read_only = read_only_paths;
    }

    if let Some((agent_config, effective_surface, initial_wallet_cards)) = update.agent_state {
        ctx.state.agent_config = agent_config;
        ctx.state.cached_agent_config_retry_policies = None;
        ctx.state.agent_effective_surface = effective_surface;
        ctx.state.agent_wallet_cards = initial_wallet_cards;
    }
    ctx.state.component_metadata = update.metadata;
}

async fn record_worker_update_failed_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    details: String,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let public_state = store.with(|mut access| get_ctx(access.data_mut()).public_state.clone());
    public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::failed_update(
            target_revision,
            Some(details.clone()),
        ))
        .await;
    tracing::warn!(
        "Worker failed to update to {}: {}, update attempt aborted",
        target_revision,
        details
    );
    Ok(())
}

async fn record_worker_update_succeeded_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    component_size: u64,
    active_plugins: HashSet<
        golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
    >,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    tracing::info!("Worker update to {} finished successfully", target_revision);
    let public_state = store.with(|mut access| get_ctx(access.data_mut()).public_state.clone());
    public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::successful_update(
            target_revision,
            component_size,
            active_plugins,
        ))
        .await;
    Ok(())
}

/// The first phase of a two-phase durable call, produced by [`CallHandle::begin`]. The durable
/// scope is already open and the begin index is known; the host-call `Start` has not yet been
/// written (live) nor claimed (replay). Finalised into a [`CallHandle`] with [`Self::start_live`]
/// (after the request has been built) or [`Self::start_replay`].
pub struct BegunCall<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
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
        // The host-call `Start` nests inside the enclosing durable scope captured at initiation
        // (its own opened scope, or the scope encoded in the function type), derived explicitly —
        // never from the set of temporally-open sibling scopes. `None` for a top-level unscoped call.
        let parent_start_index = self.execution_scope.parent_start_index;
        let (start_idx, persisted, request_upload) = if snapshotting {
            // Snapshotting mode persists nothing.
            let oplog = ctx.state.oplog.clone();
            (
                oplog.current_oplog_index().await,
                false,
                PendingUpload::already_durable(),
            )
        } else {
            let request: HostRequest = request.into();
            let function_type = self.retry.function_type().clone();
            let oplog = ctx.state.oplog.clone();
            // Reserve the request payload and append the `Start` in one guarded step: a big request
            // blob's upload is *begun* but not awaited, so the `Start` is appended in initiation
            // order before the (potentially slow) upload finishes. `add_start_with_reserved_payload`
            // forbids — at compile time — any `.await` between reserving the payload and appending
            // the `Start`, which is what keeps concurrent calls' `Start` entries in initiation
            // order. The returned upload is awaited before this call's `End` / `Cancelled`.
            let (idx, request_upload) = oplog
                .add_start_with_reserved_payload(request, move |request_payload| {
                    OplogEntry::Start {
                        timestamp: Timestamp::now_utc(),
                        parent_start_index,
                        function_name: Pair::HOST_FUNCTION_NAME,
                        request: Some(request_payload),
                        durable_function_type: function_type,
                    }
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize and store durable call request: {err}"
                    ))
                })?;
            (idx, true, request_upload)
        };
        let atomic_lease = if persisted {
            if let Some(begin_index) = self.execution_scope.atomic_region {
                let lease = ctx.state.register_atomic_region_call(
                    begin_index,
                    self.retry.can_reexecute_on_incomplete_replay(),
                );
                if lease.is_none() {
                    return Err(WorkerExecutorError::runtime(format!(
                        "durable call {start_idx} started in atomic region {begin_index}, but the region is not open"
                    )));
                }
                lease
            } else {
                None
            }
        } else {
            // Snapshotting persists nothing; keep the initiation-time region for trap/retry
            // classification without joining the live in-flight member guard.
            unregistered_atomic_lease(
                self.execution_scope.atomic_region,
                self.retry.can_reexecute_on_incomplete_replay(),
            )
        };
        let execution_scope = self.execution_scope.finish(start_idx, atomic_lease);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: true,
            persisted,
            request_upload,
            replay: None,
            finished: false,
            parked_undelivered_replay: false,
            execution_scope,
            retry: self.retry,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: Some(LiveCallPermit::new(ctx.state.live_host_call_counter())),
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
        // Replay handles never participate in the live in-flight member guard, but keep their
        // initiation-time region for trap/retry classification.
        let atomic_lease = unregistered_atomic_lease(
            self.execution_scope.atomic_region,
            self.retry.can_reexecute_on_incomplete_replay(),
        );
        let execution_scope = self.execution_scope.finish(start_idx, atomic_lease);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: false,
            persisted: false,
            request_upload: PendingUpload::already_durable(),
            replay: Some(replay),
            finished: false,
            parked_undelivered_replay: false,
            execution_scope,
            retry: self.retry,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: None,
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
                let trap_context = self.trap_context();
                P::unfinished_drop(
                    DroppedCall {
                        start_idx: self.start_idx,
                        begin_index: self.begin_index,
                        function_type: self.retry.function_type().clone(),
                        request_upload: self.request_upload.clone(),
                        atomic_lease: self.execution_scope.atomic_lease.clone(),
                        trap_context,
                        live_call_permit: self.live_call_permit.take(),
                    },
                    self.drop_sink.as_ref(),
                );
            }
            // Not persisted (snapshotting): there is nothing on disk to reconcile.
        } else {
            if opens_replay_durable_scope(
                self.retry.function_type(),
                self.retry.durable_execution_state().assume_idempotence,
            ) && let Some(sink) = &self.drop_sink
            {
                let _ = sink.send(DropEvent::CloseDurableScope {
                    function_type: self.retry.function_type().clone(),
                    begin_index: self.begin_index,
                    span_id: None,
                });
            }
            if self.parked_undelivered_replay {
                // Expected: the recorded terminal was never delivered to the guest live
                // (`Cancelled { partial: None }` or an `End` marked `CompletionDiscarded`) and
                // the deterministic guest dropped this future at the same point it did live. Any
                // scope close was enqueued above.
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::host_functions;
    use golem_common::model::{NamedRetryPolicy, Predicate, RetryPolicy, RetryPolicyState};
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::mpsc;

    fn idx(n: u64) -> OplogIndex {
        OplogIndex::from_u64(n)
    }

    fn durable_execution_state() -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: true,
            max_in_function_retry_delay: Duration::from_secs(20),
        }
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
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(3))
            }
            other => panic!("unexpected resolution for h2: {other:?}"),
        }
        assert!(rx1.try_recv().is_err());

        assert!(resolver.resolve_if_pending(idx(1), completed(4)));
        match rx1.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(4))
            }
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
            Ok(ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. })) => {
                assert_eq!(cancelled_idx, idx(2))
            }
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
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(2))
            }
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
    fn resolver_fail_all_pending_incomplete_wakes_everyone() {
        // End-of-replay must wake every still-suspended awaiter as Incomplete, not leave them
        // hanging. A resolved awaiter is already gone and is unaffected.
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx1 = resolver.register(idx(1));
        let mut rx2 = resolver.register(idx(2));
        assert!(resolver.resolve_if_pending(idx(1), completed(3)));

        resolver.fail_all_pending_incomplete();

        match rx1.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(3))
            }
            other => panic!("rx1 should already be resolved: {other:?}"),
        }
        match rx2.try_recv() {
            Ok(ResolutionOutcome::Incomplete) => {}
            other => panic!("rx2 should be Incomplete: {other:?}"),
        }
        assert!(!resolver.is_pending(idx(2)));
    }

    #[test]
    fn resolver_duplicate_resolution_is_ignored() {
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx = resolver.register(idx(1));
        assert!(resolver.resolve_if_pending(idx(1), completed(2)));
        // Second resolution: no longer pending.
        assert!(!resolver.resolve_if_pending(idx(1), completed(3)));
        match rx.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(2))
            }
            other => panic!("unexpected resolution: {other:?}"),
        }
    }

    // ---- CallHandle drop policy ----

    fn live_unfinished_handle<P: DropPolicy>(
        start_idx: OplogIndex,
        sink: mpsc::UnboundedSender<DropEvent>,
    ) -> CallHandle<host_functions::MonotonicClockNow, P> {
        live_unfinished_handle_with_atomic_region(start_idx, None, sink)
    }

    fn live_unfinished_handle_with_atomic_region<P: DropPolicy>(
        start_idx: OplogIndex,
        atomic_region: Option<OplogIndex>,
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
            request_upload: PendingUpload::already_durable(),
            replay: None,
            finished: false,
            parked_undelivered_replay: false,
            execution_scope: CallExecutionScope {
                retry_from: start_idx,
                durable_scope: None,
                atomic_lease: unregistered_atomic_lease(atomic_region, true),
                persistence_level: PersistenceLevel::Smart,
            },
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                durable_execution_state,
                "test:monotonic_clock::now",
            ),
            drop_sink: Some(sink),
            cleanup_sink: None,
            live_call_permit: None,
            _phantom: PhantomData,
        }
    }

    /// A synthetic, already-finished handle carrying an arbitrary [`CallExecutionScope`]. Used by the
    /// Seam-2 terminal-failure tests to drive `trap_context()` (the call-owned classification a
    /// terminal-step failure attaches) for a call initiated in a specific scope, without standing up
    /// a full `DurableWorkerCtx`. `finished` is set so dropping the handle is a no-op (no drop
    /// event), and no `drop_sink` is attached. `start_idx`/`begin_index` are set from
    /// `scope.retry_from` only so the struct is well-formed; the tests read nothing but the scope.
    fn synthetic_finished_handle_with_scope<P: DropPolicy>(
        scope: CallExecutionScope,
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
        let start_idx = scope.retry_from;
        CallHandle {
            start_idx,
            begin_index: start_idx,
            is_live: true,
            persisted: true,
            request_upload: PendingUpload::already_durable(),
            replay: None,
            finished: true,
            parked_undelivered_replay: false,
            execution_scope: scope,
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                durable_execution_state,
                "test:monotonic_clock::now",
            ),
            drop_sink: None,
            cleanup_sink: None,
            live_call_permit: None,
            _phantom: PhantomData,
        }
    }

    #[test]
    fn live_call_permit_tracks_live_handle_lifetime() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let _permit = LiveCallPermit::new(counter.clone());
            assert_eq!(counter.load(Ordering::Acquire), 1);
        }

        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cloning_a_live_call_permit_takes_an_extra_count() {
        let counter = Arc::new(AtomicUsize::new(0));
        let permit = LiveCallPermit::new(counter.clone());
        let cloned = permit.clone();
        assert_eq!(counter.load(Ordering::Acquire), 2);
        drop(permit);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(cloned);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    async fn cleanup_after_terminal_keeps_live_permit_until_event_is_consumed() {
        // An `AccessTerminalGuard` armed with `cleanup_after_terminal` and then dropped (a torn
        // completion future) must keep the call counted as in flight — via the queued
        // `CleanupAfterTerminal` event — until the event is consumed, so a positional boundary
        // cannot be placed before the still-pending terminal append.
        //
        // The policy here is `NotCancellable`, whose `production_drop_sink` is `None`: the
        // cleanup event must be enqueued through the policy-independent cleanup sink, exactly as
        // it is for the p3 accessor calls that use this policy.
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut guard = AccessTerminalGuard::<NotCancellable>::new(
                DroppedCall {
                    start_idx: idx(5),
                    begin_index: idx(4),
                    function_type: DurableFunctionType::ReadRemote,
                    request_upload: PendingUpload::already_durable(),
                    atomic_lease: None,
                    trap_context: DurableCallTrapContext {
                        retry_from: idx(4),
                        in_atomic_region: false,
                    },
                    live_call_permit: Some(LiveCallPermit::new(counter.clone())),
                },
                NotCancellable::production_drop_sink(Some(tx.clone())),
                Some(tx),
            );
            assert_eq!(counter.load(Ordering::Acquire), 1);
            let terminal = tokio::spawn(async move {
                let _ = gate_rx.await;
                Ok(())
            });
            guard.cleanup_after_terminal(terminal, None);
            assert_eq!(
                counter.load(Ordering::Acquire),
                1,
                "the state transition must keep the permit"
            );
            // The guard is dropped here while the terminal task is still pending.
        }
        assert_eq!(
            counter.load(Ordering::Acquire),
            1,
            "the queued CleanupAfterTerminal event must own the permit"
        );
        let event = rx
            .try_recv()
            .expect("expected a CleanupAfterTerminal drop event");
        assert_eq!(counter.load(Ordering::Acquire), 1);
        gate_tx.send(()).expect("terminal task is alive");
        drop(event);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    // ---- CompletionDelivery (deferred guest-delivery token) ----

    /// Builds a live-armed [`CompletionDelivery`] over the given oplog, exactly as
    /// [`AccessTerminalGuard::take_completion_delivery`] produces one for a persisted live call
    /// whose `End` is already durable.
    async fn live_delivery_token(
        oplog: Arc<InMemoryOplog>,
        permit_counter: Arc<AtomicUsize>,
        cleanup_tx: mpsc::UnboundedSender<DropEvent>,
    ) -> CompletionDelivery {
        let oplog_dyn: Arc<dyn Oplog> = oplog;
        // The replay state is built over a separately seeded [Start, End] oplog so the observed
        // oplog contains only what the token itself appends (and so an End gate installed on the
        // observed oplog is not tripped by the seeding).
        let seed_oplog = Arc::new(InMemoryOplog::new());
        seed_oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::MonotonicClockNow,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    golem_common::model::oplog::HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::ReadLocal,
            })
            .await;
        seed_oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: idx(1),
                response: None,
                forced_commit: false,
            })
            .await;
        let seed_oplog_dyn: Arc<dyn Oplog> = seed_oplog;
        let replay_state = ReplayState::new(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "completion-delivery-test".to_string(),
                },
            },
            seed_oplog_dyn,
            golem_common::model::regions::DeletedRegions::default(),
        )
        .await
        .expect("failed to build replay state");
        CompletionDelivery {
            state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                marker: DiscardMarker {
                    start_idx: idx(1),
                    oplog: oplog_dyn,
                    replay_state,
                },
                trap_context: DurableCallTrapContext {
                    retry_from: idx(1),
                    in_atomic_region: false,
                },
                live_call_permit: Some(LiveCallPermit::new(permit_counter)),
                cleanup_sink: Some(cleanup_tx),
                pending_append: None,
            })),
        }
    }

    #[test]
    async fn completion_delivery_delivered_and_suppress_record_no_marker() {
        // `delivered` (successful final guest transfer) and `suppress` (caller-observed
        // post-`End` error) must not record a `CompletionDiscarded` marker and must release the
        // in-flight permit without queueing a drain event (no pending ordered append).
        let variants: [fn(CompletionDelivery); 2] =
            [CompletionDelivery::delivered, CompletionDelivery::suppress];
        for consume in variants {
            let oplog = Arc::new(InMemoryOplog::new());
            let counter = Arc::new(AtomicUsize::new(0));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
            assert_eq!(counter.load(Ordering::Acquire), 1);
            consume(token);
            assert_eq!(counter.load(Ordering::Acquire), 0);
            assert!(rx.try_recv().is_err(), "no drain event may be queued");
            assert!(
                oplog.entries.lock().await.is_empty(),
                "no marker may be recorded"
            );
        }
    }

    #[test]
    async fn completion_delivery_armed_drop_records_marker_via_drain() {
        // Dropping the token while still armed (a torn delivering future) must spawn the owned
        // marker append and hand its join plus the in-flight permit to the drain queue, so
        // invocation settlement waits for the marker.
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
            assert_eq!(counter.load(Ordering::Acquire), 1);
            drop(token);
        }
        assert_eq!(
            counter.load(Ordering::Acquire),
            1,
            "the queued drain event must own the permit"
        );
        match rx.try_recv() {
            Ok(DropEvent::AwaitDiscardMarker {
                terminal,
                live_call_permit,
                ..
            }) => {
                terminal
                    .expect("the drain event must carry the marker join")
                    .await
                    .expect("marker task must not panic")
                    .expect("marker append must succeed");
                let entries = oplog.entries.lock().await;
                assert_eq!(entries.len(), 1, "expected exactly one marker entry");
                match &entries[0] {
                    OplogEntry::CompletionDiscarded { start_index, .. } => {
                        assert_eq!(*start_index, idx(1))
                    }
                    other => panic!("expected CompletionDiscarded, got {other:?}"),
                }
                drop(live_call_permit);
                assert_eq!(counter.load(Ordering::Acquire), 0);
            }
            other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
        }
    }

    #[test]
    async fn completion_delivery_discarded_appends_marker_inline() {
        // `discarded` (caller-detected silent discard) appends exactly one marker inline and
        // returns once it is durable; the permit is released and no drain event is queued.
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
        token.discarded().await.expect("discarded must succeed");
        {
            let entries = oplog.entries.lock().await;
            assert_eq!(entries.len(), 1, "expected exactly one marker entry");
            assert!(matches!(
                &entries[0],
                OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
            ));
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(rx.try_recv().is_err(), "no drain event may be queued");
    }

    #[test]
    async fn completion_delivery_discarded_is_cancellation_safe() {
        // Tearing the future awaiting `discarded()` mid-wait must not lose the marker: the
        // owned append task keeps running, and the join plus the in-flight permit are handed to
        // the drain queue so settlement still waits for the marker.
        let (reached_tx, mut reached_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let oplog = Arc::new(InMemoryOplog::with_end_gate(reached_tx, gate.clone()));
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;

        let discard_task = tokio::spawn(async move { token.discarded().await });
        // The owned marker append is provably inside the gated `add`...
        reached_rx
            .recv()
            .await
            .expect("marker append must reach the gate");
        assert_eq!(counter.load(Ordering::Acquire), 1);
        assert!(rx.try_recv().is_err());

        // ...when the awaiting future is torn.
        discard_task.abort();
        let _ = discard_task.await;

        // The drain event owns the join and the permit; the marker still lands.
        match rx.try_recv() {
            Ok(DropEvent::AwaitDiscardMarker {
                terminal,
                live_call_permit,
                ..
            }) => {
                assert_eq!(
                    counter.load(Ordering::Acquire),
                    1,
                    "the drain event must own the permit"
                );
                gate.add_permits(1);
                terminal
                    .expect("the drain event must carry the marker join")
                    .await
                    .expect("marker task must not panic")
                    .expect("marker append must succeed");
                let entries = oplog.entries.lock().await;
                assert_eq!(entries.len(), 1, "expected exactly one marker entry");
                assert!(matches!(
                    &entries[0],
                    OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
                ));
                drop(live_call_permit);
                assert_eq!(counter.load(Ordering::Acquire), 0);
            }
            other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
        }
    }

    #[test]
    async fn completion_delivery_replay_tokens_are_inert() {
        // Replay tokens mirror the recorded delivery status and never touch the oplog again:
        // - `replay_discarded` reports the discard (the caller must not redeliver the recorded
        //   response) and consuming or dropping it records nothing;
        // - `replay_delivered` reports normal delivery and is equally inert.
        // Together with `replay_resolves_completed_but_discarded` (replay_state) and the live
        // marker tests above, this pins the full discard chain: a recorded
        // `[Start, End, CompletionDiscarded]` resolves to a discarded token, the call site skips
        // redelivery, and replay appends no second marker.
        let discarded = CompletionDelivery::replay_discarded();
        assert!(discarded.is_replay_discarded());
        assert!(!discarded.is_live_armed());
        discarded
            .discarded()
            .await
            .expect("discarding a replay-discarded token is a no-op");

        let delivered = CompletionDelivery::replay_delivered();
        assert!(!delivered.is_replay_discarded());
        assert!(!delivered.is_live_armed());
        delivered.delivered();

        // Dropping an unconsumed replay-discarded token is a no-op as well (the park path drops
        // the token after waiting for the guest to abandon the delivery boundary).
        drop(CompletionDelivery::replay_discarded());
    }

    #[test]
    async fn completion_delivery_ordered_append_lands_before_marker() {
        // An `append_ordered` entry (e.g. a durable `FinishSpan`) handed to the token must land
        // *before* the torn-drop marker, preserving the recorded
        // `End → FinishSpan → CompletionDiscarded` order replay consumes positionally.
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
            token.append_ordered(OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
            });
            drop(token);
        }
        match rx.try_recv() {
            Ok(DropEvent::AwaitDiscardMarker { terminal, .. }) => {
                terminal
                    .expect("the drain event must carry the chained join")
                    .await
                    .expect("chained task must not panic")
                    .expect("chained appends must succeed");
            }
            other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
        }
        let entries = oplog.entries.lock().await;
        assert_eq!(entries.len(), 2, "expected [ordered entry, marker]");
        assert!(matches!(&entries[0], OplogEntry::NoOp { .. }));
        assert!(matches!(
            &entries[1],
            OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
        ));
    }

    /// Minimal in-memory [`Oplog`] recording appended entries, for tests that assert what a
    /// drained drop event writes durably. With an `end_gate` installed, appending an `End` or
    /// `CompletionDiscarded` entry first signals `reached` and then blocks until the gate
    /// semaphore yields a permit, so a test can hold the terminal or marker append open and
    /// observe what is (not) visible meanwhile.
    #[derive(Debug)]
    struct InMemoryOplog {
        entries: tokio::sync::Mutex<Vec<OplogEntry>>,
        end_gate: Option<(mpsc::UnboundedSender<()>, Arc<tokio::sync::Semaphore>)>,
    }

    impl InMemoryOplog {
        fn new() -> Self {
            Self {
                entries: tokio::sync::Mutex::new(Vec::new()),
                end_gate: None,
            }
        }

        fn with_end_gate(
            reached: mpsc::UnboundedSender<()>,
            gate: Arc<tokio::sync::Semaphore>,
        ) -> Self {
            Self {
                entries: tokio::sync::Mutex::new(Vec::new()),
                end_gate: Some((reached, gate)),
            }
        }
    }

    #[async_trait]
    impl Oplog for InMemoryOplog {
        async fn add(&self, entry: OplogEntry) -> OplogIndex {
            if matches!(
                entry,
                OplogEntry::End { .. } | OplogEntry::CompletionDiscarded { .. }
            ) && let Some((reached, gate)) = &self.end_gate
            {
                let _ = reached.send(());
                gate.acquire()
                    .await
                    .expect("end gate semaphore is never closed")
                    .forget();
            }
            let mut entries = self.entries.lock().await;
            entries.push(entry);
            OplogIndex::from_u64(entries.len() as u64)
        }

        async fn add_start_with_reserved_raw_payload(
            &self,
            serialized_request: Vec<u8>,
            build_start: Box<
                dyn FnOnce(
                        golem_common::model::oplog::RawOplogPayload,
                    ) -> Result<OplogEntry, String>
                    + Send,
            >,
        ) -> Result<crate::services::oplog::OrderedOplogStart, String> {
            let entry = build_start(
                golem_common::model::oplog::RawOplogPayload::SerializedInline(serialized_request),
            )?;
            let index = self.add(entry.clone()).await;
            Ok(crate::services::oplog::OrderedOplogStart {
                index,
                entry,
                pending_upload: PendingUpload::already_durable(),
            })
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            0
        }

        async fn commit(
            &self,
            _level: CommitLevel,
        ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
            std::collections::BTreeMap::new()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            OplogIndex::from_u64(self.entries.lock().await.len() as u64)
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            None
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            true
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            let entries = self.entries.lock().await;
            let idx: u64 = oplog_index.into();
            entries[(idx - 1) as usize].clone()
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
            let entries = self.entries.lock().await;
            let start: u64 = oplog_index.into();
            let mut result = std::collections::BTreeMap::new();
            for i in start..(start + n) {
                if let Some(entry) = entries.get((i - 1) as usize) {
                    result.insert(OplogIndex::from_u64(i), entry.clone());
                }
            }
            result
        }

        async fn length(&self) -> u64 {
            self.entries.lock().await.len() as u64
        }

        async fn upload_raw_payload(
            &self,
            data: Vec<u8>,
        ) -> Result<golem_common::model::oplog::RawOplogPayload, String> {
            Ok(golem_common::model::oplog::RawOplogPayload::SerializedInline(data))
        }

        async fn download_raw_payload(
            &self,
            _payload_id: golem_common::model::oplog::PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unimplemented!()
        }

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
    }

    #[test]
    async fn dropped_cancellable_call_records_cancelled_at_next_drain_point() {
        // A live `Cancellable` call dropped mid-flight enqueues an `UnfinishedCancellable`
        // snapshot; this test drives the drain's *recording step* directly
        // (`DroppedCall::append_cancelled_with_oplog`, shared by both production drains) and
        // asserts the durable outcome. Invoking a production drain itself, and the ctx-side
        // effects (durable-scope close, atomic-region unregistration), require full-worker
        // integration coverage.
        let oplog = Arc::new(InMemoryOplog::new());
        let start_idx = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::MonotonicClockNow,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    golem_common::model::oplog::HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::ReadLocal,
            })
            .await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<Cancellable>(start_idx, tx);
            // Dropped mid-flight here, e.g. by a guest cancelling the subtask driving the call.
        }

        let call = match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => call,
            other => panic!("expected an UnfinishedCancellable drop event, got {other:?}"),
        };

        // The drain point: wait for the request upload, then record the terminal.
        call.wait_request_upload()
            .await
            .expect("request upload must succeed");
        call.append_cancelled_with_oplog(oplog.clone(), None)
            .await
            .expect("recording Cancelled must succeed");

        let entries = oplog.entries.lock().await;
        assert_eq!(entries.len(), 2, "expected [Start, Cancelled]");
        match &entries[1] {
            OplogEntry::Cancelled {
                start_index,
                partial,
                ..
            } => {
                assert_eq!(*start_index, start_idx);
                assert!(partial.is_none());
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    async fn access_terminal_end_is_appended_before_cleanup_and_permit_release() {
        // Seam-2 ordering, proven against the production persistence stage itself
        // (`CallHandle::persist_access_terminal`, the exact code `complete_access_impl` runs for a
        // persisted live call): while the terminal `End` append is still in flight, the live-call
        // permit must stay held and no cleanup event may become visible; both are released only
        // after the append completes (production releases them via `disarm()` after
        // `end_durable_function_access`, strictly downstream of `wait_terminal()`).
        use golem_common::model::oplog::HostResponseMonotonicClockTimestamp;

        let (reached_tx, mut reached_rx) = mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let oplog = Arc::new(InMemoryOplog::with_end_gate(reached_tx, gate.clone()));
        let start_idx = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::MonotonicClockNow,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    golem_common::model::oplog::HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::ReadRemote,
            })
            .await;

        let permit_counter = Arc::new(AtomicUsize::new(0));
        let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel();
        let mut guard = AccessTerminalGuard::<NotCancellable>::new(
            DroppedCall {
                start_idx,
                begin_index: start_idx,
                function_type: DurableFunctionType::ReadRemote,
                request_upload: PendingUpload::already_durable(),
                atomic_lease: None,
                trap_context: DurableCallTrapContext {
                    retry_from: start_idx,
                    in_atomic_region: false,
                },
                live_call_permit: Some(LiveCallPermit::new(permit_counter.clone())),
            },
            NotCancellable::production_drop_sink(Some(cleanup_tx.clone())),
            Some(cleanup_tx),
        );
        assert_eq!(permit_counter.load(Ordering::Acquire), 1);

        let persist_oplog: Arc<dyn Oplog> = oplog.clone();
        let persist_replay_state = ReplayState::new(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "concurrent-test".to_string(),
                },
            },
            persist_oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
        )
        .await
        .expect("failed to build replay state");
        let persist = tokio::spawn(async move {
            let response = HostResponseMonotonicClockTimestamp { nanos: 42 };
            let result = CallHandle::<host_functions::MonotonicClockNow, NotCancellable>::
                persist_access_terminal(persist_oplog, persist_replay_state, &mut guard, start_idx, &response, None)
            .await;
            (result, guard)
        });

        // The owned terminal task is now provably inside the gated `End` append...
        reached_rx
            .recv()
            .await
            .expect("terminal append must reach the gate");
        // ...and while it is held open, the call is still counted as in flight and no cleanup
        // event has escaped, so a positional boundary cannot slip in before the terminal.
        assert_eq!(
            permit_counter.load(Ordering::Acquire),
            1,
            "live-call permit must be held while the terminal append is pending"
        );
        assert!(
            cleanup_rx.try_recv().is_err(),
            "no cleanup event may be visible while the terminal append is pending"
        );
        assert_eq!(
            oplog.entries.lock().await.len(),
            1,
            "the End entry must not be durable yet"
        );

        gate.add_permits(1);
        let (result, mut guard) = persist.await.expect("persist task must not panic");
        result.expect("persisting the terminal must succeed");

        // The terminal is durably appended when the persistence stage returns...
        {
            let entries = oplog.entries.lock().await;
            assert_eq!(entries.len(), 2, "expected [Start, End]");
            match &entries[1] {
                OplogEntry::End { start_index, .. } => assert_eq!(*start_index, start_idx),
                other => panic!("expected End, got {other:?}"),
            }
        }
        // ...while the guard still owns the permit and nothing has been queued: release happens
        // only at the production `disarm()`, strictly after the terminal.
        assert_eq!(
            permit_counter.load(Ordering::Acquire),
            1,
            "the guard must still own the permit after the terminal is durable"
        );
        assert!(cleanup_rx.try_recv().is_err());

        guard.disarm();
        assert_eq!(
            permit_counter.load(Ordering::Acquire),
            0,
            "disarm after the terminal releases the in-flight count"
        );
        assert!(cleanup_rx.try_recv().is_err());
    }

    #[test]
    fn dropped_unfinished_call_keeps_live_permit_until_drop_event_is_consumed() {
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
            handle.live_call_permit = Some(LiveCallPermit::new(counter.clone()));
            assert_eq!(counter.load(Ordering::Acquire), 1);
        }
        // The handle is gone, but the queued drop event now owns the permit: an in-flight check
        // (e.g. the `set_oplog_persistence_level` boundary guard) running between the drop and
        // the next drain must still count this call.
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let event = rx
            .try_recv()
            .expect("expected an UnfinishedCancellable drop event");
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(event);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn drop_cancellable_unfinished_enqueues_exactly_one_cancelled() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region(), None);
            }
            other => panic!("expected one UnfinishedCancellable, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "expected exactly one drop event");
    }

    #[test]
    fn drop_cancellable_unfinished_carries_call_owned_cancellation_state() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle =
                live_unfinished_handle_with_atomic_region::<Cancellable>(idx(5), Some(idx(2)), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region(), Some(idx(2)));
            }
            other => panic!("expected one UnfinishedCancellable, got {other:?}"),
        }
    }

    #[test]
    fn drop_not_cancellable_unfinished_signals_policy_violation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<NotCancellable>(idx(7), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedNotCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(7))
            }
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
    fn seam2_unfinished_cancellable_drops_emit_independent_call_snapshots() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let handle_a = live_unfinished_handle_with_atomic_region::<Cancellable>(
            idx(5),
            Some(idx(2)),
            tx.clone(),
        );
        let handle_b =
            live_unfinished_handle_with_atomic_region::<Cancellable>(idx(9), Some(idx(3)), tx);

        drop(handle_a);
        drop(handle_b);

        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region(), Some(idx(2)));
            }
            other => panic!("expected first cancellable drop snapshot, got {other:?}"),
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(9));
                assert_eq!(call.atomic_region(), Some(idx(3)));
            }
            other => panic!("expected second cancellable drop snapshot, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "expected exactly two drop events");
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

    // ---- call-scoped retry host ----

    struct RetryHostProbe {
        current_retry_point: OplogIndex,
        appended_retry_from: Vec<OplogIndex>,
        appended_inside_atomic_region: Vec<bool>,
        /// Atomic region begin indices the inner host reports as having recorded side effects.
        regions_with_side_effects: Vec<OplogIndex>,
        /// Ambient outermost-region side-effect state, used by the unscoped fallback path.
        outermost_has_side_effects: bool,
    }

    impl RetryHostProbe {
        fn new(current_retry_point: OplogIndex) -> Self {
            Self {
                current_retry_point,
                appended_retry_from: Vec::new(),
                appended_inside_atomic_region: Vec::new(),
                regions_with_side_effects: Vec::new(),
                outermost_has_side_effects: false,
            }
        }
    }

    #[async_trait]
    impl InFunctionRetryHost for RetryHostProbe {
        fn in_atomic_region(&self) -> bool {
            false
        }

        fn current_retry_point(&self) -> OplogIndex {
            self.current_retry_point
        }

        async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
            vec![NamedRetryPolicy {
                name: "test".to_string(),
                priority: 0,
                predicate: Predicate::True,
                policy: RetryPolicy::CountBox {
                    max_retries: 1,
                    inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
                },
            }]
        }

        async fn current_retry_state_for(
            &self,
            retry_from: OplogIndex,
        ) -> Option<golem_common::model::RetryPolicyState> {
            if retry_from == self.current_retry_point {
                Some(RetryPolicyState::CountBox {
                    attempts: 1,
                    inner: Box::new(RetryPolicyState::Counter(1)),
                })
            } else {
                None
            }
        }

        fn durable_execution_state(&self) -> DurableExecutionState {
            durable_execution_state()
        }

        fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
            self.regions_with_side_effects.contains(&begin_index)
        }

        fn retry_context_atomic_region_had_side_effects(&self) -> bool {
            self.outermost_has_side_effects
        }

        async fn append_retry_error_entry(
            &mut self,
            retry_from: OplogIndex,
            inside_atomic_region: bool,
            _retry_policy_state: Option<golem_common::model::RetryPolicyState>,
        ) {
            self.appended_retry_from.push(retry_from);
            self.appended_inside_atomic_region
                .push(inside_atomic_region);
        }
    }

    #[test]
    fn scoped_retry_host_uses_call_retry_point_not_inner_current() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::PersistRemoteSideEffects,
        };

        let retry_host = ScopedRetryHost::new(&mut inner, &scope);

        assert!(!retry_host.in_atomic_region());
        assert_eq!(retry_host.current_retry_point(), idx(42));
        assert_eq!(
            retry_host.durable_execution_state().persistence_level,
            PersistenceLevel::PersistRemoteSideEffects
        );
    }

    #[test]
    fn scoped_retry_host_uses_call_atomic_region_as_retry_point() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
            persistence_level: PersistenceLevel::Smart,
        };

        let retry_host = ScopedRetryHost::new(&mut inner, &scope);

        assert!(retry_host.in_atomic_region());
        assert_eq!(retry_host.current_retry_point(), idx(7));
    }

    #[test]
    async fn scoped_retry_host_trap_retry_uses_call_retry_point() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let mut retry_host = ScopedRetryHost::new(&mut inner, &scope);

        let outcome = try_trigger_host_trap_retry(
            &mut retry_host,
            Error::msg("transient failure"),
            RetryProperties::new(),
        )
        .await;

        assert!(
            outcome.is_err(),
            "empty state at the call-owned retry point should allow one retry; using the inner/global retry point would be exhausted"
        );
    }

    #[test]
    async fn seam2_overlapping_semantic_traps_carry_independent_retry_points() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };

        let error_a = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_a);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure a"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call A should trap for oplog-level retry")
        };

        let error_b = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_b);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure b"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call B should trap for oplog-level retry")
        };

        let override_a =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_a)
                .expect("call A trap must carry a semantic retry override");
        let override_b =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_b)
                .expect("call B trap must carry a semantic retry override");

        assert_eq!(override_a.retry_from, idx(42));
        assert_eq!(override_b.retry_from, idx(77));
    }

    #[test]
    async fn seam2_overlapping_atomic_region_traps_use_initiation_membership() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_lease: unregistered_atomic_lease(Some(idx(8)), true),
            persistence_level: PersistenceLevel::Smart,
        };

        let error_a = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_a);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure a"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call A should trap for oplog-level retry")
        };

        let error_b = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_b);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure b"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call B should trap for oplog-level retry")
        };

        let override_a =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_a)
                .expect("call A trap must carry a semantic retry override");
        let override_b =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_b)
                .expect("call B trap must carry a semantic retry override");

        assert_eq!(override_a.retry_from, idx(7));
        assert_eq!(override_b.retry_from, idx(8));
    }

    // ---- Seam-2 terminal-step failures ----

    /// Classifies a terminal-step failure the way the invocation loop does, with a deliberately
    /// *hostile* ambient fallback (a retry point and atomic-region membership belonging to no real
    /// call). A call-owned [`DurableCallTrapContext`] marker carried by the error must override both
    /// fallbacks; if the terminal path lost the marker, the classifier would silently adopt these
    /// ambient values instead — the exact §5.7.3 misclassification we are guarding against.
    fn classify_with_hostile_ambient(error: &anyhow::Error) -> crate::model::TrapType {
        crate::model::TrapType::from_error::<crate::workerctx::default::Context>(
            error,
            idx(99),
            true,
            false,
            golem_common::model::agent::AgentMode::Durable,
        )
    }

    #[test]
    fn seam2_terminal_failure_carries_call_owned_trap_context() {
        // A failure escaping a *terminal* durable-call step (`complete` / `complete_access` /
        // `cancel` / the dropped-call drain) is wrapped in a `TerminalCallError` built from the
        // call's own execution scope. Classifying it must group the retry against the call's own
        // scope and use the call's own atomic-region membership, never the ambient worker state.
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let handle = synthetic_finished_handle_with_scope::<Cancellable>(scope);

        let error: anyhow::Error = TerminalCallError::new(
            WorkerExecutorError::runtime("terminal step failure"),
            handle.trap_context(),
        )
        .into();

        match classify_with_hostile_ambient(&error) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call-owned (idx 42, non-atomic) wins over hostile ambient (idx 99, atomic).
                assert_eq!(retry_from, idx(42));
                assert!(!in_atomic_region);
            }
            other => panic!("expected TrapType::Error, got {other:?}"),
        }
    }

    #[test]
    fn seam2_overlapping_terminal_failures_carry_independent_trap_contexts() {
        // Two durable calls overlap in flight and share the same ambient worker state. Each one's
        // terminal-step failure must still be classified against its OWN retry point and
        // atomic-region membership. Before §5.7.3 these escaped unmarked and fell back to that shared
        // ambient state, so an overlapping sibling could clobber the grouping.
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let handle_a = synthetic_finished_handle_with_scope::<Cancellable>(scope_a);
        let handle_b = synthetic_finished_handle_with_scope::<Cancellable>(scope_b);

        let error_a: anyhow::Error =
            TerminalCallError::new(WorkerExecutorError::runtime("a"), handle_a.trap_context())
                .into();
        let error_b: anyhow::Error =
            TerminalCallError::new(WorkerExecutorError::runtime("b"), handle_b.trap_context())
                .into();

        // Both classified against the SAME hostile ambient state, yet each keeps its own grouping.
        match classify_with_hostile_ambient(&error_a) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call A was initiated inside an atomic region: the whole region retries from its
                // begin index (idx 7) and membership is recorded.
                assert_eq!(retry_from, idx(7));
                assert!(in_atomic_region);
            }
            other => panic!("expected TrapType::Error for call A, got {other:?}"),
        }
        match classify_with_hostile_ambient(&error_b) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call B was not in an atomic region: it retries from its own scope (idx 77).
                assert_eq!(retry_from, idx(77));
                assert!(!in_atomic_region);
            }
            other => panic!("expected TrapType::Error for call B, got {other:?}"),
        }
    }

    #[test]
    fn seam2_dropped_call_drain_failure_uses_dropped_call_trap_context() {
        // A cancellation-drain failure (deferred request upload / terminal recorder join) is driven
        // by whichever later host call happens to run the drain, but it must be classified with the
        // *dropped* call's captured context, not the drainer's ambient state. `DroppedCall::trap_context`
        // carries that captured classification; the drain wraps failures with it via
        // `TerminalCallError`.
        // An atomic dropped call: its own context (idx 3, atomic) must win over the hostile ambient.
        let dropped_atomic = DroppedCall {
            start_idx: idx(5),
            begin_index: idx(4),
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            atomic_lease: unregistered_atomic_lease(Some(idx(3)), true),
            trap_context: DurableCallTrapContext {
                retry_from: idx(3),
                in_atomic_region: true,
            },
            live_call_permit: None,
        };
        // A non-atomic dropped call: its membership (false) must win over the hostile ambient's
        // `in_atomic_region = true`, so the membership assertion is independent of the retry point.
        let dropped_non_atomic = DroppedCall {
            start_idx: idx(8),
            begin_index: idx(8),
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            atomic_lease: None,
            trap_context: DurableCallTrapContext {
                retry_from: idx(8),
                in_atomic_region: false,
            },
            live_call_permit: None,
        };

        for (dropped, expected_retry, expected_atomic) in [
            (dropped_atomic, idx(3), true),
            (dropped_non_atomic, idx(8), false),
        ] {
            let error: anyhow::Error = TerminalCallError::new(
                WorkerExecutorError::runtime("cancellation drain failed"),
                dropped.trap_context(),
            )
            .into();

            // `classify_with_hostile_ambient` supplies idx(99)/atomic-membership-true as the
            // drainer's ambient state; the dropped call's own context must win.
            match classify_with_hostile_ambient(&error) {
                crate::model::TrapType::Error {
                    retry_from,
                    in_atomic_region,
                    ..
                } => {
                    assert_eq!(retry_from, expected_retry);
                    assert_eq!(in_atomic_region, expected_atomic);
                }
                other => panic!("expected TrapType::Error, got {other:?}"),
            }
        }
    }

    // ---- call-owned execution scope ----

    #[test]
    fn begun_execution_scope_uses_parent_scope_as_retry_from() {
        let begun = BegunCallExecutionScope {
            parent_start_index: Some(idx(10)),
            atomic_region: Some(idx(2)),
            persistence_level: PersistenceLevel::PersistNothing,
        };

        let lease = unregistered_atomic_lease(begun.atomic_region, true);
        let scope = begun.finish(idx(11), lease);

        assert_eq!(scope.retry_from, idx(10));
        assert_eq!(scope.durable_scope, Some(idx(10)));
        assert_eq!(scope.atomic_region(), Some(idx(2)));
        assert_eq!(scope.persistence_level, PersistenceLevel::PersistNothing);
    }

    #[test]
    fn begun_execution_scope_uses_call_start_as_retry_from_when_unscoped() {
        let begun = BegunCallExecutionScope {
            parent_start_index: None,
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };

        let scope = begun.finish(idx(12), None);

        assert_eq!(scope.retry_from, idx(12));
        assert_eq!(scope.durable_scope, None);
        assert_eq!(scope.atomic_region(), None);
        assert_eq!(scope.persistence_level, PersistenceLevel::Smart);
    }

    #[test]
    fn call_execution_scope_owns_call_retry_point() {
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_lease: None,
            persistence_level: PersistenceLevel::Smart,
        };

        assert_eq!(scope.retry_from, idx(42));
    }

    // ---- function-type re-execution policy ----

    #[test]
    fn can_reexecute_matches_internal_retry_eligibility() {
        use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};

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
