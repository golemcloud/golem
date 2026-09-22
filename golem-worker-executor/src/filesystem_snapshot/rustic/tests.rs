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

use super::{OperationPhase, Repository, RepositoryKey, STORAGE_CALL_DEADLINE, repository_options};
use crate::filesystem_snapshot::contract_tests::fixture::{Scratch, fixture, listing, write_tree};
use crate::filesystem_snapshot::contract_tests::new_scope;
use crate::filesystem_snapshot::{SnapshotName, SnapshotScope};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::test;
use tokio::sync::watch;

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

fn key() -> RepositoryKey {
    RepositoryKey::new(std::array::from_fn(|index| index as u8))
}

fn repository(storage: &Arc<InMemoryBlobStorage>, scope: &SnapshotScope) -> Repository {
    Repository::new(storage.clone(), scope.clone(), key(), STORAGE_CALL_DEADLINE)
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
        .restore(&name("first"), into.path(), None)
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
        .restore(&name("first"), first_into.path(), None)
        .await
        .unwrap();
    repository
        .restore(&name("second"), second_into.path(), None)
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
        .restore(&name("first"), first_into.path(), None)
        .await
        .unwrap();
    let second = repository
        .restore(&name("second"), second_into.path(), None)
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
        .restore(&name("first"), into.path(), None)
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
async fn a_restore_reads_data_on_at_most_its_reader_threads() {
    // Each save adds one pack with the data of its new file. The restore of the second snapshot
    // reads the data of both packs, one read for each pack.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = Scratch::new();
    std::fs::write(tree.path().join("first.txt"), b"first").unwrap();
    repository(&storage, &scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    std::fs::write(tree.path().join("second.txt"), b"second").unwrap();
    repository(&storage, &scope)
        .save(&name("second"), tree.path())
        .await
        .unwrap();
    let restore = async |reader_threads| {
        let counting = Arc::new(OverlapCountingStorage::new(storage.clone()));
        let into = Scratch::new();
        Repository::new(
            counting.clone(),
            scope.clone(),
            key(),
            STORAGE_CALL_DEADLINE,
        )
        .restore(&name("second"), into.path(), reader_threads)
        .await
        .unwrap();
        (counting.most(), listing(into.path()))
    };

    let one_thread = restore(NonZeroUsize::new(1)).await;
    let default_threads = restore(None).await;

    assert_eq!(
        (one_thread, default_threads),
        ((1, listing(tree.path())), (2, listing(tree.path())))
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

/// The longest time that a ranged read of a reader thread waits for a second one.
const OVERLAP_WAIT: Duration = Duration::from_millis(300);

/// A blob storage that passes each call to an in-memory blob storage, and counts the ranged reads
/// of reader threads that are in progress at the same time.
///
/// A reader thread of a restore is a thread of a rayon pool. The restore reads the trees on the
/// thread that calls it, which is not a thread of a rayon pool. A ranged read of a reader thread
/// waits until a second one is in progress, or until [`OVERLAP_WAIT`] is over, before it reads.
/// Thus two reads that the restore can do at the same time are in progress at the same time.
#[derive(Debug)]
struct OverlapCountingStorage {
    inner: Arc<InMemoryBlobStorage>,
    in_progress: watch::Sender<usize>,
    most: AtomicUsize,
}

impl OverlapCountingStorage {
    fn new(inner: Arc<InMemoryBlobStorage>) -> Self {
        Self {
            inner,
            in_progress: watch::Sender::new(0),
            most: AtomicUsize::new(0),
        }
    }

    /// The highest number of ranged reads of reader threads that were in progress at one time.
    fn most(&self) -> usize {
        self.most.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BlobStorage for OverlapCountingStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw(target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.inner
            .get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if rayon::current_thread_index().is_none() {
            return self
                .inner
                .get_raw_slice(target_label, op_label, namespace, path, start, end)
                .await;
        }
        let mut changes = self.in_progress.subscribe();
        self.in_progress.send_modify(|count| {
            *count += 1;
            self.most.fetch_max(*count, Ordering::SeqCst);
        });
        // The wait ends at the timeout when no second read comes, which is not an error.
        let _ = tokio::time::timeout(OVERLAP_WAIT, changes.wait_for(|count| *count >= 2)).await;
        let result = self
            .inner
            .get_raw_slice(target_label, op_label, namespace, path, start, end)
            .await;
        self.in_progress.send_modify(|count| *count -= 1);
        result
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.inner
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.inner
            .put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.inner
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists(target_label, op_label, namespace, path)
            .await
    }
}
