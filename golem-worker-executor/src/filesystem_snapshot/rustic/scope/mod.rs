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

//! The copy and the delete of a whole scope, on the blobs of its repository.
//!
//! These operations do not read the repository format. They only know the directories of the
//! repository, its config file, and the ledger directory of the store.

use super::backend::CONFIG_PATH;
use super::fault::storage_end;
use super::files::SnapshotFiles;
use super::prune::LEDGERS_PATH;
use super::runs::RunEnd;
use futures::{StreamExt, TryStreamExt, stream};
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The directories of a repository in the order of a listing. A save writes them in the reverse
/// order, and so does a copy, so a snapshot file always has its data.
const LISTING_ORDER: [&str; 4] = [SNAPSHOTS_PATH, INDEX_PATH, "keys", DATA_PATH];

/// The directory of the snapshot files of a repository.
const SNAPSHOTS_PATH: &str = "snapshots";

/// The directory of the index files of a repository.
const INDEX_PATH: &str = "index";

/// The directory of the packs of a repository.
const DATA_PATH: &str = "data";

/// The largest number of blobs of one directory that a copy copies at the same time.
const COPY_CONCURRENCY: usize = 8;

/// The most rounds of the catch-up of one run of a copy.
pub(super) const MOST_CATCH_UPS: u32 = 16;

/// Why a run of a copy failed.
#[derive(Debug)]
pub(super) enum CopyError {
    /// A blob that the run listed outside the snapshot files is gone.
    CopySourceMissing { path: Box<Path> },
    /// The catch-up found new index files in each of its rounds.
    Race,
    /// A storage call failed after its tries.
    CallFailed(anyhow::Error),
    /// A storage call gave a failure that no try can fix.
    Permanent(anyhow::Error),
    /// The operation was cancelled.
    Cancelled(anyhow::Error),
}

impl CopyError {
    /// The error of a failed storage call.
    fn of_storage(error: anyhow::Error) -> Self {
        match storage_end(&error) {
            RunEnd::Permanent => Self::Permanent(error),
            RunEnd::Cancelled => Self::Cancelled(error),
            _ => Self::CallFailed(error),
        }
    }

    /// Gives the failure of the run.
    pub(super) fn into_failure(self) -> anyhow::Error {
        match self {
            Self::CopySourceMissing { path } => anyhow::anyhow!(
                "the blob {} that the copy listed is gone, because a prune removed it",
                path.display()
            ),
            Self::Race => anyhow::anyhow!(
                "the source of the copy wrote new index files in each round of the catch-up"
            ),
            Self::CallFailed(error) | Self::Permanent(error) | Self::Cancelled(error) => error,
        }
    }
}

/// What the catch-up of a copy does after a listing of the index files and the packs of the
/// source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CatchUp {
    /// Copy these blobs, then list again.
    Copy(Box<[Box<Path>]>),
    /// The listing holds no index file that the run did not copy.
    Done,
    /// The catch-up did not end within [`MOST_CATCH_UPS`] rounds.
    Race,
}

/// Decides the catch-up after round number `rounds` listed `listed`, when the run copied `copied`.
/// The run copies each listed blob that it did not copy, and lists again, while the listing holds
/// an index file that it did not copy.
pub(super) fn catch_up(copied: &HashSet<Box<Path>>, listed: &[Box<Path>], rounds: u32) -> CatchUp {
    let new = listed
        .iter()
        .filter(|path| !copied.contains(*path))
        .cloned()
        .collect::<Box<[_]>>();
    if !new.iter().any(|path| path.starts_with(INDEX_PATH)) {
        CatchUp::Done
    } else if rounds >= MOST_CATCH_UPS {
        CatchUp::Race
    } else {
        CatchUp::Copy(new)
    }
}

/// Copies the repository of `from` into the scope `to` in one run, with the config file last, so
/// `to` holds a repository only when all its blobs are there. It does not copy the ledger or a
/// snapshot file that a delete removes after the listing.
///
/// The storage copies each blob on its own side, so no byte of a blob but the config comes to this
/// process. The run copies the packs, the keys and the index files, at most [`COPY_CONCURRENCY`]
/// blobs at a time, each directory after the one before. Then it catches up with the index files
/// and the packs that the source got since its listing. Then it deletes each snapshot file of `to`
/// that its listing of the source did not hold, copies the snapshot files of that listing, and
/// writes the config. After an error it starts no more copies, and it waits for the copies that
/// run before it returns.
///
/// When `again` tells that an earlier run of the call ran, the run first deletes the config of
/// `to`, with one try, so the config that an earlier run wrote does not make `to` a repository
/// while this run fails. A try of that delete without an answer ends the run, and the call waits
/// until the delete has landed or can no longer land, so the delete never lands after a later
/// config write.
///
/// A source without a config is gone: the run copies nothing, and when `again` tells that an
/// earlier run of the call ran, it deletes each snapshot file of `to`.
pub(super) async fn copy_scope(
    from: &SnapshotFiles,
    to: &SnapshotFiles,
    again: bool,
) -> Result<(), CopyError> {
    if again {
        to.once()
            .delete("copy_remove_config", Path::new(CONFIG_PATH))
            .await
            .map_err(CopyError::of_storage)?;
    }
    let Some(config) = from
        .get("copy_read", Path::new(CONFIG_PATH))
        .await
        .map_err(CopyError::of_storage)?
    else {
        if again {
            remove_stale_snapshots(to, &HashSet::new()).await?;
        }
        return Ok(());
    };
    let listed = stream::iter(LISTING_ORDER)
        .then(|directory| from.list_below("copy_list", Path::new(directory)))
        .try_collect::<Vec<_>>()
        .await
        .map_err(CopyError::of_storage)?;
    let paths = |blobs: &[ListedBlob]| {
        blobs
            .iter()
            .map(|blob| blob.path.clone())
            .collect::<Box<[_]>>()
    };
    let (snapshots, data_and_index) = match listed.as_slice() {
        [snapshots, index, keys, data] => {
            (paths(snapshots), [paths(data), paths(keys), paths(index)])
        }
        _ => {
            return Err(CopyError::Permanent(anyhow::anyhow!(
                "the copy listed another number of directories than it asked for"
            )));
        }
    };
    let copied = stream::iter(data_and_index.into_iter().map(Ok))
        .try_fold(HashSet::new(), |mut copied, blobs| async move {
            copy_directory(from, to, &blobs).await?;
            copied.extend(blobs);
            Ok::<_, CopyError>(copied)
        })
        .await?;
    catch_up_rounds(from, to, copied).await?;
    let source = snapshots.iter().cloned().collect::<HashSet<_>>();
    remove_stale_snapshots(to, &source).await?;
    copy_directory(from, to, &snapshots).await?;
    to.put_if_absent("copy_write", Path::new(CONFIG_PATH), &config)
        .await
        .map(|_: PutIfAbsent| ())
        .map_err(CopyError::of_storage)
}

/// Lists the index files and the packs of `from` again, and copies each that the run did not copy,
/// as [`catch_up`] decides, until a listing holds no index file that the run did not copy.
async fn catch_up_rounds(
    from: &SnapshotFiles,
    to: &SnapshotFiles,
    copied: HashSet<Box<Path>>,
) -> Result<(), CopyError> {
    let rounds = stream::unfold(Some((copied, 1u32)), |state| async move {
        let (mut copied, rounds) = state?;
        let listed = async {
            let index = from.list_below("copy_list", Path::new(INDEX_PATH)).await?;
            let data = from.list_below("copy_list", Path::new(DATA_PATH)).await?;
            anyhow::Ok(
                index
                    .iter()
                    .chain(data.iter())
                    .map(|blob| blob.path.clone())
                    .collect::<Box<[_]>>(),
            )
        }
        .await;
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => return Some((Some(Err(CopyError::of_storage(error))), None)),
        };
        match catch_up(&copied, &listed, rounds) {
            CatchUp::Done => Some((Some(Ok(())), None)),
            CatchUp::Race => Some((Some(Err(CopyError::Race)), None)),
            CatchUp::Copy(blobs) => match copy_directory(from, to, &blobs).await {
                Ok(()) => {
                    copied.extend(blobs);
                    Some((None, Some((copied, rounds + 1))))
                }
                Err(error) => Some((Some(Err(error)), None)),
            },
        }
    });
    std::pin::pin!(rounds.filter_map(|ended| async move { ended }))
        .next()
        .await
        .unwrap_or(Ok(()))
}

/// Deletes each snapshot file of `to` that `source` does not hold.
async fn remove_stale_snapshots(
    to: &SnapshotFiles,
    source: &HashSet<Box<Path>>,
) -> Result<(), CopyError> {
    let stale = to
        .list_below("copy_list_target", Path::new(SNAPSHOTS_PATH))
        .await
        .map_err(CopyError::of_storage)?
        .iter()
        .filter(|blob| !source.contains(&blob.path))
        .map(|blob| blob.path.clone())
        .collect::<Box<[_]>>();
    stream::iter(stale.iter().map(Ok))
        .try_for_each(|path| async move {
            to.delete("copy_remove_stale", path)
                .await
                .map_err(CopyError::of_storage)
        })
        .await
}

/// Copies the blobs at `paths`, at most [`COPY_CONCURRENCY`] at a time. A copy that fails stops the
/// start of more copies, and each copy that runs is awaited. Gives the first error.
async fn copy_directory(
    from: &SnapshotFiles,
    to: &SnapshotFiles,
    paths: &[Box<Path>],
) -> Result<(), CopyError> {
    let failed = AtomicBool::new(false);
    let failed = &failed;
    // The stream owns its paths: a stream of borrowed paths makes the future of `copy_all` not
    // `Send` for each lifetime, which the async trait needs.
    stream::iter(paths.to_vec())
        .map(|path| async move { copy_unless_failed(from, to, &path, failed).await })
        .buffer_unordered(COPY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

/// Copies the blob at `path` unless an earlier copy of the directory failed, and marks `failed`
/// when this copy fails.
async fn copy_unless_failed(
    from: &SnapshotFiles,
    to: &SnapshotFiles,
    path: &Path,
    failed: &AtomicBool,
) -> Result<(), CopyError> {
    if failed.load(Ordering::SeqCst) {
        return Ok(());
    }
    let copied = copy_blob(from, to, path).await;
    if copied.is_err() {
        failed.store(true, Ordering::SeqCst);
    }
    copied
}

async fn copy_blob(from: &SnapshotFiles, to: &SnapshotFiles, path: &Path) -> Result<(), CopyError> {
    match from
        .copy_to(to, "copy", path)
        .await
        .map_err(CopyError::of_storage)?
    {
        true => Ok(()),
        // A delete removed the snapshot file after the listing, so the copy leaves it out.
        false if path.starts_with(SNAPSHOTS_PATH) => Ok(()),
        // A prune removed a blob that the copy listed, so a new run lists again.
        false => Err(CopyError::CopySourceMissing { path: path.into() }),
    }
}

/// Deletes the repository and the ledger of the scope. The config file goes first, so the scope
/// holds no repository from that step on. A scope that holds nothing gives success.
pub(super) async fn delete_scope(files: &SnapshotFiles) -> anyhow::Result<()> {
    files.delete("delete_scope", Path::new(CONFIG_PATH)).await?;
    stream::iter(
        LISTING_ORDER
            .iter()
            .map(Path::new)
            .chain(Path::new(LEDGERS_PATH).parent())
            .map(Ok),
    )
    .try_for_each(|directory| async move {
        files
            .delete_dir("delete_scope", directory)
            .await
            .map(|_| ())
    })
    .await
}

#[cfg(test)]
mod tests;
