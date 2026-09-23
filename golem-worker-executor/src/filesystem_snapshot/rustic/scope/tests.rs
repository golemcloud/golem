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

use super::super::files::SnapshotFiles;
use super::super::scripted::{Script, ScriptedBlobStorage};
use super::{copy_scope, delete_scope};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(2);

/// The blobs of a small repository, with the ledger of the store.
const REPOSITORY: [(&str, &str); 6] = [
    ("config", "config"),
    ("data/ab/abab", "pack"),
    ("golem/prune-ledger", "ledger"),
    ("index/cdcd", "index"),
    ("keys/efef", "key"),
    ("snapshots/0101", "snapshot"),
];

fn files<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    namespace: &BlobStorageNamespace,
) -> SnapshotFiles {
    SnapshotFiles {
        storage: storage.clone(),
        namespace: namespace.clone(),
        deadline: DEADLINE,
    }
}

fn new_namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::new_v4()),
    }
}

async fn put_all(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    blobs: &[(&str, &str)],
) {
    futures::future::join_all(blobs.iter().map(|(path, content)| {
        storage.put_raw(
            "test",
            "test",
            namespace.clone(),
            Path::new(path),
            content.as_bytes(),
        )
    }))
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
}

/// Gives the path and the content of each blob of the namespace, in the order of the paths.
async fn stored(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
) -> Vec<(String, String)> {
    let listed = storage
        .list_blobs_below("test", "test", namespace.clone(), Path::new(""))
        .await
        .unwrap();
    let mut blobs = futures::future::join_all(listed.iter().map(|blob| async {
        let content = storage
            .get_raw("test", "test", namespace.clone(), &blob.path)
            .await
            .unwrap()
            .unwrap();
        (
            blob.path.display().to_string(),
            String::from_utf8(content).unwrap(),
        )
    }))
    .await;
    blobs.sort();
    blobs
}

fn owned(blobs: &[(&str, &str)]) -> Vec<(String, String)> {
    blobs
        .iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

#[test]
async fn a_copy_gives_the_target_each_blob_of_the_repository_and_not_the_ledger() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (from, to) = (new_namespace(), new_namespace());
    put_all(&*storage, &from, &REPOSITORY).await;

    copy_scope(&files(&storage, &from), &files(&storage, &to))
        .await
        .unwrap();

    assert_eq!(
        (stored(&*storage, &to).await, stored(&*storage, &from).await),
        (
            owned(
                &REPOSITORY
                    .into_iter()
                    .filter(|(path, _)| !path.starts_with("golem/"))
                    .collect::<Vec<_>>()
            ),
            owned(&REPOSITORY)
        )
    );
}

#[test]
async fn a_copy_writes_the_packs_the_keys_the_index_files_the_snapshot_files_and_then_the_config() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let (from, to) = (new_namespace(), new_namespace());
    put_all(&*storage, &from, &REPOSITORY).await;

    copy_scope(&files(&storage, &from), &files(&storage, &to))
        .await
        .unwrap();

    assert_eq!(
        storage
            .calls()
            .into_iter()
            .filter(|(op_label, _)| *op_label == "copy_write")
            .map(|(_, path)| path)
            .collect::<Vec<_>>(),
        vec![
            "data/ab/abab",
            "keys/efef",
            "index/cdcd",
            "snapshots/0101",
            "config"
        ]
    );
}

#[test]
async fn a_copy_lists_the_snapshot_files_before_the_index_files_the_keys_and_the_packs() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let (from, to) = (new_namespace(), new_namespace());
    put_all(&*storage, &from, &REPOSITORY).await;

    copy_scope(&files(&storage, &from), &files(&storage, &to))
        .await
        .unwrap();

    assert_eq!(
        storage
            .calls()
            .into_iter()
            .filter(|(op_label, _)| *op_label == "copy_list")
            .map(|(_, path)| path)
            .collect::<Vec<_>>(),
        vec!["snapshots", "index", "keys", "data"]
    );
}

#[test]
async fn a_copy_of_a_namespace_without_a_config_copies_nothing() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (from, to) = (new_namespace(), new_namespace());
    put_all(
        &*storage,
        &from,
        &REPOSITORY
            .into_iter()
            .filter(|(path, _)| *path != "config")
            .collect::<Vec<_>>(),
    )
    .await;

    copy_scope(&files(&storage, &from), &files(&storage, &to))
        .await
        .unwrap();

    assert_eq!(stored(&*storage, &to).await, Vec::<(String, String)>::new());
}

#[test]
async fn a_copy_that_fails_gives_the_error_and_the_target_has_no_config() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "copy_read" && path.starts_with("index") {
                Script::Refuse
            } else {
                Script::Pass
            }
        });
    let (from, to) = (new_namespace(), new_namespace());
    put_all(&*storage, &from, &REPOSITORY).await;

    let copied = copy_scope(&files(&storage, &from), &files(&storage, &to)).await;

    assert_eq!(
        (
            copied.is_err(),
            stored(&*storage, &to)
                .await
                .into_iter()
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
        ),
        (
            true,
            vec!["data/ab/abab".to_string(), "keys/efef".to_string()]
        )
    );
}

#[test]
async fn a_deleted_scope_holds_no_blob_and_another_scope_keeps_its_blobs() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (deleted, kept) = (new_namespace(), new_namespace());
    put_all(&*storage, &deleted, &REPOSITORY).await;
    put_all(&*storage, &kept, &REPOSITORY).await;

    delete_scope(&files(&storage, &deleted)).await.unwrap();

    assert_eq!(
        (
            stored(&*storage, &deleted).await,
            stored(&*storage, &kept).await
        ),
        (Vec::new(), owned(&REPOSITORY))
    );
}

#[test]
async fn a_delete_of_a_scope_deletes_the_config_first() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let namespace = new_namespace();
    put_all(&*storage, &namespace, &REPOSITORY).await;

    delete_scope(&files(&storage, &namespace)).await.unwrap();

    assert_eq!(
        storage
            .calls()
            .into_iter()
            .filter(|(op_label, _)| *op_label == "delete_scope")
            .map(|(_, path)| path)
            .collect::<Vec<_>>(),
        vec!["config", "snapshots", "index", "keys", "data", "golem"]
    );
}

#[test]
async fn a_delete_of_an_unused_scope_succeeds_and_can_run_again() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let namespace = new_namespace();

    let first = delete_scope(&files(&storage, &namespace)).await;
    put_all(&*storage, &namespace, &REPOSITORY).await;
    let second = delete_scope(&files(&storage, &namespace)).await;
    let third = delete_scope(&files(&storage, &namespace)).await;

    assert_eq!(
        (
            first.is_ok(),
            second.is_ok(),
            third.is_ok(),
            stored(&*storage, &namespace).await
        ),
        (true, true, true, Vec::new())
    );
}
