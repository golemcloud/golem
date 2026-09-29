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
//! store. It does not know the oplog entry types or the worker: a [`Confirm`] closure writes the
//! confirmation record.
//!
//! The rules are pure functions in `rules`. The registry keeps the state of the scopes and changes
//! it only through them, and the jobs are straight-line mechanisms that call a rule at each
//! decision.

mod cleanup;
mod job;
mod registry;
mod restore;
mod retention;
mod rules;
#[cfg(test)]
mod tests;

use crate::filesystem_snapshot::{
    ChangeDetection, FilesystemSnapshotStore, InvalidSnapshotName, SnapshotInfo, SnapshotName,
    SnapshotScope, SnapshotStoreError,
};
use crate::sandbox_filesystem::FilesystemVolume;
use crate::services::agent_filesystem::FilesystemCapture;
use crate::services::golem_config::{
    FilesystemPressureConfig, FilesystemSnapshotUploadConfig, FilesystemSnapshotsConfig,
};
use futures::future::BoxFuture;
use golem_common::model::oplog::FilesystemSnapshotName;
use registry::{DeleteTicket, JobTicket, Registry, WaitTicket};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, watch};
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
    /// A shutdown, or a call of `forget_scope` for the scope, stopped the job, or the admission
    /// ended without an upload.
    Stopped,
}

/// Writes the confirmation record of the snapshot with the name it gets, and gives what it
/// found. The invocation loop makes it from a weak handle to the worker, so the service never
/// holds a worker.
pub(crate) type Confirm =
    Box<dyn FnOnce(FilesystemSnapshotName) -> BoxFuture<'static, ConfirmOutcome> + Send>;

/// Whether the store holds the snapshot that a start asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartCheck {
    /// The store holds the whole snapshot.
    Stored,
    /// The store does not hold it, the check was skipped or stopped, or it failed.
    NotStored,
}

/// Why a manual update gets no admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateRefusal {
    /// The admission gave this answer.
    Skip(SnapshotSkip),
    /// A terminal interrupt ended the wait for a running upload.
    Interrupted,
}

/// Why a manual-update upload did not save.
#[derive(Debug)]
pub(crate) enum UploadNowError {
    /// A terminal interrupt, a shutdown, or a call of `forget_scope` for the scope stopped the
    /// save.
    Stopped,
    /// The save failed, after the retries when the error allows them.
    Store(SnapshotStoreError),
}

impl std::fmt::Display for UploadNowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => {
                formatter.write_str("the upload of the filesystem snapshot was stopped")
            }
            Self::Store(error) => write!(formatter, "{error}"),
        }
    }
}

/// The volume of the agent filesystems, which tells whether it has room for a new capture.
pub(crate) enum VolumeRoom {
    /// The volume has room when its free space reaches the targets of `pressure`. An unmanaged
    /// volume always has room.
    Pressure {
        volume: FilesystemVolume,
        pressure: FilesystemPressureConfig,
    },
    /// The volume always has room.
    #[cfg(any(test, feature = "test-utils"))]
    Unlimited,
}

impl VolumeRoom {
    async fn has_room(&self) -> bool {
        match self {
            Self::Pressure { volume, pressure } => {
                let space = crate::sandbox_filesystem::observe_space(volume)
                    .await
                    .inspect_err(|error| {
                        tracing::warn!(error = %error, "Failed to observe the free space of the volume")
                    })
                    .ok();
                rules::has_room(space.as_ref(), pressure)
            }
            #[cfg(any(test, feature = "test-utils"))]
            Self::Unlimited => true,
        }
    }
}

/// A tree that a capture copied, which an upload saves and then discards.
pub(crate) struct CapturedTree {
    /// The directory that the store saves.
    directory: Arc<Path>,
    /// Removes the directory with all that is in it.
    discard: BoxFuture<'static, ()>,
}

impl CapturedTree {
    #[cfg(test)]
    fn new(directory: Arc<Path>, discard: BoxFuture<'static, ()>) -> Self {
        Self { directory, discard }
    }
}

impl From<FilesystemCapture> for CapturedTree {
    fn from(capture: FilesystemCapture) -> Self {
        Self {
            directory: Arc::from(capture.directory()),
            discard: Box::pin(async move {
                if let Err(error) = capture.discard().await {
                    tracing::warn!(error = %error, "Failed to discard a filesystem capture");
                }
            }),
        }
    }
}

/// Makes the name of the store from a name of a record. A name of a record is always a valid
/// name of the store, and the error only guards against a change of either rule.
pub(crate) fn store_name(
    name: &FilesystemSnapshotName,
) -> Result<SnapshotName, InvalidSnapshotName> {
    SnapshotName::new(name.as_str())
}

/// Where the store of an enabled service comes from.
pub struct StoreSource(Source);

enum Source {
    /// The store that the configuration names, on the blob storage of the executor, with the
    /// volume whose room decides the admissions.
    Configured(
        Arc<dyn golem_service_base::storage::blob::BlobStorage>,
        VolumeRoom,
    ),
    /// A store that a test gives, with its upload settings. It bypasses the configuration, and
    /// its volume always has room.
    #[cfg(feature = "test-utils")]
    Given(
        Arc<dyn FilesystemSnapshotStore>,
        FilesystemSnapshotUploadConfig,
    ),
}

impl StoreSource {
    /// The store that the configuration names, on `blob_storage`. `room` is the volume of the
    /// agent filesystems, which must have room for an admission.
    pub(crate) fn configured(
        blob_storage: Arc<dyn golem_service_base::storage::blob::BlobStorage>,
        room: VolumeRoom,
    ) -> Self {
        Self(Source::Configured(blob_storage, room))
    }

    /// `store` with `settings`, on any storage mode and whatever the configuration says.
    #[cfg(feature = "test-utils")]
    pub(crate) fn given(
        store: Arc<dyn FilesystemSnapshotStore>,
        settings: FilesystemSnapshotUploadConfig,
    ) -> Self {
        Self(Source::Given(store, settings))
    }
}

/// The filesystem snapshots of the agents of this executor.
///
/// A disabled service answers each admission with [`SnapshotSkip::Disabled`], each restore with
/// [`SnapshotsDisabled`] and each start with [`StartCheck::NotStored`].
pub struct AgentFilesystemSnapshots {
    core: Option<Arc<Core>>,
}

/// What an enabled service holds.
struct Core {
    store: Arc<dyn FilesystemSnapshotStore>,
    settings: FilesystemSnapshotUploadConfig,
    /// The slots of the store operations that save or delete.
    uploads: Arc<Semaphore>,
    /// The slots of the restores.
    restores: Arc<Semaphore>,
    registry: Arc<Registry>,
    room: VolumeRoom,
    cleanup: cleanup::CleanupQueue,
    /// Cancelled when the executor shuts down. The stop of each job is a child of it.
    shutdown: CancellationToken,
    /// The upload jobs, so that a shutdown can wait for them.
    jobs: TaskTracker,
}

impl AgentFilesystemSnapshots {
    /// Makes the service that the configuration asks for, as [`rules::binding`] says, with the
    /// store of `source`. `managed_storage` tells whether the sandbox provisioning uses managed
    /// XFS storage. When `shutdown` ends, the service stops its jobs and shuts the store down.
    /// This is the only constructor of the service.
    pub(crate) fn bind(
        config: &FilesystemSnapshotsConfig,
        source: StoreSource,
        managed_storage: bool,
        shutdown: &crate::services::shutdown::Shutdown,
    ) -> Result<Arc<Self>, String> {
        let snapshots = Arc::new(match source.0 {
            Source::Configured(blob_storage, room) => {
                match rules::binding(config, managed_storage)? {
                    rules::Binding::Disabled => Self::disabled(),
                    rules::Binding::Managed(config) => Self::enabled(
                        crate::filesystem_snapshot::managed_store(blob_storage, config),
                        config.uploads().clone(),
                        room,
                        shutdown.token(),
                    ),
                }
            }
            #[cfg(feature = "test-utils")]
            Source::Given(store, settings) => {
                Self::enabled(store, settings, VolumeRoom::Unlimited, shutdown.token())
            }
        });
        let token = shutdown.token();
        let stopping = Arc::clone(&snapshots);
        shutdown.spawn(async move {
            token.cancelled().await;
            stopping.shut_down().await;
        });
        Ok(snapshots)
    }

    /// Makes a service that keeps no filesystem snapshots.
    fn disabled() -> Self {
        Self { core: None }
    }

    /// Makes a service over `store` with `settings`.
    fn enabled(
        store: Arc<dyn FilesystemSnapshotStore>,
        settings: FilesystemSnapshotUploadConfig,
        room: VolumeRoom,
        shutdown: CancellationToken,
    ) -> Self {
        let uploads = Arc::new(Semaphore::new(settings.max_concurrent_uploads().get()));
        let jobs = TaskTracker::new();
        let cleanup = cleanup::CleanupQueue::start(
            Arc::clone(&store),
            Arc::clone(&uploads),
            settings.upload_retry().clone(),
            shutdown.clone(),
            &jobs,
        );
        Self {
            core: Some(Arc::new(Core {
                restores: Arc::new(Semaphore::new(settings.max_concurrent_restores().get())),
                store,
                settings,
                uploads,
                registry: Arc::default(),
                room,
                cleanup,
                shutdown,
                jobs,
            })),
        }
    }

    /// Whether this executor keeps filesystem snapshots.
    pub(crate) fn is_enabled(&self) -> bool {
        self.core.is_some()
    }

    /// Asks for an upload of a periodic snapshot of the agent of `scope`, before the guest saves.
    ///
    /// The admission holds a new name. While it exists, and while the upload that it starts
    /// runs, each other admission of the scope gives [`SnapshotSkip::UploadInFlight`]. A dropped
    /// admission frees the scope and writes nothing durable.
    pub(crate) async fn admit_periodic(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Admission, SnapshotSkip> {
        let core = self.core.as_ref().ok_or(SnapshotSkip::Disabled)?;
        Core::admit(core, scope, SnapshotKind::Periodic)
            .await
            .map_err(|refusal| refusal.skip)
    }

    /// Asks for an upload of a manual-update snapshot of the agent of `scope`. When an upload of
    /// the scope runs, the call waits once for its end, for at most `confirmation_wait`, and asks
    /// again. A shutdown ends the wait like its limit does, and a terminal interrupt that
    /// `interrupt` reports ends it with [`UpdateRefusal::Interrupted`].
    pub(crate) async fn admit_update(
        &self,
        scope: &SnapshotScope,
        interrupt: watch::Receiver<bool>,
    ) -> Result<Admission, UpdateRefusal> {
        let core = self
            .core
            .as_ref()
            .ok_or(UpdateRefusal::Skip(SnapshotSkip::Disabled))?;
        let refusal = match Core::admit(core, scope, SnapshotKind::Update).await {
            Ok(admission) => return Ok(admission),
            Err(refusal) => refusal,
        };
        let rules::UpdateAdmit::WaitForEnd(id) = rules::update_admission(refusal) else {
            return Err(UpdateRefusal::Skip(refusal.skip));
        };
        tokio::select! {
            () = core.registry.until_job_gone(scope, id) => {}
            () = tokio::time::sleep(core.settings.confirmation_wait()) => {}
            () = core.shutdown.cancelled() => {}
            () = job::interrupt_raised(interrupt) => return Err(UpdateRefusal::Interrupted),
        }
        Core::admit(core, scope, SnapshotKind::Update)
            .await
            .map_err(|refusal| UpdateRefusal::Skip(refusal.skip))
    }

    /// Tells whether the store holds the whole snapshot `name` of `scope`, before a start
    /// confirms it.
    ///
    /// When an upload of the name runs on this executor and holds a slot of the uploads, the call
    /// first waits for its decision, for at most `confirmation_wait`. Then, unless the upload gave
    /// `Superseded` or a terminal interrupt waits, it asks the store once, for at most what is
    /// left of `confirmation_wait` after a wait, or at most `store_check_limit` without one. A
    /// terminal interrupt that `interrupt` reports ends the wait and the check. The call holds no
    /// slot and no lock while it waits.
    pub(crate) async fn prepare_start(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
        interrupt: watch::Receiver<bool>,
    ) -> StartCheck {
        let Some(core) = &self.core else {
            return StartCheck::NotStored;
        };
        let started = tokio::time::Instant::now();
        let limit = core.settings.confirmation_wait();
        let (waited, decision) = match WaitTicket::start_wait(&core.registry, scope, name) {
            Ok(ticket) => {
                let decision = tokio::select! {
                    decision = ticket.decided() => Some(decision),
                    () = tokio::time::sleep(limit) => None,
                    () = job::interrupt_raised(interrupt.clone()) => return StartCheck::NotStored,
                };
                (Some(started.elapsed()), decision)
            }
            Err(decision) => (None, decision),
        };
        let limit = match rules::store_check(
            decision,
            *interrupt.borrow(),
            waited,
            rules::StoreCheckLimits {
                confirmation_wait: limit,
                store_check_limit: core.settings.store_check_limit(),
            },
        ) {
            rules::StoreCheck::Skip => return StartCheck::NotStored,
            rules::StoreCheck::Stat(limit) => limit,
        };
        let Ok(store_name) = store_name(name) else {
            return StartCheck::NotStored;
        };
        tokio::select! {
            biased;
            () = job::interrupt_raised(interrupt) => StartCheck::NotStored,
            stat = tokio::time::timeout(limit, core.store.stat(scope, &store_name)) => match stat {
                Ok(Ok(Some(_))) => StartCheck::Stored,
                Ok(Ok(None)) | Err(_) => StartCheck::NotStored,
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "Failed to check a filesystem snapshot before a start");
                    StartCheck::NotStored
                }
            },
        }
    }

    /// Gives the restore of the filesystem snapshot `name` of `scope`. The restore does its work
    /// when the lifecycle calls it, and it waits for a slot of the restores then. This call asks
    /// the store for nothing and waits for nothing.
    pub(crate) fn restore(
        &self,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<StoreRestore, SnapshotsDisabled> {
        let core = self.core.as_ref().ok_or(SnapshotsDisabled)?;
        Ok(StoreRestore::new(
            Arc::clone(&core.store),
            scope.clone(),
            name.clone(),
            Arc::clone(&core.restores),
        ))
    }

    /// Deletes the filesystem snapshots `names` of `scope` in the background. The call returns at
    /// once and cannot fail. The clean-up retries, and after the retries it logs and counts the
    /// names that stay.
    #[allow(dead_code)]
    pub(crate) fn forget(&self, scope: &SnapshotScope, names: Box<[FilesystemSnapshotName]>) {
        if let Some(core) = &self.core {
            core.cleanup.delete(scope.clone(), names);
        }
    }

    /// Deletes the scope in the background, after the job of the scope ended. The call stops
    /// that job first, so it sends nothing more; a confirmation that it already sent can still be
    /// appended. The call returns at once and cannot fail.
    ///
    /// Until the delete ends, with success or with an error, an admission of the scope gives
    /// [`SnapshotSkip::ScopeDeleting`].
    #[allow(dead_code)]
    pub(crate) fn forget_scope(&self, scope: &SnapshotScope) {
        if let Some(core) = &self.core {
            let (ticket, stop) = DeleteTicket::forget_scope(&core.registry, scope);
            if let Some(stop) = stop {
                stop.cancel();
            }
            core.cleanup.delete_scope(scope.clone(), ticket);
        }
    }

    /// Copies each filesystem snapshot of `from` into the empty scope `to`.
    #[allow(dead_code)]
    pub(crate) async fn duplicate_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        match &self.core {
            Some(core) => core.store.copy_scope(from, to).await,
            None => Ok(()),
        }
    }

    /// Stops the jobs and the clean-up queue, and waits for them, then shuts the store down.
    async fn shut_down(&self) {
        if let Some(core) = &self.core {
            core.shutdown.cancel();
            core.jobs.close();
            core.jobs.wait().await;
            core.store.shut_down().await;
        }
    }
}

impl Core {
    /// Admits a job of `kind` for `scope` with a new name. The error carries the job that runs
    /// for the scope.
    async fn admit(
        core: &Arc<Self>,
        scope: &SnapshotScope,
        kind: SnapshotKind,
    ) -> Result<Admission, rules::Refusal> {
        let room = core.room.has_room().await;
        let name = match kind {
            SnapshotKind::Periodic => FilesystemSnapshotName::periodic(),
            SnapshotKind::Update => FilesystemSnapshotName::update(),
        };
        let ticket = JobTicket::admit(
            &core.registry,
            scope,
            &name,
            core.shutdown.child_token(),
            room,
        )?;
        Ok(Admission {
            core: Arc::clone(core),
            ticket,
            name,
            kind,
        })
    }
}

/// The permission of one upload, with its name. Only an admission gives a name, and an admission
/// cannot be cloned, so each name reaches at most one capture. Dropped, it frees the scope.
pub(crate) struct Admission {
    core: Arc<Core>,
    ticket: JobTicket,
    name: FilesystemSnapshotName,
    kind: SnapshotKind,
}

impl Admission {
    /// The name that the admission made.
    pub(crate) fn name(&self) -> &FilesystemSnapshotName {
        &self.name
    }

    /// How long the capture of the upload waits for open file calls.
    pub(crate) fn capture_wait(&self) -> Duration {
        self.core.settings.capture_wait()
    }

    /// Uploads `tree` in the background, then confirms it with `confirm`.
    ///
    /// The job waits for a slot of the uploads, saves with retries, discards the tree, and
    /// confirms. On `Confirmed` it applies retention. On `Superseded` it deletes the snapshot and
    /// runs no retention. On `Deferred` it keeps the snapshot and runs no retention, because a
    /// later start can confirm it. When the retries are used up, it confirms nothing.
    /// A shutdown, or a call of `forget_scope` for the scope, stops the job at each step, also in
    /// its retention or its delete: it then sends nothing more and deletes nothing more. A confirmation that it
    /// already sent can still be appended.
    pub(crate) fn submit(
        self,
        tree: CapturedTree,
        parent: Option<(FilesystemSnapshotName, ChangeDetection)>,
        confirm: Confirm,
    ) {
        let jobs = self.core.jobs.clone();
        jobs.spawn(job::run_job(self, tree, parent, confirm));
    }
}

/// A manual-update snapshot that the store holds. The loop retains it after the update record
/// commits. Dropped, it deletes nothing and frees the scope.
#[must_use = "a dropped saved update runs no retention; retain it after the update record commits"]
pub(crate) struct SavedUpdate {
    core: Arc<Core>,
    ticket: JobTicket,
    name: FilesystemSnapshotName,
    info: SnapshotInfo,
}

impl SavedUpdate {
    /// Applies retention in the background, under a slot of the uploads: it keeps the own
    /// snapshot and the newest older update snapshots, with the rules of periodic retention, and
    /// it never deletes `kept`, the snapshot of the last successful manual update, whose record a
    /// start restores without a fallback. A shutdown, or a call of `forget_scope` for the scope,
    /// stops it.
    pub(crate) fn retain(self, kept: Option<&FilesystemSnapshotName>) {
        let kept = kept.and_then(|name| store_name(name).ok());
        let Self {
            core,
            ticket,
            name,
            info,
        } = self;
        let jobs = core.jobs.clone();
        jobs.spawn(async move {
            job::retain(
                &core,
                &ticket,
                &name,
                SnapshotKind::Update,
                &info,
                kept.as_ref(),
                None,
            )
            .await;
        });
    }
}
