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
use super::failure_classification::{ended_by, is_file_missing, is_storage_failure};
use super::operation::Operation;
use super::run_outcome::{Ran, RunResult};
use crate::filesystem_snapshot::errors::{CallError, ReadError};
use crate::filesystem_snapshot::snapshot_name::SnapshotName;
use crate::filesystem_snapshot::{SnapshotInfo, newest_first};
use golem_common::model::Timestamp;
use rustic_core::repofile::{SnapshotFile, SnapshotId};
use rustic_core::{FileType, Open, Repository as RusticRepository};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Reads each snapshot file of the repository, whose blobs `backend` reads. A failed storage call
/// fails the read, a file that a delete removed after the listing is left out, and each other
/// failure counts as a failed check.
pub(super) fn repository_snapshots<S: Open>(
    repository: &RusticRepository<S>,
    backend: &BlobBackend,
) -> anyhow::Result<RepositorySnapshots> {
    repository_snapshots_except(repository, backend, &HashSet::new()).map(|(found, _)| found)
}

/// Reads each snapshot file of the repository whose id is not in `known`, as
/// [`repository_snapshots`] does, and gives also the ids of every snapshot file that the listing found.
/// The files are read ahead concurrently through `backend` before rustic reads and decrypts them
/// one after the other, so the reads of the storage overlap. The order of the files does not
/// matter, because each reader sorts or filters them.
pub(super) fn repository_snapshots_except<S: Open>(
    repository: &RusticRepository<S>,
    backend: &BlobBackend,
    known: &HashSet<SnapshotId>,
) -> anyhow::Result<(RepositorySnapshots, HashSet<SnapshotId>)> {
    let listed = repository.list::<SnapshotId>()?.collect::<HashSet<_>>();
    let new = listed
        .iter()
        .filter(|id| !known.contains(id))
        .copied()
        .collect::<Vec<_>>();
    backend.read_ahead(FileType::Snapshot, new.iter().map(|id| **id))?;
    new.into_iter()
        .try_fold(
            RepositorySnapshots {
                readable: Vec::new(),
                unreadable: false,
            },
            |found, id| read_snapshot_file(repository, found, id),
        )
        .map(|found| (found, listed))
}

/// Adds the snapshot file `id` of the repository to `found`, as [`repository_snapshots`] says.
fn read_snapshot_file<S: Open>(
    repository: &RusticRepository<S>,
    mut found: RepositorySnapshots,
    id: SnapshotId,
) -> anyhow::Result<RepositorySnapshots> {
    match repository.get_file::<SnapshotFile>(&id) {
        Ok(mut snapshot) => {
            snapshot.id = id;
            found.readable.push(snapshot);
            Ok(found)
        }
        Err(error) if is_storage_failure(&*error) => Err(anyhow::Error::from(error)),
        Err(error) if is_file_missing(&*error) => Ok(found),
        Err(_) => Ok(RepositorySnapshots {
            unreadable: true,
            ..found
        }),
    }
}

pub(super) fn has_name(found: &RepositorySnapshots, name: &SnapshotName) -> bool {
    found
        .readable
        .iter()
        .any(|snapshot| snapshot.label == name.as_str())
}

/// Finds the snapshot with the name. Of the snapshot files with the name, the one with the least
/// time and id wins. When no file has the name and a file failed its integrity check, the result is
/// `Corrupt`, because that file can have the name.
pub(super) fn lookup(found: RepositorySnapshots, name: &SnapshotName) -> Lookup {
    match (named(&found.readable, name), found.unreadable) {
        (Some(snapshot), _) => match snapshot_info(snapshot) {
            Some(info) => Lookup::Found(Box::new(snapshot.clone()), info),
            None => Lookup::Corrupt(anyhow::anyhow!(
                "the snapshot {} does not describe its tree",
                snapshot.id
            )),
        },
        (None, true) => Lookup::Corrupt(anyhow::anyhow!(
            "a snapshot file of the scope failed its integrity check"
        )),
        (None, false) => Lookup::Missing,
    }
}

/// Of the snapshot files with the name, gives the one with the least time and id.
pub(super) fn named<'a>(
    snapshots: &'a [SnapshotFile],
    name: &SnapshotName,
) -> Option<&'a SnapshotFile> {
    snapshots
        .iter()
        .filter(|snapshot| snapshot.label == name.as_str())
        .min_by_key(|snapshot| (snapshot.time.timestamp(), snapshot.id))
}

/// Gives the name and the info of each snapshot whose label is a name and whose description
/// parses. Of the files with one name, only the one that [`lookup`] takes stays.
fn listed(found: RepositorySnapshots) -> Vec<(SnapshotName, SnapshotInfo)> {
    let mut snapshots = found.readable;
    snapshots.sort_by(|left, right| {
        (&left.label, left.time.timestamp(), left.id).cmp(&(
            &right.label,
            right.time.timestamp(),
            right.id,
        ))
    });
    snapshots.dedup_by(|later, first| later.label == first.label);
    snapshots
        .iter()
        .filter_map(|snapshot| {
            SnapshotName::new(&snapshot.label)
                .ok()
                .zip(snapshot_info(snapshot))
        })
        .collect()
}

/// Gives the info of a snapshot from its time and its description.
pub(super) fn snapshot_info(snapshot: &SnapshotFile) -> Option<SnapshotInfo> {
    let content = serde_json::from_str::<TreeContent>(snapshot.description.as_deref()?).ok()?;
    let created_at = u64::try_from(snapshot.time.timestamp().as_millisecond()).ok()?;
    Some(SnapshotInfo {
        created_at: Timestamp::from(created_at),
        files: content.files,
        bytes: content.bytes,
    })
}

/// One run of a `stat`.
pub(super) async fn stat_run(
    operation: &Operation<'_>,
    name: &SnapshotName,
) -> RunResult<Result<Option<SnapshotInfo>, ReadError>> {
    let mut run = operation.begin_run()?;
    let name = name.clone();
    let found = operation
        .read_existing(run.take_backend(), move |repository, backend| {
            Ok(match repository {
                Some(repository) => lookup(repository_snapshots(&repository, backend)?, &name),
                None => Lookup::Missing,
            })
        })
        .await
        .map_err(ended_by)?;
    Ok(Ran::Answered(match found {
        Lookup::Found(_, info) => Ok(Some(info)),
        Lookup::Missing => Ok(None),
        Lookup::Corrupt(error) => Err(ReadError::Corrupt(error)),
    }))
}

/// One run of a `list`.
pub(super) async fn list_run(
    operation: &Operation<'_>,
) -> RunResult<Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError>> {
    let mut run = operation.begin_run()?;
    let listed = operation
        .read_existing(run.take_backend(), move |repository, backend| {
            Ok(match repository {
                Some(repository) => {
                    newest_first(listed(repository_snapshots(&repository, backend)?))
                }
                None => Box::default(),
            })
        })
        .await
        .map_err(ended_by)?;
    Ok(Ran::Answered(Ok(listed)))
}
/// What the tree of a snapshot holds. The store keeps it as the description of the snapshot,
/// because rustic counts a symlink as a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TreeContent {
    pub(super) files: u64,
    pub(super) bytes: u64,
}

/// The snapshot files of a scope that rustic could read, and whether a file failed its integrity
/// check. A file that a delete removed after the listing is in neither.
pub(super) struct RepositorySnapshots {
    pub(super) readable: Vec<SnapshotFile>,
    pub(super) unreadable: bool,
}

/// What a lookup of a name found.
pub(super) enum Lookup {
    Found(Box<SnapshotFile>, SnapshotInfo),
    Missing,
    Corrupt(anyhow::Error),
}

#[cfg(test)]
mod tests;
