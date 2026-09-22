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

//! Save, restore, forget and prune of a repository on the in-memory blob storage.
//!
//! The tests with a held call give each call a short deadline. Each of them keeps the gate of the
//! held calls closed until the storage is dropped. So the threads of rustic stop because of the
//! deadline, and not because the gate opens.

use super::backend::BlobBackend;
use super::holding::{holding_storage, reached_deadline};
use super::{
    OperationPhase, Repository, RepositoryKey, STORAGE_CALL_DEADLINE, open_existing,
    repository_options, run_blocking,
};
use crate::filesystem_snapshot::contract_tests::fixture::{
    Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::contract_tests::new_scope;
use crate::filesystem_snapshot::{SnapshotName, SnapshotScope};
use anyhow::Context;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use rustic_core::repofile::{BlobType, IndexFile};
use rustic_core::{OpenStatus, PruneOptions, Repository as RusticRepository, RusticResult};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::test;
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio::time::error::Elapsed;

/// The deadline of each call in the tests that hold a call.
const SHORT_DEADLINE: Duration = Duration::from_millis(200);

/// The longest time that a test waits for an operation, or for the threads of an operation to
/// stop.
const LIMIT: Duration = Duration::from_secs(10);

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

/// Gives the id in hex of each pack of data blobs in the repository of the scope.
async fn data_packs(storage: &Arc<InMemoryBlobStorage>, scope: &SnapshotScope) -> Box<[Box<str>]> {
    with_existing_repository(
        storage.clone(),
        scope,
        STORAGE_CALL_DEADLINE,
        |repository| {
            let indexes = repository
                .stream_files::<IndexFile>()?
                .collect::<RusticResult<Vec<_>>>()?;
            Ok(indexes
                .into_iter()
                .flat_map(|(_, index)| index.packs)
                .filter(|pack| pack.blob_type() == BlobType::Data)
                .map(|pack| Box::from(pack.id.to_hex().as_str()))
                .collect())
        },
    )
    .await
    .unwrap()
}

/// Prunes the repository of the scope with the options, on a blocking thread. Each call on the
/// storage waits for at most `deadline`.
async fn prune(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
    deadline: Duration,
    options: PruneOptions,
) -> anyhow::Result<()> {
    with_existing_repository(storage, scope, deadline, move |repository| {
        let plan = repository.prune_plan(&options)?;
        Ok(repository.prune(&options, plan)?)
    })
    .await
}

/// Opens the repository of the scope over the storage, and does the work with it on a blocking
/// thread. Each call on the storage waits for at most `deadline`.
async fn with_existing_repository<R: Send + 'static>(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
    deadline: Duration,
    work: impl FnOnce(&RusticRepository<OpenStatus>) -> anyhow::Result<R> + Send + 'static,
) -> anyhow::Result<R> {
    let backend = Arc::new(BlobBackend::new(
        storage,
        scope.0.clone(),
        Handle::current(),
        deadline,
    ));
    run_blocking(move || {
        let repository = open_existing(backend, &key())?.context("the scope has no repository")?;
        work(&repository)
    })
    .await
}

/// Tells whether an operation ended within the limit with the error of a call that got no answer
/// within its deadline. `None` means that the operation did not end within the limit.
fn failed_at_deadline<T>(outcome: Result<anyhow::Result<T>, Elapsed>) -> Option<bool> {
    outcome
        .ok()
        .map(|result| result.is_err_and(|error| reached_deadline(error.as_ref())))
}

/// Tells whether the storage is dropped within the limit. The storage is dropped only when each
/// thread that held a copy of it has ended.
async fn dropped_within_limit(dropped: oneshot::Receiver<()>) -> bool {
    tokio::time::timeout(LIMIT, dropped).await.is_ok()
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

#[test]
async fn a_save_whose_pack_write_gets_no_answer_fails_with_no_snapshot_and_its_threads_stop() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let (storage, _gate, dropped) = holding_storage(inner.clone(), |op_label, path| {
        op_label == "write" && path.starts_with("data")
    });
    let repository = Repository::new(storage, scope.clone(), key(), SHORT_DEADLINE);
    let tree = fixture_tree();

    let saved = tokio::time::timeout(LIMIT, repository.save(&name("first"), tree.path())).await;
    drop(repository);
    let stopped = dropped_within_limit(dropped).await;
    let snapshots = stored_paths(&inner, &scope)
        .await
        .into_iter()
        .filter(|path| path.starts_with("snapshots/"))
        .collect::<Vec<_>>();

    assert_eq!(
        (failed_at_deadline(saved), snapshots, stopped),
        (Some(true), Vec::<String>::new(), true)
    );
}

#[test]
async fn a_restore_whose_data_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = fixture_tree();
    repository(&inner, &scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let data_packs = data_packs(&inner, &scope).await;
    let has_data_packs = !data_packs.is_empty();
    let (storage, _gate, dropped) = holding_storage(inner, move |op_label, path| {
        op_label == "read_range"
            && path
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| data_packs.iter().any(|pack| **pack == *file))
    });
    let repository = Repository::new(storage, scope, key(), SHORT_DEADLINE);
    let into = Scratch::new();
    let directories = fixture()
        .into_iter()
        .filter(|(_, spec)| matches!(spec, Spec::Directory { .. }))
        .map(|(path, _)| path)
        .collect::<Vec<_>>();

    let restored =
        tokio::time::timeout(LIMIT, repository.restore(&name("first"), into.path(), None)).await;
    drop(repository);
    let stopped = dropped_within_limit(dropped).await;
    // The plan phase of a restore makes each directory before the content phase reads data.
    let made = directories
        .iter()
        .map(|path| (*path, into.path().join(path).is_dir()))
        .collect::<Vec<_>>();

    assert_eq!(
        (
            has_data_packs,
            failed_at_deadline(restored),
            stopped,
            directories.is_empty(),
            made
        ),
        (
            true,
            Some(true),
            true,
            false,
            directories
                .iter()
                .map(|path| (*path, true))
                .collect::<Vec<_>>()
        )
    );
}

#[test]
async fn a_prune_whose_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = fixture_tree();
    repository(&inner, &scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let (storage, _gate, dropped) = holding_storage(inner, |op_label, path| {
        op_label == "read_range" && path.starts_with("data")
    });

    let pruned = tokio::time::timeout(
        LIMIT,
        prune(storage, &scope, SHORT_DEADLINE, PruneOptions::default()),
    )
    .await;
    let stopped = dropped_within_limit(dropped).await;

    assert_eq!((failed_at_deadline(pruned), stopped), (Some(true), true));
}
