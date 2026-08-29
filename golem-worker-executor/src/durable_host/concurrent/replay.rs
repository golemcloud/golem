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
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::watch;

/// Replayable single-shot channel used to deliver a call's [`Resolution`] from the replay cursor
/// to the awaiting [`DurableCallSession`].
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
        /// The physical guest-delivery boundary for oplogs recorded with completion markers.
        /// Host-side replay continues after `End`, but the result may not cross to the guest until
        /// the matching token positionally consumes this marker.
        delivery_marker: Option<OplogIndex>,
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
    /// ([`DurableCallSession::replay_access_deferred`]) must decode it to reconstruct deterministic
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

/// The result of [`DurableCallSession::replay`].
///
/// Transient: callers destructure it immediately, so the size difference between the variants
/// never lives beyond the replay call itself.
#[allow(clippy::large_enum_variant)]
pub enum CallReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    /// The call's `End` was replayed and decoded into its response.
    Replayed(Pair::Resp),
    /// The call's `Start` was committed but its `End` never was. The returned handle has been
    /// switched to live completion of that existing `Start`: the caller must re-run the side effect
    /// and call [`DurableCallSession::complete`] (which appends the missing `End`). Only produced for
    /// function types that are safe to re-execute.
    Incomplete(DurableCallSession<Pair, P>),
}

/// Replay outcome used by executor-owned entity reconstruction tasks. Cancellation is observable
/// here because the executor, rather than a deterministic guest future, owns and fences the
/// transient body task.
#[allow(clippy::large_enum_variant)]
pub enum ReconstructionReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    Replayed(Pair::Resp),
    Cancelled(Pair::Resp),
    Incomplete(DurableCallSession<Pair, P>),
}

pub(in crate::durable_host) struct ResolvedReconstructionTerminal {
    pub(super) function_type: DurableFunctionType,
    pub(super) begin_index: OplogIndex,
    pub(super) delivery: CompletionDelivery,
    pub(super) cancelled: bool,
}

impl ResolvedReconstructionTerminal {
    pub(in crate::durable_host) fn cancelled(&self) -> bool {
        self.cancelled
    }
}

/// The result of [`DurableCallSession::replay_access_deferred`]: like [`CallReplayOutcome`], but each
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
    /// via [`DurableCallSession::complete_access_deferred`].
    Incomplete(DurableCallSession<Pair, P>),
}

/// Matches replayed `End`/`Cancelled` entries back to the [`DurableCallSession`]s awaiting them, keyed by
/// the `OplogIndex` of the call's `Start`.
///
/// Lives inside the replay state behind its lock. It is fed **only** from the committed-consume
/// hook (see [`crate::durable_host::replay_state::ReplayState`]); speculative cursor reads that
/// roll back must never reach it.
#[derive(Debug)]
pub struct ConcurrentReplayResolver {
    /// Awaiters that have registered but whose resolution has not been observed yet.
    pending: HashMap<OplogIndex, ReplayableOneshot<ResolutionOutcome>>,
    /// Marked successful completions whose payload has been read ahead and delivered to their host
    /// continuation without advancing the positional cursor. The matching terminal still has to
    /// be auto-drained when the cursor reaches its recorded index.
    prefetched_terminals: HashMap<OplogIndex, OplogIndex>,
    /// Resolutions observed before their awaiter registered. The await-resolution guard
    /// guarantees a call's `Start` is claimed before its `End`/`Cancelled` is consumed, so on the
    /// replay path this stays empty; it covers the resolver's own unit tests and any future entry
    /// point that resolves without that ordering guarantee.
    buffered: HashMap<OplogIndex, ResolutionOutcome>,
    reconstruction_claims: Arc<ReconstructionClaimState>,
}

#[derive(Debug)]
pub(crate) struct ReconstructionClaimState {
    active_fences: watch::Sender<HashSet<OplogIndex>>,
    active_bodies: watch::Sender<HashSet<OplogIndex>>,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoricalReconstruction {
    inner: Arc<HistoricalReconstructionInner>,
}

#[derive(Debug)]
struct HistoricalReconstructionInner {
    state: Arc<ReconstructionClaimState>,
    start_index: OplogIndex,
    body: Mutex<Option<OplogIndex>>,
}

impl Default for ConcurrentReplayResolver {
    fn default() -> Self {
        let (active_fences, _) = watch::channel(HashSet::new());
        let (active_bodies, _) = watch::channel(HashSet::new());
        Self {
            pending: HashMap::new(),
            prefetched_terminals: HashMap::new(),
            buffered: HashMap::new(),
            reconstruction_claims: Arc::new(ReconstructionClaimState {
                active_fences,
                active_bodies,
            }),
        }
    }
}

impl ReconstructionClaimState {
    fn register(self: &Arc<Self>, start_index: OplogIndex) -> HistoricalReconstruction {
        let mut active = 0;
        self.active_fences.send_modify(|fences| {
            assert!(
                fences.insert(start_index),
                "historical entity reconstruction at {start_index} was registered twice"
            );
            active = fences.len();
        });
        self.active_bodies.send_modify(|bodies| {
            assert!(
                bodies.insert(start_index),
                "entity body reconstruction at {start_index} was registered twice"
            );
        });
        tracing::debug!(
            start_index = start_index.as_u64(),
            active,
            "Registered resolver-owned historical entity reconstruction"
        );
        HistoricalReconstruction {
            inner: Arc::new(HistoricalReconstructionInner {
                state: self.clone(),
                start_index,
                body: Mutex::new(Some(start_index)),
            }),
        }
    }

    fn release_incomplete(&self, start_index: OplogIndex) {
        let mut removed = false;
        let mut remaining = 0;
        self.active_fences.send_modify(|fences| {
            removed = fences.remove(&start_index);
            remaining = fences.len();
        });
        if removed {
            tracing::debug!(
                start_index = start_index.as_u64(),
                active = remaining,
                "Released incomplete resolver-owned historical entity reconstruction fence"
            );
        }
    }

    fn settle_fence(&self, start_index: OplogIndex) {
        let mut removed = false;
        let mut remaining = 0;
        self.active_fences.send_modify(|fences| {
            removed = fences.remove(&start_index);
            remaining = fences.len();
        });
        if removed {
            tracing::debug!(
                start_index = start_index.as_u64(),
                active = remaining,
                "Historical entity reconstruction validated"
            );
        }
    }

    fn settle_body(&self, start_index: OplogIndex) {
        self.active_bodies.send_modify(|bodies| {
            assert!(
                bodies.remove(&start_index),
                "entity body reconstruction at {start_index} was not registered"
            );
        });
    }

    pub(crate) fn subscribe_bodies(&self) -> watch::Receiver<HashSet<OplogIndex>> {
        self.active_bodies.subscribe()
    }

    pub(crate) async fn wait_for_fences(&self) {
        let mut active = self.active_fences.subscribe();
        tracing::debug!(
            active = active.borrow().len(),
            "Waiting for resolver-owned historical entity reconstructions"
        );
        active
            .wait_for(HashSet::is_empty)
            .await
            .expect("replay cursor retains the reconstruction claim state");
        tracing::debug!("Resolver-owned historical entity reconstructions settled");
    }

    pub(crate) fn ensure_empty(&self) -> Result<(), WorkerExecutorError> {
        let fences = self.active_fences.borrow().clone();
        let bodies = self.active_bodies.borrow().clone();
        if fences.is_empty() && bodies.is_empty() {
            Ok(())
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "cannot install a replay generation while historical reconstruction claims remain active (fences: {fences:?}, bodies: {bodies:?})"
            )))
        }
    }

    #[cfg(test)]
    pub(crate) fn active_fences(&self) -> HashSet<OplogIndex> {
        self.active_fences.borrow().clone()
    }

    #[cfg(test)]
    pub(crate) fn active_bodies(&self) -> HashSet<OplogIndex> {
        self.active_bodies.borrow().clone()
    }
}

impl HistoricalReconstruction {
    pub(crate) fn body_settled(&mut self) {
        if let Some(start_index) = self.inner.body.lock().unwrap().take() {
            self.inner.state.settle_body(start_index);
        }
    }
}

impl Drop for HistoricalReconstructionInner {
    fn drop(&mut self) {
        if let Some(start_index) = self.body.lock().unwrap().take() {
            self.state.settle_body(start_index);
        }
        self.state.settle_fence(self.start_index);
    }
}

impl ConcurrentReplayResolver {
    pub(crate) fn reconstruction_claims(&self) -> Arc<ReconstructionClaimState> {
        self.reconstruction_claims.clone()
    }

    pub(crate) fn register_reconstruction(
        &self,
        start_index: OplogIndex,
    ) -> HistoricalReconstruction {
        self.reconstruction_claims.register(start_index)
    }

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
    pub fn resolve_if_pending(
        &mut self,
        start_idx: OplogIndex,
        terminal_idx: OplogIndex,
        resolution: Resolution,
    ) -> bool {
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(ResolutionOutcome::Resolved(resolution));
            true
        } else if self.prefetched_terminals.get(&start_idx) == Some(&terminal_idx) {
            self.prefetched_terminals.remove(&start_idx);
            true
        } else {
            false
        }
    }

    /// Resolves an already-registered marked completion from a non-consuming oplog lookahead.
    /// The terminal index remains registered so the positional cursor auto-drains that exact entry
    /// later instead of exposing it to another reader.
    pub fn resolve_prefetched(
        &mut self,
        start_idx: OplogIndex,
        terminal_idx: OplogIndex,
        resolution: Resolution,
    ) {
        let tx = self
            .pending
            .remove(&start_idx)
            .expect("a completion is prefetched only while registering its claimed Start");
        self.prefetched_terminals.insert(start_idx, terminal_idx);
        let _ = tx.send(ResolutionOutcome::Resolved(resolution));
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn resolve_prefetched_for_test(
        &mut self,
        start_idx: OplogIndex,
        terminal_idx: OplogIndex,
        resolution: Resolution,
    ) {
        if self.pending.contains_key(&start_idx) {
            self.resolve_prefetched(start_idx, terminal_idx, resolution);
        } else {
            assert_eq!(
                self.prefetched_terminals.get(&start_idx),
                Some(&terminal_idx),
                "test replay driver expected an active claim for Start {start_idx}"
            );
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
        for (start_idx, tx) in self.pending.drain() {
            self.reconstruction_claims.release_incomplete(start_idx);
            let _ = tx.send(ResolutionOutcome::Incomplete);
        }
        self.prefetched_terminals.clear();
    }

    /// Removes a registered awaiter without resolving it. Used when a claimed call turns out to be
    /// incomplete on replay (its `Start` is committed but its `End` never was): the awaiter is
    /// switched to live completion, so its pending registration must not linger in the resolver.
    pub fn unregister(&mut self, start_idx: OplogIndex) {
        self.pending.remove(&start_idx);
    }

    pub fn unregister_incomplete(&mut self, start_idx: OplogIndex) {
        self.pending.remove(&start_idx);
        self.reconstruction_claims.release_incomplete(start_idx);
    }

    /// Returns whether the terminal at `terminal_idx` belongs to an active resolver claim. This is
    /// true both for an unresolved awaiter and for a marked completion that was prefetched without
    /// consuming its terminal.
    ///
    /// The replay cursor uses this to decide which `End`/`Cancelled` entries are *awaited
    /// terminals* it may auto-drain (and route back to their awaiter) versus the ones it must leave
    /// for their own positional consumer: scope `End`s, unclaimed `Start`s, and deterministic
    /// markers.
    pub fn owns_terminal(&self, start_idx: OplogIndex, terminal_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx)
            || self.prefetched_terminals.get(&start_idx) == Some(&terminal_idx)
    }

    #[cfg(test)]
    pub fn is_pending(&self, start_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx)
    }

    pub fn has_claim(&self, start_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx) || self.prefetched_terminals.contains_key(&start_idx)
    }

    /// Returns whether the registered resolution receiver is still alive. A dropped replay handle
    /// remains pending so its eventual terminal can be drained, but it cannot make further body
    /// progress and therefore is not an active consumer for structural-divergence detection.
    pub fn is_awaited(&self, start_idx: OplogIndex) -> bool {
        self.pending
            .get(&start_idx)
            .is_some_and(|sender| !sender.is_closed())
    }
}

/// Replay-side state for a single in-flight call: the `Start` index it claimed and the receiver
/// that will deliver its [`Resolution`].
#[derive(Debug)]
pub struct ReplayCallHandle {
    start_idx: OplogIndex,
    receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
    historical_reconstruction: Option<HistoricalReconstruction>,
}

impl ReplayCallHandle {
    pub fn new(
        start_idx: OplogIndex,
        receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
    ) -> Self {
        Self {
            start_idx,
            receiver,
            historical_reconstruction: None,
        }
    }

    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    pub(crate) fn attach_historical_reconstruction(
        &mut self,
        reconstruction: HistoricalReconstruction,
    ) {
        assert!(
            self.historical_reconstruction
                .replace(reconstruction)
                .is_none(),
            "replay call at {} received two historical reconstruction claims",
            self.start_idx
        );
    }

    pub(crate) fn take_historical_reconstruction(&mut self) -> Option<HistoricalReconstruction> {
        self.historical_reconstruction.take()
    }

    /// Consumes the handle into its parts (used by the replay-state driver).
    pub fn into_parts(
        self,
    ) -> (
        OplogIndex,
        ReplayableOneshotReceiver<ResolutionOutcome>,
        Option<HistoricalReconstruction>,
    ) {
        (
            self.start_idx,
            self.receiver,
            self.historical_reconstruction,
        )
    }
}
