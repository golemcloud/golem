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
    CapturedTree, ConfirmOutcome, Inner, JobDecision, ScopeJob, SnapshotClock, SnapshotConfirmer,
    SnapshotKind, retention, store_name,
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
    pub(super) inner: Arc<Inner>,
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
    /// `forget_scope` or a shutdown stopped the upload.
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
        let (info, permit) = match self.save(capture, parent).await {
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
                    self.apply_retention(&info).await;
                }
            }
            ConfirmOutcome::Superseded => {
                crate::metrics::filesystem_snapshots::record_dropped_confirmation(outcome.label());
                self.delete_own_snapshot().await;
            }
            ConfirmOutcome::Deferred => {
                crate::metrics::filesystem_snapshots::record_dropped_confirmation(outcome.label());
            }
        }
        drop(permit);
    }

    /// Saves and discards the capture, and gives the info of the snapshot.
    pub(super) async fn run_now(
        self,
        capture: impl CapturedTree,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let started = Instant::now();
        let result = match self.save(capture, None).await {
            SaveOutcome::Saved(info, _) => Ok(info),
            SaveOutcome::Failed(error) => Err(error),
            SaveOutcome::Stopped => Err(SnapshotStoreError::Storage {
                retryable: true,
                source: anyhow::anyhow!("the upload of the filesystem snapshot was stopped"),
            }),
        };
        let outcome = if result.is_ok() { "saved" } else { "failed" };
        crate::metrics::filesystem_snapshots::record_upload(
            self.kind.label(),
            outcome,
            started.elapsed(),
        );
        if let Ok(info) = &result {
            crate::metrics::filesystem_snapshots::record_uploaded_bytes(
                self.kind.label(),
                info.bytes,
            );
        }
        result
    }

    /// Whether `forget_scope` or a shutdown stopped the upload.
    fn is_stopped(&self) -> bool {
        self.job.cancel.is_cancelled() || self.inner.shutdown.is_cancelled()
    }

    /// Completes when `forget_scope` or a shutdown stops the upload.
    async fn stopped(&self) {
        tokio::select! {
            () = self.job.cancel.cancelled() => {}
            () = self.inner.shutdown.cancelled() => {}
        }
    }

    /// Waits for a slot, saves with retries, and discards the capture in each case.
    async fn save(
        &self,
        capture: impl CapturedTree,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
    ) -> SaveOutcome {
        let result = self.save_capture(capture.directory(), parent).await;
        capture.discard().await;
        result
    }

    async fn save_capture(
        &self,
        tree: &std::path::Path,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
    ) -> SaveOutcome {
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
            permit = Arc::clone(&self.inner.uploads).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => return SaveOutcome::Stopped,
            },
            () = self.stopped() => return SaveOutcome::Stopped,
        };
        crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
        self.job
            .holds_slot
            .store(true, std::sync::atomic::Ordering::Release);
        let parent = parent.as_ref().map(|(name, detection)| (name, *detection));
        let saved = tokio::select! {
            saved = retrying(
                self.inner.settings.upload_retry(),
                self.inner.clock.as_ref(),
                || save_own_name(&self.inner, &self.scope, &name, tree, parent),
            ) => Some(saved),
            () = self.stopped() => None,
        };
        crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
        match saved {
            Some(Ok(info)) => SaveOutcome::Saved(info, permit),
            Some(Err(error)) => SaveOutcome::Failed(error),
            None => SaveOutcome::Stopped,
        }
    }

    /// Keeps the own snapshot and the newest older snapshots of its kind, and deletes the rest of
    /// its kind that are older than it. `info` is the info of the own snapshot.
    async fn apply_retention(&self, info: &SnapshotInfo) {
        let Ok(own) = store_name(&self.name) else {
            return;
        };
        let listing = match self.inner.store.list(&self.scope).await {
            Ok(listing) => listing,
            Err(error) => {
                tracing::warn!(error = %error, "Failed to list the filesystem snapshots for retention");
                return;
            }
        };
        let keep = match self.kind {
            SnapshotKind::Periodic => self.inner.settings.retained_periodic_snapshots(),
            SnapshotKind::Update => self.inner.settings.retained_update_snapshots(),
        };
        let victims = retention::victims(&listing, &own, info, keep.get());
        futures::stream::iter(victims.iter())
            .for_each(|name| async move {
                if let Err(error) = retrying(
                    self.inner.settings.upload_retry(),
                    self.inner.clock.as_ref(),
                    || self.inner.store.delete(&self.scope, name),
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
    }

    /// Deletes the snapshot of this upload, which no confirmation record names.
    async fn delete_own_snapshot(&self) {
        let Ok(name) = store_name(&self.name) else {
            return;
        };
        if let Err(error) = retrying(
            self.inner.settings.upload_retry(),
            self.inner.clock.as_ref(),
            || self.inner.store.delete(&self.scope, &name),
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
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        self.inner.end(&self.scope, &self.job);
    }
}

/// Saves the tree under the name of the job. A name that the store already holds is the tree of
/// an earlier attempt of the same job, because each name belongs to one capture, so it counts as
/// saved.
async fn save_own_name(
    inner: &Inner,
    scope: &SnapshotScope,
    name: &SnapshotName,
    tree: &std::path::Path,
    parent: Option<(&SnapshotName, ChangeDetection)>,
) -> Result<SnapshotInfo, SnapshotStoreError> {
    match inner.store.save(scope, name, tree, parent).await {
        Err(SnapshotStoreError::AlreadyExists) => {
            inner
                .store
                .stat(scope, name)
                .await?
                .ok_or_else(|| SnapshotStoreError::Storage {
                    retryable: true,
                    source: anyhow::anyhow!(
                        "the filesystem snapshot {name} exists at the save and not after it"
                    ),
                })
        }
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
