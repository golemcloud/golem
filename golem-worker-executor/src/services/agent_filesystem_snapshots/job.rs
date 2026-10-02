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

//! One job: upload, confirm, then a delete of the older snapshots or of the superseded one. Each
//! step is a straight line that asks a rule of `rules` what to do next, and the store calls go
//! through `store_calls`.

use super::registry::JobTicket;
use super::rules::{self, FollowUp};
use super::store_calls::{Deleted, Stops, Upload, UploadError};
use super::{
    Admission, CapturedTree, Confirm, Core, JobDecision, SavedUpdate, SnapshotKind, UploadNowError,
    retention, store_name,
};
use crate::filesystem_snapshot::{ChangeDetection, SnapshotInfo, SnapshotName, SnapshotStoreError};
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

/// The store names of the own name and of the parent of a job, or the error of a name that breaks
/// a rule of the store.
fn store_names(
    name: &FilesystemSnapshotName,
    parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
) -> Result<(SnapshotName, Option<(SnapshotName, ChangeDetection)>), SnapshotStoreError> {
    let parent = parent
        .map(|(name, detection)| store_name(&name).map(|name| (name, detection)))
        .transpose();
    match (store_name(name), parent) {
        (Ok(name), Ok(parent)) => Ok((name, parent)),
        (Err(error), _) | (_, Err(error)) => Err(SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::Error::new(error),
        }),
    }
}

/// Saves the tree of `admission`, and confirms, as [`Admission::submit`] says.
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
        deadline: _,
    } = admission;
    let started = Instant::now();
    let uploaded = match store_names(&name, parent) {
        Ok((own, parent)) => {
            core.store
                .upload(&ticket, &own, tree, parent, Stops::none(), None)
                .await
        }
        Err(error) => {
            tree.discard.await;
            Upload::Failed(UploadError::Store(error))
        }
    };
    let info = match uploaded {
        Upload::Saved(info) => info,
        Upload::Failed(error) => {
            ticket.decide(JobDecision::SaveFailed);
            tracing::warn!(
                error = ?error,
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
        Upload::Stopped => return,
    };
    crate::metrics::filesystem_snapshots::record_uploaded_bytes(kind.label(), info.bytes);
    if ticket.stop_requested() {
        return;
    }
    let confirmation = tokio::select! {
        confirmation = confirm(name.clone()) => confirmation,
        () = ticket.until_stopped() => return,
    };
    let outcome = confirmation.outcome();
    ticket.decide(JobDecision::Confirmed(outcome));
    crate::metrics::filesystem_snapshots::record_upload(
        kind.label(),
        outcome.label(),
        started.elapsed(),
    );
    let Ok(own) = store_name(&name) else {
        return;
    };
    match rules::follow_up(kind, outcome) {
        FollowUp::DeleteOlder => {
            let kept = confirmation
                .selectable()
                .iter()
                .filter_map(|name| store_name(name).ok())
                .collect::<Box<[_]>>();
            delete_older_snapshots(&core, &ticket, &own, kind, &info, &kept).await
        }
        FollowUp::DeleteSuperseded => {
            crate::metrics::filesystem_snapshots::record_dropped_confirmation(outcome.label());
            delete_superseded(&core, &ticket, own).await;
        }
        FollowUp::Keep => {}
    }
}

impl Admission {
    /// Uploads `tree` while the caller waits, and gives the [`SavedUpdate`] for after the commit
    /// of the record. It differs from [`Admission::submit`] only in who waits and in what
    /// follows: both go through the same upload, in a task of the jobs. A manual update calls
    /// this before it writes its record. The agent stays reserved until the [`SavedUpdate`]
    /// deletes the older snapshots or is dropped.
    ///
    /// A terminal interrupt that `stop` reports, a caller that stops waiting, a lost shard that
    /// `lost_shard` reports, a shutdown, or a delete of all snapshots of the agent gives
    /// [`UploadNowError::Stopped`] at once and ends the admission. A save that runs then goes on
    /// until it returns, and the tree is discarded after it; a lost shard also cancels that save
    /// in the store, so it publishes nothing. The waits for a running save of the agent and for a
    /// slot end at the deadline of the admission with [`UploadNowError::SaveRunning`] or
    /// [`UploadNowError::NoSlot`].
    pub(crate) async fn upload_now(
        self,
        tree: CapturedTree,
        stop: watch::Receiver<bool>,
        lost_shard: watch::Receiver<bool>,
    ) -> Result<SavedUpdate, UploadNowError> {
        let (answer, answered) = oneshot::channel();
        let caller_gone = CancellationToken::new();
        let jobs = self.core.jobs.clone();
        let stops = Stops::of(
            {
                let caller_gone = caller_gone.clone();
                async move {
                    tokio::select! {
                        () = interrupt_raised(stop) => {}
                        () = caller_gone.cancelled() => {}
                    }
                }
            },
            lost_shard,
        );
        jobs.spawn(async move {
            let _ = answer.send(self.upload_in_job(tree, stops).await);
        });
        // A caller that stops waiting drops this guard, which stops the upload.
        let guard = caller_gone.drop_guard();
        let answer = answered_now(answered.await);
        guard.disarm();
        answer
    }

    /// The upload of [`Admission::upload_now`], in its job task.
    async fn upload_in_job(
        self,
        tree: CapturedTree,
        stops: Stops,
    ) -> Result<SavedUpdate, UploadNowError> {
        let Admission {
            core,
            ticket,
            name,
            kind,
            deadline,
        } = self;
        let started = Instant::now();
        let uploaded = match store_names(&name, None) {
            Ok((own, _)) => {
                core.store
                    .upload(&ticket, &own, tree, None, stops, deadline)
                    .await
            }
            Err(error) => {
                tree.discard.await;
                Upload::Failed(UploadError::Store(error))
            }
        };
        let result = match uploaded {
            Upload::Saved(info) => Ok(info),
            Upload::Failed(UploadError::Store(error)) => Err(UploadNowError::Store(error)),
            Upload::Failed(UploadError::SaveRunning) => Err(UploadNowError::SaveRunning),
            Upload::Failed(UploadError::NoSlot) => Err(UploadNowError::NoSlot),
            Upload::Stopped => Err(UploadNowError::Stopped),
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
            Err(
                UploadNowError::Store(_) | UploadNowError::SaveRunning | UploadNowError::NoSlot,
            ) => {
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

/// The answer of an upload that a job task gave through its channel. A job task that is gone
/// without an answer stopped the upload.
fn answered_now<T>(
    answered: Result<Result<T, UploadNowError>, oneshot::error::RecvError>,
) -> Result<T, UploadNowError> {
    answered.unwrap_or(Err(UploadNowError::Stopped))
}

/// Completes when `stop` reports a terminal interrupt. It never completes when the sender is
/// gone.
pub(super) async fn interrupt_raised(mut stop: watch::Receiver<bool>) {
    if stop.wait_for(|raised| *raised).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Keeps the own snapshot `own` and the newest older snapshots of its kind, and deletes the rest
/// of its kind that are older than it. `info` is the info of the own snapshot, and `kept` the
/// snapshots that it never deletes and does not count. The listing and the delete each take a
/// slot of the uploads. The stop of the deletes of the job ends it at once, also in its waits; a
/// delete that the store already runs goes on, and a later retention deletes what it left.
pub(super) async fn delete_older_snapshots(
    core: &Core,
    ticket: &JobTicket,
    own: &SnapshotName,
    kind: SnapshotKind,
    info: &SnapshotInfo,
    kept: &[SnapshotName],
) {
    let agent = ticket.agent();
    let retention = async {
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
        let victims: Arc<[SnapshotName]> =
            retention::victims(&listing, own, info, keep.get(), kept).into();
        if victims.is_empty() {
            return;
        }
        if let Deleted::Leaked(error) = core
            .store
            .delete(agent, Arc::clone(&victims), ticket.until_deletes_stopped())
            .await
        {
            tracing::warn!(
                error = %error,
                agent = ?agent,
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

/// Deletes the snapshot `own` of the job, which no confirmation record names, under its own slot
/// of the uploads. The stop of the deletes of the job ends it at once, and the snapshot stays
/// until a retention of its kind deletes it.
async fn delete_superseded(core: &Core, ticket: &JobTicket, own: SnapshotName) {
    let deleted = core
        .store
        .delete(
            ticket.agent(),
            Arc::from([own.clone()]),
            ticket.until_deletes_stopped(),
        )
        .await;
    if let Deleted::Leaked(error) = deleted {
        tracing::warn!(
            error = %error,
            agent = ?ticket.agent(),
            name = %own,
            "Failed to delete a filesystem snapshot that no confirmation record names"
        );
        crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn an_upload_now_whose_job_task_is_gone_gives_stopped() {
        let (sender, receiver) = oneshot::channel::<Result<(), UploadNowError>>();
        drop(sender);
        let gone = futures::executor::block_on(receiver);

        assert!(matches!(answered_now(gone), Err(UploadNowError::Stopped)));
        assert!(matches!(answered_now(Ok(Ok(()))), Ok(())));
    }
}
