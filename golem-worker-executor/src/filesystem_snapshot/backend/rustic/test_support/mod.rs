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

//! Shared rustic store fixtures, scripted storage and test-only policy values.

use self::holding::reached_deadline;
use self::scripted::{Script, ScriptedBlobStorage};
use crate::filesystem_snapshot::backend::rustic::RusticSnapshotStore;
use crate::filesystem_snapshot::backend::rustic::blob_backend::{BlobBackend, KEPT_PACKS_LIMIT};
use crate::filesystem_snapshot::backend::rustic::blob_io::SnapshotFiles;
use crate::filesystem_snapshot::backend::rustic::failure_classification::{
    BlobCallFailed, CallFailure, FileMissing, call_failure,
};
use crate::filesystem_snapshot::backend::rustic::pruning::claim::{ClaimEntry, parse_claim_entry};
use crate::filesystem_snapshot::backend::rustic::pruning::eligibility::Percent;
use crate::filesystem_snapshot::backend::rustic::pruning::records::{
    PruneLedger, parse_freed, read_ledger,
};
use crate::filesystem_snapshot::backend::rustic::pruning::timing::CLOCK_SKEW_MARGIN;
use crate::filesystem_snapshot::backend::rustic::publication::PublishBound;
use crate::filesystem_snapshot::backend::rustic::repository::{RepositoryKey, open_existing};
use crate::filesystem_snapshot::backend::rustic::repository_layout::LEDGERS_PATH;
use crate::filesystem_snapshot::backend::rustic::snapshot_catalog::scope_snapshots;
use crate::filesystem_snapshot::backend::rustic::store_policy::{PruneSettings, StorePolicy};
use crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking;
use crate::filesystem_snapshot::clock::SystemClock;
use crate::filesystem_snapshot::test_support::fixture::{
    Listed, Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::test_support::{FailureOf, never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    AgentSnapshots, FilesystemSnapshotStore, RestoreFailure, RunSlots, Slot, SnapshotName,
    Unlimited, Withdrawal,
};
use crate::services::golem_config::{
    DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE,
    FilesystemSnapshotStoreConfig,
};
use anyhow::Context;
use futures::StreamExt;
use futures::future::BoxFuture;
use golem_common::model::RetryConfig;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use rustic_core::repofile::{BlobType, IndexFile};
use rustic_core::{
    ErrorKind, OpenStatus, Repository as RusticRepository, RusticError, RusticResult,
};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{Semaphore, oneshot};
use tokio::time::error::Elapsed;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub(super) mod counting;
pub(super) mod holding;
pub(super) mod scripted;

/// The deadline of each call in the tests that hold a call.
pub(super) const SHORT_DEADLINE: Duration = Duration::from_millis(200);

/// The longest time that a test waits for an operation, or for the threads of an operation to
/// stop.
pub(super) const REPOSITORY_LIMIT: Duration = Duration::from_secs(10);

/// The longest time that a test waits for an operation to reach a call, or to answer when only a
/// store that never answers would take longer. A save, a forget or a prune runs on a blocking
/// thread, and a save or a prune also runs at nice 19. So on a busy host, such as one that runs
/// four copies of the tests at once, a save, a restore or a prune of the fixture tree can take
/// longer than [`REPOSITORY_LIMIT`] before it reaches its first blob call or gives its answer. A wait to this limit ends as soon
/// as its condition holds, so it costs no time on a quiet host. Each test that waits this long has
/// a timeout of 120 s, which covers its setup, this wait and its later steps.
pub(super) const REACH_LIMIT: Duration = Duration::from_secs(45);

/// Gives the blobs of the namespace of the storage, with a tracker of their own. Their calls make
/// one try, wait for at most the deadline, and stop when the token is cancelled.
pub(super) fn files_of(
    storage: Arc<dyn BlobStorage>,
    namespace: BlobStorageNamespace,
    deadline: Duration,
    cancel: CancellationToken,
) -> SnapshotFiles {
    SnapshotFiles::new(storage, namespace, deadline, cancel, TaskTracker::new()).once()
}

/// The runs of a call in the tests that do not test them: one run, so a failed call answers at
/// once.
pub(super) fn one_run() -> golem_common::model::RetryConfig {
    golem_common::model::RetryConfig {
        max_attempts: 1,
        min_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(120),
        multiplier: 4.0,
        max_jitter_factor: None,
    }
}

pub(super) fn repository_key() -> RepositoryKey {
    RepositoryKey::new(std::array::from_fn(|index| index as u8))
}

/// Gives the publish bound of a test store: on when the configuration would allow the deadline and
/// the grace period, that is a deadline of at least the shortest publish try and at most an eighth
/// of the grace period, and off otherwise.
pub(super) fn publish_bound_for(deadline: Duration, grace: Duration) -> PublishBound {
    if deadline >= crate::filesystem_snapshot::backend::rustic::publication::MIN_PUBLISH_TRY
        && deadline.saturating_mul(8) <= grace
    {
        PublishBound::On
    } else {
        PublishBound::Off
    }
}

/// Gives a store over the storage whose calls wait for at most `deadline`. A delete never prunes.
pub(super) fn store_with_deadline(
    storage: Arc<dyn BlobStorage>,
    deadline: Duration,
) -> RusticSnapshotStore {
    RusticSnapshotStore::with_policy(
        storage,
        repository_key(),
        StorePolicy {
            deadline,
            save_threads: None,
            restore_reader_threads: NonZeroUsize::new(6).unwrap(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(15 * 60),
            },
            prune_threshold: Percent(u16::MAX),
            retry: one_run(),
            publish_bound: publish_bound_for(deadline, Duration::from_secs(15 * 60)),
            in_call_tries: 1,
        },
        Arc::new(SystemClock),
    )
}

/// Writes the fixture of the contract suite into a new directory, and gives the directory.
pub(super) fn repository_fixture_tree() -> Scratch {
    let tree = Scratch::new();
    write_tree(tree.path(), &fixture());
    tree
}

/// Gives the path of each blob of the scope, in the order of the paths.
pub(super) async fn stored_paths(
    storage: &InMemoryBlobStorage,
    scope: &AgentSnapshots,
) -> Vec<String> {
    let mut paths = storage
        .list_blobs_below("test", "test", (*scope.0).clone(), Path::new(""))
        .await
        .unwrap()
        .iter()
        .map(|blob| blob.path.display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Gives the id in hex of each pack of data blobs in the repository of the scope.
pub(super) async fn data_packs(
    storage: &Arc<InMemoryBlobStorage>,
    scope: &AgentSnapshots,
) -> Box<[Box<str>]> {
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

/// The time between two checks of [`polled_until`].
pub(super) const POLL_STEP: Duration = Duration::from_millis(5);

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
    scope: &AgentSnapshots,
    deadline: Duration,
) -> Arc<BlobBackend> {
    Arc::new(BlobBackend::new(
        files_of(
            storage,
            (*scope.0).clone(),
            deadline,
            CancellationToken::new(),
        ),
        Handle::current(),
        KEPT_PACKS_LIMIT,
    ))
}

/// Opens the repository of the scope over the storage, and does the work with it on a blocking
/// thread. Each call on the storage waits for at most `deadline`.
pub(super) async fn with_existing_repository<R: Send + 'static>(
    storage: Arc<dyn BlobStorage>,
    scope: &AgentSnapshots,
    deadline: Duration,
    work: impl FnOnce(&RusticRepository<OpenStatus>) -> anyhow::Result<R> + Send + 'static,
) -> anyhow::Result<R> {
    let backend = backend_of(storage, scope, deadline);
    run_blocking(move || {
        let repository =
            open_existing(backend, &repository_key())?.context("the scope has no repository")?;
        work(&repository)
    })
    .await
}

/// Tells whether an operation of the store ended within the limit with the storage error of a
/// call that got no answer within its deadline. `None` means that the operation did not end within
/// the limit.
pub(super) fn failed_at_deadline<T, E: FailureOf>(
    outcome: Result<Result<T, E>, Elapsed>,
) -> Option<bool> {
    outcome.ok().map(|result| {
        result
            .err()
            .and_then(|error| {
                error
                    .failure()
                    .map(|failure| reached_deadline(failure.as_ref()))
            })
            .unwrap_or(false)
    })
}

/// Tells whether the storage is dropped within the limit. The storage is dropped only when each
/// thread that held a copy of it has ended.
pub(super) async fn dropped_within_limit(dropped: oneshot::Receiver<()>) -> bool {
    tokio::time::timeout(REPOSITORY_LIMIT, dropped)
        .await
        .is_ok()
}

/// Gives the modification time of the file.
pub(super) fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Sets the modification time of the file.
pub(super) fn set_modified(path: &Path, time: std::time::SystemTime) {
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
#[cfg(unix)]
pub(super) fn changed_at(path: &Path) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    (metadata.ctime(), metadata.ctime_nsec())
}

/// The longest time that [`wait_past_change_times`] waits.
#[cfg(unix)]
pub(super) const CHANGE_TIME_WAIT: Duration = Duration::from_secs(10);

/// Waits until a file that changes now gets a later change time than each of the files.
///
/// The kernel takes the change time from a clock that moves in ticks of some milliseconds, so two
/// changes within one tick get the same change time. The wait changes a probe file in its own
/// directory until the change time of the probe is later than the latest change time of the
/// files. It fails the test when that does not happen within [`CHANGE_TIME_WAIT`].
#[cfg(unix)]
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
pub(super) fn seconds(span: rustic_core::jiff::Span) -> f64 {
    span.total(rustic_core::jiff::Unit::Second).unwrap()
}

/// A tree of the fixture with one more file whose content is `content`, so that each save of a
/// new content changes a few blobs.
pub(super) fn changed_tree(content: &str) -> Scratch {
    let tree = repository_fixture_tree();
    std::fs::write(tree.path().join("changing.txt"), content).unwrap();
    tree
}

/// Saves the snapshots `p-<index>` for each index of `indexes`, each of its own content.
pub(super) async fn save_numbered(
    store_with_deadline: &RusticSnapshotStore,
    scope: &AgentSnapshots,
    indexes: std::ops::Range<usize>,
) {
    futures::stream::iter(indexes)
        .for_each(|index| async move {
            let tree = changed_tree(&format!("content {index}"));
            store_with_deadline
                .save(
                    scope,
                    &name(&format!("p-{index}")),
                    tree.path(),
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
                .unwrap();
        })
        .await;
}

/// The id of a snapshot file that no scope holds, as the content of a record of freed bytes.
pub(super) const GONE_SNAPSHOT: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The longest time that a test waits for an operation or for the work of a store to end.
pub(super) const LIMIT: Duration = Duration::from_secs(10);

/// A prune threshold that a delete never reaches.
pub(super) const NEVER: Percent = Percent(u16::MAX);

/// A prune threshold of zero bytes, so each delete that frees bytes prunes.
pub(super) const ALWAYS: Percent = Percent(0);

/// A deadline that no call of these tests reaches, so a held call ends only by a cancel.
pub(super) const LONG_DEADLINE: Duration = Duration::from_secs(60);

/// The deadline of a test whose refused write the save waits for: a refused write can still land,
/// so the call waits one deadline after it.
pub(super) const SHORT_WRITE_DEADLINE: Duration = Duration::from_secs(2);

pub(super) const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\
                   202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

pub(super) fn config() -> FilesystemSnapshotStoreConfig {
    FilesystemSnapshotStoreConfig::new(KEY, Duration::from_secs(30), 4, 3).unwrap()
}

pub(super) fn key() -> RepositoryKey {
    RepositoryKey::new(*config().repository_key().bytes())
}

/// The policy of the configuration, with the deadline, the prune threshold and the grace period of
/// the test. A call makes one run and a blob call one try, so a failure gives the answer at once;
/// the tests of the runs and the tries set their own. A save has the publish bound when the
/// configuration would allow the deadline and the grace period.
pub(super) fn policy(deadline: Duration, prune_threshold: Percent, grace: Duration) -> StorePolicy {
    StorePolicy {
        deadline,
        prune: PruneSettings {
            keep_delete: grace,
            ..StorePolicy::from_config(&config()).prune
        },
        prune_threshold,
        retry: one_run(),
        publish_bound: publish_bound_for(deadline, grace),
        in_call_tries: 1,
        ..StorePolicy::from_config(&config())
    }
}

pub(super) fn store(
    storage: Arc<dyn BlobStorage>,
    policy: StorePolicy,
) -> Arc<RusticSnapshotStore> {
    Arc::new(RusticSnapshotStore::with_policy(
        storage,
        key(),
        policy,
        Arc::new(SystemClock),
    ))
}

pub(super) fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

/// Writes a tree of one file with the content into a new directory, and gives the directory.
pub(super) fn one_file_tree(content: &str) -> Scratch {
    let tree = Scratch::new();
    write_tree(
        tree.path(),
        &[(
            "file.txt",
            Spec::File {
                content: Box::from(content.as_bytes()),
                mode: 0o644,
            },
        )],
    );
    tree
}

pub(super) fn fixture_tree() -> Scratch {
    let tree = Scratch::new();
    write_tree(tree.path(), &fixture());
    tree
}

/// Restores the name into a new directory, and gives the listing of the directory.
pub(super) async fn restored_listing(
    store: &RusticSnapshotStore,
    scope: &AgentSnapshots,
    name: &SnapshotName,
) -> Result<Vec<Listed>, RestoreFailure> {
    let into = Scratch::new();
    store.restore(scope, name, into.path(), &Unlimited).await?;
    Ok(listing(into.path()))
}

pub(super) async fn listed_names(
    store: &RusticSnapshotStore,
    scope: &AgentSnapshots,
) -> Vec<String> {
    store
        .list(scope, &Unlimited)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Gives the path of each blob of the namespace whose path starts with the prefix.
pub(super) async fn blobs(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    prefix: &str,
) -> Vec<String> {
    let mut paths = storage
        .list_blobs_below("test", "test", namespace.clone(), Path::new(""))
        .await
        .unwrap()
        .iter()
        .map(|blob| blob.path.display().to_string())
        .filter(|path| path.starts_with(prefix))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Deletes `p-1` without a prune, and then lets a prune that does not see the snapshot file that
/// is left mark the packs that only that snapshot uses, as a prune that read the snapshots before
/// the publish of that snapshot does. Puts the snapshot file back after the prune. Gives the
/// snapshot files before the delete, and whether the prune found unused packs.
pub(super) async fn mark_the_packs_of_a_hidden_snapshot(
    storage: Arc<dyn BlobStorage>,
    scope: &AgentSnapshots,
    grace: Duration,
) -> (Vec<String>, bool) {
    let before = blobs(&*storage, &scope.0, "snapshots/").await;
    let no_prune = store(storage.clone(), policy(LONG_DEADLINE, NEVER, grace));
    no_prune
        .delete(scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    no_prune.shut_down().await;
    let hidden = blobs(&*storage, &scope.0, "snapshots/").await;
    assert_eq!(hidden.len() + 1, before.len());
    let hidden_path = Path::new(&hidden[0]);
    let hidden_bytes = storage
        .get_raw("test", "test", (*scope.0).clone(), hidden_path)
        .await
        .unwrap()
        .unwrap();
    storage
        .delete("test", "test", (*scope.0).clone(), hidden_path)
        .await
        .unwrap();
    let marking = backend_of(storage.clone(), scope, LONG_DEADLINE);
    let report =
        crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking(move || {
            crate::filesystem_snapshot::backend::rustic::pruning::prune(
                marking,
                &key(),
                &PruneSettings {
                    fast_repack: true,
                    keep_delete: grace,
                },
            )
        })
        .await
        .unwrap();
    storage
        .put_raw(
            "test",
            "test",
            (*scope.0).clone(),
            hidden_path,
            &hidden_bytes,
        )
        .await
        .unwrap();
    (before, report.is_some_and(|report| report.packs_unused > 0))
}

pub(super) async fn ledger<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &AgentSnapshots,
) -> PruneLedger {
    read_ledger(
        &files_of(
            storage.clone(),
            (*scope.0).clone(),
            Duration::from_secs(2),
            CancellationToken::new(),
        ),
        &SystemClock,
    )
    .await
    .unwrap()
}

/// Gives the sum of the bytes in the names of the records of freed bytes of the scope.
pub(super) async fn freed<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &AgentSnapshots,
) -> u64 {
    let listed = storage
        .list_blobs_below(
            "test",
            "test",
            (*scope.0).clone(),
            Path::new("golem/prune-freed"),
        )
        .await
        .unwrap();
    listed
        .iter()
        .filter_map(|blob| parse_freed(blob.path.file_name()?.to_str()?))
        .fold(0, u64::saturating_add)
}

/// Waits until the condition holds, or until [`LIMIT`] ends. Gives whether the condition holds.
pub(super) async fn eventually(condition: impl Fn() -> bool) -> bool {
    polled_until(LIMIT, condition).await
}

/// Runs the operation until the calls of the storage match the condition, and then drops it.
/// Gives the output of the operation when it ends first.
pub(super) async fn drop_when<T>(
    storage: &ScriptedBlobStorage,
    condition: impl Fn(&[(&'static str, String)]) -> bool,
    operation: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        output = operation => Some(output),
        _ = eventually(|| condition(&storage.calls())) => None,
    }
}

/// Tells whether the error of a call is `Failed`.
pub(super) fn is_failed<E: FailureOf>(error: &E) -> bool {
    error.failure().is_some()
}

/// Tells whether the error of a call is `Stopped`.
pub(super) fn is_stopped<E: FailureOf>(error: &E) -> bool {
    error.stopped()
}

/// Counts the listings of the packs among the recorded calls.
pub(super) fn data_listings(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == "list_data")
        .count()
}

/// Makes the ledger one entry with no marked packs and a last prune at the time, and writes a
/// record of one freed byte.
pub(super) async fn set_last_prune<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &AgentSnapshots,
    last_prune: golem_common::model::Timestamp,
) {
    storage
        .put_raw(
            "test",
            "test",
            (*scope.0).clone(),
            Path::new("golem/prune-freed/1-test"),
            GONE_SNAPSHOT.as_bytes(),
        )
        .await
        .unwrap();
    storage
        .delete_dir("test", "test", (*scope.0).clone(), Path::new(LEDGERS_PATH))
        .await
        .unwrap();
    put_ledger_entry(
        storage,
        scope,
        &format!("{}-0-test", last_prune.to_millis()),
    )
    .await;
}

/// Makes the ledger one entry with the marked packs of `ledger` and a time before it by two hours,
/// which is more than the grace period of each test and the margin for clock skew.
pub(super) async fn age_ledger<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &AgentSnapshots,
    ledger: &PruneLedger,
) {
    let back = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap() + 2 * 3_600_000;
    let aged = ledger
        .last_prune
        .map_or(0, |last| last.to_millis().saturating_sub(back));
    storage
        .delete_dir("test", "test", (*scope.0).clone(), Path::new(LEDGERS_PATH))
        .await
        .unwrap();
    put_ledger_entry(
        storage,
        scope,
        &format!("{aged}-{}-aged", u8::from(ledger.awaiting_removal)),
    )
    .await;
}

/// Writes a ledger entry with the name.
pub(super) async fn put_ledger_entry<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &AgentSnapshots,
    name: &str,
) {
    storage
        .put_raw(
            "test",
            "test",
            (*scope.0).clone(),
            &Path::new(LEDGERS_PATH).join(name),
            b"",
        )
        .await
        .unwrap();
}

/// Counts the listings of the snapshot files of a repository in `calls`.
pub(super) fn snapshot_listings(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, path)| *op_label == "list" && path.starts_with("snapshots"))
        .count()
}

/// Counts the prunes among the recorded calls. A prune lists the packs, and no other step of a
/// delete makes that call.
/// How each prune of the store that failed since the last take failed.
pub(super) fn failed_prunes(store: &RusticSnapshotStore) -> Vec<CallFailure> {
    store
        .take_failed_prunes()
        .iter()
        .map(call_failure)
        .collect()
}

pub(super) fn prunes(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, path)| *op_label == "list" && path == "data")
        .count()
}

/// Saves a tree of one file with the content under each name.
pub(super) async fn save_each(store: &RusticSnapshotStore, scope: &AgentSnapshots, names: &[&str]) {
    futures::stream::iter(names)
        .for_each(|text| async move {
            let tree = one_file_tree(text);
            store
                .save(
                    scope,
                    &name(text),
                    tree.path(),
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
                .unwrap();
        })
        .await;
}

/// Moves each marker of the claims of the scope back by two hours and the margin, so no marker
/// holds its ledger.
pub(super) async fn age_claims<S: BlobStorage + 'static>(storage: &Arc<S>, scope: &AgentSnapshots) {
    let stale = golem_common::model::Timestamp::now_utc()
        .to_millis()
        .saturating_sub(2 * 3_600_000 + 2 * 60_000);
    let markers = storage
        .list_blobs_below(
            "test",
            "test",
            (*scope.0).clone(),
            Path::new("golem/prune-claims"),
        )
        .await
        .unwrap()
        .iter()
        .filter_map(|blob| {
            let entry = parse_claim_entry(blob.path.file_name()?.to_str()?)?;
            match entry {
                ClaimEntry::Marker(number, _) => Some((blob.path.clone(), number)),
                ClaimEntry::Claim(_) => None,
            }
        })
        .collect::<Vec<_>>();
    futures::stream::iter(markers)
        .for_each(|(path, number)| {
            let (storage, scope) = (storage.clone(), scope.clone());
            async move {
                storage
                    .delete("test", "test", (*scope.0).clone(), &path)
                    .await
                    .unwrap();
                let aged = path
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(format!("{number}@{stale}-aged"));
                storage
                    .put_raw("test", "test", (*scope.0).clone(), &aged, b"")
                    .await
                    .unwrap();
            }
        })
        .await;
}

/// Tells whether the operation label is a call of a rustic backend.
pub(super) fn is_backend_call(op_label: &str) -> bool {
    matches!(
        op_label,
        "stat" | "list" | "read" | "read_range" | "write" | "delete"
    )
}

/// Gives the entries of the claims of the scope.
pub(super) async fn claim_entries(
    storage: &ScriptedBlobStorage,
    scope: &AgentSnapshots,
) -> Vec<ClaimEntry> {
    blobs(storage, &scope.0, "golem/prune-claims/")
        .await
        .iter()
        .filter_map(|path| parse_claim_entry(Path::new(path).file_name()?.to_str()?))
        .collect()
}

/// Gives the time in the name of each blob that the calls with the operation label wrote. The time
/// is the number before the first `-` of the name, after the `@` of a marker.
pub(super) fn written_times(calls: &[(&'static str, String)], op_label: &str) -> Box<[u64]> {
    calls
        .iter()
        .filter(|(label, _)| *label == op_label)
        .filter_map(|(_, path)| {
            let file = Path::new(path).file_name()?.to_str()?;
            let time = file.split_once('@').map_or(file, |(_, rest)| rest);
            time.split('-').next()?.parse().ok()
        })
        .collect()
}

/// Gives the path of each record of freed bytes of the scope.
pub(super) async fn records(storage: &InMemoryBlobStorage, scope: &AgentSnapshots) -> Vec<String> {
    blobs(storage, &scope.0, "golem/prune-freed/").await
}

/// Gives the number of markers of the claims of the scope.
pub(super) async fn markers(storage: &ScriptedBlobStorage, scope: &AgentSnapshots) -> usize {
    claim_entries(storage, scope)
        .await
        .iter()
        .filter(|entry| matches!(entry, ClaimEntry::Marker(..)))
        .count()
}

/// A store over the shared storage whose index reads wait at the gate while `hold` is set, and the
/// number of index reads that reached the storage while it was set.
pub(super) fn holding_index_reads(
    shared: &Arc<InMemoryBlobStorage>,
) -> (
    Arc<RusticSnapshotStore>,
    Arc<ScriptedBlobStorage>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
) {
    let (hold, held) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let storage = ScriptedBlobStorage::new(shared.clone(), {
        let (hold, held) = (hold.clone(), held.clone());
        move |op_label, path| {
            if op_label == "read" && path.starts_with("index") && hold.load(Ordering::SeqCst) {
                held.fetch_add(1, Ordering::SeqCst);
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    (store, storage, hold, held)
}

/// Saves `p-1` and `p-2` of two trees in a new scope of the shared storage, and deletes `p-2`
/// through a store that prunes at once. The prune deletes the index file of `p-2`.
pub(super) async fn scope_with_a_prune_to_come(
    shared: &Arc<InMemoryBlobStorage>,
) -> (AgentSnapshots, Arc<RusticSnapshotStore>, Scratch) {
    let pruning = store(
        shared.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (kept, deleted) = (fixture_tree(), one_file_tree("deleted"));
    pruning
        .save(
            &scope,
            &name("p-1"),
            kept.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    pruning
        .save(
            &scope,
            &name("p-2"),
            deleted.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    (scope, pruning, kept)
}

/// Gives the snapshot files of the scope that the bridge reads, on a blocking thread.
pub(super) async fn snapshot_files(
    storage: Arc<dyn BlobStorage>,
    scope: &AgentSnapshots,
) -> Vec<rustic_core::repofile::SnapshotFile> {
    let backend = backend_of(storage, scope, LONG_DEADLINE);
    tokio::task::spawn_blocking(move || {
        let repository = open_existing(backend.clone(), &key()).unwrap().unwrap();
        scope_snapshots(&repository, &backend).unwrap().readable
    })
    .await
    .unwrap()
}

/// The operation label, the path, and the name and the nice value of the calling thread of each
/// storage call.
#[cfg(target_os = "linux")]
pub(super) type NiceCalls = Arc<std::sync::Mutex<Vec<(String, String, String, i32)>>>;

/// A storage that records the nice value of the thread of each call.
#[cfg(target_os = "linux")]
pub(super) fn nice_recording_storage() -> (Arc<ScriptedBlobStorage>, NiceCalls) {
    let calls = NiceCalls::default();
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let calls = calls.clone();
        move |op_label, path| {
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    op_label.to_string(),
                    path.display().to_string(),
                    std::thread::current()
                        .name()
                        .unwrap_or_default()
                        .to_string(),
                    crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice(),
                ));
            Script::Pass
        }
    });
    (storage, calls)
}

/// Takes the recorded calls as the operation label, the path and the nice value.
#[cfg(target_os = "linux")]
pub(super) fn taken_calls(calls: &NiceCalls) -> Vec<(String, String, i32)> {
    std::mem::take(
        &mut *calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_iter()
    .map(|(op_label, path, _, nice)| (op_label, path, nice))
    .collect()
}

/// Takes the recorded calls with the operation label and a path below the directory, as the path
/// and the name of the thread of each call.
#[cfg(target_os = "linux")]
pub(super) fn taken_threads(
    calls: &NiceCalls,
    op_label: &str,
    directory: &str,
) -> Vec<(String, String)> {
    std::mem::take(
        &mut *calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_iter()
    .filter(|(op, path, _, _)| op == op_label && path.starts_with(directory))
    .map(|(_, path, thread, _)| (path, thread))
    .collect()
}

/// Gives the operation labels of the calls, and each call that does not run at nice 19.
#[cfg(target_os = "linux")]
pub(super) fn labels_and_calls_not_at_nice_19(
    calls: &[(String, String, i32)],
) -> (Vec<&str>, Vec<&(String, String, i32)>) {
    let mut labels = calls
        .iter()
        .map(|(op_label, _, _)| op_label.as_str())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    let not_at_nice_19 = calls.iter().filter(|(_, _, nice)| *nice != 19).collect();
    (labels, not_at_nice_19)
}

/// The runs of the tests of this module: three runs, with short waits.
pub(super) fn three_runs() -> RetryConfig {
    RetryConfig {
        max_attempts: 3,
        min_delay: Duration::from_millis(20),
        max_delay: Duration::from_millis(80),
        multiplier: 2.0,
        max_jitter_factor: None,
    }
}

/// The policy of the tests of this module: a deadline of `deadline`, three runs, and the tries of
/// `tries`.
pub(super) fn runs_policy(deadline: Duration, tries: u32) -> StorePolicy {
    StorePolicy {
        retry: three_runs(),
        in_call_tries: tries,
        ..policy(deadline, NEVER, Duration::ZERO)
    }
}

/// A limiter of `permits` slots that counts its takes, the slots that live and the most slots
/// that lived at once. Its clones share the slots and the counts.
#[derive(Clone)]
pub(super) struct SharedSlots {
    pub(super) semaphore: Arc<Semaphore>,
    pub(super) takes: Arc<AtomicUsize>,
    pub(super) live: Arc<AtomicUsize>,
    pub(super) most: Arc<AtomicUsize>,
    pub(super) failed_waits: Arc<AtomicUsize>,
    pub(super) late_waits: Arc<AtomicUsize>,
}

impl SharedSlots {
    pub(super) fn new(permits: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
            takes: Arc::default(),
            live: Arc::default(),
            most: Arc::default(),
            failed_waits: Arc::default(),
            late_waits: Arc::default(),
        }
    }

    /// The waits for late writes that the store reported.
    pub(super) fn late_waits(&self) -> usize {
        self.late_waits.load(Ordering::SeqCst)
    }

    /// The takes, the slots that live now, the most slots that lived at once, and the waits after
    /// a failed run that the store reported.
    pub(super) fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.takes.load(Ordering::SeqCst),
            self.live.load(Ordering::SeqCst),
            self.most.load(Ordering::SeqCst),
            self.failed_waits.load(Ordering::SeqCst),
        )
    }
}

/// A slot of a [`SharedSlots`]: a permit, counted as live until it drops.
pub(super) struct SharedSlot {
    pub(super) _permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) live: Arc<AtomicUsize>,
}

impl Drop for SharedSlot {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RunSlots for SharedSlots {
    fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        Box::pin(async move {
            let permit = Arc::clone(&self.semaphore)
                .acquire_owned()
                .await
                .map_err(|_| Withdrawal::Stopped)?;
            self.takes.fetch_add(1, Ordering::SeqCst);
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(live, Ordering::SeqCst);
            Ok(Slot::new(SharedSlot {
                _permit: permit,
                live: Arc::clone(&self.live),
            }))
        })
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(std::future::pending())
    }

    fn waiting_after_failure(&self) {
        self.failed_waits.fetch_add(1, Ordering::SeqCst);
    }

    fn waiting_for_late_writes(&self) {
        self.late_waits.fetch_add(1, Ordering::SeqCst);
    }
}

/// A storage that refuses the first `refused` calls that `selected` picks, and passes each other
/// call.
pub(super) fn refusing(
    refused: usize,
    selected: impl Fn(&str, &Path) -> bool + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::with_state(
        Arc::new(InMemoryBlobStorage::new()),
        0usize,
        move |seen, op_label, path| match selected(op_label, path) {
            true if *seen < refused => (Script::Refuse, seen + 1),
            true => (Script::Pass, seen + 1),
            false => (Script::Pass, *seen),
        },
    )
}

/// Gives the number of calls of the storage with the operation label.
pub(super) fn calls_with(calls: &[(&'static str, String)], label: &str) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == label)
        .count()
}

/// Gives the number of calls of the storage with the operation label.
pub(super) fn calls_of(storage: &ScriptedBlobStorage, label: &str) -> usize {
    storage
        .calls()
        .iter()
        .filter(|(op_label, _)| *op_label == label)
        .count()
}

/// A storage over `inner` whose publish tries follow `script(n)` for the `n`-th try, counted from
/// 1, and whose other calls pass.
pub(super) fn scripted_publish(
    inner: Arc<InMemoryBlobStorage>,
    script: impl Fn(usize) -> Script + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::with_state(inner, 0usize, move |tries, op_label, _| {
        if op_label == "publish" {
            (script(tries + 1), tries + 1)
        } else {
            (Script::Pass, *tries)
        }
    })
}

/// A store over `inner` with the policy of the tests, which passes each call.
pub(super) fn store_over(inner: &Arc<InMemoryBlobStorage>) -> Arc<RusticSnapshotStore> {
    store(inner.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO))
}

/// A storage over `inner` whose first write of a pack follows `first`, and whose other calls
/// follow `other`.
pub(super) fn scripted_first_pack(
    inner: Arc<InMemoryBlobStorage>,
    first: Script,
    other: impl Fn(&str, &Path) -> Script + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::with_state(inner, 0usize, move |packs, op_label, path| {
        match op_label == "write" && path.starts_with("data") {
            true if *packs == 0 => (first, packs + 1),
            true => (other(op_label, path), packs + 1),
            false => (other(op_label, path), *packs),
        }
    })
}

/// Whether the storage saw a write of a pack.
pub(super) fn wrote_a_pack(storage: &ScriptedBlobStorage) -> bool {
    storage
        .calls()
        .iter()
        .any(|(op_label, path)| *op_label == "write" && path.starts_with("data"))
}

/// Gives a mapping_rustic error whose source is the error, as mapping_rustic gives it to the store.
pub(super) fn mapping_rustic(
    source: impl std::error::Error + Send + Sync + 'static,
) -> anyhow::Error {
    anyhow::Error::new(RusticError::with_source(
        ErrorKind::Backend,
        "the operation failed",
        source,
    ))
}

/// A blob call that failed with `failure`, as the backend gives it to mapping_rustic.
pub(super) fn mapping_failed_call(failure: anyhow::Error) -> anyhow::Error {
    mapping_rustic(BlobCallFailed::new(failure))
}

pub(super) fn mapping_missing(path: &str) -> anyhow::Error {
    mapping_rustic(FileMissing {
        path: Path::new(path).into(),
    })
}

/// An error that holds no storage failure and no I/O error.
pub(super) fn mapping_refused() -> anyhow::Error {
    anyhow::Error::new(RusticError::new(
        ErrorKind::Cryptography,
        "the data failed its check",
    ))
}

/// The cause of a local I/O error as the fork gives it: an error that shows the OS error in its
/// text and has no source.
#[derive(Debug)]
pub(super) struct OpeningFileFailed;

impl std::fmt::Display for OpeningFileFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "opening file failed: `Os { code: 13, kind: PermissionDenied, message: \"Permission denied\" }`",
        )
    }
}

impl std::error::Error for OpeningFileFailed {}

/// The error of a write of a restore whose file cannot be opened, as the fork gives it.
pub(super) fn mapping_local_io_without_source() -> anyhow::Error {
    anyhow::Error::new(RusticError::with_source(
        ErrorKind::InputOutput,
        "Failed to set the length of the file `{path}`. Please check the path and try again.",
        OpeningFileFailed,
    ))
}
