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

//! One job: upload, discard, confirm, then a delete of the older snapshots or of the superseded
//! one. Each step is a straight line that asks a rule of `rules` what to do next.

use super::registry::JobTicket;
use super::rules::{self, FollowUp, SaveAttempt};
use super::{
    Admission, CapturedTree, Confirm, Core, JobDecision, SavedUpdate, SnapshotKind, UploadNowError,
    retention, store_name,
};
use crate::filesystem_snapshot::{ChangeDetection, SnapshotInfo, SnapshotName, SnapshotStoreError};
use futures::StreamExt as _;
use golem_common::model::RetryConfig;
use golem_common::model::oplog::FilesystemSnapshotName;
use rand::Rng as _;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{OwnedSemaphorePermit, watch};

/// What the upload of a job gave.
enum UploadOutcome {
    /// The store holds the snapshot.
    Saved(SnapshotInfo),
    /// The save failed, after the retries when the error allows them.
    Failed(SnapshotStoreError),
    /// A shutdown, a call of `delete_all_snapshots` for the agent, or the stop of the caller
    /// stopped the upload.
    Stopped,
}

/// A slot of the uploads that one store attempt of an upload holds. It counts the attempt in the
/// gauge of the uploads in progress while it lives, so a stop that drops the attempt during its
/// store call leaves the gauge right.
struct UploadSlot {
    _permit: OwnedSemaphorePermit,
    #[cfg(test)]
    attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl UploadSlot {
    fn new(permit: OwnedSemaphorePermit, core: &Core) -> Self {
        crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
        #[cfg(test)]
        core.upload_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let _ = core;
        Self {
            _permit: permit,
            #[cfg(test)]
            attempts: Arc::clone(&core.upload_attempts),
        }
    }
}

impl Drop for UploadSlot {
    fn drop(&mut self) {
        crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
        #[cfg(test)]
        self.attempts
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Saves the tree of `admission`, discards it, and confirms, as [`Admission::submit`] says.
pub(super) async fn run_job(
    admission: Admission,
    tree: CapturedTree,
    parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
    confirm: Confirm,
) {
    let Admission {
        core,
        ticket,
        name,
        kind,
    } = admission;
    let started = Instant::now();
    let saved = upload(
        &core,
        &ticket,
        &name,
        parent,
        &tree.directory,
        std::future::pending(),
    )
    .await;
    tree.discard.await;
    let info = match saved {
        UploadOutcome::Saved(info) => info,
        UploadOutcome::Failed(error) => {
            ticket.decide(JobDecision::SaveFailed);
            tracing::warn!(
                error = %error,
                name = %name,
                "Failed to upload a filesystem snapshot; no confirmation record follows its record"
            );
            crate::metrics::filesystem_snapshots::record_upload(
                kind.label(),
                "failed",
                started.elapsed(),
            );
            return;
        }
        UploadOutcome::Stopped => return,
    };
    crate::metrics::filesystem_snapshots::record_uploaded_bytes(kind.label(), info.bytes);
    if ticket.stop_requested() {
        return;
    }
    let outcome = tokio::select! {
        outcome = confirm(name.clone()) => outcome,
        () = ticket.until_stopped() => return,
    };
    ticket.decide(JobDecision::Confirmed(outcome));
    crate::metrics::filesystem_snapshots::record_upload(
        kind.label(),
        outcome.label(),
        started.elapsed(),
    );
    match rules::follow_up(kind, outcome) {
        FollowUp::DeleteOlder => {
            delete_older_snapshots(&core, &ticket, &name, kind, &info, None).await
        }
        FollowUp::DeleteSuperseded => {
            crate::metrics::filesystem_snapshots::record_dropped_confirmation(outcome.label());
            delete_superseded(&core, &ticket, &name).await;
        }
        FollowUp::Keep => {}
    }
}

impl Admission {
    /// Uploads `tree` while the caller waits, and gives the [`SavedUpdate`] for after the commit
    /// of the record. It differs from [`Admission::submit`] only in who waits and in what
    /// follows: both go through the same upload. A manual update calls this before it writes its
    /// record. The agent stays reserved until the [`SavedUpdate`] deletes the older snapshots or
    /// is dropped. When `stop` reports a terminal interrupt, or a shutdown or a call of
    /// `delete_all_snapshots` for the agent stops the upload, the tree is discarded and the call
    /// gives [`UploadNowError::Stopped`].
    pub(crate) async fn upload_now(
        self,
        tree: CapturedTree,
        stop: watch::Receiver<bool>,
    ) -> Result<SavedUpdate, UploadNowError> {
        let Admission {
            core,
            ticket,
            name,
            kind,
        } = self;
        let started = Instant::now();
        let saved = upload(
            &core,
            &ticket,
            &name,
            None,
            &tree.directory,
            interrupt_raised(stop),
        )
        .await;
        tree.discard.await;
        let result = match saved {
            UploadOutcome::Saved(info) => Ok(info),
            UploadOutcome::Failed(error) => Err(UploadNowError::Store(error)),
            UploadOutcome::Stopped => Err(UploadNowError::Stopped),
        };
        match &result {
            Ok(info) => {
                crate::metrics::filesystem_snapshots::record_upload(
                    kind.label(),
                    "saved",
                    started.elapsed(),
                );
                crate::metrics::filesystem_snapshots::record_uploaded_bytes(
                    kind.label(),
                    info.bytes,
                );
            }
            Err(UploadNowError::Store(_)) => {
                crate::metrics::filesystem_snapshots::record_upload(
                    kind.label(),
                    "failed",
                    started.elapsed(),
                );
            }
            Err(UploadNowError::Stopped) => {}
        }
        result.map(|info| SavedUpdate {
            core,
            ticket,
            name,
            info,
        })
    }
}

/// Completes when `stop` reports a terminal interrupt. It never completes when the sender is
/// gone.
pub(super) async fn interrupt_raised(mut stop: watch::Receiver<bool>) {
    if stop.wait_for(|raised| *raised).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Uploads the tree of a job: saves the tree under the own name of the job with retries. Each
/// attempt waits for a slot of the uploads and holds it for one store `save`, and for the `stat`
/// of the own name when [`rules::save_attempt`] asks for it. The attempt gives the slot back
/// before the wait for the next attempt. `stop`, a shutdown, or a call of `delete_all_snapshots`
/// for the agent ends the upload with `Stopped`, and no store call starts after the attempt sees
/// such a stop.
async fn upload(
    core: &Core,
    ticket: &JobTicket,
    name: &FilesystemSnapshotName,
    parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
    tree: &Path,
    stop: impl Future<Output = ()> + Send,
) -> UploadOutcome {
    let stop = futures::FutureExt::shared(futures::FutureExt::boxed(stop));
    let (name, parent) = match (
        store_name(name),
        parent
            .map(|(name, detection)| store_name(&name).map(|name| (name, detection)))
            .transpose(),
    ) {
        (Ok(name), Ok(parent)) => (name, parent),
        (Err(error), _) | (_, Err(error)) => {
            return UploadOutcome::Failed(SnapshotStoreError::Storage {
                retryable: false,
                source: anyhow::Error::new(error),
            });
        }
    };
    let parent = parent.as_ref().map(|(name, detection)| (name, *detection));
    let agent = ticket.agent();
    // The stops that the `select!` below watches. An attempt that sees one after its grant gives
    // its slot back and waits for that `select!` to end the upload in the same poll.
    let stopped =
        || ticket.stop_requested() || futures::FutureExt::now_or_never(stop.clone()).is_some();
    let attempt = || async {
        let slot = match Arc::clone(&core.uploads).acquire_owned().await {
            Ok(permit) => UploadSlot::new(permit, core),
            Err(_) => {
                // The slots are gone: the job stops, and the `select!` below ends the upload.
                ticket.stop_job();
                return std::future::pending().await;
            }
        };
        ticket.saving();
        if stopped() {
            drop(slot);
            return std::future::pending().await;
        }
        let saved = match rules::save_attempt(core.store.save(agent, &name, tree, parent).await) {
            SaveAttempt::Saved(info) => Ok(info),
            SaveAttempt::StatOwn => {
                if stopped() {
                    drop(slot);
                    return std::future::pending().await;
                }
                core.store.stat(agent, &name).await.and_then(|found| {
                    found.ok_or_else(|| SnapshotStoreError::Storage {
                        retryable: true,
                        source: anyhow::anyhow!(
                            "the filesystem snapshot {name} exists at the save and not after it"
                        ),
                    })
                })
            }
            SaveAttempt::Failed(error) => Err(error),
        };
        drop(slot);
        saved
    };
    let saved = tokio::select! {
        biased;
        saved = retrying(core.settings.upload_retry(), attempt) => Some(saved),
        () = ticket.until_stopped() => None,
        () = stop.clone() => None,
    };
    match saved {
        Some(Ok(info)) => UploadOutcome::Saved(info),
        Some(Err(error)) => UploadOutcome::Failed(error),
        None => UploadOutcome::Stopped,
    }
}

/// Waits for a slot of the uploads for a delete of the job. Gives `None` when the stop of the
/// deletes of the job ends the wait, is seen after the grant, or when the slots are gone.
async fn delete_slot(core: &Core, ticket: &JobTicket) -> Option<OwnedSemaphorePermit> {
    let slot = tokio::select! {
        biased;
        () = ticket.until_deletes_stopped() => return None,
        slot = Arc::clone(&core.uploads).acquire_owned() => slot.ok()?,
    };
    (!ticket.deletes_stop_requested()).then_some(slot)
}

/// Keeps the own snapshot and the newest older snapshots of its kind, and deletes the rest of
/// its kind that are older than it, under its own slot of the uploads. `info` is the info of the
/// own snapshot, and `kept` a snapshot that it never deletes. The stop of the deletes of the job
/// ends it at once, also in its wait for the slot and between two attempts, and a later
/// retention deletes what it left.
pub(super) async fn delete_older_snapshots(
    core: &Core,
    ticket: &JobTicket,
    name: &FilesystemSnapshotName,
    kind: SnapshotKind,
    info: &SnapshotInfo,
    kept: Option<&SnapshotName>,
) {
    let Some(_slot) = delete_slot(core, ticket).await else {
        return;
    };
    let agent = ticket.agent();
    let retention = async {
        let Ok(own) = store_name(name) else {
            return;
        };
        let listing = match core.store.list(agent).await {
            Ok(listing) => listing,
            Err(error) => {
                tracing::warn!(error = %error, "Failed to list the filesystem snapshots for retention");
                return;
            }
        };
        let keep = match kind {
            SnapshotKind::Periodic => core.settings.retained_periodic_snapshots(),
            SnapshotKind::Update => core.settings.retained_update_snapshots(),
        };
        let victims = retention::victims(&listing, &own, info, keep.get(), kept);
        if victims.is_empty() {
            return;
        }
        if let Err(error) = retrying(core.settings.upload_retry(), || {
            core.store.delete(agent, &victims)
        })
        .await
        {
            tracing::warn!(
                error = %error,
                names = ?victims,
                "Failed to delete old filesystem snapshots; the next retention tries again"
            );
        }
    };
    tokio::select! {
        biased;
        () = ticket.until_deletes_stopped() => {}
        () = retention => {}
    }
}

/// Deletes the snapshot of the job, which no confirmation record names, under its own slot of
/// the uploads. The stop of the deletes of the job ends it at once, and the snapshot stays until
/// a retention of its kind deletes it.
async fn delete_superseded(core: &Core, ticket: &JobTicket, name: &FilesystemSnapshotName) {
    let Some(_slot) = delete_slot(core, ticket).await else {
        return;
    };
    let agent = ticket.agent();
    let delete = async {
        let Ok(name) = store_name(name) else {
            return;
        };
        let names = [name];
        if let Err(error) = retrying(core.settings.upload_retry(), || {
            core.store.delete(agent, &names)
        })
        .await
        {
            tracing::warn!(
                error = %error,
                name = %names[0],
                "Failed to delete a filesystem snapshot that no confirmation record names"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete");
        }
    };
    tokio::select! {
        biased;
        () = ticket.until_deletes_stopped() => {}
        () = delete => {}
    }
}

/// Draws the jitter factor of a retry delay below the `max_jitter_factor` of `retry`. The
/// settings allow a factor from 0 to 1, and a factor of 0 gives no jitter.
fn jitter(retry: &RetryConfig) -> f64 {
    retry
        .max_jitter_factor
        .filter(|factor| *factor > 0.0)
        .map_or(0.0, |factor| rand::rng().random_range(0.0..factor))
}

/// Runs `operation`, and runs it again after the delay that [`rules::retry_delay`] gives while
/// it fails.
pub(super) async fn retrying<T, Operation, Attempt>(
    retry: &RetryConfig,
    operation: Operation,
) -> Result<T, SnapshotStoreError>
where
    Operation: Fn() -> Attempt,
    Attempt: Future<Output = Result<T, SnapshotStoreError>>,
{
    let operation = &operation;
    futures::stream::unfold(Some(1u32), |attempt| async move {
        let attempt = attempt?;
        let result = operation().await;
        let next = match &result {
            Err(error) => match rules::retry_delay(retry, attempt, error, jitter(retry)) {
                Some(delay) => {
                    tokio::time::sleep(delay).await;
                    Some(attempt + 1)
                }
                None => None,
            },
            Ok(_) => None,
        };
        Some((result, next))
    })
    .fold(None, |_, result| async move { Some(result) })
    .await
    .unwrap_or_else(|| {
        Err(SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::anyhow!("the store operation made no attempt"),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn a_jitter_is_drawn_below_a_positive_factor_and_is_zero_otherwise() {
        let retry = |max_jitter_factor| RetryConfig {
            max_attempts: 3,
            min_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            multiplier: 4.0,
            max_jitter_factor,
        };
        let drawn = (0..200)
            .map(|_| jitter(&retry(Some(0.5))))
            .collect::<Vec<_>>();

        assert_eq!(
            (jitter(&retry(None)), jitter(&retry(Some(0.0)))),
            (0.0, 0.0)
        );
        assert!(drawn.iter().all(|factor| (0.0..0.5).contains(factor)));
        assert!(drawn.iter().any(|factor| *factor > 0.0));
    }
}
