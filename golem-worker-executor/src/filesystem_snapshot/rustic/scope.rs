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

use super::backend::answer_within;
use super::prune::LEDGER_PATH;
use futures::{StreamExt, TryStreamExt, stream};
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, PutIfAbsent};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The target label of each blob storage call on a scope.
const TARGET_LABEL: &str = "filesystem_snapshot";

/// The path of the config file of a repository.
const CONFIG_PATH: &str = "config";

/// The directories of a repository in the order of a listing. A save writes them in the reverse
/// order, so each snapshot file in a listing has its index files and packs in the later listings.
/// A copy writes them in the reverse order too, so a snapshot file in the target always has its
/// data.
const LISTING_ORDER: [&str; 4] = ["snapshots", "index", "keys", "data"];

/// Copies the repository of the namespace `from` into the empty namespace `to`.
///
/// A namespace without a config file holds no repository, so the call copies nothing. The call
/// writes the config file of `to` last, so `to` holds a repository only when all its blobs are
/// there. It does not copy the ledger. A blob that is gone when the call reads it was deleted
/// after the listing, and the call does not copy it.
pub(super) async fn copy_scope(
    storage: &dyn BlobStorage,
    from: &BlobStorageNamespace,
    to: &BlobStorageNamespace,
    deadline: Duration,
) -> anyhow::Result<()> {
    let Some(config) = answer_within(
        deadline,
        storage.get_raw(
            TARGET_LABEL,
            "copy_read",
            from.clone(),
            Path::new(CONFIG_PATH),
        ),
    )
    .await?
    else {
        return Ok(());
    };
    let listed = stream::iter(LISTING_ORDER)
        .then(|directory| async move {
            answer_within(
                deadline,
                storage.list_blobs_below(
                    TARGET_LABEL,
                    "copy_list",
                    from.clone(),
                    Path::new(directory),
                ),
            )
            .await
        })
        .try_collect::<Vec<_>>()
        .await?;
    let paths = listed
        .iter()
        .rev()
        .flat_map(|blobs| blobs.iter().map(|blob| blob.path.clone()))
        .collect::<Box<[_]>>();
    stream::iter(paths.iter().map(Ok))
        .try_for_each(|path| copy_blob(storage, from, to, path, deadline))
        .await?;
    answer_within(
        deadline,
        storage.put_raw_if_absent(
            TARGET_LABEL,
            "copy_write",
            to.clone(),
            Path::new(CONFIG_PATH),
            &config,
        ),
    )
    .await
    .map(|_: PutIfAbsent| ())
}

async fn copy_blob(
    storage: &dyn BlobStorage,
    from: &BlobStorageNamespace,
    to: &BlobStorageNamespace,
    path: &Path,
    deadline: Duration,
) -> anyhow::Result<()> {
    let content = answer_within(
        deadline,
        storage.get_raw(TARGET_LABEL, "copy_read", from.clone(), path),
    )
    .await?;
    match content {
        Some(content) => {
            answer_within(
                deadline,
                storage.put_raw(TARGET_LABEL, "copy_write", to.clone(), path, &content),
            )
            .await
        }
        None => Ok(()),
    }
}

/// Deletes the repository of the namespace, and the ledger of the store.
///
/// The call deletes the config file first, so the namespace holds no repository from that step on.
/// Then it deletes each directory. A namespace that holds nothing gives success.
pub(super) async fn delete_scope(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    deadline: Duration,
) -> anyhow::Result<()> {
    answer_within(
        deadline,
        storage.delete(
            TARGET_LABEL,
            "delete_scope",
            namespace.clone(),
            Path::new(CONFIG_PATH),
        ),
    )
    .await?;
    let ledger_directory = Path::new(LEDGER_PATH)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let directories = LISTING_ORDER
        .iter()
        .map(PathBuf::from)
        .chain(std::iter::once(ledger_directory))
        .collect::<Box<[_]>>();
    stream::iter(directories.iter().map(Ok))
        .try_for_each(|directory| async move {
            answer_within(
                deadline,
                storage.delete_dir(TARGET_LABEL, "delete_scope", namespace.clone(), directory),
            )
            .await
            .map(|_| ())
        })
        .await
}

#[cfg(test)]
mod tests;
