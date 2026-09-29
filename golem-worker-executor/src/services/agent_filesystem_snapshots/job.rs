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
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{OwnedSemaphorePermit, watch};

/// What the upload of a job gave.
enum UploadOutcome {
    /// The store holds the snapshot. The permit is the slot of the upload.
    Saved(SnapshotInfo, OwnedSemaphorePermit),
    /// The save failed, after the retries when the error allows them.
    Failed(SnapshotStoreError),
    /// A shutdown, a call of `delete_all_snapshots` for the agent, or the stop of the caller
    /// stopped the upload.
    Stopped,
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
    let (info, permit) = match saved {
        UploadOutcome::Saved(info, permit) => (info, permit),
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
    if ticket.stop().is_cancelled() {
        return;
    }
    let outcome = tokio::select! {
        outcome = confirm(name.clone()) => outcome,
        () = ticket.stop().cancelled() => return,
    };
    ticket.decide(JobDecision::Confirmed(outcome));
    crate::metrics::filesystem_snapshots::record_upload(
        kind.label(),
        outcome.label(),
        started.elapsed(),
    );
    match rules::follow_up(kind, outcome) {
        FollowUp::DeleteOlder => {
            delete_older_snapshots(&core, &ticket, &name, kind, &info, None, Some(permit)).await
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
            UploadOutcome::Saved(info, _slot) => Ok(info),
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

/// Uploads the tree of a job: waits for a slot of the uploads, then saves the tree under the own
/// name of the job with retries. Each attempt is one store `save`; when [`rules::save_attempt`]
/// asks for it, the attempt then asks the store with `stat` whether it holds the own name. `stop`,
/// a shutdown, or a call of `delete_all_snapshots` for the agent ends the upload with `Stopped`.
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
    let permit = tokio::select! {
        permit = Arc::clone(&core.uploads).acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => return UploadOutcome::Stopped,
        },
        () = ticket.stop().cancelled() => return UploadOutcome::Stopped,
        () = stop.clone() => return UploadOutcome::Stopped,
    };
    crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
    ticket.saving();
    let parent = parent.as_ref().map(|(name, detection)| (name, *detection));
    let agent = ticket.agent();
    let saved = tokio::select! {
        biased;
        saved = retrying(core.settings.upload_retry(), || async {
            match rules::save_attempt(core.store.save(agent, &name, tree, parent).await) {
                SaveAttempt::Saved(info) => Ok(info),
                SaveAttempt::StatOwn => core.store.stat(agent, &name).await?.ok_or_else(|| {
                    SnapshotStoreError::Storage {
                        retryable: true,
                        source: anyhow::anyhow!(
                            "the filesystem snapshot {name} exists at the save and not after it"
                        ),
                    }
                }),
                SaveAttempt::Failed(error) => Err(error),
            }
        }) => Some(saved),
        () = ticket.stop().cancelled() => None,
        () = stop.clone() => None,
    };
    crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
    match saved {
        Some(Ok(info)) => UploadOutcome::Saved(info, permit),
        Some(Err(error)) => UploadOutcome::Failed(error),
        None => UploadOutcome::Stopped,
    }
}

/// Keeps the own snapshot and the newest older snapshots of its kind, and deletes the rest of
/// its kind that are older than it, under the slot of the uploads `slot`. Without a slot it
/// first waits for one. `info` is the info of the own snapshot, and `kept` a snapshot that it
/// never deletes. A stop ends it at once, and a later retention deletes what it left.
#[allow(clippy::too_many_arguments)]
pub(super) async fn delete_older_snapshots(
    core: &Core,
    ticket: &JobTicket,
    name: &FilesystemSnapshotName,
    kind: SnapshotKind,
    info: &SnapshotInfo,
    kept: Option<&SnapshotName>,
    slot: Option<OwnedSemaphorePermit>,
) {
    let _slot = match slot {
        Some(slot) => slot,
        None => tokio::select! {
            slot = Arc::clone(&core.uploads).acquire_owned() => match slot {
                Ok(slot) => slot,
                Err(_) => return,
            },
            () = ticket.stop().cancelled() => return,
        },
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
        futures::stream::iter(victims.iter())
            .for_each(|victim| async move {
                if let Err(error) = retrying(core.settings.upload_retry(), || {
                    core.store.delete(agent, victim)
                })
                .await
                {
                    tracing::warn!(
                        error = %error,
                        name = %victim,
                        "Failed to delete an old filesystem snapshot; the next retention tries again"
                    );
                }
            })
            .await;
    };
    tokio::select! {
        () = retention => {}
        () = ticket.stop().cancelled() => {}
    }
}

/// Deletes the snapshot of the job, which no confirmation record names. A stop ends it at once,
/// and the snapshot stays until a retention of its kind deletes it.
async fn delete_superseded(core: &Core, ticket: &JobTicket, name: &FilesystemSnapshotName) {
    let agent = ticket.agent();
    let delete = async {
        let Ok(name) = store_name(name) else {
            return;
        };
        if let Err(error) = retrying(core.settings.upload_retry(), || {
            core.store.delete(agent, &name)
        })
        .await
        {
            tracing::warn!(
                error = %error,
                name = %name,
                "Failed to delete a filesystem snapshot that no confirmation record names"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete");
        }
    };
    tokio::select! {
        () = delete => {}
        () = ticket.stop().cancelled() => {}
    }
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
            Err(error) => match rules::retry_delay(retry, attempt, error) {
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
