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

#[cfg(test)]
use super::ClearHook;
use super::{
    AgentSnapshots, Arc, BlobBackend, ControlFlow, Ended, FileType, Found, HashSet, Id, IndexFile,
    IndexId, IndexedFullStatus, Learned, LocalDestination, Lookup, LowPriority, LsOptions,
    Observed, OperationWork, Outcome, Path, Phase, Ran, ReadBackend, RepositoryKey, Reread,
    RestoreError, RestoreFailure, RestoreOptions, RunEnd, RusticRepository, RusticSnapshotStore,
    SnapshotFile, SnapshotInfo, SnapshotName, Step, clear, ended_by, is_file_missing,
    is_index_missing, lookup, open_existing, reload, restore_error, scope_snapshots,
    store_restore_options,
};

/// The most steps of one run of a restore. A run loads at most once more for each index file that
/// its listings give, so a run that reaches this bound found new index files at each step.
const MOST_RESTORE_STEPS: usize = 1024;

/// The state of one run of a restore, which [`reload::next_step`] drives. It runs on a blocking
/// thread.
struct RestoreRun {
    backend: Arc<BlobBackend>,
    key: RepositoryKey,
    name: SnapshotName,
    into: Box<Path>,
    options: RestoreOptions,
    #[cfg(test)]
    hook: Arc<dyn ClearHook>,
}

/// What a run of a restore holds between its steps.
struct Restoring {
    /// The index files that a listing of the run gave.
    listed: HashSet<Box<Path>>,
    /// The index files that a load of the run found gone.
    missed: HashSet<Box<Path>>,
    /// The snapshot of the name and its info, after a load found it.
    found: Option<(SnapshotFile, SnapshotInfo)>,
    /// The repository with the index of the last load that found the name.
    indexed: Option<RusticRepository<IndexedFullStatus>>,
    /// The error that the last step gave.
    last_error: Option<anyhow::Error>,
    /// The pack that a step found gone.
    missing_pack: Option<Id>,
    /// Whether the run wrote into the directory.
    written: bool,
    /// The I/O error of a write into the directory.
    destination: Option<std::io::Error>,
    /// The info of a run that wrote the whole tree.
    restored: Option<SnapshotInfo>,
}

/// The result of one step of a run of a restore.
enum Stepped {
    Observed(Observed),
    Restored(SnapshotInfo),
}

impl RestoreRun {
    /// Runs the steps of the restore until one gives the end of the run.
    fn run(self) -> Ran<Result<SnapshotInfo, RestoreFailure>> {
        // A directory that is not empty holds what an earlier run wrote, also when its clear failed
        // part way.
        if let Err(error) = self.clear_if_written() {
            return Ran::Ended(error);
        }
        let mut restoring = Restoring {
            listed: HashSet::new(),
            missed: HashSet::new(),
            found: None,
            indexed: None,
            last_error: None,
            missing_pack: None,
            written: false,
            destination: None,
            restored: None,
        };
        let first = self.load(&mut restoring);
        let ended = (0..MOST_RESTORE_STEPS).try_fold(first, |observed, _| {
            let observed = match observed {
                Stepped::Restored(info) => {
                    restoring.restored = Some(info);
                    return ControlFlow::Break(None);
                }
                Stepped::Observed(observed) => observed,
            };
            let (step, learned) =
                reload::next_step(&observed, &restoring.listed, &restoring.missed);
            match learned {
                Learned::Nothing => {}
                Learned::Missed(path) => {
                    restoring.missed.insert(path);
                }
                Learned::Listed(names) => restoring.listed.extend(names),
            }
            match step {
                Step::Check => ControlFlow::Continue(self.check_and_write(&mut restoring)),
                Step::LoadAgain => ControlFlow::Continue(self.load(&mut restoring)),
                Step::ListIndexAgain => ControlFlow::Continue(self.list_index(&mut restoring)),
                Step::ReadSnapshotsAgain(reason) => {
                    ControlFlow::Continue(self.read_snapshots(&mut restoring, reason))
                }
                Step::LoadIndexAgain => ControlFlow::Continue(self.load_index(&mut restoring)),
                Step::ClearAndEnd(end) => ControlFlow::Break(Some(match self.clear() {
                    Ok(()) => Ran::Ended(Ended::new(end, last_error(&mut restoring))),
                    Err(error) => Ran::Ended(error),
                })),
                Step::RunEnded(end) => ControlFlow::Break(Some(Ran::Ended(Ended::new(
                    end,
                    last_error(&mut restoring),
                )))),
                Step::Answer(outcome) => {
                    ControlFlow::Break(Some(Ran::Answered(Err(answer(outcome, &mut restoring)))))
                }
            }
        });
        match ended {
            ControlFlow::Break(Some(ran)) => ran,
            ControlFlow::Break(None) => match restoring.restored {
                Some(info) => Ran::Answered(Ok(info)),
                None => Ran::Ended(Ended::new(RunEnd::CallFailed, last_error(&mut restoring))),
            },
            ControlFlow::Continue(_) => Ran::Ended(Ended::new(
                RunEnd::CallFailed,
                anyhow::anyhow!("the restore found new index files at each of its steps"),
            )),
        }
    }

    /// Clears the directory when it is not empty. A failed clear ends the run: on unix a new run
    /// clears again, and elsewhere no run can clear.
    fn clear_if_written(&self) -> Result<(), Ended> {
        match clear::is_empty(&self.into) {
            Ok(true) => Ok(()),
            Ok(false) => self.clear(),
            Err(error) => Err(Ended::new(
                RunEnd::CallFailed,
                anyhow::Error::new(error).context("read the directory of the restore"),
            )),
        }
    }

    /// Removes every entry of the directory, as [`clear::clear`] says.
    fn clear(&self) -> Result<(), Ended> {
        #[cfg(test)]
        let cleared = clear::clear_with(&self.into, &*self.hook);
        #[cfg(not(test))]
        let cleared = clear::clear(&self.into);
        cleared.map_err(|error| {
            let end = if error.kind() == std::io::ErrorKind::Unsupported {
                RunEnd::Permanent
            } else {
                RunEnd::CallFailed
            };
            Ended::new(
                end,
                anyhow::Error::new(error)
                    .context("remove what an earlier run of the restore wrote"),
            )
        })
    }

    /// Loads the repository, the snapshot of the name and the index.
    fn load(&self, restoring: &mut Restoring) -> Stepped {
        let loaded = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(Stepped::Observed(Observed::ConfigMissing));
            };
            match lookup(scope_snapshots(&repository, &self.backend)?, &self.name) {
                Lookup::Missing => Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Missing))),
                Lookup::Corrupt(error) => {
                    restoring.last_error = Some(error);
                    Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Corrupt)))
                }
                Lookup::Found(snapshot, info) => {
                    let indexed = repository.to_indexed();
                    restoring.listed.extend(self.backend.take_index_listed());
                    restoring.indexed = Some(indexed?);
                    restoring.found = Some((*snapshot, info));
                    Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Found)))
                }
            }
        })();
        loaded.unwrap_or_else(|error: anyhow::Error| {
            restoring.listed.extend(self.backend.take_index_listed());
            Stepped::Observed(self.observed_error(restoring, error))
        })
    }

    /// Checks that the index of the last load holds the whole tree, without a write, and then
    /// writes the tree with the same index.
    fn check_and_write(&self, restoring: &mut Restoring) -> Stepped {
        let (Some((snapshot, info)), Some(repository)) =
            (restoring.found.clone(), restoring.indexed.take())
        else {
            // A check follows only a load that found the name. When the run holds no such load,
            // it ends without a claim about the name, and a new run loads again.
            restoring.last_error = Some(anyhow::anyhow!(
                "the restore came to its check without a load that found the name"
            ));
            return Stepped::Observed(Observed::CallFailed {
                written: restoring.written,
            });
        };
        let checked = (|| {
            let into = self.into.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the directory of a restore must have a UTF-8 path",
                )
            })?;
            let destination = LocalDestination::new(into, false, false)?;
            let node = repository.node_from_snapshot_and_path(&snapshot, "")?;
            anyhow::Ok((destination, node))
        })();
        let (destination, node) = match checked {
            Ok(checked) => checked,
            Err(error) => return Stepped::Observed(self.observed_check_error(restoring, error)),
        };
        let entries = match repository.ls(&node, &LsOptions::default()) {
            Ok(entries) => entries,
            Err(error) => {
                return Stepped::Observed(
                    self.observed_check_error(restoring, anyhow::Error::from(error)),
                );
            }
        };
        if let Err(error) =
            repository.prepare_restore(&self.options, entries.clone(), &destination, true)
        {
            return Stepped::Observed(
                self.observed_check_error(restoring, anyhow::Error::from(error)),
            );
        }
        restoring.written = true;
        let written = repository
            .prepare_restore(&self.options, entries.clone(), &destination, false)
            .and_then(|plan| repository.restore(plan, &self.options, entries, &destination));
        match written {
            Ok(()) => Stepped::Restored(info),
            Err(error) => {
                Stepped::Observed(self.observed_write_error(restoring, anyhow::Error::from(error)))
            }
        }
    }

    /// Lists the index files again.
    fn list_index(&self, restoring: &mut Restoring) -> Stepped {
        match self.backend.list_with_size(FileType::Index) {
            Ok(_) => {
                Stepped::Observed(Observed::IndexListedAgain(self.backend.take_index_listed()))
            }
            Err(error) => {
                Stepped::Observed(self.observed_error(restoring, anyhow::Error::from(error)))
            }
        }
    }

    /// Reads the snapshot files again, and looks for the name.
    fn read_snapshots(&self, restoring: &mut Restoring, reason: Reread) -> Stepped {
        let read = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(Found::NoName);
            };
            Ok(
                match lookup(scope_snapshots(&repository, &self.backend)?, &self.name) {
                    Lookup::Found(..) => Found::Name,
                    Lookup::Missing => Found::NoName,
                    Lookup::Corrupt(error) => {
                        restoring.last_error = Some(error);
                        Found::Corrupt
                    }
                },
            )
        })();
        match read {
            Ok(found) => Stepped::Observed(Observed::SnapshotsReadAgain { reason, found }),
            Err(error) => Stepped::Observed(self.observed_error(restoring, error)),
        }
    }

    /// Loads the index again, and tells whether it lists the pack that was gone, as a pack or as a
    /// pack that a prune marked for deletion, as the index of a load does. An index file that is
    /// gone during the load counts as an index that does not list the pack.
    fn load_index(&self, restoring: &mut Restoring) -> Stepped {
        let Some(pack) = restoring.missing_pack else {
            return Stepped::Observed(Observed::PackIndexed(false));
        };
        let loaded = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(None);
            };
            let ids = repository.list::<IndexId>()?.collect::<Vec<_>>();
            ids.iter()
                .try_fold(false, |indexed, id| {
                    if indexed {
                        return Ok(true);
                    }
                    match repository.get_file::<IndexFile>(id) {
                        Ok(index) => Ok(index
                            .packs
                            .iter()
                            .chain(&index.packs_to_delete)
                            .any(|listed| *listed.id == pack)),
                        Err(error) if is_file_missing(&*error) => Ok(false),
                        Err(error) => Err(anyhow::Error::from(error)),
                    }
                })
                .map(Some)
        })();
        let _ = self.backend.take_index_listed();
        match loaded {
            // A delete of all snapshots removed the repository after the load.
            Ok(None) => Stepped::Observed(Observed::ConfigMissing),
            Ok(Some(indexed)) => Stepped::Observed(Observed::PackIndexed(indexed)),
            Err(error) if is_index_missing(error.as_ref()) => {
                Stepped::Observed(Observed::PackIndexed(false))
            }
            Err(error) => Stepped::Observed(self.observed_error(restoring, error)),
        }
    }

    /// Gives what a failed load or read observed.
    fn observed_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Load)
    }

    /// Gives what a failed check observed.
    fn observed_check_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Check)
    }

    /// Gives what a failed write observed.
    fn observed_write_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Write)
    }
}

/// Gives what a run of a restore observed after the error of a failed step in `phase`, and keeps
/// what the next steps need: the missing pack, the error of the check, the I/O error of the
/// directory, and the error that the run ends with.
fn observe(restoring: &mut Restoring, error: anyhow::Error, phase: Phase) -> Observed {
    let written = restoring.written;
    let observed = match restore_error(&error, phase) {
        RestoreError::PackMissing(pack) => {
            restoring.missing_pack = pack;
            Observed::PackMissing { written }
        }
        RestoreError::FileMissing(path) => Observed::IndexFileMissing(path),
        RestoreError::ConfigMissing => Observed::ConfigMissing,
        RestoreError::Cancelled => Observed::Cancelled,
        RestoreError::Permanent { range } => Observed::Permanent { range },
        RestoreError::CallFailed => Observed::CallFailed { written },
        RestoreError::Destination(kind) => {
            restoring.destination = Some(std::io::Error::new(kind, format!("{error:#}")));
            Observed::Destination
        }
        RestoreError::CheckFailed => Observed::CheckFailed,
        RestoreError::Corrupt => Observed::Permanent { range: true },
    };
    restoring.last_error = Some(error);
    observed
}

/// Gives the error that a run of a restore ends with.
fn last_error(restoring: &mut Restoring) -> anyhow::Error {
    restoring
        .last_error
        .take()
        .unwrap_or_else(|| anyhow::anyhow!("the run of the restore ended"))
}

/// Gives the answer of a restore for the outcome of a step.
fn answer(outcome: Outcome, restoring: &mut Restoring) -> RestoreFailure {
    match outcome {
        Outcome::NotFound => RestoreFailure::NotFound,
        Outcome::Corrupt => RestoreFailure::Corrupt(last_error(restoring)),
        Outcome::Destination => RestoreFailure::Destination(
            restoring
                .destination
                .take()
                .unwrap_or_else(|| std::io::Error::other(last_error(restoring))),
        ),
    }
}

impl RusticSnapshotStore {
    /// One run of a restore, as [`reload::next_step`] decides each step.
    pub(super) async fn restore_run(
        &self,
        work: &OperationWork,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
    ) -> Ran<Result<SnapshotInfo, RestoreFailure>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(work, scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let run = RestoreRun {
            backend,
            key: self.key.clone(),
            name: name.clone(),
            into: into.into(),
            options: store_restore_options(&self.policy),
            #[cfg(test)]
            hook: self.clear_hook.clone(),
        };
        // The index load of a restore uses rayon, so the restore runs in a pool of its own, with
        // the reader threads of a restore. The blocking thread starts the threads of that pool, so
        // they have its normal priority.
        let pool = LowPriority {
            threads: Some(self.policy.restore_reader_threads),
            ..self.low_priority
        };
        self.blocking(work, move || {
            pool.in_own_pool("fs-snap-restore", move || Ok(run.run()))
        })
        .await
        .unwrap_or_else(|error| Ran::Ended(ended_by(error)))
    }
}
