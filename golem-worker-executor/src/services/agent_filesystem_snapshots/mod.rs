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
//! The rules are pure functions in `rules`. The registry keeps the state of the agents and changes
//! it only through them, and the jobs are straight-line mechanisms that call a rule at each
//! decision. Each call to the store goes through `store_calls`, which is the only owner of the
//! store.

mod cleanup;
mod job;
mod registry;
mod retention;
mod rules;
mod store_calls;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-utils"))]
use crate::filesystem_snapshot::FilesystemSnapshotStore;
use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, InvalidSnapshotName, SaveError, SnapshotInfo,
    SnapshotName,
};
use crate::sandbox_filesystem::FilesystemVolume;
use crate::services::agent_filesystem::FilesystemCapture;
use crate::services::golem_config::{
    FilesystemPressureConfig, FilesystemSnapshotUploadConfig, FilesystemSnapshotsConfig,
};
use futures::future::BoxFuture;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::FilesystemSnapshotName;
use registry::{JobTicket, Registry, WaitTicket};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use store_calls::{StoreCalls, StoreOf};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub(crate) use store_calls::{StoreKey, StoreRestore};

/// The kind of a filesystem snapshot. It gives the prefix of the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotKind {
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
    /// An upload of the agent runs now. The loop skips the snapshot.
    UploadInFlight,
    /// The volume has less free space than the pressure target. The loop skips the snapshot.
    VolumeUnderPressure,
    /// A delete of all snapshots of the agent is queued or runs. The loop skips the snapshot.
    DeletingAllSnapshots,
}

impl std::fmt::Display for SnapshotSkip {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UploadInFlight => "an upload of a filesystem snapshot of the agent runs now",
            Self::VolumeUnderPressure => "the volume of the agent filesystems is under pressure",
            Self::DeletingAllSnapshots => "the filesystem snapshots of the agent are being deleted",
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
enum JobDecision {
    /// The confirmation gave this outcome.
    Confirmed(ConfirmOutcome),
    /// The save failed after its runs. The store does not hold the snapshot.
    SaveFailed,
    /// A shutdown, or a call of `delete_all_snapshots` for the agent, stopped the job, or the
    /// admission ended without an upload.
    Stopped,
    /// A newer admission replaced the job while it waited for its next run after a failed run.
    /// The job never confirms its snapshot.
    Replaced,
}

/// What a confirmation found.
///
/// `Confirmed` carries the filesystem snapshot names of the automatic snapshot records that a
/// start of the agent can select, as `snapshot_selection::start_candidates` gives them from the
/// status that the worker holds after the append of the confirmation record returned. Retention
/// keeps these names whatever their age. An entry that the status folds between the append and
/// that read, such as the `SuccessfulUpdate` of an automatic update, can change the names; count
/// retention then still keeps the newest older names. While the job holds the admission of its
/// agent:
///
/// * no other automatic snapshot record can be written for the agent: each such record needs an
///   admission, and the admission answers `UploadInFlight`;
/// * no manual-update record can be written: a manual update first stops the deletes of the job
///   and waits for its end.
///
/// A revert can still make an older record a candidate. It stops the deletes that the job has not
/// sent yet, but a delete already sent runs to its end and can remove that name. A start then
/// falls back past it. A path that writes an automatic snapshot record without an admission
/// breaks the first point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Confirmation {
    Confirmed {
        selectable: Box<[FilesystemSnapshotName]>,
    },
    Superseded,
    Deferred,
}

impl Confirmation {
    /// The outcome of the confirmation, without the names.
    pub(crate) fn outcome(&self) -> ConfirmOutcome {
        match self {
            Self::Confirmed { .. } => ConfirmOutcome::Confirmed,
            Self::Superseded => ConfirmOutcome::Superseded,
            Self::Deferred => ConfirmOutcome::Deferred,
        }
    }

    /// The names that a start can select, which retention keeps. Only a confirmed snapshot has
    /// them.
    fn selectable(&self) -> &[FilesystemSnapshotName] {
        match self {
            Self::Confirmed { selectable } => selectable,
            Self::Superseded | Self::Deferred => &[],
        }
    }
}

/// Writes the confirmation record of the snapshot with the name it gets, and gives what it
/// found. The invocation loop makes it from a weak handle to the worker, so the service never
/// holds a worker.
pub(crate) type Confirm =
    Box<dyn FnOnce(FilesystemSnapshotName) -> BoxFuture<'static, Confirmation> + Send>;

/// Whether the store holds the snapshot that a start asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartCheck {
    /// The store holds the whole snapshot.
    Stored,
    /// The store does not hold it, the check was skipped or stopped, or it failed.
    NotStored,
}

/// What an admission of a periodic snapshot gave.
pub(crate) enum Admitted {
    /// The upload of a snapshot with a new name.
    Upload(Admission),
    /// The agent keeps no filesystem snapshots here: the service is disabled, or the agent keeps
    /// no files. The record gets no name.
    WithoutName,
    /// The snapshot is skipped for this reason.
    Skip(SnapshotSkip),
}

#[cfg(test)]
impl Admitted {
    /// The admission of the upload. Panics on another answer.
    pub(crate) fn unwrap(self) -> Admission {
        match self {
            Self::Upload(admission) => admission,
            Self::WithoutName => panic!("the admission gave no name"),
            Self::Skip(skip) => panic!("the admission skipped the snapshot: {skip}"),
        }
    }

    /// Whether the admission gave an upload.
    pub(crate) fn is_ok(&self) -> bool {
        matches!(self, Self::Upload(_))
    }

    /// The reason of a skip.
    pub(crate) fn err(self) -> Option<SnapshotSkip> {
        match self {
            Self::Skip(skip) => Some(skip),
            Self::Upload(_) | Self::WithoutName => None,
        }
    }

    /// Whether the admission gave no name.
    pub(crate) fn without_name(&self) -> bool {
        matches!(self, Self::WithoutName)
    }

    /// The upload of the admission, as a `Result` with the reason of a skip.
    pub(crate) fn map<T>(
        self,
        map: impl FnOnce(Admission) -> T,
    ) -> Result<T, Option<SnapshotSkip>> {
        match self {
            Self::Upload(admission) => Ok(map(admission)),
            Self::WithoutName => Err(None),
            Self::Skip(skip) => Err(Some(skip)),
        }
    }
}

/// Why a manual update gets no upload, for the tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateNotAdmitted {
    WithoutName,
    Skip(SnapshotSkip),
    Interrupted,
}

#[cfg(test)]
impl UpdateAdmitted {
    /// The admission of the upload. Panics on another answer.
    pub(crate) fn unwrap(self) -> Admission {
        match self {
            Self::Upload(admission) => admission,
            other => panic!("the update gave no upload: {:?}", other.err()),
        }
    }

    /// Whether the admission gave an upload.
    pub(crate) fn is_ok(&self) -> bool {
        matches!(self, Self::Upload(_))
    }

    /// Why the update gets no upload.
    pub(crate) fn err(self) -> Option<UpdateNotAdmitted> {
        match self {
            Self::Upload(_) => None,
            Self::WithoutName => Some(UpdateNotAdmitted::WithoutName),
            Self::Skip(skip) => Some(UpdateNotAdmitted::Skip(skip)),
            Self::Interrupted => Some(UpdateNotAdmitted::Interrupted),
        }
    }
}

/// What an admission of a manual-update snapshot gave.
pub(crate) enum UpdateAdmitted {
    /// The upload of a snapshot with a new name.
    Upload(Admission),
    /// The agent keeps no filesystem snapshots here: the service is disabled, or the agent keeps
    /// no files. The record gets no name.
    WithoutName,
    /// The update gets no snapshot for this reason.
    Skip(SnapshotSkip),
    /// A terminal interrupt ended the wait for a running upload.
    Interrupted,
}

/// Why a manual-update upload did not save.
#[derive(Debug)]
pub(crate) enum UploadNowError {
    /// A terminal interrupt, a lost shard, a caller that stopped waiting, a shutdown, or a call
    /// of `delete_all_snapshots` for the agent stopped the save.
    Stopped,
    /// The save failed.
    Store(SaveError),
    /// The deadline of the admission passed while the upload waited for a running save of the
    /// agent.
    SaveRunning,
    /// The deadline of the admission passed while the upload waited for a slot of the uploads.
    NoSlot,
}

impl std::fmt::Display for UploadNowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => {
                formatter.write_str("the upload of the filesystem snapshot was stopped")
            }
            Self::Store(error) => write!(formatter, "{error}"),
            Self::SaveRunning => formatter.write_str(
                "an earlier save of a filesystem snapshot of the agent still runs after the wait",
            ),
            Self::NoSlot => formatter
                .write_str("no slot of the uploads of filesystem snapshots was free in the wait"),
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
fn store_name(name: &FilesystemSnapshotName) -> Result<SnapshotName, InvalidSnapshotName> {
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
/// A disabled service answers each admission without a name, each restore with
/// [`SnapshotsDisabled`] and each start with [`StartCheck::NotStored`].
pub struct AgentFilesystemSnapshots {
    core: Option<Arc<Core>>,
    /// The captures of the agent filesystems of this executor by outcome: a count of the same
    /// captures as the metric, which all executors of the process share. Only tests read it.
    #[cfg(any(test, feature = "test-utils"))]
    captures: std::sync::Mutex<std::collections::HashMap<&'static str, u64>>,
}

/// What an enabled service holds.
struct Core {
    /// The calls to the store. Nothing else holds the store.
    calls: Arc<StoreCalls>,
    settings: FilesystemSnapshotUploadConfig,
    registry: Arc<Registry>,
    room: VolumeRoom,
    /// Cancelled when the executor shuts down. The stop of each job is a child of it.
    shutdown: CancellationToken,
    /// The jobs, the store calls and the workers of the clean-ups, so that a shutdown can wait
    /// for them.
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
                        StoreOf::Managed(blob_storage, config),
                        config.uploads().clone(),
                        room,
                        shutdown.token(),
                    ),
                }
            }
            #[cfg(feature = "test-utils")]
            Source::Given(store, settings) => Self::enabled(
                StoreOf::Given(store),
                settings,
                VolumeRoom::Unlimited,
                shutdown.token(),
            ),
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
        Self {
            core: None,
            #[cfg(any(test, feature = "test-utils"))]
            captures: std::sync::Mutex::default(),
        }
    }

    /// Makes a service over `store` with `settings`, with the workers of its clean-ups.
    fn enabled(
        store: StoreOf<'_>,
        settings: FilesystemSnapshotUploadConfig,
        room: VolumeRoom,
        shutdown: CancellationToken,
    ) -> Self {
        let jobs = TaskTracker::new();
        let registry = Arc::new(Registry::new(rules::Limits::retaining(
            settings.retained_periodic_snapshots(),
        )));
        let calls = Arc::new(StoreCalls::bind(
            store,
            &settings,
            Arc::clone(&registry),
            shutdown.clone(),
            jobs.clone(),
        ));
        cleanup::start(
            &calls,
            &registry,
            &shutdown,
            &jobs,
            settings.max_concurrent_uploads().get(),
        );
        Self {
            core: Some(Arc::new(Core {
                calls,
                settings,
                registry,
                room,
                shutdown,
                jobs,
            })),
            #[cfg(any(test, feature = "test-utils"))]
            captures: std::sync::Mutex::default(),
        }
    }

    /// Whether this executor keeps filesystem snapshots.
    pub(crate) fn is_enabled(&self) -> bool {
        self.core.is_some()
    }

    /// Records a capture of an agent filesystem with the metric label `outcome`, which stopped
    /// the file calls for `elapsed`, in the metric. A test build also counts it for this
    /// executor.
    pub(crate) fn record_capture(&self, outcome: &'static str, elapsed: Duration) {
        crate::metrics::filesystem_snapshots::record_capture(outcome, elapsed);
        #[cfg(any(test, feature = "test-utils"))]
        {
            *self
                .captures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(outcome)
                .or_default() += 1;
        }
    }

    /// The number of captures of agent filesystems on this executor with the metric label
    /// `outcome`.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn captures(&self, outcome: &str) -> u64 {
        self.captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(outcome)
            .copied()
            .unwrap_or_default()
    }

    /// Asks for an upload of a periodic snapshot of the agent `agent` in `mode`, before the guest
    /// saves.
    ///
    /// The admission holds a new name. While it exists, and while the upload that it starts
    /// runs, each other admission of the agent gives [`SnapshotSkip::UploadInFlight`] at once,
    /// unless the upload is a periodic one that waits for its next run after a failed run: a new
    /// admission replaces such an upload, which then ends at once, never confirms its snapshot,
    /// and discards its capture. A dropped admission frees the agent and writes nothing durable.
    /// An agent that keeps no files, such as an ephemeral agent, gets
    /// [`Admitted::WithoutName`], as a disabled service gives.
    pub(crate) async fn admit_periodic(&self, agent: &AgentSnapshots, mode: AgentMode) -> Admitted {
        let Some(core) = self.enabled_for(mode) else {
            return Admitted::WithoutName;
        };
        match Core::admit(core, agent, SnapshotKind::Periodic, None).await {
            Ok(admission) => Admitted::Upload(admission),
            Err(refusal) => Admitted::Skip(refusal.skip),
        }
    }

    /// Asks for an upload of a manual-update snapshot of the agent `agent` in `mode`. When a job of
    /// the agent runs, a periodic upload or an update job, the call stops the deletes that the
    /// running job makes after its save, and waits until the job is gone or an admission can
    /// replace it: a periodic job that waits for its next run after a failed run and has not
    /// decided. Then it asks again, and an admission replaces such a job. So the update waits for
    /// at most the one run in flight, a write of that run that can still land, and the
    /// confirmation of the running job. The whole wait of
    /// the update, here and in its upload, ends `confirmation_wait` after this call started: that
    /// is the deadline of the admission, and the admission at the deadline gives its refusal. A
    /// shutdown ends the wait like the deadline does, and a terminal interrupt that `interrupt`
    /// reports ends it with [`UpdateAdmitted::Interrupted`]. An agent that keeps no files gets
    /// [`UpdateAdmitted::WithoutName`], as a disabled service gives.
    pub(crate) async fn admit_update(
        &self,
        agent: &AgentSnapshots,
        mode: AgentMode,
        interrupt: watch::Receiver<bool>,
    ) -> UpdateAdmitted {
        let Some(core) = self.enabled_for(mode) else {
            return UpdateAdmitted::WithoutName;
        };
        let deadline = tokio::time::Instant::now() + core.settings.confirmation_wait();
        let asks = futures::stream::unfold(Some(None), |stopped| {
            let interrupt = interrupt.clone();
            async move {
                let stopped: Option<rules::JobId> = stopped?;
                let refusal = match Core::admit(core, agent, SnapshotKind::Update, Some(deadline))
                    .await
                {
                    Ok(admission) => return Some((Some(UpdateAdmitted::Upload(admission)), None)),
                    Err(refusal) => refusal,
                };
                let skip = refusal.skip;
                let rules::UpdateAdmit::WaitForEndOrFailure {
                    running,
                    stop_deletes,
                } = rules::update_admission(
                    refusal,
                    tokio::time::Instant::now() >= deadline,
                    core.shutdown.is_cancelled(),
                    stopped,
                )
                else {
                    return Some((Some(UpdateAdmitted::Skip(skip)), None));
                };
                // The running job ends its save and its confirmation, and deletes nothing more.
                if stop_deletes {
                    running.retention_stop.cancel();
                }
                tokio::select! {
                    () = core.registry.until_job_gone_or_replaceable(agent, running.id) => {}
                    () = tokio::time::sleep_until(deadline) => {}
                    () = core.shutdown.cancelled() => {}
                    () = job::interrupt_raised(interrupt) => {
                        return Some((Some(UpdateAdmitted::Interrupted), None));
                    }
                }
                Some((None, Some(Some(running.id))))
            }
        });
        futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
            asks,
            |answer| async move { answer }
        )))
        .await
        .unwrap_or(UpdateAdmitted::Skip(SnapshotSkip::UploadInFlight))
    }

    /// The core of an enabled service, for an agent in `mode` that keeps files.
    fn enabled_for(&self, mode: AgentMode) -> Option<&Arc<Core>> {
        self.core.as_ref().filter(|_| rules::keeps_files(mode))
    }

    /// Tells whether the store holds the whole snapshot `name` of `agent`, before a start
    /// confirms it.
    ///
    /// When an upload of the name runs on this executor and has started saving, the call
    /// first waits for its decision, for at most `confirmation_wait`. Then, unless the upload gave
    /// `Superseded` or a terminal interrupt waits, it asks the store once, for at most what is
    /// left of `confirmation_wait` after a wait, or at most `store_check_limit` without one. A
    /// terminal interrupt that `interrupt` reports ends the wait and the check. The call holds no
    /// slot and no lock while it waits.
    pub(crate) async fn prepare_start(
        &self,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        interrupt: watch::Receiver<bool>,
    ) -> StartCheck {
        let Some(core) = &self.core else {
            return StartCheck::NotStored;
        };
        let started = tokio::time::Instant::now();
        let limit = core.settings.confirmation_wait();
        let (waited, decision) = match WaitTicket::start_wait(&core.registry, agent, name) {
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
            stat = core.calls.stat(agent, &store_name, limit) => match stat {
                Ok(Some(_)) => StartCheck::Stored,
                Ok(None) => StartCheck::NotStored,
                Err(error) => {
                    tracing::warn!(error = %error, "Failed to check a filesystem snapshot before a start");
                    StartCheck::NotStored
                }
            },
        }
    }

    /// Gives the restore of the filesystem snapshot `name` of `agent`. The restore does its work
    /// when the lifecycle calls it, and it waits for a slot of the restores then. This call asks
    /// the store for nothing and waits for nothing.
    pub(crate) fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> Result<StoreRestore, SnapshotsDisabled> {
        let core = self.core.as_ref().ok_or(SnapshotsDisabled)?;
        Ok(core.calls.restore(agent, name))
    }

    /// Deletes the filesystem snapshots `names` of `agent` in the background, after every store
    /// call of the agent ended. `names` comes newest first. When the name of the job of the agent
    /// is one of them, the call stops that job. The call returns at once and cannot fail. The
    /// request merges into the pending work of the agent, which is bounded; names past the bound
    /// are counted as leaked. The clean-up retries, and after the retries it logs and counts the
    /// names that stay. Production code deletes names through [`RevertHold::delete_snapshots`].
    #[cfg(test)]
    pub(crate) fn delete_snapshots(
        &self,
        agent: &AgentSnapshots,
        names: Box<[FilesystemSnapshotName]>,
    ) {
        if let Some(core) = &self.core {
            core.request_names(agent, &names);
        }
    }

    /// Deletes all filesystem snapshots of `agent` in the background, after every job and every
    /// store call of the agent ended. The call stops the job of the agent first, so it sends
    /// nothing more; a confirmation that it already sent can still be appended. The call returns
    /// at once and cannot fail.
    ///
    /// Until the delete ends, with success or with an error, an admission of the agent gives
    /// [`SnapshotSkip::DeletingAllSnapshots`], unless the bound of the clean-ups drops the request,
    /// which is counted as `overflow`. An agent in `mode` that keeps no files has no
    /// snapshots, and the call does nothing for it.
    pub(crate) fn delete_all_snapshots(&self, agent: &AgentSnapshots, mode: AgentMode) {
        if let Some(core) = self.enabled_for(mode) {
            let requested = core
                .registry
                .apply(|state| rules::request_all(state, agent));
            Self::requested(agent, requested);
        }
    }

    /// Begins a revert of `agent`: until the hold that this gives ends, the deletes of the jobs of
    /// the agent stop. The job that runs now sends no further delete, a delete that it sent
    /// answers at once and its store call ends at its next wait between runs, and each job that
    /// an admission gives while the hold lives deletes nothing. The call returns at once and
    /// cannot fail.
    pub(crate) fn begin_revert(&self, agent: &AgentSnapshots) -> RevertHold {
        let core = self.core.clone();
        if let Some(core) = &core
            && let Some(retention_stop) = core
                .registry
                .apply(|state| rules::revert_began(state, agent))
        {
            retention_stop.cancel();
        }
        RevertHold {
            core,
            agent: agent.clone(),
        }
    }

    /// Stops the job that a request stops, and counts a request that the bound dropped.
    fn requested(agent: &AgentSnapshots, requested: rules::Requested) {
        if let Some(stop) = requested.stop {
            stop.cancel();
        }
        if requested.overflow {
            tracing::warn!(
                agent = ?agent,
                "The clean-up queue of filesystem snapshots is full; a clean-up is lost"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup("overflow");
        }
    }

    /// Copies each filesystem snapshot of `from` into `to`, which holds none.
    #[allow(dead_code)]
    pub(crate) async fn copy_all_snapshots(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), CallError> {
        match &self.core {
            Some(core) => core.calls.copy(from, to).await,
            None => Ok(()),
        }
    }

    /// Stops the jobs, the store calls and the clean-ups in the order of
    /// [`StoreCalls::shut_down`], and waits for them.
    async fn shut_down(&self) {
        if let Some(core) = &self.core {
            core.calls.shut_down().await;
        }
    }
}

impl Core {
    /// Requests the delete of the snapshots `names` of `agent`.
    fn request_names(&self, agent: &AgentSnapshots, names: &[FilesystemSnapshotName]) {
        let names = names
            .iter()
            .filter_map(|name| store_name(name).ok())
            .collect::<Box<[_]>>();
        let requested = self
            .registry
            .apply(|state| rules::request_names(state, agent, &names));
        AgentFilesystemSnapshots::requested(agent, requested);
    }

    /// Admits a job of `kind` for `agent` with a new name, whose waits end at `deadline`. The
    /// error carries the job that runs for the agent. A replaced job is stopped after the
    /// transition that replaced it.
    async fn admit(
        core: &Arc<Self>,
        agent: &AgentSnapshots,
        kind: SnapshotKind,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<Admission, rules::Refusal> {
        let room = core.room.has_room().await;
        let name = match kind {
            SnapshotKind::Periodic => FilesystemSnapshotName::periodic(),
            SnapshotKind::Update => FilesystemSnapshotName::update(),
        };
        let (ticket, replaced) = JobTicket::admit(
            &core.registry,
            agent,
            &name,
            kind,
            core.shutdown.child_token(),
            room,
        )?;
        if let Some(replaced) = replaced {
            replaced.cancel();
        }
        Ok(Admission {
            core: Arc::clone(core),
            ticket,
            name,
            kind,
            deadline,
        })
    }
}

/// The permission of one upload, with its name. Only an admission gives a name, and an admission
/// cannot be cloned, so each name reaches at most one capture. Dropped, it frees the agent.
pub(crate) struct Admission {
    core: Arc<Core>,
    ticket: JobTicket,
    name: FilesystemSnapshotName,
    kind: SnapshotKind,
    /// When the waits of the upload end. Only a manual update has one.
    deadline: Option<tokio::time::Instant>,
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

    /// Uploads `tree` in the background, then confirms it with `confirm` and deletes the older
    /// snapshots. It differs from [`Admission::upload_now`] only in who waits and in what
    /// follows: both go through the same upload.
    ///
    /// The job uploads and confirms. On `Confirmed` it deletes the older snapshots of its kind,
    /// except the names that a start can select. On `Superseded` it deletes its own snapshot and
    /// no older one. On `Deferred` it keeps the snapshot and deletes nothing, because a later
    /// start can confirm it. When the save failed, it confirms nothing. A shutdown, a call
    /// of `delete_all_snapshots` for the agent, or a call of `delete_snapshots` with its name stops
    /// the job at each step, also in its deletes: it then sends nothing more and deletes nothing
    /// more. A store call that it already sent runs to its end, and the tree is discarded after
    /// its save returned. A confirmation that it already sent can still be appended.
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

/// The hold of a revert of an agent on the deletes of its jobs. Dropped, it ends the hold.
pub(crate) struct RevertHold {
    core: Option<Arc<Core>>,
    agent: AgentSnapshots,
}

impl RevertHold {
    /// Deletes the snapshots `names` of the reverted agent in the background, as
    /// [`AgentFilesystemSnapshots::delete_snapshots`] does, and then ends the hold.
    pub(crate) fn delete_snapshots(self, names: Box<[FilesystemSnapshotName]>) {
        if let Some(core) = &self.core {
            core.request_names(&self.agent, &names);
        }
    }
}

impl Drop for RevertHold {
    fn drop(&mut self) {
        if let Some(core) = &self.core {
            core.registry
                .apply(|state| rules::revert_ended(state, &self.agent));
        }
    }
}

/// A manual-update snapshot that the store holds. The loop deletes the older update snapshots after
/// the update record commits. Dropped, it deletes nothing and frees the agent.
#[must_use = "a dropped saved update deletes no older snapshot; delete them after the record commits"]
pub(crate) struct SavedUpdate {
    core: Arc<Core>,
    ticket: JobTicket,
    name: FilesystemSnapshotName,
    info: SnapshotInfo,
}

impl SavedUpdate {
    /// Applies retention in the background: it keeps the own snapshot and the newest older update
    /// snapshots, with the rules of periodic retention, and it never deletes `kept`, the
    /// snapshots of the successful and pending updates that a valid cut of the agent can still
    /// make a baseline. A shutdown, a call of `delete_all_snapshots` for the agent, a revert of
    /// the agent, or a later manual update of the agent stops it.
    pub(crate) fn delete_older_snapshots(self, kept: &[FilesystemSnapshotName]) {
        let kept = kept
            .iter()
            .filter_map(|name| store_name(name).ok())
            .collect::<Box<[_]>>();
        let Self {
            core,
            ticket,
            name,
            info,
        } = self;
        let Ok(own) = store_name(&name) else {
            return;
        };
        let jobs = core.jobs.clone();
        jobs.spawn(async move {
            job::delete_older_snapshots(&core, &ticket, &own, SnapshotKind::Update, &info, &kept)
                .await;
        });
    }
}
