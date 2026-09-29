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

//! Save, restore, delete and prune of a repository on the in-memory blob storage, through the
//! store and through the free functions of the bridge.
//!
//! The tests in which a held call gets no answer give each call a short deadline. Each of them
//! keeps the gate of the held calls closed until the storage is dropped. So the threads of rustic
//! stop because of the deadline, and not because the gate opens.

pub(super) mod holding;
pub(super) mod scripted;

use self::holding::{holding_storage, reached_deadline};
use self::scripted::{Script, ScriptedBlobStorage};
use super::backend::{BlobBackend, KEPT_PACKS_LIMIT};
use super::files::SnapshotFiles;
use super::prune::Percent;
use super::store::{RusticSnapshotStore, StorePolicy, named};
use super::{
    PruneReport, PruneSettings, RepositoryKey, backup_options, open_existing, open_or_create,
    prune_options, repository_options, restore_snapshot, run_blocking,
};
use crate::filesystem_snapshot::clock::SystemClock;
use crate::filesystem_snapshot::contract_tests::fixture::{
    Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::contract_tests::new_scope;
use crate::filesystem_snapshot::{
    ChangeDetection as StoreChangeDetection, FilesystemSnapshotStore, SnapshotName, SnapshotScope,
    SnapshotStoreError,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use anyhow::Context;
use async_trait::async_trait;
use futures::StreamExt;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob,
    NormalizedBlobPath, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use rustic_core::repofile::{BlobType, IndexFile};
use rustic_core::{OpenStatus, Repository as RusticRepository, RestoreOptions, RusticResult};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::runtime::Handle;
use tokio::sync::{Notify, oneshot, watch};
use tokio::time::error::Elapsed;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The deadline of each call in the tests that hold a call.
const SHORT_DEADLINE: Duration = Duration::from_millis(200);

/// The longest time that a test waits for an operation, or for the threads of an operation to
/// stop.
const LIMIT: Duration = Duration::from_secs(10);

/// Gives the blobs of the namespace of the storage, with a tracker of their own. Their calls wait
/// for at most the deadline, and they stop when the token is cancelled.
pub(super) fn files_of(
    storage: Arc<dyn BlobStorage>,
    namespace: BlobStorageNamespace,
    deadline: Duration,
    cancel: CancellationToken,
) -> SnapshotFiles {
    SnapshotFiles::new(storage, namespace, deadline, cancel, TaskTracker::new())
}

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

fn key() -> RepositoryKey {
    RepositoryKey::new(std::array::from_fn(|index| index as u8))
}

/// Gives a store over the storage whose calls wait for at most `deadline`. A delete never prunes.
fn store(storage: Arc<dyn BlobStorage>, deadline: Duration) -> RusticSnapshotStore {
    RusticSnapshotStore::with_policy(
        storage,
        key(),
        StorePolicy {
            deadline,
            save_threads: None,
            restore_reader_threads: NonZeroUsize::new(6).unwrap(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(15 * 60),
            },
            prune_threshold: Percent(u16::MAX),
        },
        Arc::new(SystemClock),
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

/// The time between two checks of [`polled_until`].
const POLL_STEP: Duration = Duration::from_millis(5);

/// Checks the condition every [`POLL_STEP`] until it holds or `bound` ends. Gives whether the
/// condition held.
pub(super) async fn polled_until(bound: Duration, condition: impl Fn() -> bool) -> bool {
    tokio::time::timeout(bound, async {
        futures::stream::repeat(())
            .then(|()| tokio::time::sleep(POLL_STEP))
            .take_while(|()| std::future::ready(!condition()))
            .for_each(|()| std::future::ready(()))
            .await
    })
    .await
    .is_ok()
}

/// Gives a backend over the repository of the scope in the storage, on the current runtime, whose
/// calls wait for at most `deadline`.
pub(super) fn backend_of(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
    deadline: Duration,
) -> Arc<BlobBackend> {
    Arc::new(BlobBackend::new(
        files_of(storage, scope.0.clone(), deadline, CancellationToken::new()),
        Handle::current(),
        KEPT_PACKS_LIMIT,
    ))
}

/// Prunes the repository of the scope with the settings through the prune of the store, on a
/// blocking thread. Each call on the storage waits for at most `deadline`.
async fn prune_with(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
    deadline: Duration,
    settings: PruneSettings,
) -> anyhow::Result<Option<PruneReport>> {
    let backend = backend_of(storage, scope, deadline);
    run_blocking(move || super::prune(backend, &key(), &settings)).await
}

/// Finds the snapshot with the name in the scope with the lookup rule of the store. Then it
/// restores that snapshot into the empty directory `into` with the options, on a blocking thread.
async fn restore_named(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
    name: &SnapshotName,
    into: &Path,
    options: RestoreOptions,
) -> anyhow::Result<()> {
    let backend = backend_of(storage, scope, STORAGE_CALL_DEADLINE);
    let (name, into) = (name.clone(), Box::<Path>::from(into));
    run_blocking(move || {
        let repository = open_existing(backend, &key())?.context("the scope has no repository")?;
        let snapshots = repository.get_all_snapshots()?;
        let snapshot = named(&snapshots, &name).context("no snapshot has the name")?;
        restore_snapshot(repository, snapshot, &into, &options)
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
    let backend = backend_of(storage, scope, deadline);
    run_blocking(move || {
        let repository = open_existing(backend, &key())?.context("the scope has no repository")?;
        work(&repository)
    })
    .await
}

/// Tells whether an operation of the store ended within the limit with the storage error of a
/// call that got no answer within its deadline. `None` means that the operation did not end within
/// the limit.
fn failed_at_deadline<T>(outcome: Result<Result<T, SnapshotStoreError>, Elapsed>) -> Option<bool> {
    outcome.ok().map(|result| {
        matches!(result, Err(SnapshotStoreError::Storage { source, .. }) if reached_deadline(source.as_ref()))
    })
}

/// Tells whether the storage is dropped within the limit. The storage is dropped only when each
/// thread that held a copy of it has ended.
async fn dropped_within_limit(dropped: oneshot::Receiver<()>) -> bool {
    tokio::time::timeout(LIMIT, dropped).await.is_ok()
}

#[test]
#[timeout("60s")]
async fn a_restore_reads_each_tree_pack_one_time_in_full_and_no_range_of_a_tree_pack() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = many_directories_tree(60);
    store(inner.clone(), STORAGE_CALL_DEADLINE)
        .save(&scope, &name("p-first"), tree.path(), None)
        .await
        .unwrap();
    let tree_packs = tree_packs(&inner, &scope).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let into = Scratch::new();

    store(storage.clone(), STORAGE_CALL_DEADLINE)
        .restore(&scope, &name("p-first"), into.path())
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
#[timeout("60s")]
async fn the_first_save_creates_the_repository_with_no_key_file_and_later_saves_open_it() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store = store(storage.clone(), STORAGE_CALL_DEADLINE);
    let tree = fixture_tree();

    store
        .save(&scope, &name("p-first"), tree.path(), None)
        .await
        .unwrap();
    let config_after_the_first = stored_paths(&storage, &scope)
        .await
        .into_iter()
        .filter(|path| path == "config")
        .count();
    store
        .save(&scope, &name("p-second"), tree.path(), None)
        .await
        .unwrap();
    let paths = stored_paths(&storage, &scope).await;

    assert_eq!(
        (
            config_after_the_first,
            paths.iter().filter(|path| *path == "config").count(),
            paths
                .iter()
                .filter(|path| path.starts_with("keys/"))
                .count(),
            paths
                .iter()
                .filter(|path| path.starts_with("snapshots/"))
                .count(),
        ),
        (1, 1, 0, 2)
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_reads_data_on_at_most_its_reader_threads() {
    // Each save adds one pack with the data of its new file. The restore of the second snapshot
    // reads the data of both packs, one read for each pack.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store = store(storage.clone(), STORAGE_CALL_DEADLINE);
    let tree = Scratch::new();
    std::fs::write(tree.path().join("first.txt"), b"first").unwrap();
    store
        .save(&scope, &name("p-first"), tree.path(), None)
        .await
        .unwrap();
    std::fs::write(tree.path().join("second.txt"), b"second").unwrap();
    store
        .save(&scope, &name("p-second"), tree.path(), None)
        .await
        .unwrap();
    let restore = async |reader_threads| {
        let counting = Arc::new(OverlapCountingStorage::new(storage.clone()));
        let into = Scratch::new();
        restore_named(
            counting.clone(),
            &scope,
            &name("p-second"),
            into.path(),
            RestoreOptions::default().reader_threads(reader_threads),
        )
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
#[timeout("60s")]
async fn a_create_whose_config_write_finds_the_config_of_another_writer_opens_that_repository() {
    // The gate holds the config write of the losing create after both of its checks found no
    // config. The winning create writes the config meanwhile, so the held write finds it.
    let shared = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let losing = ScriptedBlobStorage::new(shared.clone(), |op_label, path| {
        if op_label == "write" && path == Path::new("config") {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let backend =
        |storage: Arc<dyn BlobStorage>| backend_of(storage, &scope, STORAGE_CALL_DEADLINE);
    let loser = backend(losing.clone());
    let lost = tokio::task::spawn_blocking(move || {
        open_or_create(loser, &key()).map(|repository| repository.config().id)
    });
    let held = polled_until(Duration::from_secs(10), || {
        losing
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "write" && path == "config")
    })
    .await;

    let winner = backend(shared.clone());
    let won = run_blocking(move || Ok(open_or_create(winner, &key())?.config().id)).await;
    losing.open_gate();
    let lost = tokio::time::timeout(LIMIT, lost).await;

    let won = won.unwrap();
    let lost = lost
        .ok()
        .and_then(Result::ok)
        .map(|opened| opened.map_err(|error| error.to_string()));
    assert_eq!((held, lost), (true, Some(Ok(won))));
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
impl BlobStorageBackend for OverlapCountingStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_at(target_label, op_label, namespace, path)
            .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<golem_service_base::storage::blob::BlobRangeStream>> {
        self.inner
            .get_range_stream_at(target_label, op_label, namespace, path, offset, length)
            .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if rayon::current_thread_index().is_none() {
            return self
                .inner
                .get_raw_slice_at(target_label, op_label, namespace, path, start, end)
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
            .get_raw_slice_at(target_label, op_label, namespace, path, start, end)
            .await;
        self.in_progress.send_modify(|count| *count -= 1);
        result
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata_at(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.inner
            .put_raw_at(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .delete_at(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below_at(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        self.inner
            .delete_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists_at(target_label, op_label, namespace, path)
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
    let store = store(storage, SHORT_DEADLINE);
    let tree = fixture_tree();

    let saved = tokio::time::timeout(
        LIMIT,
        store.save(&scope, &name("p-first"), tree.path(), None),
    )
    .await;
    drop(store);
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
    let store = store(storage, Duration::from_secs(2));
    let tree = fixture_tree();
    let into = Scratch::new();
    let opener = tokio::spawn(async move {
        held.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(gate);
    });

    let saved = tokio::time::timeout(
        LIMIT,
        store.save(&scope, &name("p-first"), tree.path(), None),
    )
    .await
    .map(|result| result.map(|_| ()).map_err(|error| format!("{error:#}")));
    let opened = tokio::time::timeout(LIMIT, opener)
        .await
        .is_ok_and(|joined| joined.is_ok());
    let restored = store.restore(&scope, &name("p-first"), into.path()).await;

    assert_eq!(
        (saved, opened, restored.is_ok(), listing(into.path())),
        (Ok(Ok(())), true, true, listing(tree.path()))
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_data_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = fixture_tree();
    store(inner.clone(), STORAGE_CALL_DEADLINE)
        .save(&scope, &name("p-first"), tree.path(), None)
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
    let store = store(storage, SHORT_DEADLINE);
    let into = Scratch::new();
    let directories = fixture()
        .into_iter()
        .filter(|(_, spec)| matches!(spec, Spec::Directory { .. }))
        .map(|(path, _)| path)
        .collect::<Vec<_>>();

    let restored =
        tokio::time::timeout(LIMIT, store.restore(&scope, &name("p-first"), into.path())).await;
    drop(store);
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
    store(inner.clone(), STORAGE_CALL_DEADLINE)
        .save(&scope, &name("p-first"), tree.path(), None)
        .await
        .unwrap();
    // A prune reads the trees of the snapshots, and each tree read is a full read of a pack.
    let (storage, _gate, dropped) = holding_storage(inner, |op_label, path| {
        op_label == "read" && path.starts_with("data")
    });

    let pruned = tokio::time::timeout(
        LIMIT,
        prune_with(
            storage,
            &scope,
            SHORT_DEADLINE,
            PruneSettings {
                fast_repack: true,
                keep_delete: Duration::ZERO,
            },
        ),
    )
    .await;
    let stopped = dropped_within_limit(dropped).await;

    assert_eq!(
        (
            pruned
                .ok()
                .map(|result| result.is_err_and(|error| reached_deadline(error.as_ref()))),
            stopped
        ),
        (Some(true), true)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_after_a_delete_deletes_the_packs_of_that_name_and_the_other_name_restores() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store = store(inner.clone(), STORAGE_CALL_DEADLINE);
    let first_tree = one_file_tree("first.txt", "only in the first tree");
    let second_tree = one_file_tree("second.txt", "only in the second tree");
    store
        .save(&scope, &name("p-first"), first_tree.path(), None)
        .await
        .unwrap();
    let first_packs = pack_paths(&inner, &scope).await;
    store
        .save(&scope, &name("p-second"), second_tree.path(), None)
        .await
        .unwrap();
    let second_packs = pack_paths(&inner, &scope)
        .await
        .into_iter()
        .filter(|path| !first_packs.contains(path))
        .collect::<Vec<_>>();
    let into = Scratch::new();

    // The first prune marks the packs that only the deleted name used, and the second prune
    // deletes them, because they stay marked for no time.
    store.delete(&scope, &name("p-first")).await.unwrap();
    let settings = PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
    };
    prune_with(inner.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
        .await
        .unwrap();
    prune_with(inner.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
        .await
        .unwrap();
    let restored = store.restore(&scope, &name("p-second"), into.path()).await;

    assert_eq!(
        (
            first_packs.is_empty(),
            second_packs.is_empty(),
            pack_paths(&inner, &scope).await,
            restored.is_ok(),
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
fn the_off_side_of_each_save_and_prune_setting_goes_into_its_rustic_option() {
    let backup = backup_options(None);
    let prune = prune_options(&PruneSettings {
        fast_repack: false,
        keep_delete: Duration::from_secs(23 * 3600),
    })
    .unwrap();

    assert_eq!(
        (
            backup.threads,
            prune.fast_repack,
            seconds(prune.keep_delete)
        ),
        (None, false, 82_800.0)
    );
}

#[test]
fn each_setting_goes_into_its_rustic_option() {
    let backup = backup_options(NonZeroUsize::new(2));
    let prune = prune_options(&PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
    })
    .unwrap();

    assert_eq!(
        (
            backup.threads,
            backup.parent_opts.group_by.is_some(),
            backup.as_path,
            prune.fast_repack,
            seconds(prune.keep_delete),
            format!("{:?}", (prune.max_unused, prune.max_repack)),
        ),
        (
            NonZeroUsize::new(2),
            true,
            Some(PathBuf::from("/")),
            true,
            0.0,
            format!(
                "{:?}",
                (
                    rustic_core::LimitOption::Percentage(5),
                    rustic_core::LimitOption::Percentage(10)
                )
            ),
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_size_and_mtime_save_misses_a_rewrite_of_the_same_size_with_the_old_mtime_and_a_full_save_reads_it()
 {
    let storage = Arc::new(InMemoryBlobStorage::new());
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
    let changed = async |detection: StoreChangeDetection| {
        let tree = three_file_tree();
        let scope = new_scope();
        let store = store(storage.clone(), STORAGE_CALL_DEADLINE);
        store
            .save(&scope, &name("p-first"), tree.path(), None)
            .await
            .unwrap();
        rewrite(tree.path());
        store
            .save(
                &scope,
                &name("p-second"),
                tree.path(),
                Some((&name("p-first"), detection)),
            )
            .await
            .unwrap();
        let into = Scratch::new();
        store
            .restore(&scope, &name("p-second"), into.path())
            .await
            .unwrap();
        let counts = with_existing_repository(
            storage.clone(),
            &scope,
            STORAGE_CALL_DEADLINE,
            |repository| {
                repository
                    .get_all_snapshots()?
                    .into_iter()
                    .find(|snapshot| snapshot.label == "p-second")
                    .and_then(|snapshot| snapshot.summary)
                    .map(|summary| {
                        (
                            summary.files_new,
                            summary.files_changed,
                            summary.files_unmodified,
                        )
                    })
                    .context("the second snapshot has no summary")
            },
        )
        .await
        .unwrap();
        (counts, std::fs::read(into.path().join("a.txt")).unwrap())
    };

    let missed = changed(StoreChangeDetection::SizeMtime).await;
    let seen = changed(StoreChangeDetection::Full).await;

    assert_eq!(
        (missed, seen),
        (
            ((0, 0, 3), b"a.txt".to_vec()),
            ((3, 0, 0), b"A.TXT".to_vec())
        )
    );
}

#[test]
#[timeout("60s")]
async fn two_prunes_without_a_grace_period_under_the_limits_of_rustic_give_back_the_data_that_no_snapshot_uses()
 {
    // The first save puts both files in one pack. The second save rewrites one of them. After the
    // delete, that pack holds a used and an unused blob. The used blob is far below 10% of the
    // repository, and the unused blob is above 5% of the used data. So a prune under the limits of
    // rustic repacks it.
    let prune_twice = async |fast_repack| {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let scope = new_scope();
        let store = store(storage.clone(), STORAGE_CALL_DEADLINE);
        let tree = Scratch::new();
        std::fs::write(tree.path().join("kept.txt"), b"kept in both snapshots").unwrap();
        std::fs::write(
            tree.path().join("changed.txt"),
            b"only in the first snapshot",
        )
        .unwrap();
        store
            .save(&scope, &name("p-first"), tree.path(), None)
            .await
            .unwrap();
        let first_packs = data_packs(&storage, &scope).await;
        std::fs::write(tree.path().join("changed.txt"), b"in the second snapshot").unwrap();
        store
            .save(&scope, &name("p-second"), tree.path(), None)
            .await
            .unwrap();
        store.delete(&scope, &name("p-first")).await.unwrap();
        let settings = PruneSettings {
            fast_repack,
            keep_delete: Duration::ZERO,
        };

        let stored = |paths: Vec<String>| {
            first_packs
                .iter()
                .map(|pack| paths.iter().any(|path| path.ends_with(&**pack)))
                .collect::<Vec<_>>()
        };

        let marked = prune_with(storage.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
            .await
            .unwrap()
            .unwrap();
        let after_mark = stored(pack_paths(&storage, &scope).await);
        let deleted = prune_with(storage.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
            .await
            .unwrap()
            .unwrap();
        let after_delete = stored(pack_paths(&storage, &scope).await);
        let into = Scratch::new();
        store
            .restore(&scope, &name("p-second"), into.path())
            .await
            .unwrap();
        (
            marked.packs_repacked,
            first_packs.len(),
            after_mark,
            after_delete,
            deleted.packs_repacked,
            listing(into.path()) == listing(tree.path()),
        )
    };
    // The first prune repacks the data pack of the first save and marks it. The second prune
    // deletes it.
    let expected = (1, 1, vec![true], vec![false], 0, true);

    assert_eq!(
        (prune_twice(false).await, prune_twice(true).await),
        (expected.clone(), expected)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_of_a_scope_without_a_repository_gives_nothing() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let pruned = prune_with(
        storage,
        &new_scope(),
        STORAGE_CALL_DEADLINE,
        PruneSettings {
            fast_repack: false,
            keep_delete: Duration::ZERO,
        },
    )
    .await
    .unwrap();

    assert_eq!(pruned, None);
}
