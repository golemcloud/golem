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

//! Save, restore and forget of a repository on the in-memory blob storage.

use super::{OperationPhase, Repository, RepositoryKey, repository_options};
use crate::filesystem_snapshot::contract_tests::fixture::{Scratch, fixture, listing, write_tree};
use crate::filesystem_snapshot::contract_tests::new_scope;
use crate::filesystem_snapshot::{SnapshotName, SnapshotScope};
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use test_r::test;

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

fn repository(storage: &Arc<InMemoryBlobStorage>, scope: &SnapshotScope) -> Repository {
    Repository::new(
        storage.clone(),
        scope.clone(),
        RepositoryKey::new(std::array::from_fn(|index| index as u8)),
    )
}

/// Writes the fixture of the contract suite into a new directory, and gives the directory.
fn fixture_tree() -> Scratch {
    let tree = Scratch::new();
    write_tree(tree.path(), &fixture());
    tree
}

/// Gives the path of each blob of the scope, in the order of the paths.
async fn stored_paths(storage: &InMemoryBlobStorage, scope: &SnapshotScope) -> Vec<String> {
    let mut paths = storage
        .list_blobs_below("test", "test", scope.0.clone(), Path::new(""))
        .await
        .unwrap()
        .iter()
        .map(|blob| blob.path.display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
async fn a_saved_tree_comes_back_the_same() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let repository = repository(&storage, &new_scope());
    let tree = fixture_tree();
    let into = Scratch::new();

    repository.save(&name("first"), tree.path()).await.unwrap();
    let restored = repository
        .restore(&name("first"), into.path())
        .await
        .unwrap();

    assert_eq!(
        (restored.is_some(), listing(into.path())),
        (true, listing(tree.path()))
    );
}

#[test]
async fn a_second_save_has_the_first_as_parent_and_reads_only_the_changed_file() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let repository = repository(&storage, &new_scope());
    let tree = fixture_tree();
    let first_listing = listing(tree.path());

    let first = repository.save(&name("first"), tree.path()).await.unwrap();
    std::fs::write(tree.path().join("a.txt"), b"changed").unwrap();
    let second = repository.save(&name("second"), tree.path()).await.unwrap();
    let first_into = Scratch::new();
    let second_into = Scratch::new();
    repository
        .restore(&name("first"), first_into.path())
        .await
        .unwrap();
    repository
        .restore(&name("second"), second_into.path())
        .await
        .unwrap();

    assert_eq!(
        (
            second.parent.as_deref(),
            second.files_new,
            second.files_changed,
            second.files_unmodified,
            listing(first_into.path()),
            listing(second_into.path()),
        ),
        (
            Some(&*first.snapshot),
            0,
            1,
            first.files_new - 1,
            first_listing,
            listing(tree.path()),
        )
    );
}

#[test]
async fn a_forgotten_name_does_not_restore_and_the_other_names_do() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let repository = repository(&storage, &new_scope());
    let tree = fixture_tree();
    repository.save(&name("first"), tree.path()).await.unwrap();
    repository.save(&name("second"), tree.path()).await.unwrap();
    let first_into = Scratch::new();
    let second_into = Scratch::new();

    let forgotten = repository.forget(&name("first")).await.unwrap();
    let forgotten_again = repository.forget(&name("first")).await.unwrap();
    let first = repository
        .restore(&name("first"), first_into.path())
        .await
        .unwrap();
    let second = repository
        .restore(&name("second"), second_into.path())
        .await
        .unwrap();

    assert_eq!(
        (
            forgotten,
            forgotten_again,
            first.is_none(),
            second.is_some(),
            listing(second_into.path())
        ),
        (1, 0, true, true, listing(tree.path()))
    );
}

#[test]
async fn the_first_save_creates_the_repository_with_no_key_file_and_later_saves_open_it() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let repository = repository(&storage, &scope);
    let tree = fixture_tree();
    let phases = |report: &super::SaveReport| {
        report
            .phases
            .iter()
            .map(|time| time.phase)
            .collect::<Vec<_>>()
    };

    let first = repository.save(&name("first"), tree.path()).await.unwrap();
    let second = repository.save(&name("second"), tree.path()).await.unwrap();
    let paths = stored_paths(&storage, &scope).await;

    assert_eq!(
        (
            phases(&first),
            phases(&second),
            paths.iter().filter(|path| *path == "config").count(),
            paths
                .iter()
                .filter(|path| path.starts_with("keys/"))
                .count(),
        ),
        (
            vec![
                OperationPhase::Create,
                OperationPhase::IndexLoad,
                OperationPhase::Backup
            ],
            vec![
                OperationPhase::Open,
                OperationPhase::IndexLoad,
                OperationPhase::Backup
            ],
            1,
            0,
        )
    );
}

#[test]
async fn each_scope_is_its_own_repository() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (one, other) = (new_scope(), new_scope());
    let tree = fixture_tree();
    let into = Scratch::new();

    repository(&storage, &one)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let restored = repository(&storage, &other)
        .restore(&name("first"), into.path())
        .await
        .unwrap();
    let forgotten = repository(&storage, &other)
        .forget(&name("first"))
        .await
        .unwrap();

    assert_eq!(
        (
            restored.is_none(),
            forgotten,
            stored_paths(&storage, &one).await.is_empty(),
            stored_paths(&storage, &other).await,
        ),
        (true, 0, false, Vec::<String>::new())
    );
}

#[test]
fn a_repository_uses_no_rustic_cache() {
    assert!(repository_options().no_cache);
}

#[test]
fn the_key_is_the_aes_key_then_k_then_r() {
    let key = RepositoryKey::new(std::array::from_fn(|index| index as u8)).master_key();

    assert_eq!(
        (key.encrypt, key.mac.k, key.mac.r),
        (
            (0..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
            (48..64).collect::<Vec<u8>>()
        )
    );
}
