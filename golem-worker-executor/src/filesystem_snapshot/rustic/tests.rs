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
//! The tests in which a held call gets no answer give each call a short deadline. Each of them
//! keeps the gate of the held calls closed until the storage is dropped. So the threads of rustic
//! stop because of the deadline, and not because the gate opens.

use super::backend::BlobBackend;
use super::holding::{holding_storage, reached_deadline};
use super::scripted::{Script, ScriptedBlobStorage};
use super::{
    ChangeDetection, Chunking, Compression, OperationPhase, PruneSettings, RepackLimits,
    Repository, RepositoryKey, RepositorySettings, SaveSettings, backup_options, config_options,
    open_existing, open_or_create, prune_options, repository_options, run_blocking,
};
use crate::filesystem_snapshot::contract_tests::fixture::{
    Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::contract_tests::new_scope;
use crate::filesystem_snapshot::{SnapshotName, SnapshotScope};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
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
use std::num::{NonZeroI32, NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::runtime::Handle;
use tokio::sync::{Notify, oneshot, watch};
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

/// Gives the path of each pack of the scope, in the order of the paths.
async fn pack_paths(storage: &InMemoryBlobStorage, scope: &SnapshotScope) -> Vec<String> {
    stored_paths(storage, scope)
        .await
        .into_iter()
        .filter(|path| path.starts_with("data/"))
        .collect()
}

/// Writes a tree of one file with the name and the content into a new directory, and gives the
/// directory.
fn one_file_tree(file: &'static str, content: &str) -> Scratch {
    let tree = Scratch::new();
    write_tree(
        tree.path(),
        &[(
            file,
            Spec::File {
                content: Box::from(content.as_bytes()),
                mode: 0o644,
            },
        )],
    );
    tree
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

/// Gives the id in hex of each pack of tree blobs in the repository of the scope.
async fn tree_packs(storage: &Arc<InMemoryBlobStorage>, scope: &SnapshotScope) -> Box<[Box<str>]> {
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
                .filter(|pack| pack.blob_type() == BlobType::Tree)
                .map(|pack| Box::from(pack.id.to_hex().as_str()))
                .collect())
        },
    )
    .await
    .unwrap()
}

/// Writes a tree of `count` directories, each with one small file, into a new directory.
fn many_directories_tree(count: usize) -> Scratch {
    let tree = Scratch::new();
    let names = (0..count)
        .flat_map(|index| [format!("dir-{index}"), format!("dir-{index}/file.txt")])
        .collect::<Vec<_>>();
    let entries = names
        .iter()
        .map(|name| {
            let spec = if name.ends_with(".txt") {
                Spec::File {
                    content: Box::from(name.as_bytes()),
                    mode: 0o644,
                }
            } else {
                Spec::Directory { mode: 0o755 }
            };
            (name.as_str(), spec)
        })
        .collect::<Vec<_>>();
    write_tree(tree.path(), &entries);
    tree
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
async fn a_restore_report_gives_each_phase_of_the_restore_in_order() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let repository = repository(&storage, &new_scope());
    let tree = fixture_tree();
    let into = Scratch::new();
    repository.save(&name("first"), tree.path()).await.unwrap();

    let restored = repository
        .restore(&name("first"), into.path(), None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        restored
            .phases
            .iter()
            .map(|phase| phase.phase)
            .collect::<Vec<_>>(),
        vec![
            OperationPhase::Open,
            OperationPhase::Lookup,
            OperationPhase::IndexLoad,
            OperationPhase::RestorePlan,
            OperationPhase::Restore,
        ]
    );
}

#[test]
async fn a_restore_reads_each_tree_pack_one_time_in_full_and_no_range_of_a_tree_pack() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = many_directories_tree(60);
    repository(&inner, &scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let tree_packs = tree_packs(&inner, &scope).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let into = Scratch::new();

    Repository::new(storage.clone(), scope.clone(), key(), STORAGE_CALL_DEADLINE)
        .restore(&name("first"), into.path(), None)
        .await
        .unwrap();
    let calls_on_tree_packs = |op: &str| {
        tree_packs
            .iter()
            .map(|pack| {
                storage
                    .calls()
                    .iter()
                    .filter(|(op_label, path)| *op_label == op && path.ends_with(&**pack))
                    .count()
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        (
            calls_on_tree_packs("read"),
            calls_on_tree_packs("read_range").iter().sum::<usize>(),
            listing(into.path())
        ),
        (vec![1; tree_packs.len()], 0, listing(tree.path()))
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
#[timeout("60s")]
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
#[timeout("60s")]
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
#[timeout("60s")]
async fn a_save_whose_pack_writes_answer_before_the_deadline_succeeds_and_restores() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let held = Arc::new(Notify::new());
    let (storage, gate, _dropped) = holding_storage(inner, {
        let held = held.clone();
        move |op_label, path| {
            let selected = op_label == "write" && path.starts_with("data");
            if selected {
                held.notify_one();
            }
            selected
        }
    });
    let repository = Repository::new(storage, scope, key(), Duration::from_secs(2));
    let tree = fixture_tree();
    let into = Scratch::new();
    let opener = tokio::spawn(async move {
        held.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(gate);
    });

    let saved = tokio::time::timeout(LIMIT, repository.save(&name("first"), tree.path()))
        .await
        .map(|result| result.map(|_| ()).map_err(|error| format!("{error:#}")));
    let opened = tokio::time::timeout(LIMIT, opener)
        .await
        .is_ok_and(|joined| joined.is_ok());
    let restored = repository
        .restore(&name("first"), into.path(), None)
        .await
        .unwrap();

    assert_eq!(
        (saved, opened, restored.is_some(), listing(into.path())),
        (Ok(Ok(())), true, true, listing(tree.path()))
    );
}

#[test]
#[timeout("60s")]
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
#[timeout("60s")]
async fn a_prune_whose_tree_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = fixture_tree();
    repository(&inner, &scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    // A prune reads the trees of the snapshots, and each tree read is a full read of a pack.
    let (storage, _gate, dropped) = holding_storage(inner, |op_label, path| {
        op_label == "read" && path.starts_with("data")
    });

    let pruned = tokio::time::timeout(
        LIMIT,
        prune(storage, &scope, SHORT_DEADLINE, PruneOptions::default()),
    )
    .await;
    let stopped = dropped_within_limit(dropped).await;

    assert_eq!((failed_at_deadline(pruned), stopped), (Some(true), true));
}

#[test]
async fn a_prune_after_a_forget_deletes_the_packs_of_that_name_and_the_other_name_restores() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let repository = repository(&inner, &scope);
    let first_tree = one_file_tree("first.txt", "only in the first tree");
    let second_tree = one_file_tree("second.txt", "only in the second tree");
    repository
        .save(&name("first"), first_tree.path())
        .await
        .unwrap();
    let first_packs = pack_paths(&inner, &scope).await;
    repository
        .save(&name("second"), second_tree.path())
        .await
        .unwrap();
    let second_packs = pack_paths(&inner, &scope)
        .await
        .into_iter()
        .filter(|path| !first_packs.contains(path))
        .collect::<Vec<_>>();
    let into = Scratch::new();

    repository.forget(&name("first")).await.unwrap();
    prune(
        inner.clone(),
        &scope,
        STORAGE_CALL_DEADLINE,
        PruneOptions::default().instant_delete(true),
    )
    .await
    .unwrap();
    let restored = repository
        .restore(&name("second"), into.path(), None)
        .await
        .unwrap();

    assert_eq!(
        (
            first_packs.is_empty(),
            second_packs.is_empty(),
            pack_paths(&inner, &scope).await,
            restored.is_some(),
            listing(into.path()),
        ),
        (
            false,
            false,
            second_packs.clone(),
            true,
            listing(second_tree.path())
        )
    );
}

/// Gives the modification time of the file.
fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Sets the modification time of the file.
fn set_modified(path: &Path, time: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// Copies each file of the flat tree `from` into the new directory `to`, with its modification
/// time. Each copy is a new inode with a new change time, as a capture gives.
pub(super) fn copy_flat_tree(from: &Path, to: &Path) {
    std::fs::read_dir(from).unwrap().for_each(|entry| {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        std::fs::copy(entry.path(), &target).unwrap();
        set_modified(&target, modified(&entry.path()));
    });
}

/// Gives the change time of the file as seconds and nanoseconds.
fn changed_at(path: &Path) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    (metadata.ctime(), metadata.ctime_nsec())
}

/// The longest time that [`wait_past_change_times`] waits.
const CHANGE_TIME_WAIT: Duration = Duration::from_secs(10);

/// Waits until a file that changes now gets a later change time than each of the files.
///
/// The kernel takes the change time from a clock that moves in ticks of some milliseconds, so two
/// changes within one tick get the same change time. The wait changes a probe file in its own
/// directory until the change time of the probe is later than the latest change time of the
/// files. It fails the test when that does not happen within [`CHANGE_TIME_WAIT`].
pub(super) fn wait_past_change_times(files: &[PathBuf]) {
    let latest = files.iter().map(|file| changed_at(file)).max().unwrap();
    let probe_directory = Scratch::new();
    let probe = probe_directory.path().join("probe");
    let started = std::time::Instant::now();
    let passed = std::iter::repeat_with(|| {
        std::fs::write(&probe, b"probe").unwrap();
        changed_at(&probe)
    })
    .take_while(|_| started.elapsed() < CHANGE_TIME_WAIT)
    .find(|probe_time| *probe_time > latest);
    assert!(
        passed.is_some(),
        "the change time of a new change did not pass {latest:?} within {CHANGE_TIME_WAIT:?}"
    );
}

/// Gives the path of each entry of the directory, in the order of the names.
pub(super) fn entries(directory: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Writes a tree of three files into a new directory, and gives the directory.
pub(super) fn three_file_tree() -> Scratch {
    let tree = Scratch::new();
    ["a.txt", "b.txt", "c.txt"]
        .iter()
        .for_each(|file| std::fs::write(tree.path().join(file), file.as_bytes()).unwrap());
    tree
}

/// Gives the length of the span in seconds.
fn seconds(span: rustic_core::jiff::Span) -> f64 {
    span.total(rustic_core::jiff::Unit::Second).unwrap()
}

#[test]
fn the_default_settings_give_the_options_of_rustic() {
    let config = config_options(&RepositorySettings::default());
    let backup = backup_options(&SaveSettings::default());
    let prune = prune_options(&PruneSettings::default()).unwrap();
    let rustic_prune = rustic_core::PruneOptions::default();

    assert_eq!(
        (
            config.set_chunker,
            config.set_chunk_size,
            config.set_compression,
            config.set_extra_verify,
            backup.threads,
            backup.parent_opts.ignore_ctime,
            backup.parent_opts.ignore_inode,
            prune.fast_repack,
            seconds(prune.keep_delete),
            format!("{:?}", (prune.max_unused, prune.max_repack)),
        ),
        (
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
            seconds(rustic_prune.keep_delete),
            format!("{:?}", (rustic_prune.max_unused, rustic_prune.max_repack)),
        )
    );
}

#[test]
fn each_setting_goes_into_its_rustic_option() {
    let config = config_options(&RepositorySettings {
        chunking: Chunking::Fixed(NonZeroU32::new(65_536).unwrap()),
        compression: Compression::Level(NonZeroI32::new(9).unwrap()),
        extra_verify: false,
    });
    let off = config_options(&RepositorySettings {
        compression: Compression::Off,
        ..RepositorySettings::default()
    });
    let backup = backup_options(&SaveSettings {
        threads: NonZeroUsize::new(2),
        detection: ChangeDetection::SizeMtime,
    });
    let prune = prune_options(&PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
        repack: RepackLimits::Unlimited,
    })
    .unwrap();

    assert_eq!(
        (
            (
                config.set_chunker,
                config.set_chunk_size.map(|size| size.as_u64()),
                config.set_compression,
                config.set_extra_verify,
                off.set_compression,
            ),
            backup.threads,
            backup.parent_opts.ignore_ctime,
            backup.parent_opts.ignore_inode,
            backup.parent_opts.group_by.is_some(),
            backup.as_path,
            prune.fast_repack,
            seconds(prune.keep_delete),
            format!("{:?}", (prune.max_unused, prune.max_repack)),
        ),
        (
            (
                Some(rustic_core::repofile::Chunker::FixedSize),
                Some(65_536),
                Some(9),
                Some(false),
                Some(0),
            ),
            NonZeroUsize::new(2),
            true,
            false,
            true,
            Some(PathBuf::from("/")),
            true,
            0.0,
            format!(
                "{:?}",
                (
                    rustic_core::LimitOption::Percentage(0),
                    rustic_core::LimitOption::Unlimited
                )
            ),
        )
    );
}

#[test]
async fn a_repository_keeps_the_settings_of_its_creation_and_a_bridge_save_uses_the_defaults() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let (created_scope, saved_scope) = (new_scope(), new_scope());
    let settings = RepositorySettings {
        chunking: Chunking::Fixed(NonZeroU32::new(65_536).unwrap()),
        compression: Compression::Off,
        extra_verify: false,
    };
    let backend = Arc::new(BlobBackend::new(
        storage.clone(),
        created_scope.0.clone(),
        Handle::current(),
        STORAGE_CALL_DEADLINE,
    ));
    run_blocking(move || {
        open_or_create(backend, &key(), &settings)?;
        Ok(())
    })
    .await
    .unwrap();
    // 4 chunks of 64 KiB and one of 1 byte, each with other bytes. Rabin keeps a file below its
    // smallest chunk of 512 KiB in one chunk.
    let tree = Scratch::new();
    let content = (0..4 * 65_536 + 1)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(tree.path().join("data"), &content).unwrap();
    let config = async |scope: &SnapshotScope| {
        with_existing_repository(
            storage.clone(),
            scope,
            STORAGE_CALL_DEADLINE,
            |repository| {
                let config = repository.config();
                Ok((
                    config.chunker(),
                    config.chunk_size(),
                    config.compression,
                    config.extra_verify(),
                ))
            },
        )
        .await
        .unwrap()
    };

    let fixed_save = repository(&storage, &created_scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let default_save = repository(&storage, &saved_scope)
        .save(&name("first"), tree.path())
        .await
        .unwrap();
    let (fixed_chunker, fixed_chunk_size, fixed_compression, fixed_extra_verify) =
        config(&created_scope).await;
    let (default_chunker, _, default_compression, default_extra_verify) =
        config(&saved_scope).await;

    assert_eq!(
        (
            (
                fixed_save.data_blobs,
                fixed_save.data_added_packed >= fixed_save.data_added,
                fixed_chunker,
                fixed_chunk_size,
                fixed_compression,
                fixed_extra_verify,
            ),
            (
                default_save.data_blobs,
                default_save.data_added_packed < default_save.data_added,
                default_chunker,
                default_compression,
                default_extra_verify,
            ),
        ),
        (
            (
                5,
                true,
                rustic_core::repofile::Chunker::FixedSize,
                65_536,
                Some(0),
                false,
            ),
            (1, true, rustic_core::repofile::Chunker::Rabin, None, true),
        )
    );
}

#[test]
async fn a_size_and_mtime_save_of_a_copied_tree_reads_no_file_and_a_ctime_save_reads_each() {
    // A copy gives each file a new inode and a new change time, and keeps its size and its
    // modification time. The size-and-mtime form must also not compare inodes: in rustic,
    // `ignore_inode` is false by default, and then no inode is compared.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let tree = three_file_tree();
    let copy = Scratch::new();
    wait_past_change_times(&entries(tree.path()));
    copy_flat_tree(tree.path(), copy.path());
    let same_change_time = entries(tree.path())
        .iter()
        .zip(entries(copy.path()))
        .filter(|(original, copied)| changed_at(original) == changed_at(copied))
        .map(|(original, _)| original.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        same_change_time.is_empty(),
        "a copy has the change time of its original: {same_change_time:?}"
    );
    let second_save = async |detection| {
        let repository = repository(&storage, &new_scope());
        repository.save(&name("first"), tree.path()).await.unwrap();
        let report = repository
            .save_with(
                &name("second"),
                copy.path(),
                SaveSettings {
                    threads: None,
                    detection,
                },
            )
            .await
            .unwrap();
        (
            report.files_new,
            report.files_changed,
            report.files_unmodified,
        )
    };

    let size_mtime = second_save(ChangeDetection::SizeMtime).await;
    let ctime = second_save(ChangeDetection::Ctime).await;

    assert_eq!((size_mtime, ctime), ((0, 0, 3), (0, 3, 0)));
}

#[test]
async fn a_size_and_mtime_save_misses_a_rewrite_of_the_same_size_with_the_old_mtime() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let size_mtime = SaveSettings {
        threads: None,
        detection: ChangeDetection::SizeMtime,
    };
    let rewrite = |tree: &Path| {
        let path = tree.join("a.txt");
        let old = modified(&path);
        let old_change_time = changed_at(&path);
        wait_past_change_times(std::slice::from_ref(&path));
        std::fs::write(&path, b"A.TXT").unwrap();
        set_modified(&path, old);
        assert_ne!(
            changed_at(&path),
            old_change_time,
            "the rewrite kept the change time of the file"
        );
    };
    let changed = async |detection: SaveSettings| {
        let tree = three_file_tree();
        let repository = repository(&storage, &new_scope());
        repository
            .save_with(&name("first"), tree.path(), detection)
            .await
            .unwrap();
        rewrite(tree.path());
        let report = repository
            .save_with(&name("second"), tree.path(), detection)
            .await
            .unwrap();
        let into = Scratch::new();
        repository
            .restore(&name("second"), into.path(), None)
            .await
            .unwrap();
        (
            report.files_changed,
            std::fs::read(into.path().join("a.txt")).unwrap(),
        )
    };

    let missed = changed(size_mtime).await;
    let seen = changed(SaveSettings::default()).await;

    assert_eq!(
        (missed, seen),
        ((0, b"a.txt".to_vec()), (1, b"A.TXT".to_vec()))
    );
}

#[test]
async fn two_prunes_without_a_grace_period_give_back_the_data_that_no_snapshot_uses() {
    // The first save puts both files in one pack. The second save rewrites one of them. After the
    // forget, that pack holds a used and an unused blob, so a prune without limits repacks it.
    let prune_twice = async |fast_repack| {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let scope = new_scope();
        let repository = repository(&storage, &scope);
        let tree = Scratch::new();
        std::fs::write(tree.path().join("kept.txt"), b"kept in both snapshots").unwrap();
        std::fs::write(
            tree.path().join("changed.txt"),
            b"only in the first snapshot",
        )
        .unwrap();
        repository.save(&name("first"), tree.path()).await.unwrap();
        let first_packs = data_packs(&storage, &scope).await;
        std::fs::write(tree.path().join("changed.txt"), b"in the second snapshot").unwrap();
        repository.save(&name("second"), tree.path()).await.unwrap();
        repository.forget(&name("first")).await.unwrap();
        let settings = PruneSettings {
            fast_repack,
            keep_delete: Duration::ZERO,
            repack: RepackLimits::Unlimited,
        };

        let stored = |paths: Vec<String>| {
            first_packs
                .iter()
                .map(|pack| paths.iter().any(|path| path.ends_with(&**pack)))
                .collect::<Vec<_>>()
        };

        let marked = repository.prune(settings).await.unwrap().unwrap();
        let after_mark = stored(pack_paths(&storage, &scope).await);
        let deleted = repository.prune(settings).await.unwrap().unwrap();
        let after_delete = stored(pack_paths(&storage, &scope).await);
        let into = Scratch::new();
        repository
            .restore(&name("second"), into.path(), None)
            .await
            .unwrap();
        (
            marked.packs_repacked,
            marked.bytes_repack_removed > 0,
            first_packs.len(),
            after_mark,
            after_delete,
            deleted.marked_packs_deleted,
            marked
                .phases
                .iter()
                .map(|time| time.phase)
                .collect::<Vec<_>>(),
            listing(into.path()) == listing(tree.path()),
        )
    };
    // The first prune repacks 2 packs, and the second deletes the 3 packs that the first marked.
    // The data pack of the first save is one of them.
    let expected = (
        2,
        true,
        1,
        vec![true],
        vec![false],
        3,
        vec![
            OperationPhase::Open,
            OperationPhase::PrunePlan,
            OperationPhase::Prune,
        ],
        true,
    );

    assert_eq!(
        (prune_twice(false).await, prune_twice(true).await),
        (expected.clone(), expected)
    );
}

#[test]
async fn a_prune_of_a_scope_without_a_repository_gives_nothing() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let pruned = repository(&storage, &new_scope())
        .prune(PruneSettings::default())
        .await
        .unwrap();

    assert_eq!(pruned, None);
}
