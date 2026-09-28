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
use crate::services::golem_config::{
    FilesystemSnapshotStoreConfig, FilesystemSnapshotUploadConfig, FilesystemSnapshotsConfig,
};
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
    /// A delete of the scope is queued or runs. The loop skips the snapshot.
    ScopeDeleting,
}

impl std::fmt::Display for SnapshotSkip {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disabled => "filesystem snapshots are disabled on this executor",
            Self::UploadInFlight => "an upload of a filesystem snapshot of the agent runs now",
            Self::VolumeUnderPressure => "the volume of the agent filesystems is under pressure",
            Self::ScopeDeleting => "the filesystem snapshots of the agent are being deleted",
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
    /// The last automatic snapshot record of the agent does not have the name, because an update
    /// or a revert cleared it. Nothing was written, and no start selects the snapshot.
    Superseded,
    /// The confirmation was not written now: the agent does not run, or its state was not known.
    /// A later start of the agent can confirm the snapshot, so the snapshot stays.
    Deferred,
}

impl ConfirmOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Superseded => "superseded",
            Self::Deferred => "deferred",
        }
    }
}

/// How an upload job ended its work on the snapshot, before retention or a delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobDecision {
    /// The confirmation gave this outcome.
    Confirmed(ConfirmOutcome),
    /// The save failed after its retries. The store does not hold the snapshot.
    SaveFailed,
    /// `forget_scope` or a shutdown stopped the job, or the admission ended without an upload.
    Stopped,
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

/// The source of the waits of the service. A sleep completes when its clock says the duration
/// passed.
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
    /// Gives `true` when the volume has room for a new capture now, and `false` when it has not
    /// or when its free space cannot be observed.
    async fn has_room(&self) -> bool;
}

/// A volume that always has room.
#[cfg(any(test, feature = "test-utils"))]
pub(crate) struct UnlimitedRoom;

#[cfg(any(test, feature = "test-utils"))]
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
    enabled: Option<Arc<EnabledSnapshots>>,
}

/// The state of an enabled service.
struct EnabledSnapshots {
    store: Arc<dyn FilesystemSnapshotStore>,
    settings: FilesystemSnapshotUploadConfig,
    /// The slots of the store operations that save or delete.
    uploads: Arc<Semaphore>,
    /// The slots of the restores.
    restores: Arc<Semaphore>,
    /// The job of each scope that has an admission or an upload, and the scopes whose delete is
    /// queued or runs.
    scopes: Arc<Mutex<Scopes>>,
    next_job: AtomicU64,
    clock: Arc<dyn SnapshotClock>,
    room: Arc<dyn VolumeRoom>,
    cleanup: cleanup::CleanupQueue,
    /// Cancelled when the executor shuts down.
    shutdown: CancellationToken,
    /// The upload jobs, so that a shutdown can wait for them.
    jobs: TaskTracker,
}

/// The jobs of the scopes, and the scopes whose delete is queued or runs.
#[derive(Default)]
struct Scopes {
    jobs: HashMap<SnapshotScope, ScopeJob>,
    /// The number of queued or running deletes of each scope.
    deleting: HashMap<SnapshotScope, usize>,
}

/// Gives the scopes behind `scopes`. No lock is held across an await, so the scopes of a
/// poisoned lock are used as they are.
fn scopes_of(scopes: &Mutex<Scopes>) -> MutexGuard<'_, Scopes> {
    scopes.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The job of one scope, from its admission to its end.
#[derive(Clone)]
struct ScopeJob {
    id: u64,
    /// The name of the snapshot of the job.
    name: FilesystemSnapshotName,
    /// Cancelled by `forget_scope`. The job stops and sends nothing more; a confirmation that it
    /// already sent can still be appended.
    cancel: CancellationToken,
    /// Cancelled when the job ended.
    ended: CancellationToken,
    /// Set while the save of the job holds a slot of the uploads.
    holds_slot: Arc<std::sync::atomic::AtomicBool>,
    /// The decision of the job, sent once, before its retention or delete.
    decided: Arc<tokio::sync::watch::Sender<Option<JobDecision>>>,
}

impl ScopeJob {
    /// Records the decision of the job. A later decision does not replace the first one.
    fn decide(&self, decision: JobDecision) {
        self.decided.send_if_modified(|current| {
            let first = current.is_none();
            if first {
                *current = Some(decision);
            }
            first
        });
    }
}

/// What a start can see of an upload of its agent.
pub(crate) struct UploadView {
    /// Whether the save holds a slot of the uploads. A job that waits for a slot is not waited
    /// for.
    pub(crate) holds_slot: bool,
    /// The decision of the job, once it is known.
    pub(crate) decided: tokio::sync::watch::Receiver<Option<JobDecision>>,
}

/// The mark of a scope whose delete is queued or runs. Dropping it removes the mark.
pub(super) struct DeletingMark {
    scopes: Arc<Mutex<Scopes>>,
    scope: SnapshotScope,
}

impl Drop for DeletingMark {
    fn drop(&mut self) {
        let mut scopes = scopes_of(&self.scopes);
        let left = scopes
            .deleting
            .get_mut(&self.scope)
            .map(|count| {
                *count = count.saturating_sub(1);
                *count
            })
            .unwrap_or(0);
        if left == 0 {
            scopes.deleting.remove(&self.scope);
        }
    }
}

impl AgentFilesystemSnapshots {
    /// Makes the service that the configuration asks for.
    ///
    /// `Managed` needs a sandbox provisioning on managed XFS storage, and `managed_storage` tells
    /// whether the executor has it. `store` makes the store of an enabled service from its
    /// settings. A shutdown of the service also shuts the store down.
    pub(crate) fn bind(
        config: &FilesystemSnapshotsConfig,
        managed_storage: bool,
        store: impl FnOnce(&FilesystemSnapshotStoreConfig) -> Arc<dyn FilesystemSnapshotStore>,
        room: Arc<dyn VolumeRoom>,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        match config {
            FilesystemSnapshotsConfig::Disabled(_) => Ok(Self::disabled()),
            FilesystemSnapshotsConfig::Managed(_) if !managed_storage => {
                Err("filesystem snapshots require managed XFS storage".to_string())
            }
            FilesystemSnapshotsConfig::Managed(config) => Ok(Self::enabled(
                store(config),
                config.uploads().clone(),
                Arc::new(TokioClock),
                room,
                shutdown,
            )),
        }
    }

    /// Makes a service that keeps no filesystem snapshots.
    fn disabled() -> Self {
        Self { enabled: None }
    }

    /// Makes an enabled service over `store` without the storage check of [`Self::bind`], so
    /// that executor tests can keep filesystem snapshots on unmanaged storage.
    #[cfg(feature = "test-utils")]
    pub(crate) fn enabled_without_storage_check(
        store: Arc<dyn FilesystemSnapshotStore>,
        settings: FilesystemSnapshotUploadConfig,
        room: Arc<dyn VolumeRoom>,
        shutdown: CancellationToken,
    ) -> Self {
        Self::enabled(store, settings, Arc::new(TokioClock), room, shutdown)
    }

    /// Makes a service over `store` with `settings`.
    fn enabled(
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
            enabled: Some(Arc::new(EnabledSnapshots {
                restores: Arc::new(Semaphore::new(settings.max_concurrent_restores().get())),
                store,
                settings,
                uploads,
                scopes: Arc::default(),
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
        self.enabled.is_some()
    }

    /// Gives the upload of the snapshot `name` of `scope` that runs on this executor now.
    pub(crate) fn upload_of(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Option<UploadView> {
        let enabled = self.enabled.as_ref()?;
        let scopes = enabled.jobs_of_scope();
        let job = scopes.jobs.get(scope).filter(|job| &job.name == name)?;
        Some(UploadView {
            holds_slot: job.holds_slot.load(Ordering::Acquire),
            decided: job.decided.subscribe(),
        })
    }

    /// Tells whether the store holds the whole snapshot `name` of `scope`.
    pub(crate) async fn is_stored(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<bool, SnapshotStoreError> {
        let enabled = self.enabled.as_ref().ok_or(SnapshotStoreError::NotFound)?;
        let name = store_name(name).map_err(|error| SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::Error::new(error),
        })?;
        enabled
            .store
            .stat(scope, &name)
            .await
            .map(|info| info.is_some())
    }

    /// How long a capture waits for open file calls, or `None` when the service is disabled.
    pub(crate) fn capture_wait(&self) -> Option<Duration> {
        self.enabled
            .as_ref()
            .map(|enabled| enabled.settings.capture_wait())
    }

    /// How long a start waits for an upload of its agent in progress, or `None` when the
    /// service is disabled.
    pub(crate) fn confirmation_wait(&self) -> Option<Duration> {
        self.enabled
            .as_ref()
            .map(|enabled| enabled.settings.confirmation_wait())
    }

    /// How long a start that did not wait for an upload asks the store for its snapshot, or
    /// `None` when the service is disabled.
    pub(crate) fn store_check_limit(&self) -> Option<Duration> {
        self.enabled
            .as_ref()
            .map(|enabled| enabled.settings.store_check_limit())
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
        let enabled = self.enabled.as_ref().ok_or(SnapshotSkip::Disabled)?;
        if !enabled.room.has_room().await {
            return Err(SnapshotSkip::VolumeUnderPressure);
        }
        let name = match kind {
            SnapshotKind::Periodic => FilesystemSnapshotName::periodic(),
            SnapshotKind::Update => FilesystemSnapshotName::update(),
        };
        let job = enabled.reserve(scope, &name)?;
        Ok(UploadAdmission {
            enabled: Arc::clone(enabled),
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
        let enabled = self.enabled.as_ref().ok_or(SnapshotsDisabled)?;
        Ok(StoreRestore::new(
            Arc::clone(&enabled.store),
            scope.clone(),
            name.clone(),
            Arc::clone(&enabled.restores),
        ))
    }

    /// Deletes the filesystem snapshots `names` of `scope` in the background. The call returns at
    /// once and cannot fail. The clean-up retries, and after the retries it logs and counts the
    /// names that stay.
    #[allow(dead_code)]
    pub(crate) fn forget(&self, scope: &SnapshotScope, names: Box<[FilesystemSnapshotName]>) {
        if let Some(enabled) = &self.enabled {
            enabled.cleanup.delete(scope.clone(), names);
        }
    }

    /// Deletes the scope in the background, after the job of the scope ended. The call cancels
    /// that job first, so it sends nothing more; a confirmation that it already sent can still be
    /// appended. The call returns at once and cannot fail.
    ///
    /// Until the delete ends, with success or with an error, an admission of the scope gives
    /// [`SnapshotSkip::ScopeDeleting`].
    #[allow(dead_code)]
    pub(crate) fn forget_scope(&self, scope: &SnapshotScope) {
        if let Some(enabled) = &self.enabled {
            let (job, mark) = {
                let mut scopes = scopes_of(&enabled.scopes);
                let job = scopes.jobs.get(scope).cloned();
                *scopes.deleting.entry(scope.clone()).or_insert(0) += 1;
                (
                    job,
                    DeletingMark {
                        scopes: Arc::clone(&enabled.scopes),
                        scope: scope.clone(),
                    },
                )
            };
            if let Some(job) = &job {
                job.cancel.cancel();
            }
            enabled
                .cleanup
                .delete_scope(scope.clone(), job.map(|job| job.ended), mark);
        }
    }

    /// Copies each filesystem snapshot of `from` into the empty scope `to`.
    #[allow(dead_code)]
    pub(crate) async fn duplicate_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        match &self.enabled {
            Some(enabled) => enabled.store.copy_scope(from, to).await,
            None => Ok(()),
        }
    }

    /// Waits until the job of `scope` decided and ended, for at most `confirmation_wait`. A
    /// manual update calls it when its admission gave [`SnapshotSkip::UploadInFlight`], and then
    /// asks again. The job holds the scope until it ends, after its retention, so the wait covers
    /// both.
    pub(crate) async fn wait_for_upload_of_scope(&self, scope: &SnapshotScope) {
        let Some(enabled) = &self.enabled else {
            return;
        };
        let Some(job) = enabled.jobs_of_scope().jobs.get(scope).cloned() else {
            return;
        };
        let mut decided = job.decided.subscribe();
        let decided_and_ended = async {
            let _ = decided.wait_for(|decision| decision.is_some()).await;
            job.ended.cancelled().await;
        };
        tokio::select! {
            () = decided_and_ended => {}
            () = enabled.clock.sleep(enabled.settings.confirmation_wait()) => {}
            () = enabled.shutdown.cancelled() => {}
        }
    }

    /// Stops the jobs and the clean-up queue, and waits for them. Call it once, when the executor
    /// shuts down.
    pub(crate) async fn shut_down(&self) {
        if let Some(enabled) = &self.enabled {
            enabled.shutdown.cancel();
            enabled.jobs.close();
            enabled.jobs.wait().await;
            enabled.store.shut_down().await;
        }
    }
}

impl EnabledSnapshots {
    /// Gives the jobs of the scopes. No lock is held across an await, so the map of a poisoned
    /// lock is used as it is.
    fn jobs_of_scope(&self) -> MutexGuard<'_, Scopes> {
        scopes_of(&self.scopes)
    }

    /// Reserves `scope` for a new job with the snapshot `name`. Gives `UploadInFlight` when a job
    /// of the scope exists, and `ScopeDeleting` when a delete of the scope is queued or runs.
    fn reserve(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<ScopeJob, SnapshotSkip> {
        let mut scopes = self.jobs_of_scope();
        if scopes.deleting.contains_key(scope) {
            return Err(SnapshotSkip::ScopeDeleting);
        }
        if scopes.jobs.contains_key(scope) {
            return Err(SnapshotSkip::UploadInFlight);
        }
        let job = ScopeJob {
            id: self.next_job.fetch_add(1, Ordering::Relaxed),
            name: name.clone(),
            cancel: CancellationToken::new(),
            ended: CancellationToken::new(),
            holds_slot: Arc::default(),
            decided: Arc::new(tokio::sync::watch::channel(None).0),
        };
        scopes.jobs.insert(scope.clone(), job.clone());
        Ok(job)
    }

    /// Frees `scope` when `job` is its job, and marks the job as decided and ended.
    fn end(&self, scope: &SnapshotScope, job: &ScopeJob) {
        let mut scopes = self.jobs_of_scope();
        if scopes
            .jobs
            .get(scope)
            .is_some_and(|current| current.id == job.id)
        {
            scopes.jobs.remove(scope);
        }
        drop(scopes);
        job.decide(JobDecision::Stopped);
        job.ended.cancel();
    }
}

/// The permission of one upload, with its name. Only an admission gives a name, and an admission
/// cannot be cloned, so each name reaches at most one capture.
pub(crate) struct UploadAdmission {
    enabled: Arc<EnabledSnapshots>,
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
    /// confirms. On `Confirmed` it applies retention. On `Superseded` it deletes the snapshot and
    /// runs no retention. On `Deferred` it keeps the snapshot and runs no retention, because a
    /// later start can confirm it. When the retries are used up, it confirms nothing.
    /// `forget_scope` or a shutdown stops the job at each step, also in its retention or its
    /// delete: it then sends nothing more and deletes nothing more. A confirmation that it
    /// already sent can still be appended.
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
            enabled: Arc::clone(&self.enabled),
            scope: self.scope.clone(),
            job,
            name: self.name.clone(),
            kind: self.kind,
        };
        self.enabled
            .jobs
            .spawn(upload.run_in_background(capture, parent, confirmer));
    }

    /// Uploads `capture` and waits until the store holds it. A manual update calls this before it
    /// writes its record. The scope stays reserved until the retention that the call gives runs
    /// or is dropped. When `stop` completes first, the save stops, the capture is discarded, and
    /// the call gives an error.
    pub(crate) async fn upload_now(
        mut self,
        capture: impl CapturedTree,
        stop: impl std::future::Future<Output = ()> + Send,
    ) -> Result<UpdateRetention, SnapshotStoreError> {
        let Some(job) = self.job.take() else {
            return Err(SnapshotStoreError::NotFound);
        };
        let upload = job::Upload {
            enabled: Arc::clone(&self.enabled),
            scope: self.scope.clone(),
            job,
            name: self.name.clone(),
            kind: self.kind,
        };
        upload
            .run_now(capture, stop)
            .await
            .map(|(info, upload)| UpdateRetention { upload, info })
    }
}

/// The retention after a manual-update snapshot that the store holds. The loop runs it after the
/// update record commits. Dropped, it deletes nothing and frees the scope.
pub(crate) struct UpdateRetention {
    upload: job::Upload,
    info: SnapshotInfo,
}

impl UpdateRetention {
    /// Applies retention in the background, under a slot of the uploads: it keeps the own
    /// snapshot and the newest older update snapshots, with the rules of periodic retention, and
    /// it never deletes `baseline`, the snapshot of the last successful manual update, whose
    /// record a start restores without a fallback. `forget_scope` or a shutdown stops it.
    pub(crate) fn run(self, baseline: Option<&FilesystemSnapshotName>) {
        let kept = baseline.and_then(|name| store_name(name).ok());
        let jobs = self.upload.enabled.jobs.clone();
        jobs.spawn(self.upload.retain_in_background(self.info, kept));
    }
}

impl Drop for UploadAdmission {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            self.enabled.end(&self.scope, &job);
        }
    }
}
