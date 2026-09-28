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

//! Connects the filesystem snapshots of the store to the records of an agent: names, uploads in
//! the background, confirmations, retention, restores and clean-up.
//!
//! This is the only module that knows both a capture of an agent filesystem and a name of the
//! store. It does not know the oplog entry types or the worker: a [`SnapshotConfirmer`] writes the
//! confirmation record.

mod cleanup;
mod job;
mod restore;
mod retention;
#[cfg(test)]
mod tests;

use crate::filesystem_snapshot::{
    ChangeDetection, FilesystemSnapshotStore, InvalidSnapshotName, SnapshotInfo, SnapshotName,
    SnapshotScope, SnapshotStoreError,
};
use crate::services::agent_filesystem::FilesystemCapture;
use crate::services::golem_config::{FilesystemSnapshotUploadConfig, FilesystemSnapshotsConfig};
use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub(crate) use restore::StoreRestore;

/// The kind of a filesystem snapshot. It gives the prefix of the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotKind {
    /// A snapshot of a periodic snapshot record, named `p-<uuid>`.
    Periodic,
    /// A snapshot of a manual update, named `u-<uuid>`.
    Update,
}

impl SnapshotKind {
    fn label(self) -> &'static str {
        match self {
            Self::Periodic => "periodic",
            Self::Update => "update",
        }
    }
}

/// Why an admission gives no upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotSkip {
    /// This executor keeps no filesystem snapshots. The record gets no name.
    Disabled,
    /// An upload of the agent runs now. The loop skips the snapshot.
    UploadInFlight,
    /// The volume has less free space than the pressure target. The loop skips the snapshot.
    VolumeUnderPressure,
}

impl std::fmt::Display for SnapshotSkip {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disabled => "filesystem snapshots are disabled on this executor",
            Self::UploadInFlight => "an upload of a filesystem snapshot of the agent runs now",
            Self::VolumeUnderPressure => "the volume of the agent filesystems is under pressure",
        })
    }
}

/// The record names a filesystem snapshot, and this executor keeps no filesystem snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SnapshotsDisabled;

impl std::fmt::Display for SnapshotsDisabled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "the record names a filesystem snapshot, and filesystem snapshots are disabled on \
             this executor",
        )
    }
}

/// What a confirmation found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmOutcome {
    /// The confirmation record is in the oplog.
    Confirmed,
    /// The status of the worker no longer holds the snapshot record with the name, because an
    /// update or a revert cleared it. Nothing is written.
    Superseded,
    /// The worker is not in the memory of this executor, or this executor no longer owns it.
    /// Nothing is written.
    Gone,
}

impl ConfirmOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Superseded => "superseded",
            Self::Gone => "gone",
        }
    }
}

/// Writes the confirmation record of a filesystem snapshot.
#[async_trait]
pub(crate) trait ConfirmSnapshot: Send + Sync {
    /// Writes a `SnapshotConfirmed` record with `name` when the status of the worker still holds
    /// the snapshot record with the name.
    async fn confirm(&self, name: &FilesystemSnapshotName) -> ConfirmOutcome;
}

/// The writer of the confirmation record of one upload. The invocation loop makes it from a weak
/// handle to the worker, so the service never holds a worker.
#[derive(Clone)]
pub(crate) struct SnapshotConfirmer(Arc<dyn ConfirmSnapshot>);

impl SnapshotConfirmer {
    pub(crate) fn new(confirm: Arc<dyn ConfirmSnapshot>) -> Self {
        Self(confirm)
    }
}

/// The source of the waits of the service. Tests give a clock that they move themselves.
pub(crate) trait SnapshotClock: Send + Sync {
    /// Completes after `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// The clock of the runtime.
pub(crate) struct TokioClock;

impl SnapshotClock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Tells whether the volume of the agent filesystems has room for a new capture.
#[async_trait]
pub(crate) trait VolumeRoom: Send + Sync {
    async fn has_room(&self) -> bool;
}

/// A volume that always has room.
pub(crate) struct UnlimitedRoom;

#[async_trait]
impl VolumeRoom for UnlimitedRoom {
    async fn has_room(&self) -> bool {
        true
    }
}

/// The room of the volume of the agent filesystems: it has room when its free space reaches the
/// targets of the pressure configuration. An unmanaged volume always has room.
pub(crate) struct PressureTargetRoom {
    volume: crate::sandbox_filesystem::FilesystemVolume,
    pressure: crate::services::golem_config::FilesystemPressureConfig,
}

impl PressureTargetRoom {
    pub(crate) fn new(
        volume: crate::sandbox_filesystem::FilesystemVolume,
        pressure: crate::services::golem_config::FilesystemPressureConfig,
    ) -> Self {
        Self { volume, pressure }
    }
}

#[async_trait]
impl VolumeRoom for PressureTargetRoom {
    async fn has_room(&self) -> bool {
        match crate::sandbox_filesystem::observe_space(&self.volume).await {
            Ok(crate::sandbox_filesystem::FilesystemSpace::Unlimited) => true,
            Ok(crate::sandbox_filesystem::FilesystemSpace::Observed {
                available_bytes,
                available_filesystem_objects,
                ..
            }) => {
                available_bytes >= self.pressure.target_available_bytes()
                    && available_filesystem_objects
                        >= self.pressure.target_available_filesystem_objects()
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to observe the free space of the volume");
                false
            }
        }
    }
}

/// A tree that a capture copied, which an upload saves and then discards.
pub(crate) trait CapturedTree: Send + 'static {
    /// The directory that the store saves.
    fn directory(&self) -> &Path;
    /// Removes the directory with all that is in it.
    fn discard(self) -> BoxFuture<'static, ()>;
}

impl CapturedTree for FilesystemCapture {
    fn directory(&self) -> &Path {
        FilesystemCapture::directory(self)
    }

    fn discard(self) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            if let Err(error) = FilesystemCapture::discard(self).await {
                tracing::warn!(error = %error, "Failed to discard a filesystem capture");
            }
        })
    }
}

/// Makes the name of the store from a name of a record. A name of a record is always a valid
/// name of the store, and the error only guards against a change of either rule.
pub(crate) fn store_name(
    name: &FilesystemSnapshotName,
) -> Result<SnapshotName, InvalidSnapshotName> {
    SnapshotName::new(name.as_str())
}

/// The filesystem snapshots of the agents of this executor.
///
/// A disabled service answers each admission with [`SnapshotSkip::Disabled`] and each restore
/// with [`SnapshotsDisabled`].
pub struct AgentFilesystemSnapshots {
    inner: Option<Arc<Inner>>,
}

/// The state of an enabled service.
struct Inner {
    store: Arc<dyn FilesystemSnapshotStore>,
    settings: FilesystemSnapshotUploadConfig,
    /// The slots of the store operations that save or delete.
    uploads: Arc<Semaphore>,
    /// The slots of the restores.
    restores: Arc<Semaphore>,
    /// The job of each scope that has an admission or an upload.
    scopes: Mutex<HashMap<SnapshotScope, ScopeJob>>,
    next_job: AtomicU64,
    clock: Arc<dyn SnapshotClock>,
    room: Arc<dyn VolumeRoom>,
    cleanup: cleanup::CleanupQueue,
    /// Cancelled when the executor shuts down.
    shutdown: CancellationToken,
    /// The upload jobs, so that a shutdown can wait for them.
    jobs: TaskTracker,
}

/// The job of one scope, from its admission to its end.
#[derive(Clone)]
struct ScopeJob {
    id: u64,
    /// Cancelled by `forget_scope`. The job stops and writes nothing.
    cancel: CancellationToken,
    /// Cancelled when the job ended.
    ended: CancellationToken,
}

impl AgentFilesystemSnapshots {
    /// Makes the service that the configuration asks for.
    ///
    /// `Managed` needs a sandbox provisioning on managed XFS storage, and `managed_storage` tells
    /// whether the executor has it. `store` makes the store of an enabled service.
    pub(crate) fn bind(
        config: &FilesystemSnapshotsConfig,
        managed_storage: bool,
        store: impl FnOnce() -> Arc<dyn FilesystemSnapshotStore>,
        room: Arc<dyn VolumeRoom>,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        match config {
            FilesystemSnapshotsConfig::Disabled(_) => Ok(Self::disabled()),
            FilesystemSnapshotsConfig::Managed(_) if !managed_storage => {
                Err("filesystem snapshots require managed XFS storage".to_string())
            }
            FilesystemSnapshotsConfig::Managed(config) => Ok(Self::enabled(
                store(),
                config.uploads().clone(),
                Arc::new(TokioClock),
                room,
                shutdown,
            )),
        }
    }

    /// Makes a service that keeps no filesystem snapshots.
    pub(crate) fn disabled() -> Self {
        Self { inner: None }
    }

    /// Makes a service over `store` with `settings`.
    pub(crate) fn enabled(
        store: Arc<dyn FilesystemSnapshotStore>,
        settings: FilesystemSnapshotUploadConfig,
        clock: Arc<dyn SnapshotClock>,
        room: Arc<dyn VolumeRoom>,
        shutdown: CancellationToken,
    ) -> Self {
        let uploads = Arc::new(Semaphore::new(settings.max_concurrent_uploads().get()));
        let jobs = TaskTracker::new();
        let cleanup = cleanup::CleanupQueue::start(
            Arc::clone(&store),
            Arc::clone(&uploads),
            settings.upload_retry().clone(),
            Arc::clone(&clock),
            shutdown.clone(),
            &jobs,
        );
        Self {
            inner: Some(Arc::new(Inner {
                restores: Arc::new(Semaphore::new(settings.max_concurrent_restores().get())),
                store,
                settings,
                uploads,
                scopes: Mutex::default(),
                next_job: AtomicU64::new(1),
                clock,
                room,
                cleanup,
                shutdown,
                jobs,
            })),
        }
    }

    /// Whether this executor keeps filesystem snapshots.
    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// How long a capture waits for open file calls, or `None` when the service is disabled.
    pub(crate) fn capture_wait(&self) -> Option<Duration> {
        self.inner
            .as_ref()
            .map(|inner| inner.settings.capture_wait())
    }

    /// How long a stopped worker stays in memory for an upload in progress, or `None` when the
    /// service is disabled.
    pub(crate) fn confirmation_wait(&self) -> Option<Duration> {
        self.inner
            .as_ref()
            .map(|inner| inner.settings.confirmation_wait())
    }

    /// Asks for an upload of the agent of `scope`, before the guest saves.
    ///
    /// The admission holds a new name of `kind`. While it exists, and while the upload that it
    /// starts runs, each other admission of the scope gives [`SnapshotSkip::UploadInFlight`]. A
    /// dropped admission frees the scope and writes nothing durable.
    pub(crate) async fn admit(
        &self,
        scope: &SnapshotScope,
        kind: SnapshotKind,
    ) -> Result<UploadAdmission, SnapshotSkip> {
        let inner = self.inner.as_ref().ok_or(SnapshotSkip::Disabled)?;
        if !inner.room.has_room().await {
            return Err(SnapshotSkip::VolumeUnderPressure);
        }
        let job = inner.reserve(scope)?;
        let name = match kind {
            SnapshotKind::Periodic => FilesystemSnapshotName::periodic(),
            SnapshotKind::Update => FilesystemSnapshotName::update(),
        };
        Ok(UploadAdmission {
            inner: Arc::clone(inner),
            scope: scope.clone(),
            job: Some(job),
            name,
            kind,
        })
    }

    /// Gives the restore of the filesystem snapshot `name` of `scope`. The restore does its work
    /// when the lifecycle calls it, and it waits for a slot of the restores then. This call asks
    /// the store for nothing and waits for nothing.
    pub(crate) fn restore(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<StoreRestore, SnapshotsDisabled> {
        let inner = self.inner.as_ref().ok_or(SnapshotsDisabled)?;
        Ok(StoreRestore::new(
            Arc::clone(&inner.store),
            scope.clone(),
            name.clone(),
            Arc::clone(&inner.restores),
        ))
    }

    /// Deletes the filesystem snapshots `names` of `scope` in the background. The call returns at
    /// once and cannot fail. The clean-up retries, and after the retries it logs and counts the
    /// names that stay.
    pub(crate) fn forget(&self, scope: &SnapshotScope, names: Box<[FilesystemSnapshotName]>) {
        if let Some(inner) = &self.inner {
            inner.cleanup.delete(scope.clone(), names);
        }
    }

    /// Deletes the scope in the background, after the job of the scope ended. The call cancels
    /// that job first, so it writes nothing more. The call returns at once and cannot fail.
    pub(crate) fn forget_scope(&self, scope: &SnapshotScope) {
        if let Some(inner) = &self.inner {
            let job = inner.jobs_of_scope().get(scope).cloned();
            if let Some(job) = &job {
                job.cancel.cancel();
            }
            inner
                .cleanup
                .delete_scope(scope.clone(), job.map(|job| job.ended));
        }
    }

    /// Copies each filesystem snapshot of `from` into the empty scope `to`.
    pub(crate) async fn duplicate_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        match &self.inner {
            Some(inner) => inner.store.copy_scope(from, to).await,
            None => Ok(()),
        }
    }

    /// Waits until the job of `scope` ended, for at most `limit`. Gives `true` when no job of the
    /// scope runs at the return.
    pub(crate) async fn wait_for_pending(&self, scope: &SnapshotScope, limit: Duration) -> bool {
        let Some(inner) = &self.inner else {
            return true;
        };
        let Some(job) = inner.jobs_of_scope().get(scope).cloned() else {
            return true;
        };
        tokio::select! {
            () = job.ended.cancelled() => true,
            () = inner.clock.sleep(limit) => job.ended.is_cancelled(),
        }
    }

    /// Stops the jobs and the clean-up queue, and waits for them. Call it once, when the executor
    /// shuts down.
    pub(crate) async fn shut_down(&self) {
        if let Some(inner) = &self.inner {
            inner.shutdown.cancel();
            inner.jobs.close();
            inner.jobs.wait().await;
        }
    }
}

impl Inner {
    /// Gives the jobs of the scopes. No lock is held across an await, so the map of a poisoned
    /// lock is used as it is.
    fn jobs_of_scope(&self) -> MutexGuard<'_, HashMap<SnapshotScope, ScopeJob>> {
        self.scopes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reserves `scope` for a new job, or gives `UploadInFlight` when a job of the scope exists.
    fn reserve(&self, scope: &SnapshotScope) -> Result<ScopeJob, SnapshotSkip> {
        let mut scopes = self.jobs_of_scope();
        if scopes.contains_key(scope) {
            return Err(SnapshotSkip::UploadInFlight);
        }
        let job = ScopeJob {
            id: self.next_job.fetch_add(1, Ordering::Relaxed),
            cancel: CancellationToken::new(),
            ended: CancellationToken::new(),
        };
        scopes.insert(scope.clone(), job.clone());
        Ok(job)
    }

    /// Frees `scope` when `job` is its job, and marks the job as ended.
    fn end(&self, scope: &SnapshotScope, job: &ScopeJob) {
        let mut scopes = self.jobs_of_scope();
        if scopes
            .get(scope)
            .is_some_and(|current| current.id == job.id)
        {
            scopes.remove(scope);
        }
        drop(scopes);
        job.ended.cancel();
    }
}

/// The permission of one upload, with its name. Only an admission gives a name, and an admission
/// cannot be cloned, so each name reaches at most one capture.
pub(crate) struct UploadAdmission {
    inner: Arc<Inner>,
    scope: SnapshotScope,
    /// The job of the scope. `None` after the admission handed it to an upload.
    job: Option<ScopeJob>,
    name: FilesystemSnapshotName,
    kind: SnapshotKind,
}

impl UploadAdmission {
    /// The name that the admission made.
    pub(crate) fn name(&self) -> &FilesystemSnapshotName {
        &self.name
    }

    /// Uploads `capture` in the background, then confirms it with `confirmer`.
    ///
    /// The job waits for a slot of the uploads, saves with retries, discards the capture, and
    /// confirms. On `Confirmed` it applies retention. On `Superseded` or `Gone` it deletes the
    /// snapshot, runs no retention, and counts a dropped confirmation. When the retries are used
    /// up, it confirms nothing. A job that `forget_scope` or a shutdown stops writes and deletes
    /// nothing.
    pub(crate) fn submit(
        mut self,
        capture: impl CapturedTree,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
        confirmer: SnapshotConfirmer,
    ) {
        let Some(job) = self.job.take() else {
            return;
        };
        let upload = job::Upload {
            inner: Arc::clone(&self.inner),
            scope: self.scope.clone(),
            job,
            name: self.name.clone(),
            kind: self.kind,
        };
        self.inner
            .jobs
            .spawn(upload.run_in_background(capture, parent, confirmer));
    }

    /// Uploads `capture` and waits until the store holds it. A manual update calls this before it
    /// writes its record.
    pub(crate) async fn upload_now(
        mut self,
        capture: impl CapturedTree,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let Some(job) = self.job.take() else {
            return Err(SnapshotStoreError::NotFound);
        };
        let upload = job::Upload {
            inner: Arc::clone(&self.inner),
            scope: self.scope.clone(),
            job,
            name: self.name.clone(),
            kind: self.kind,
        };
        upload.run_now(capture).await
    }
}

impl Drop for UploadAdmission {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            self.inner.end(&self.scope, &job);
        }
    }
}
