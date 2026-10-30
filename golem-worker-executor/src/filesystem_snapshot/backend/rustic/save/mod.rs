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

use super::RusticSnapshotStore;
use super::blob_backend::BlobBackend;
use super::blob_io::{LateWrites, lands_by, later};
use super::call_execution::{
    Checked, Ended, LateWrite, OwnFile, Ran, RunEnd, Settle, Settled, answered_after,
};
use super::failure_classification::{SaveFault, is_index_missing, missing_file_path, save_fault};
use super::publication::{Published, SnapshotStage, StagedSnapshot, publish};
use super::repository::{RepositoryKey, open_existing, open_or_create};
use super::snapshot_catalog::{
    has_name, named, scope_snapshots, scope_snapshots_except, snapshot_info,
};
use super::store_policy::{StorePolicy, store_backup_options};
use crate::filesystem_snapshot::clock::Clock;
use crate::filesystem_snapshot::operation_drain::OperationLease;
use crate::filesystem_snapshot::{
    AgentSnapshots, ChangeDetection, SaveError, SnapshotInfo, SnapshotName, snapshot_time,
};
use crate::metrics::filesystem_snapshots::StoredSource;
use anyhow::Context;
use golem_common::model::Timestamp;
use rustic_core::FileType;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp as SnapshotTime, Zoned};
use rustic_core::{PathList, SnapshotOptions};
use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::warn;

pub(super) mod source_tree;
/// How a save run ended, before its late writes are known.
#[derive(Debug)]
pub(super) enum SaveExit {
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
pub(super) struct RunLate {
    pub(super) backend: Option<Instant>,
    pub(super) publish: Option<Instant>,
}

/// Tells whether a run of a save can have a write that can still land after its exit: its backup
/// did not finish, so the threads of rustic can still hold a try; or a recorder holds a try. After
/// a finished backup, `finalize` joined the threads and the publish has returned, so the recorders
/// are final at the exit.
pub(super) fn settle_may_land(backup_finished: bool, recorded: RunLate) -> bool {
    !backup_finished || recorded.backend.is_some() || recorded.publish.is_some()
}

/// Gives the run of a save from how it ended and its late tries, read after every try of the run
/// ended. Every answer waits until each write of the run has landed or can no longer land. The own
/// name is checked only when a publish try can still land.
pub(super) fn save_ran(
    exit: SaveExit,
    late: RunLate,
    deadline: Duration,
) -> Ran<Result<SnapshotInfo, SaveError>> {
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
        } => Ran::Ended(Ended {
            end,
            failure,
            late: until.map(|until| LateWrite {
                until,
                own: late.publish.and(staged),
            }),
        }),
    }
}

/// Gives the run of a save whose settle a shutdown ended: an answered run keeps its answer, as row
/// 1 keeps it after a shutdown in the wait of row 0; any other run ends `Cancelled`.
pub(super) fn ran_at_shutdown(exit: SaveExit) -> Ran<Result<SnapshotInfo, SaveError>> {
    match exit {
        SaveExit::Answered(answer) => Ran::Answered(answer),
        SaveExit::Ended { .. } => Ran::Ended(Ended::new(
            RunEnd::Cancelled,
            anyhow::anyhow!("the filesystem snapshot store shut down while a save settled"),
        )),
    }
}

/// Gives the settle of a save run that ended with `exit`: it waits until `run_calls`, which each
/// holder of the blobs of the run counts, has no holder left, and then reads the two recorders.
/// The wait has no time bound, so every try of the run has ended when the recorders are read; a
/// warning names the save of `name` in `scope` when the wait takes longer than one `deadline`.
#[allow(clippy::too_many_arguments)]
pub(super) fn save_settle(
    scope: AgentSnapshots,
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
                    snapshots = ?scope,
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
pub(super) fn save_failure(error: anyhow::Error) -> SaveExit {
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
    policy: &StorePolicy,
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
            policy,
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
    policy: &StorePolicy,
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
    let (before, listed) = scope_snapshots_except(&repository, &backend, &HashSet::new())?;
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
        &store_backup_options(policy, parent),
        &PathList::from_iter(Some(tree)),
        snapshot,
    )?;
    let staged = stage
        .take()
        .context("the backup gave no snapshot file to the stage")?;
    // Only a file that is new since the first read can have the name now: a late publish of an
    // earlier attempt of the same job. So the save reads only those files again.
    let (since, _) = scope_snapshots_except(&repository, &backend, &listed)?;
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
pub(super) fn whole_millis_from(time: Timestamp) -> Timestamp {
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

impl RusticSnapshotStore {
    /// One run of a save: the backup, then the publish of its snapshot file. A backup that has not
    /// returned at `backup_end` is cancelled, and the run publishes nothing. The run then cancels
    /// its token and gives the settle to the shell: the shell gives the slot back and waits until
    /// every blob call of the run has ended, and then the run answers after its writes that can
    /// still land.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn save_run(
        &self,
        work: &OperationLease,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        t0: Instant,
    ) -> Ran<Result<SnapshotInfo, SaveError>> {
        let (token, _guard) = self.run_token();
        self.link_cancel(work, cancel, &token);
        let run_calls = TaskTracker::new();
        let backend_late = Arc::new(LateWrites::default());
        let publish_late = Arc::new(LateWrites::default());
        let stage = Arc::new(SnapshotStage::default());
        let backend_files = self
            .files(work, scope, &token)
            .recording(backend_late.clone())
            .held_by(run_calls.token());
        let backend = match self.backend(backend_files) {
            Ok(backend) => Arc::new(
                backend
                    .staging_in(stage.clone())
                    .storing(StoredSource::Save),
            ),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        let policy = self.policy.clone();
        let saved = name.clone();
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        let low_priority = self.low_priority;
        let clock = self.clock.clone();
        let mut backup = pin!(self.blocking(work, move || {
            low_priority.run("fs-snap-save", move || {
                Ok(stage_save_again_after_a_missing_index(
                    &backend, &stage, &key, &policy, &name, &tree, parent, &*clock,
                ))
            })
        }));
        let backup_end = self.policy.backup_end(t0);
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
                            let bound = self.policy.index_read_bound(t0);
                            let files = self
                                .files(work, scope, &self.root)
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
        Ran::Settling(save_settle(
            scope.clone(),
            saved,
            exit,
            settle_may_land(backup_finished, recorded),
            run_calls,
            backend_late,
            publish_late,
            self.policy.deadline,
        ))
    }

    /// Checks the own name of a save after a publish that ended without an answer: the file that
    /// the last publish staged, another snapshot file with the name, no such file, or a check that
    /// could not read. It holds no slot and ends only at the shutdown.
    pub(super) async fn own_name_check(
        &self,
        work: &OperationLease,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        own: OwnFile,
    ) -> Checked {
        let OwnFile { path, info } = own;
        let files = self.files(work, scope, &self.root);
        let listed = match files
            .list_below("check_name", Path::new(FileType::Snapshot.dirname()))
            .await
        {
            Ok(listed) => listed,
            Err(_) => return Checked::Unreadable,
        };
        if listed.iter().any(|blob| *blob.path == *path) {
            return Checked::Found(info);
        }
        let key = self.key.clone();
        let name = name.clone();
        let backend = match self.backend(files) {
            Ok(backend) => Arc::new(backend),
            Err(_) => return Checked::Unreadable,
        };
        let named = self
            .blocking(work, move || {
                Ok(match open_existing(backend.clone(), &key)? {
                    Some(repository) => has_name(&scope_snapshots(&repository, &backend)?, &name),
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
}

#[cfg(test)]
mod tests;
