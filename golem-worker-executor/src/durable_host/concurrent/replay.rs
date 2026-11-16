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
use crate::worker::owner_wallet::OwnerWallet;
use std::collections::HashSet;
use std::sync::{Mutex, Weak};
use tokio::sync::watch;

/// Replayable single-shot channel used to deliver a call's [`Resolution`] from the replay cursor
/// to the awaiting [`DurableCallSession`].
///
/// `tokio::sync::oneshot` already supports send-before-await, which is all this currently needs.
/// Registration precedes resolution. The replay cursor owns the per-Start lifecycle.
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
    LiveAdmissionCancelled(DurableCallSession<Pair, P>),
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

#[derive(Debug)]
pub(crate) struct ReconstructionClaimState {
    active_fences: watch::Sender<HashSet<OplogIndex>>,
    active_bodies: watch::Sender<HashSet<OplogIndex>>,
}

#[derive(Clone, Debug)]
pub struct HistoricalReconstruction {
    inner: Arc<HistoricalReconstructionInner>,
}

#[derive(Debug)]
struct HistoricalReconstructionInner {
    state: Arc<ReconstructionClaimState>,
    start_index: OplogIndex,
    body: Mutex<Option<OplogIndex>>,
    wallet: Option<Weak<OwnerWallet>>,
}

impl Default for ReconstructionClaimState {
    fn default() -> Self {
        let (active_fences, _) = watch::channel(HashSet::new());
        let (active_bodies, _) = watch::channel(HashSet::new());
        Self {
            active_fences,
            active_bodies,
        }
    }
}

impl ReconstructionClaimState {
    pub(crate) fn register(
        self: &Arc<Self>,
        start_index: OplogIndex,
        wallet: Option<Weak<OwnerWallet>>,
    ) -> HistoricalReconstruction {
        if let Some(wallet) = wallet.as_ref().and_then(Weak::upgrade) {
            wallet.register_replayed_entity(start_index);
        }
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
                wallet,
            }),
        }
    }

    pub(crate) fn release_incomplete(&self, start_index: OplogIndex) {
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

    /// Whether the entity body started at `start_index` is claimed and has not settled yet.
    pub(crate) fn is_body_active(&self, start_index: OplogIndex) -> bool {
        self.active_bodies.borrow().contains(&start_index)
    }

    #[cfg(test)]
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

    pub(crate) fn subscribe_fences(&self) -> watch::Receiver<HashSet<OplogIndex>> {
        self.active_fences.subscribe()
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
        if let Some(wallet) = self.wallet.as_ref().and_then(Weak::upgrade) {
            wallet.finish_replayed_entity(self.start_index);
        }
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

    #[cfg(test)]
    pub(crate) fn resolution_ready(&self) -> bool {
        !self.receiver.is_empty() || self.receiver.is_terminated()
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
