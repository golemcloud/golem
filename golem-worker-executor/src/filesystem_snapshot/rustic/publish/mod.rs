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

//! The publish of a snapshot file, and the time before which a save lands all its writes.
//!
//! In a save of the store, the backend keeps the snapshot file in a [`SnapshotStage`] and does not
//! write it. The save writes it later with [`publish`], after the blocking work returns. That
//! write is the step that makes the snapshot visible. A publish whose tries all failed does not
//! delete the file: a try that ended without an answer can still land, and the save waits for that
//! and then checks its own name.
//!
//! A prune can delete a pack that a save wrote or reused, when the prune reads the index before the
//! index file and the snapshot file of the save land. [`index_read_bound`] is the earliest time of
//! such a read after the first slot of the save, so a save lands every index file and its snapshot
//! file before it.

use super::fault::CallFailure;
use super::files::{CallAgain, SnapshotFiles, call_again};
use super::runs::{RunEnd, now};
use bytes::Bytes;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// The shortest try of a publish. A try that would be shorter does not start.
pub(super) const MIN_PUBLISH_TRY: Duration = Duration::from_secs(1);

/// Whether a save lands its writes before [`index_read_bound`]. A test with a short grace period
/// turns it off, because the bound would fail every save of such a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublishBound {
    On,
    #[cfg(test)]
    Off,
}

/// A snapshot file that the backend kept and did not write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StagedSnapshot {
    /// The path of the file, relative to the root of the namespace. Its name is the hash of the
    /// content.
    pub(super) path: Arc<Path>,
    pub(super) content: Bytes,
}

/// The place where the backend of one save keeps its snapshot file.
#[derive(Debug, Default)]
pub(super) struct SnapshotStage(Mutex<Option<StagedSnapshot>>);

impl SnapshotStage {
    /// Keeps the file. A stage holds one file, so a second file gives it back as the error.
    pub(super) fn keep(&self, staged: StagedSnapshot) -> Result<(), StagedSnapshot> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match *slot {
            Some(_) => Err(staged),
            None => {
                *slot = Some(staged);
                Ok(())
            }
        }
    }

    /// Takes the file out of the stage.
    pub(super) fn take(&self) -> Option<StagedSnapshot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Gives the earliest time at which a prune can read an index that lets it delete a pack of a save
/// whose first slot was at `t0`: `t0`, plus the grace period of a marked pack, less two storage
/// call deadlines, less one refresh period `period` of a prune claim.
pub(super) fn index_read_bound(
    t0: Instant,
    grace: Duration,
    deadline: Duration,
    period: Duration,
) -> Instant {
    t0 + grace
        .saturating_sub(deadline.saturating_mul(2))
        .saturating_sub(period)
}

/// Gives the time after which the backup of a save does not run: two storage call deadlines
/// before `bound`, so the last index write of the backup lands by one deadline before `bound` and
/// the publish has time for a try.
pub(super) fn backup_end(bound: Instant, deadline: Duration) -> Instant {
    bound
        .checked_sub(deadline.saturating_mul(2))
        .unwrap_or(bound)
}

/// Gives the cut of the next try of a publish that would start at `now`, when the tries of the
/// publish took `spent` and `bound` is the time before which the write must land: the least of the
/// time left of the window of the call and the time before `bound` less one deadline. Gives `None`
/// when that is less than [`MIN_PUBLISH_TRY`]. Without a bound, the time left of the window cuts
/// the try.
pub(super) fn publish_try(
    now: Instant,
    spent: Duration,
    bound: Option<Instant>,
    deadline: Duration,
) -> Option<Duration> {
    let window = deadline.saturating_sub(spent);
    match bound {
        Some(bound) => {
            let before_bound = bound
                .saturating_duration_since(now)
                .saturating_sub(deadline);
            Some(window.min(before_bound)).filter(|cut| *cut >= MIN_PUBLISH_TRY)
        }
        None => Some(window).filter(|cut| !cut.is_zero()),
    }
}

/// What a publish gave.
#[derive(Debug)]
pub(super) enum Published {
    /// The file is there.
    Written,
    /// The publish did not write the file.
    NotWritten {
        /// How the run of the save ends.
        end: RunEnd,
        failure: anyhow::Error,
        /// The instant after which a try that ended without an answer has landed or never lands.
        late: Option<Instant>,
    },
}

/// The state of a publish between its tries.
struct Trying {
    tried: u32,
    spent: Duration,
    /// The end of the last try that ended without an answer.
    lost: Option<Instant>,
}

/// Writes the staged file only when its path has no blob, which makes the snapshot visible. The
/// name is the hash of the content, so a blob at the path is this file, and `AlreadyExists` after
/// a try whose answer was lost counts as written. The publish has the tries of a blob call, each
/// cut by [`publish_try`] at `bound`, and it checks `cancel` before each try. The wait between two
/// tries ends at a cancel of the operation of `files` or a stop of its tries. A try that ended
/// without an answer is not undone: [`Published::NotWritten`] gives the instant after which it has
/// landed or never lands.
pub(super) async fn publish(
    files: &SnapshotFiles,
    staged: &StagedSnapshot,
    bound: Option<Instant>,
    cancel: &CancellationToken,
) -> Published {
    let deadline = files.deadline();
    let late = |lost: Option<Instant>| lost.map(|ended| ended + deadline);
    let tries = futures::stream::unfold(
        Some(Trying {
            tried: 1,
            spent: Duration::ZERO,
            lost: None,
        }),
        |state| async move {
            let trying = state?;
            if cancel.is_cancelled() {
                return Some((
                    Some(Published::NotWritten {
                        end: RunEnd::Cancelled,
                        failure: anyhow::anyhow!(
                            "the save of the filesystem snapshot was cancelled before its publish"
                        ),
                        late: late(trying.lost),
                    }),
                    None,
                ));
            }
            let started = now();
            let Some(cut) = publish_try(started, trying.spent, bound, deadline) else {
                return Some((
                    Some(Published::NotWritten {
                        end: RunEnd::BoundPassed,
                        failure: anyhow::anyhow!(
                            "the publish of the filesystem snapshot has no time left for a try"
                        ),
                        late: late(trying.lost),
                    }),
                    None,
                ));
            };
            let written = files
                .put_if_absent_once_within("publish", &staged.path, &staged.content, cut)
                .await;
            let ended = now();
            let error = match written {
                Ok(_) => return Some((Some(Published::Written), None)),
                Err(error) => error,
            };
            let failure = super::fault::call_failure(&error);
            let lost = if failure == CallFailure::Permanent {
                trying.lost
            } else {
                Some(ended)
            };
            let spent = trying.spent + ended.saturating_duration_since(started);
            match call_again(trying.tried, files.tries(), failure, spent, deadline) {
                CallAgain::After { wait, .. } if files.wait_between_tries(wait).await => Some((
                    None,
                    Some(Trying {
                        tried: trying.tried + 1,
                        spent,
                        lost,
                    }),
                )),
                CallAgain::After { .. } => Some((
                    Some(Published::NotWritten {
                        end: RunEnd::Cancelled,
                        failure: error.context(
                            "the publish of the filesystem snapshot stopped between two tries",
                        ),
                        late: late(lost),
                    }),
                    None,
                )),
                CallAgain::End => Some((
                    Some(Published::NotWritten {
                        end: match failure {
                            CallFailure::Permanent => RunEnd::Permanent,
                            CallFailure::Cancelled => RunEnd::Cancelled,
                            CallFailure::LeaseExpired
                            | CallFailure::TimedOut
                            | CallFailure::Failed => RunEnd::CallFailed,
                        },
                        failure: error,
                        late: late(lost),
                    }),
                    None,
                )),
            }
        },
    );
    futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
        tries,
        |published| async move { published }
    )))
    .await
    .unwrap_or(Published::NotWritten {
        end: RunEnd::CallFailed,
        failure: anyhow::anyhow!("the publish of the filesystem snapshot ended without an answer"),
        late: None,
    })
}

#[cfg(test)]
mod tests;
