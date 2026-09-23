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

use super::files::SnapshotFiles;
use super::prune::LEDGER_PATH;
use futures::{StreamExt, TryStreamExt, stream};
use golem_service_base::storage::blob::PutIfAbsent;
use std::path::Path;

/// The path of the config file of a repository.
const CONFIG_PATH: &str = "config";

/// The directories of a repository in the order of a listing. A save writes them in the reverse
/// order, and so does a copy, so a snapshot file always has its data.
const LISTING_ORDER: [&str; 4] = ["snapshots", "index", "keys", "data"];

/// Copies the repository of `from` into the empty scope `to`, with the config file last, so `to`
/// holds a repository only when all its blobs are there. It copies nothing without a config file,
/// and it does not copy the ledger or a blob that a delete removes after the listing.
pub(super) async fn copy_scope(from: &SnapshotFiles, to: &SnapshotFiles) -> anyhow::Result<()> {
    let Some(config) = from.get("copy_read", Path::new(CONFIG_PATH)).await? else {
        return Ok(());
    };
    let listed = stream::iter(LISTING_ORDER)
        .then(|directory| from.list_below("copy_list", Path::new(directory)))
        .try_collect::<Vec<_>>()
        .await?;
    let paths = listed
        .iter()
        .rev()
        .flat_map(|blobs| blobs.iter().map(|blob| blob.path.clone()))
        .collect::<Box<[_]>>();
    stream::iter(paths.iter().map(Ok))
        .try_for_each(|path| copy_blob(from, to, path))
        .await?;
    to.put_if_absent("copy_write", Path::new(CONFIG_PATH), &config)
        .await
        .map(|_: PutIfAbsent| ())
}

async fn copy_blob(from: &SnapshotFiles, to: &SnapshotFiles, path: &Path) -> anyhow::Result<()> {
    match from.get("copy_read", path).await? {
        Some(content) => to.put("copy_write", path, &content).await,
        None => Ok(()),
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
            .chain(Path::new(LEDGER_PATH).parent())
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
