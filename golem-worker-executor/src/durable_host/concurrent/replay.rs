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
