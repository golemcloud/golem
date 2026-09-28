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

//! One upload: save, discard, confirm, then retention or a delete.

use super::{
    CapturedTree, ConfirmOutcome, EnabledSnapshots, JobDecision, ScopeJob, SnapshotClock,
    SnapshotConfirmer, SnapshotKind, retention, store_name,
};
use crate::filesystem_snapshot::{
    ChangeDetection, SnapshotInfo, SnapshotName, SnapshotScope, SnapshotStoreError,
};
use futures::StreamExt as _;
use golem_common::model::RetryConfig;
use golem_common::model::oplog::FilesystemSnapshotName;
use golem_common::retries::get_delay;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::OwnedSemaphorePermit;

/// The upload of one admission. The scope stays reserved until the upload is dropped.
pub(super) struct Upload {
    pub(super) enabled: Arc<EnabledSnapshots>,
    pub(super) scope: SnapshotScope,
    pub(super) job: ScopeJob,
    pub(super) name: FilesystemSnapshotName,
    pub(super) kind: SnapshotKind,
}

/// What a save gave.
enum SaveOutcome {
    /// The store holds the snapshot. The permit is the slot of the upload.
    Saved(SnapshotInfo, OwnedSemaphorePermit),
    /// The save failed, after the retries when the error allows them.
    Failed(SnapshotStoreError),
    /// `forget_scope`, a shutdown or the stop of the caller stopped the upload.
    Stopped,
}

impl Upload {
    /// Saves, discards the capture, and confirms, as [`super::UploadAdmission::submit`] says.
    pub(super) async fn run_in_background(
        self,
        capture: impl CapturedTree,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
        confirmer: SnapshotConfirmer,
    ) {
        let started = Instant::now();
        let (info, permit) = match self
            .save_and_discard(capture, parent, std::future::pending())
            .await
        {
            SaveOutcome::Saved(info, permit) => (info, permit),
            SaveOutcome::Failed(error) => {
                self.job.decide(JobDecision::SaveFailed);
                tracing::warn!(
                    error = %error,
                    name = %self.name,
                    "Failed to upload a filesystem snapshot; no confirmation record follows its record"
                );
                crate::metrics::filesystem_snapshots::record_upload(
                    self.kind.label(),
                    "failed",
                    started.elapsed(),
                );
                return;
            }
            SaveOutcome::Stopped => return,
        };
        crate::metrics::filesystem_snapshots::record_uploaded_bytes(self.kind.label(), info.bytes);
        if self.is_stopped() {
            return;
        }
        let outcome = tokio::select! {
            outcome = confirmer.0.confirm(&self.name) => outcome,
            () = self.stopped() => return,
        };
        self.job.decide(JobDecision::Confirmed(outcome));
        crate::metrics::filesystem_snapshots::record_upload(
            self.kind.label(),
            outcome.label(),
            started.elapsed(),
        );
        match outcome {
            ConfirmOutcome::Confirmed => {
                if self.kind == SnapshotKind::Periodic {
                    self.apply_retention(&info, None).await;
                }
            }
            ConfirmOutcome::Superseded => {
                crate::metrics::filesystem_snapshots::record_dropped_confirmation(outcome.label());
                self.delete_own_snapshot().await;
            }
            ConfirmOutcome::Deferred => {}
        }
        drop(permit);
    }

    /// Saves and discards the capture, and gives the info of the snapshot with the upload, whose
    /// retention runs later. `stop` stops the save like a shutdown does, and the capture is still
    /// discarded.
    pub(super) async fn run_now(
        self,
        capture: impl CapturedTree,
        stop: impl Future<Output = ()> + Send,
    ) -> Result<(SnapshotInfo, Self), SnapshotStoreError> {
        let started = Instant::now();
        let (result, outcome) = match self.save_and_discard(capture, None, stop).await {
            SaveOutcome::Saved(info, _) => (Ok(info), Some("saved")),
            SaveOutcome::Failed(error) => (Err(error), Some("failed")),
            SaveOutcome::Stopped => (
                Err(SnapshotStoreError::Storage {
                    retryable: true,
                    source: anyhow::anyhow!("the upload of the filesystem snapshot was stopped"),
                }),
                None,
            ),
        };
        if let Some(outcome) = outcome {
            crate::metrics::filesystem_snapshots::record_upload(
                self.kind.label(),
                outcome,
                started.elapsed(),
            );
        }
        if let Ok(info) = &result {
            crate::metrics::filesystem_snapshots::record_uploaded_bytes(
                self.kind.label(),
                info.bytes,
            );
        }
        result.map(|info| (info, self))
    }

    /// Waits for a slot of the uploads and applies retention after the save that gave `info`.
    /// `kept` is a snapshot that the retention never deletes.
    pub(super) async fn retain_in_background(self, info: SnapshotInfo, kept: Option<SnapshotName>) {
        let _permit = tokio::select! {
            permit = Arc::clone(&self.enabled.uploads).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => return,
            },
            () = self.stopped() => return,
        };
        self.apply_retention(&info, kept.as_ref()).await;
    }

    /// Whether `forget_scope` or a shutdown stopped the upload.
    fn is_stopped(&self) -> bool {
        self.job.cancel.is_cancelled() || self.enabled.shutdown.is_cancelled()
    }

    /// Completes when `forget_scope` or a shutdown stops the upload.
    async fn stopped(&self) {
        tokio::select! {
            () = self.job.cancel.cancelled() => {}
            () = self.enabled.shutdown.cancelled() => {}
        }
    }

    /// Saves the capture with [`Self::save_tree`], then discards the capture, also when the save
    /// fails or stops.
    async fn save_and_discard(
        &self,
        capture: impl CapturedTree,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
        stop: impl Future<Output = ()> + Send,
    ) -> SaveOutcome {
        let result = self.save_tree(capture.directory(), parent, stop).await;
        capture.discard().await;
        result
    }

    /// Waits for an upload slot and saves the tree with retries, one [`save_attempt`] at a time.
    /// `stop`, `forget_scope` or a shutdown ends the save with `Stopped`.
    async fn save_tree(
        &self,
        tree: &std::path::Path,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
        stop: impl Future<Output = ()> + Send,
    ) -> SaveOutcome {
        let stop = futures::FutureExt::shared(futures::FutureExt::boxed(stop));
        let (name, parent) = match (
            store_name(&self.name),
            parent
                .map(|(name, detection)| store_name(&name).map(|name| (name, detection)))
                .transpose(),
        ) {
            (Ok(name), Ok(parent)) => (name, parent),
            (Err(error), _) | (_, Err(error)) => {
                return SaveOutcome::Failed(SnapshotStoreError::Storage {
                    retryable: false,
                    source: anyhow::Error::new(error),
                });
            }
        };
        let permit = tokio::select! {
            permit = Arc::clone(&self.enabled.uploads).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => return SaveOutcome::Stopped,
            },
            () = self.stopped() => return SaveOutcome::Stopped,
            () = stop.clone() => return SaveOutcome::Stopped,
        };
        crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
        self.job
            .holds_slot
            .store(true, std::sync::atomic::Ordering::Release);
        let parent = parent.as_ref().map(|(name, detection)| (name, *detection));
        let saved = tokio::select! {
            biased;
            saved = retrying(
                self.enabled.settings.upload_retry(),
                self.enabled.clock.as_ref(),
                || save_attempt(&self.enabled, &self.scope, &name, tree, parent),
            ) => Some(saved),
            () = self.stopped() => None,
            () = stop.clone() => None,
        };
        crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
        match saved {
            Some(Ok(info)) => SaveOutcome::Saved(info, permit),
            Some(Err(error)) => SaveOutcome::Failed(error),
            None => SaveOutcome::Stopped,
        }
    }

    /// Keeps the own snapshot and the newest older snapshots of its kind, and deletes the rest of
    /// its kind that are older than it. `info` is the info of the own snapshot. A stop ends it at
    /// once, and a later retention deletes what it left.
    async fn apply_retention(&self, info: &SnapshotInfo, kept: Option<&SnapshotName>) {
        let retention = async {
            let Ok(own) = store_name(&self.name) else {
                return;
            };
            let listing = match self.enabled.store.list(&self.scope).await {
                Ok(listing) => listing,
                Err(error) => {
                    tracing::warn!(error = %error, "Failed to list the filesystem snapshots for retention");
                    return;
                }
            };
            let keep = match self.kind {
                SnapshotKind::Periodic => self.enabled.settings.retained_periodic_snapshots(),
                SnapshotKind::Update => self.enabled.settings.retained_update_snapshots(),
            };
            let victims = retention::victims(&listing, &own, info, keep.get(), kept);
            futures::stream::iter(victims.iter())
                .for_each(|name| async move {
                    if let Err(error) = retrying(
                        self.enabled.settings.upload_retry(),
                        self.enabled.clock.as_ref(),
                        || self.enabled.store.delete(&self.scope, name),
                    )
                    .await
                    {
                        tracing::warn!(
                            error = %error,
                            name = %name,
                            "Failed to delete an old filesystem snapshot; the next retention tries again"
                        );
                    }
                })
                .await;
        };
        tokio::select! {
            () = retention => {}
            () = self.stopped() => {}
        }
    }

    /// Deletes the snapshot of this upload, which no confirmation record names. A stop ends it
    /// at once, and the snapshot stays until a retention of its kind deletes it.
    async fn delete_own_snapshot(&self) {
        let delete = async {
            let Ok(name) = store_name(&self.name) else {
                return;
            };
            if let Err(error) = retrying(
                self.enabled.settings.upload_retry(),
                self.enabled.clock.as_ref(),
                || self.enabled.store.delete(&self.scope, &name),
            )
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
            () = self.stopped() => {}
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        self.enabled.end(&self.scope, &self.job);
    }
}

/// Saves the tree under the name of the job, one time. A name that the store already holds is the tree of
/// an earlier attempt of the same job, because each name belongs to one capture, so it counts as
/// saved.
async fn save_attempt(
    enabled: &EnabledSnapshots,
    scope: &SnapshotScope,
    name: &SnapshotName,
    tree: &std::path::Path,
    parent: Option<(&SnapshotName, ChangeDetection)>,
) -> Result<SnapshotInfo, SnapshotStoreError> {
    match enabled.store.save(scope, name, tree, parent).await {
        Err(SnapshotStoreError::AlreadyExists) => enabled
            .store
            .stat(scope, name)
            .await?
            .ok_or_else(|| SnapshotStoreError::Storage {
                retryable: true,
                source: anyhow::anyhow!(
                    "the filesystem snapshot {name} exists at the save and not after it"
                ),
            }),
        other => other,
    }
}

/// Whether a new attempt of a store operation can succeed without a change.
fn is_retryable(error: &SnapshotStoreError) -> bool {
    matches!(
        error,
        SnapshotStoreError::Storage {
            retryable: true,
            ..
        }
    )
}

/// Runs `operation`, and runs it again after the delays of `retry` while it fails with an error
/// that allows a retry.
pub(super) async fn retrying<T, Operation, Attempt>(
    retry: &RetryConfig,
    clock: &dyn SnapshotClock,
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
            Err(error) if is_retryable(error) => match get_delay(retry, attempt) {
                Some(delay) => {
                    clock.sleep(delay).await;
                    Some(attempt + 1)
                }
                None => None,
            },
            _ => None,
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
