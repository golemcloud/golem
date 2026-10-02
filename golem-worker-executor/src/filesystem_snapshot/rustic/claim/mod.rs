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

//! The claim of a prune that a delete holds, from just before the write of its first marker until
//! the final marker of its prune.
//!
//! When the delete stops before its prune starts, or the second read of the ledger finds another
//! time of the last prune than the first read, the claim is released: the first marker and each
//! refresh marker whose write succeeded are deleted, and the claim when this delete knows that it
//! wrote it. The marker of a refresh write that is in flight when the refresh stops, at the end of
//! the prune or at a drop, or that lands and loses its answer, is not deleted. Such a marker only
//! delays a prune. When the delete is dropped after the prune started and before the final marker
//! is written, the final marker is written, and the claim is not released. A drop runs them in a
//! task, so they also run when the caller drops the delete. After the start, only the delete
//! releases the claim, when each attempt of the prune found a snapshot file gone, because such a
//! prune changed nothing and counts as a prune that did not run.
//!
//! [`transition`] holds these rules. The claim and the blocking task of its prune share the state,
//! and each event replaces the state with the result of [`transition`] in one step under a lock. So
//! the start of the prune and a drop of the delete give one of two orders: a released claim never
//! starts a prune, and a drop after the prune started does not release the claim.

use super::files::{Lease, SnapshotFiles};
use super::prune::{
    ClaimName, PrunePolicy, keep_claim_fresh, lease_span, marker_path, marker_time, refresh_period,
    release_claim, take_claim, write_marker,
};
use super::spawner::Spawner;
use crate::filesystem_snapshot::clock::Clock;
use futures::StreamExt;
use golem_common::model::Timestamp;
use std::future::ready;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::task::task_tracker::TaskTrackerToken;
use tracing::warn;

/// The markers of a claim that this delete keeps: the first marker, which `take_claim` also uses,
/// and each refresh marker whose write succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Markers {
    first: Arc<Path>,
    refreshed: Vec<Box<Path>>,
}

/// What a delete still owes the claim.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClaimState {
    /// The first marker is written or being written, and this delete does not know that it wrote
    /// the claim.
    Marking { markers: Markers },
    /// This delete wrote the claim, and its prune did not start.
    Claimed { markers: Markers },
    /// The prune started, so the claim needs its final marker. It stays, unless each attempt of
    /// the prune found a snapshot file gone.
    Started { markers: Markers },
    /// The claim needs nothing more from this delete.
    Ended,
}

/// Something that happened to a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClaimEvent {
    /// The write of the claim succeeded.
    Won,
    /// The claim write found a claim at its path, so this delete does not hold the claim, and it
    /// tried to delete its marker. A marker that stays only delays a prune.
    Lost,
    /// The blocking task of the prune asks to start the rustic prune.
    Start,
    /// A refresh wrote the marker.
    Refreshed(Box<Path>),
    /// No prune ran: an error came before the prune, the second read of the ledger found another
    /// time of the last prune than the first read, or each attempt of the prune found a snapshot
    /// file gone.
    Release,
    /// The blocking task of the prune ended, and the delete writes the final marker when the prune
    /// started. It also comes before the start, when the blocking task failed before it started the
    /// prune.
    Finish,
    /// The final marker is written.
    FinalMarkerWritten,
    /// The delete dropped the claim.
    Dropped,
}

/// The storage work that a claim still needs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cleanup {
    /// Delete the claim when `claimed`, then the first marker, and then each refresh marker, by
    /// name.
    Release {
        claimed: bool,
        first: Arc<Path>,
        refreshed: Box<[Box<Path>]>,
    },
    /// Write the final marker, with the time of the write.
    FinalMarker,
}

/// Gives the state of the claim after the event, and the cleanup that the event needs.
///
/// - A won claim write moves `Marking` to `Claimed`. A lost one ends the claim with no cleanup,
///   because `take_claim` already tried to delete the marker.
/// - A drop before the prune started releases the claim. The claim itself is deleted only when
///   this delete knows that it wrote it, which is after `Won`.
/// - The prune starts only from `Claimed`. A start after the release does nothing, and the prune
///   does not run.
/// - A drop after the start writes the final marker and keeps the claim.
/// - A release after `Won`, before or after the start, deletes the claim and each marker that the
///   state holds. After the start, the delete releases only when each attempt of the prune found
///   a snapshot file gone. A release in `Marking` deletes only the markers.
/// - A finish after the start writes the final marker, and the state stays `Started` until the
///   write succeeded, so a drop after a failed write tries again. A finish before the start does
///   nothing: the blocking task failed before it started the prune, and the drop releases the
///   claim.
/// - Each refresh marker whose write succeeded before the claim ended is kept, so a release
///   deletes it.
/// - Each other event keeps the state and needs no cleanup.
fn transition(state: ClaimState, event: ClaimEvent) -> (ClaimState, Option<Cleanup>) {
    use ClaimEvent as E;
    use ClaimState as S;
    let release = |claimed, Markers { first, refreshed }: Markers| {
        Some(Cleanup::Release {
            claimed,
            first,
            refreshed: refreshed.into_boxed_slice(),
        })
    };
    match (state, event) {
        (S::Marking { markers }, E::Won) => (S::Claimed { markers }, None),
        (S::Marking { .. }, E::Lost) => (S::Ended, None),
        (S::Marking { markers }, E::Release | E::Dropped) => (S::Ended, release(false, markers)),
        (S::Claimed { markers }, E::Start) => (S::Started { markers }, None),
        (S::Claimed { markers }, E::Release | E::Dropped) => (S::Ended, release(true, markers)),
        (S::Started { markers }, E::Release) => (S::Ended, release(true, markers)),
        (S::Started { markers }, E::Finish) => (S::Started { markers }, Some(Cleanup::FinalMarker)),
        (S::Started { .. }, E::FinalMarkerWritten) => (S::Ended, None),
        (S::Started { .. }, E::Dropped) => (S::Ended, Some(Cleanup::FinalMarker)),
        (S::Marking { markers }, E::Refreshed(marker)) => (
            S::Marking {
                markers: kept(markers, marker),
            },
            None,
        ),
        (S::Claimed { markers }, E::Refreshed(marker)) => (
            S::Claimed {
                markers: kept(markers, marker),
            },
            None,
        ),
        (S::Started { markers }, E::Refreshed(marker)) => (
            S::Started {
                markers: kept(markers, marker),
            },
            None,
        ),
        (state, _) => (state, None),
    }
}

/// Gives the markers with the refresh marker added.
fn kept(mut markers: Markers, marker: Box<Path>) -> Markers {
    markers.refreshed.push(marker);
    markers
}

/// Applies the event to the shared state in one step under its lock. Gives whether this event
/// started the prune, which is a move from `Claimed` to `Started`, and the cleanup that the event
/// needs. The lock is never held across an await or while a cleanup runs.
fn apply(state: &Mutex<ClaimState>, event: ClaimEvent) -> (bool, Option<Cleanup>) {
    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let previous = std::mem::replace(&mut *state, ClaimState::Ended);
    let claimed = matches!(previous, ClaimState::Claimed { .. });
    let (next, cleanup) = transition(previous, event);
    let started = claimed && matches!(next, ClaimState::Started { .. });
    *state = next;
    (started, cleanup)
}

impl Cleanup {
    /// Runs the cleanup on the claim, and tells whether it completed. A release always completes,
    /// because each failed delete only gives a warning. A final marker completes when its write
    /// succeeded.
    async fn run(self, files: &SnapshotFiles, claim: &ClaimName, clock: &dyn Clock) -> bool {
        match self {
            Self::Release {
                claimed,
                first,
                refreshed,
            } => {
                let markers = std::iter::once(&*first).chain(refreshed.iter().map(Box::as_ref));
                release_claim(files, &claim.directory, claim.number, claimed, markers).await;
                true
            }
            Self::FinalMarker => write_final_marker(files, claim, clock.now()).await,
        }
    }
}

/// Writes the final marker of the claim at the time, and tells whether the write succeeded. A
/// failed write gives a warning.
async fn write_final_marker(files: &SnapshotFiles, claim: &ClaimName, time: Timestamp) -> bool {
    write_marker(files, "final_marker", &claim.directory, claim.number, time)
        .await
        .inspect_err(|error| {
            warn!(
                error = %format!("{error:#}"),
                "Failed to write the final marker of the prune claim of a filesystem snapshot scope"
            );
        })
        .is_ok()
}

/// The claim of a prune that this delete holds.
///
/// It exists before the write of the first marker. The spawner runs each cleanup that a drop or a
/// release needs, and the token of the tracker that the claim holds stays until that task is
/// counted, so `shut_down` waits for the claim and for each of its cleanups.
pub(super) struct Claim {
    name: ClaimName,
    /// The lease of the prune. It ends one lease span after the `Instant` of the first marker. A
    /// refresh write that succeeds and that started before the end moves the end to one span after
    /// its own start, when that is later. Nothing else moves it.
    lease: Arc<Lease>,
    /// The time from the start of a marker write to the end of the lease that it gives.
    span: Duration,
    /// The time between two markers while the prune runs.
    refresh: Duration,
    state: Arc<Mutex<ClaimState>>,
    /// The blobs of the scope, with a token that nothing cancels and no lease, so a release and a
    /// final marker also run after a cancel or a drop.
    files: SnapshotFiles,
    /// Gives the time of each marker.
    clock: Arc<dyn Clock>,
    spawner: Spawner,
    _tracked: TaskTrackerToken,
}

/// The part of a claim that the blocking task of its prune holds.
pub(super) struct ClaimStart(Arc<Mutex<ClaimState>>);

impl ClaimStart {
    /// Starts the prune, and tells whether it may run. A claim that the delete released does not
    /// start, and a claim whose prune started does not start again. So one claim starts at most
    /// one prune.
    pub(super) fn start(self) -> bool {
        apply(&self.0, ClaimEvent::Start).0
    }
}

impl Claim {
    /// Writes the first marker of the claim, then takes the claim, and gives the claim when this
    /// delete holds it. The lease starts with the marker write. [`marker_time`] reads an `Instant`
    /// and the time in the name of the marker. The lease ends one lease span after that `Instant`.
    /// The claim exists before the marker write. So a drop or an error during the marker write or
    /// the claim write deletes the first marker.
    pub(super) async fn take(
        files: &SnapshotFiles,
        name: ClaimName,
        policy: &PrunePolicy,
        clock: Arc<dyn Clock>,
        spawner: Spawner,
        tracked: TaskTrackerToken,
    ) -> anyhow::Result<Option<Claim>> {
        let span = lease_span(policy.grace, policy.deadline);
        let (started, time) = marker_time(&*clock);
        // The state and `take_claim` both use the first marker.
        let first: Arc<Path> = marker_path(&name.directory, name.number, time).into();
        let claim = Claim {
            lease: Arc::new(Lease::until(started + span)),
            span,
            refresh: refresh_period(policy.grace, policy.deadline),
            state: Arc::new(Mutex::new(ClaimState::Marking {
                markers: Markers {
                    first: first.clone(),
                    refreshed: Vec::new(),
                },
            })),
            files: files.detached(),
            clock,
            spawner,
            _tracked: tracked,
            name,
        };
        let won = take_claim(files, &claim.name, &first).await?;
        apply(
            &claim.state,
            if won {
                ClaimEvent::Won
            } else {
                ClaimEvent::Lost
            },
        );
        Ok(won.then_some(claim))
    }

    /// Gives the directory of the claims of the ledger of the claim.
    pub(super) fn directory(&self) -> &Path {
        &self.name.directory
    }

    /// Gives the same blobs, whose calls the lease of the claim fences.
    pub(super) fn leased(&self, files: &SnapshotFiles) -> SnapshotFiles {
        files.leased(self.lease.clone())
    }

    /// Gives the part of the claim that the blocking task of the prune uses to start the prune.
    pub(super) fn start(&self) -> ClaimStart {
        ClaimStart(self.state.clone())
    }

    /// Writes a new marker of the claim at each refresh period through the files, and keeps each
    /// marker whose write succeeded in the claim. It ends when the operation of the files is
    /// cancelled, or when the caller drops it.
    pub(super) fn keep_fresh<'a>(
        &'a self,
        files: &'a SnapshotFiles,
    ) -> impl Future<Output = ()> + 'a {
        keep_claim_fresh(
            files,
            &self.name,
            self.refresh,
            &self.lease,
            self.span,
            &*self.clock,
        )
        .for_each(move |marker| {
            apply(&self.state, ClaimEvent::Refreshed(marker));
            ready(())
        })
    }

    /// Releases the claim and waits for the release. The release runs in a task, so it also ends
    /// when the caller drops the delete.
    pub(super) async fn release(&self) {
        if let (_, Some(cleanup)) = apply(&self.state, ClaimEvent::Release)
            && let Err(error) = self.spawn(cleanup).await
        {
            warn!(
                error = %error,
                "The release of the prune claim of a filesystem snapshot scope did not end"
            );
        }
    }

    /// Writes the final marker of a prune that started. A claim whose prune did not start writes
    /// nothing. When the write fails, the drop of the claim tries it again.
    pub(super) async fn finish(&self) {
        if let (_, Some(cleanup)) = apply(&self.state, ClaimEvent::Finish)
            && cleanup.run(&self.files, &self.name, &*self.clock).await
        {
            apply(&self.state, ClaimEvent::FinalMarkerWritten);
        }
    }

    /// Runs the cleanup in a task of the spawner.
    fn spawn(&self, cleanup: Cleanup) -> JoinHandle<()> {
        let (files, name, clock) = (self.files.clone(), self.name.clone(), self.clock.clone());
        self.spawner.spawn(async move {
            cleanup.run(&files, &name, &*clock).await;
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let (_, Some(cleanup)) = apply(&self.state, ClaimEvent::Dropped) {
            drop(self.spawn(cleanup));
        }
    }
}

#[cfg(test)]
mod tests;
