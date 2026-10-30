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

use super::blob_backend::BlobBackend;
use super::blob_io::{LateWrites, lands_by, later};
use super::failure_classification::{
    io_error_kind, is_index_missing, missing_file_path, rustic_storage_end,
};
use super::operation::Operation;
use super::pruning::timing::refresh_period;
use super::publication::{
    PublishBound, Published, SnapshotStage, StagedSnapshot, backup_end, index_read_bound, publish,
};
use super::repository::{RepositoryKey, open_existing, open_or_create};
use super::run_outcome::{
    Checked, Ended, LateWrite, OwnFile, Ran, RunEnd, RunResult, Settle, Settled, answered_after,
};
use super::snapshot_catalog::{
    has_name, named, repository_snapshots, repository_snapshots_except, snapshot_info,
};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use crate::filesystem_snapshot::clock::Clock;
use crate::filesystem_snapshot::errors::SaveError;
use crate::filesystem_snapshot::snapshot_name::SnapshotName;
use crate::filesystem_snapshot::{ChangeDetection, SnapshotInfo, snapshot_time};
use crate::metrics::filesystem_snapshots::StoredSource;
use anyhow::Context;
use golem_common::model::Timestamp;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp as SnapshotTime, Zoned};
use rustic_core::repofile::SnapshotId;
use rustic_core::{
    BackupOptions, DevIdOption, LocalSourceSaveOptions, ParentOptions, PathList,
    SnapshotGroupCriterion, SnapshotOptions,
};
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::warn;

pub(super) mod source_tree;

/// Settings of a save's backup and publication bound.
#[derive(Clone, Copy)]
pub(super) struct SaveSettings {
    pub(super) threads: Option<NonZeroUsize>,
    pub(super) grace: Duration,
    pub(super) publish_bound: PublishBound,
}

impl SaveSettings {
    /// The earliest prune index read that can delete this save's packs, or no bound.
    fn index_read_bound(&self, t0: Instant, deadline: Duration) -> Option<Instant> {
        match self.publish_bound {
            PublishBound::On => Some(index_read_bound(
                t0,
                self.grace,
                deadline,
                refresh_period(self.grace, deadline),
            )),
            #[cfg(test)]
            PublishBound::Off => None,
        }
    }

    /// The time after which no new backup starts, or no bound.
    pub(super) fn backup_end(&self, t0: Instant, deadline: Duration) -> Option<Instant> {
        self.index_read_bound(t0, deadline)
            .map(|bound| backup_end(bound, deadline))
    }
}

/// How a save run ended, before its late writes are known.
#[derive(Debug)]
enum SaveExit {
    /// The run gives this answer: `Ok(info)` after a publish, `NameInUse`, or `Source`.
    Answered(Result<SnapshotInfo, SaveError>),
    /// The run ended without an answer. `staged` is the file that the run staged, when it got that
    /// far.
    Ended {
        end: RunEnd,
        failure: anyhow::Error,
        staged: Option<OwnFile>,
    },
}

/// The ends of the latest tries of a save run that can still land: of the writes of its backend,
/// and of its publish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RunLate {
    backend: Option<Instant>,
    publish: Option<Instant>,
}

/// Tells whether a run of a save can have a write that can still land after its exit: its backup
/// did not finish, so the threads of rustic can still hold a try; or a recorder holds a try. After
/// a finished backup, `finalize` joined the threads and the publish has returned, so the recorders
/// are final at the exit.
fn settle_may_land(backup_finished: bool, recorded: RunLate) -> bool {
    !backup_finished || recorded.backend.is_some() || recorded.publish.is_some()
}

/// Gives the run of a save from how it ended and its late tries, read after every try of the run
/// ended. Every answer waits until each write of the run has landed or can no longer land. The own
/// name is checked only when a publish try can still land.
fn save_ran(
    exit: SaveExit,
    late: RunLate,
    deadline: Duration,
) -> RunResult<Result<SnapshotInfo, SaveError>> {
    let latest = match (late.backend, late.publish) {
        (Some(backend), publish) => Some(later(publish, backend)),
        (None, publish) => publish,
    };
    let until = lands_by(latest, deadline);
    match exit {
        SaveExit::Answered(answer) => answered_after(answer, until),
        SaveExit::Ended {
            end,
            failure,
            staged,
        } => Err(Ended {
            end,
            failure,
            late: until.map(|until| LateWrite {
                until,
                own: late.publish.and(staged),
            }),
        }),
    }
}

/// A shutdown during settlement preserves an answer already chosen by the save. A run without
/// an answer ends `Cancelled`.
fn ran_at_shutdown(exit: SaveExit) -> RunResult<Result<SnapshotInfo, SaveError>> {
    match exit {
        SaveExit::Answered(answer) => Ok(Ran::Answered(answer)),
        SaveExit::Ended { .. } => Err(Ended::new(
            RunEnd::Cancelled,
            anyhow::anyhow!("the filesystem snapshot store shut down while a save settled"),
        )),
    }
}

/// Gives the settle of a save run that ended with `exit`: it waits until `run_calls`, which each
/// holder of the blobs of the run counts, has no holder left, and then reads the two recorders.
/// The wait has no time bound, so every try of the run has ended when the recorders are read; a
/// warning names the save of `name` for `agent` when the wait takes longer than one `deadline`.
#[allow(clippy::too_many_arguments)]
fn save_settle(
    agent: AgentSnapshots,
    name: SnapshotName,
    exit: SaveExit,
    may_land: bool,
    run_calls: TaskTracker,
    backend_late: Arc<LateWrites>,
    publish_late: Arc<LateWrites>,
    deadline: Duration,
) -> Settle<Result<SnapshotInfo, SaveError>> {
    Settle {
        wait: Box::pin(async move {
            run_calls.close();
            let slow = async {
                tokio::time::sleep(deadline).await;
                warn!(
                    snapshots = ?agent,
                    snapshot = %name,
                    deadline = ?deadline,
                    "A save of a filesystem snapshot still waits for the threads of its backup"
                );
                std::future::pending::<()>().await
            };
            tokio::select! {
                () = run_calls.wait() => {}
                () = slow => {}
            }
        }),
        finish: Box::new(move |settled| match settled {
            Settled::Waited => save_ran(
                exit,
                RunLate {
                    backend: backend_late.latest(),
                    publish: publish_late.latest(),
                },
                deadline,
            ),
            Settled::ShutDown => ran_at_shutdown(exit),
        }),
        may_land,
    }
}

/// Gives the exit of a save run whose backup failed with `error`: a failed storage call ends the
/// run, a local I/O error answers `Source`, and each other error cannot change.
fn save_failure(error: anyhow::Error) -> SaveExit {
    match save_fault(&error) {
        SaveFault::Ended(end) => SaveExit::Ended {
            end,
            failure: error,
            staged: None,
        },
        SaveFault::Source(kind) => SaveExit::Answered(Err(SaveError::Source(std::io::Error::new(
            kind,
            format!("{error:#}"),
        )))),
    }
}

/// Runs [`stage_save`] again with an empty stage after a listed index file was gone at its read,
/// once for each such file. A file that is gone in two loads ends the backup with its error.
#[allow(clippy::too_many_arguments)]
fn stage_save_again_after_a_missing_index(
    backend: &Arc<BlobBackend>,
    stage: &SnapshotStage,
    key: &RepositoryKey,
    settings: &SaveSettings,
    name: &SnapshotName,
    tree: &Path,
    parent: Option<(SnapshotName, ChangeDetection)>,
    clock: &dyn Clock,
) -> anyhow::Result<Option<(StagedSnapshot, SnapshotInfo)>> {
    let attempt = |missed: HashSet<Box<Path>>| {
        let _ = stage.take();
        match stage_save(
            backend.clone(),
            stage,
            key,
            settings,
            name,
            tree,
            parent.clone(),
            clock,
        ) {
            Err(error) if is_index_missing(error.as_ref()) => {
                match missing_file_path(error.as_ref()).map(Box::<Path>::from) {
                    Some(path) if !missed.contains(&path) => ControlFlow::Continue(
                        missed.into_iter().chain(std::iter::once(path)).collect(),
                    ),
                    _ => ControlFlow::Break(Err(error)),
                }
            }
            result => ControlFlow::Break(result),
        }
    };
    match std::iter::repeat(()).try_fold(HashSet::new(), |missed, ()| attempt(missed)) {
        ControlFlow::Break(result) => result,
        ControlFlow::Continue(_) => Err(anyhow::anyhow!("the backup ended without an answer")),
    }
}

/// Backs up the tree with the snapshot file in the stage, and gives the staged file with the info
/// of the snapshot. The result is `None` when a snapshot of the scope already has the name. The
/// parent is the snapshot that [`super::snapshot_catalog::lookup`] finds for its name, and a name without a snapshot
/// gives no parent. The backup dedups only against the packs that an index file lists.
fn stage_save(
    backend: Arc<BlobBackend>,
    stage: &SnapshotStage,
    key: &RepositoryKey,
    settings: &SaveSettings,
    name: &SnapshotName,
    tree: &Path,
    parent: Option<(SnapshotName, ChangeDetection)>,
    clock: &dyn Clock,
) -> anyhow::Result<Option<(StagedSnapshot, SnapshotInfo)>> {
    // The init of the fork checks the config a second time before its write, so a save that loses
    // the race to create the repository can fail at that check with an error that is not
    // `ConfigExists`. A config that is there after the error is the repository of the winner.
    let repository = match open_or_create(backend.clone(), key) {
        Ok(repository) => repository,
        Err(error) => open_existing(backend.clone(), key)
            .ok()
            .flatten()
            .ok_or(error)?,
    };
    let (before, listed) = repository_snapshots_except(&repository, &backend, &HashSet::new())?;
    if has_name(&before, name) {
        return Ok(None);
    }
    let parent = parent.and_then(|(parent, detection)| {
        named(&before.readable, &parent).map(|snapshot| (snapshot.id, detection))
    });
    let newest = before
        .readable
        .iter()
        .filter_map(snapshot_info)
        .map(|info| info.created_at)
        .max();
    let created_at = snapshot_time(whole_millis_from(clock.now()), newest);
    let (tree, content) = source_tree::read(tree)?;
    let snapshot = SnapshotOptions::default()
        .label(name.as_str().to_string())
        .time(snapshot_zoned(created_at)?)
        .description(serde_json::to_string(&content)?)
        .to_snapshot()?;
    let repository = repository.to_indexed_ids()?;
    repository.backup(
        &store_backup_options(settings, parent),
        &PathList::from_iter(Some(tree)),
        snapshot,
    )?;
    let staged = stage
        .take()
        .context("the backup gave no snapshot file to the stage")?;
    // Only a file that is new since the first read can have the name now: a late publish of an
    // earlier attempt of the same job. So the save reads only those files again.
    let (since, _) = repository_snapshots_except(&repository, &backend, &listed)?;
    if has_name(&since, name) {
        return Ok(None);
    }
    Ok(Some((
        staged,
        SnapshotInfo {
            created_at,
            files: content.files,
            bytes: content.bytes,
        },
    )))
}

/// Gives the first time in whole milliseconds that is not before the time. A snapshot keeps its
/// time in milliseconds, so the time of a save is not before the call.
fn whole_millis_from(time: Timestamp) -> Timestamp {
    let truncated = Timestamp::from(time.to_millis());
    if truncated < time {
        Timestamp::from(time.to_millis().saturating_add(1))
    } else {
        truncated
    }
}

/// Gives the time as the time of a snapshot, in UTC.
fn snapshot_zoned(time: Timestamp) -> anyhow::Result<Zoned> {
    Ok(SnapshotTime::from_millisecond(i64::try_from(time.to_millis())?)?.to_zoned(TimeZone::UTC))
}

/// The snapshot keeps paths relative to the saved tree. The parent group has no criterion.
/// Each parallel stage uses `threads` threads, or the available CPUs when it is `None`.
fn backup_options(threads: Option<NonZeroUsize>) -> BackupOptions {
    BackupOptions::default()
        .as_path(PathBuf::from("/"))
        .parent_opts(ParentOptions::default().group_by(SnapshotGroupCriterion::new()))
        .threads(threads)
}

/// A failed entry read fails the save, and no device id is kept. `SizeMtime` compares the type,
/// size and modification time with the named parent, but not the change time or inode. `Full`
/// and a save without a parent read every file.
fn store_backup_options(
    settings: &SaveSettings,
    parent: Option<(SnapshotId, ChangeDetection)>,
) -> BackupOptions {
    let base = backup_options(settings.threads)
        .fail_on_read_error(true)
        .ignore_save_opts(LocalSourceSaveOptions::default().set_devid(DevIdOption::No));
    let parent_opts = match parent {
        // rustic compares the inodes only when `ignore_inode` is true.
        Some((id, ChangeDetection::SizeMtime)) => base
            .parent_opts
            .clone()
            .parents(vec![id.to_hex().to_string()])
            .ignore_ctime(true)
            .ignore_inode(false),
        None | Some((_, ChangeDetection::Full)) => base.parent_opts.clone().force(true),
    };
    base.parent_opts(parent_opts)
}

/// One run of a save: the backup, then the publish of its snapshot file. A backup that has not
/// returned at `backup_end` is cancelled, and the run publishes nothing. The run then cancels
/// its token and gives the settle to the call driver: the driver gives the slot back and waits until
/// every blob call of the run has ended, and then the run answers after its writes that can
/// still land.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    operation: &Operation<'_>,
    settings: SaveSettings,
    name: &SnapshotName,
    tree: &Path,
    parent: Option<(SnapshotName, ChangeDetection)>,
    cancel: &CancellationToken,
    t0: Instant,
) -> RunResult<Result<SnapshotInfo, SaveError>> {
    let (token, _guard) = operation.run_token();
    operation.link_cancel(cancel, &token);
    let run_calls = TaskTracker::new();
    let backend_late = Arc::new(LateWrites::default());
    let publish_late = Arc::new(LateWrites::default());
    let stage = Arc::new(SnapshotStage::default());
    let backend_files = operation
        .files(&token)
        .recording(backend_late.clone())
        .held_by(run_calls.token());
    let backend = match operation.backend(backend_files) {
        Ok(backend) => Arc::new(
            backend
                .staging_in(stage.clone())
                .storing(StoredSource::Save),
        ),
        Err(error) => return Err(Ended::new(RunEnd::Permanent, error)),
    };
    let key = operation.key().clone();
    let saved = name.clone();
    let name = name.clone();
    let tree: Box<Path> = tree.into();
    let low_priority = operation.low_priority();
    let clock = Arc::clone(operation.clock());
    let mut backup = pin!(operation.blocking(move || {
        low_priority.run("fs-snap-save", move || {
            Ok(stage_save_again_after_a_missing_index(
                &backend, &stage, &key, &settings, &name, &tree, parent, &*clock,
            ))
        })
    }));
    let backup_end = settings.backup_end(t0, operation.deadline());
    let staged = match backup_end {
        Some(end) => {
            tokio::select! {
                staged = &mut backup => Some(staged),
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(end)) => None,
            }
        }
        None => Some((&mut backup).await),
    };
    let (exit, backup_finished) = match staged {
        None => {
            // The backup did not end in time: the run stops it, and waits until the store reads
            // the tree no more.
            token.cancel();
            let _ = backup.await;
            (
                SaveExit::Ended {
                    end: RunEnd::BoundPassed,
                    failure: anyhow::anyhow!(
                        "the backup of the save would take longer than the filesystem snapshot store allows"
                    ),
                    staged: None,
                },
                false,
            )
        }
        Some(staged) => {
            let staged = staged.and_then(|staged| staged);
            let backup_finished = matches!(staged, Ok(Some(_)));
            // The backup has returned, so the store reads the tree no more. A cancel before
            // this point publishes nothing, whatever the backup gave.
            let exit = if cancel.is_cancelled() {
                SaveExit::Ended {
                    end: RunEnd::Cancelled,
                    failure: anyhow::anyhow!(
                        "the save of the filesystem snapshot was cancelled before its publish"
                    ),
                    staged: None,
                }
            } else {
                match staged {
                    Ok(Some((staged, info))) => {
                        let bound = settings.index_read_bound(t0, operation.deadline());
                        let files = operation
                            .files(operation.root())
                            .recording(publish_late.clone())
                            .held_by(run_calls.token());
                        match publish(&files, &staged, bound, cancel).await {
                            Published::Written => SaveExit::Answered(Ok(info)),
                            Published::NotWritten { end, failure } => SaveExit::Ended {
                                end,
                                failure,
                                staged: Some(OwnFile {
                                    path: staged.path.clone(),
                                    info,
                                }),
                            },
                        }
                    }
                    Ok(None) => SaveExit::Answered(Err(SaveError::NameInUse)),
                    Err(error) => save_failure(error),
                }
            };
            (exit, backup_finished)
        }
    };
    // No try of the run starts from here, and a try that runs ends at once.
    token.cancel();
    let recorded = RunLate {
        backend: backend_late.latest(),
        publish: publish_late.latest(),
    };
    Ok(Ran::Settling(save_settle(
        operation.agent().clone(),
        saved,
        exit,
        settle_may_land(backup_finished, recorded),
        run_calls,
        backend_late,
        publish_late,
        operation.deadline(),
    )))
}

/// Checks the own name of a save after a publish that ended without an answer: the file that
/// the last publish staged, another snapshot file with the name, no such file, or a check that
/// could not read. It holds no slot and ends only at the shutdown.
pub(super) async fn own_name_check(
    operation: &Operation<'_>,
    name: &SnapshotName,
    own: OwnFile,
) -> Checked {
    let OwnFile { path, info } = own;
    let files = operation.files(operation.root());
    let listed = match files
        .list_below(
            "check_name",
            Path::new(super::repository_layout::SNAPSHOTS_PATH),
        )
        .await
    {
        Ok(listed) => listed,
        Err(_) => return Checked::Unreadable,
    };
    if listed.iter().any(|blob| *blob.path == *path) {
        return Checked::Found(info);
    }
    let name = name.clone();
    let backend = match operation.backend(files) {
        Ok(backend) => Arc::new(backend),
        Err(_) => return Checked::Unreadable,
    };
    let named = operation
        .read_existing(backend, move |repository, backend| {
            Ok(match repository {
                Some(repository) => has_name(&repository_snapshots(&repository, backend)?, &name),
                None => false,
            })
        })
        .await;
    match named {
        Ok(true) => Checked::Other,
        Ok(false) => Checked::Absent,
        Err(_) => Checked::Unreadable,
    }
}
/// What the error of a failed backup of a save is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveFault {
    /// The run ends this way.
    Ended(RunEnd),
    /// The local filesystem of the tree gave this kind of I/O error.
    Source(std::io::ErrorKind),
}

/// Classifies the error of a failed backup of a save. The storage decides first: a failed storage
/// call, then an index file that was gone at its read, which a new run can find again. A local I/O
/// error is `Source`. Any other error is permanent.
fn save_fault(error: &anyhow::Error) -> SaveFault {
    if let Some(end) = rustic_storage_end(error) {
        return SaveFault::Ended(end);
    }
    if is_index_missing(error.as_ref()) {
        return SaveFault::Ended(RunEnd::CallFailed);
    }
    match io_error_kind(error.as_ref()) {
        Some(kind) => SaveFault::Source(kind),
        None => SaveFault::Ended(RunEnd::Permanent),
    }
}

#[cfg(test)]
mod tests;
