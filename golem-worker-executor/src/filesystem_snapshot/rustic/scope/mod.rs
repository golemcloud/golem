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
use super::files::SnapshotFiles;
use super::prune::LEDGERS_PATH;
use futures::{StreamExt, TryStreamExt, stream};
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The directories of a repository in the order of a listing. A save writes them in the reverse
/// order, and so does a copy, so a snapshot file always has its data.
const LISTING_ORDER: [&str; 4] = [SNAPSHOTS_PATH, "index", "keys", "data"];

/// The directory of the snapshot files of a repository.
const SNAPSHOTS_PATH: &str = "snapshots";

/// The largest number of blobs of one directory that a copy copies at the same time.
const COPY_CONCURRENCY: usize = 8;

/// Copies the repository of `from` into the empty scope `to`, with the config file last, so `to`
/// holds a repository only when all its blobs are there. It copies nothing without a config file,
/// and it does not copy the ledger or a blob that a delete removes after the listing.
///
/// The storage copies each blob on its own side, so no byte of a blob but the config comes to
/// this process. The copy goes directory by directory, packs first and snapshot files last, at
/// most [`COPY_CONCURRENCY`] blobs at a time, and each directory ends before the next starts. So
/// a snapshot file is never copied before its packs and index files. After an error it starts no
/// more copies, and it waits for the copies that run before it returns, so no blob lands after
/// it returned.
pub(super) async fn copy_scope(from: &SnapshotFiles, to: &SnapshotFiles) -> anyhow::Result<()> {
    let Some(config) = from.get("copy_read", Path::new(CONFIG_PATH)).await? else {
        return Ok(());
    };
    let listed = stream::iter(LISTING_ORDER)
        .then(|directory| from.list_below("copy_list", Path::new(directory)))
        .try_collect::<Vec<_>>()
        .await?
        .into_boxed_slice();
    stream::iter(listed.iter().rev().map(Ok))
        .try_for_each(|blobs| copy_directory(from, to, blobs))
        .await?;
    to.put_if_absent("copy_write", Path::new(CONFIG_PATH), &config)
        .await
        .map(|_: PutIfAbsent| ())
}

/// Copies the blobs of one directory, at most [`COPY_CONCURRENCY`] at a time. A copy that fails
/// stops the start of more copies, and each copy that runs is awaited. Gives the first error.
async fn copy_directory(
    from: &SnapshotFiles,
    to: &SnapshotFiles,
    blobs: &[ListedBlob],
) -> anyhow::Result<()> {
    let failed = AtomicBool::new(false);
    let failed = &failed;
    stream::iter(
        blobs
            .iter()
            .map(|blob| blob.path.clone())
            .collect::<Vec<_>>(),
    )
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
) -> anyhow::Result<()> {
    if failed.load(Ordering::SeqCst) {
        return Ok(());
    }
    let copied = copy_blob(from, to, path).await;
    if copied.is_err() {
        failed.store(true, Ordering::SeqCst);
    }
    copied
}

async fn copy_blob(from: &SnapshotFiles, to: &SnapshotFiles, path: &Path) -> anyhow::Result<()> {
    match from.copy_to(to, "copy", path).await? {
        true => Ok(()),
        // A delete removed the snapshot file after the listing, so the copy leaves it out.
        false if path.starts_with(SNAPSHOTS_PATH) => Ok(()),
        // A prune removed a file that the copy listed, so the copy can be incomplete. It fails
        // before the config write, so the target holds no repository, and a new copy can succeed.
        false => Err(anyhow::anyhow!(
            "the blob {} that the copy listed is gone, because a prune removed it",
            path.display()
        )),
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
