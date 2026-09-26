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

//! The rustic store through the interface of the store, on the in-memory blob storage.
//!
//! The contract suite runs on the store with the policy of the configuration. The other tests
//! give the store a short or a long deadline and a prune policy that the test controls.

use super::super::files::SnapshotFiles;
use super::super::prune::{
    CLOCK_SKEW_MARGIN, ClaimEntry, LEDGERS_PATH, Percent, PruneLedger, parse_claim_entry,
    parse_freed, read_ledger,
};
use super::super::scripted::{Script, ScriptedBlobStorage};
use super::super::tests::{copy_flat_tree, entries, three_file_tree, wait_past_change_times};
use super::super::{PruneReport, PruneSettings, RepackLimits, RepositoryKey, open_existing};
use super::{
    PublishGate, RusticSnapshotStore, StorePolicy, leaves_marked_packs, scope_snapshots,
    store_backup_options, store_restore_options, whole_millis_from,
};
use crate::filesystem_snapshot::contract_tests::fixture::{
    Listed, Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::contract_tests::{self, OpenStore, new_scope};
use crate::filesystem_snapshot::{
    ChangeDetection, FilesystemSnapshotStore, SnapshotInfo, SnapshotName, SnapshotScope,
    SnapshotStoreError,
};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use futures::{FutureExt, StreamExt, TryStreamExt};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use rustic_core::repofile::SnapshotId;
use std::future::Future;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::core::DynamicTestRegistration;
use test_r::{test, test_gen, timeout};

/// The id of a snapshot file that no scope holds, as the content of a record of freed bytes.
const GONE_SNAPSHOT: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The longest time that a test waits for an operation or for the work of a store to end.
const LIMIT: Duration = Duration::from_secs(10);

/// A prune threshold that a delete never reaches.
const NEVER: Percent = Percent(u16::MAX);

/// A prune threshold of zero bytes, so each delete that frees bytes prunes.
const ALWAYS: Percent = Percent(0);

/// A deadline that no call of these tests reaches, so a held call ends only by a cancel.
const LONG_DEADLINE: Duration = Duration::from_secs(60);

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\
                   202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

fn config() -> FilesystemSnapshotStoreConfig {
    FilesystemSnapshotStoreConfig::new(KEY, Duration::from_secs(30), 4, 3).unwrap()
}

fn key() -> RepositoryKey {
    RepositoryKey::new(*config().repository_key().bytes())
}

/// The policy of the configuration, with the deadline, the prune threshold and the grace period
/// of the test.
fn policy(deadline: Duration, prune_threshold: Percent, grace: Duration) -> StorePolicy {
    StorePolicy {
        deadline,
        prune: PruneSettings {
            keep_delete: grace,
            ..StorePolicy::from_config(&config()).prune
        },
        prune_threshold,
        ..StorePolicy::from_config(&config())
    }
}

fn store(storage: Arc<dyn BlobStorage>, policy: StorePolicy) -> Arc<RusticSnapshotStore> {
    Arc::new(RusticSnapshotStore::with_policy(storage, key(), policy))
}

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

/// Writes a tree of one file with the content into a new directory, and gives the directory.
fn one_file_tree(content: &str) -> Scratch {
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

fn fixture_tree() -> Scratch {
    let tree = Scratch::new();
    write_tree(tree.path(), &fixture());
    tree
}

/// Restores the name into a new directory, and gives the listing of the directory.
async fn restored_listing(
    store: &RusticSnapshotStore,
    scope: &SnapshotScope,
    name: &SnapshotName,
) -> Result<Vec<Listed>, SnapshotStoreError> {
    let into = Scratch::new();
    store.restore(scope, name, into.path()).await?;
    Ok(listing(into.path()))
}

async fn listed_names(store: &RusticSnapshotStore, scope: &SnapshotScope) -> Vec<String> {
    store
        .list(scope)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Gives the path of each blob of the namespace whose path starts with the prefix.
async fn blobs(
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

async fn ledger<S: BlobStorage + 'static>(storage: &Arc<S>, scope: &SnapshotScope) -> PruneLedger {
    read_ledger(&SnapshotFiles {
        storage: storage.clone(),
        namespace: scope.0.clone(),
        deadline: Duration::from_secs(2),
        cancel: tokio_util::sync::CancellationToken::new(),
        tracker: tokio_util::task::TaskTracker::new(),
    })
    .await
    .unwrap()
}

/// Gives the sum of the bytes in the names of the records of freed bytes of the scope.
async fn freed<S: BlobStorage + 'static>(storage: &Arc<S>, scope: &SnapshotScope) -> u64 {
    let listed = storage
        .list_blobs_below(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-freed"),
        )
        .await
        .unwrap();
    listed
        .iter()
        .filter_map(|blob| parse_freed(blob.path.file_name()?.to_str()?))
        .fold(0, u64::saturating_add)
}

/// Waits until the condition holds, or until the limit ends. Gives whether the condition holds.
async fn eventually(condition: impl Fn() -> bool) -> bool {
    tokio::time::timeout(LIMIT, async {
        futures::stream::repeat(())
            .then(|()| tokio::time::sleep(Duration::from_millis(5)))
            .take_while(|()| std::future::ready(!condition()))
            .for_each(|()| std::future::ready(()))
            .await
    })
    .await
    .is_ok()
}

/// Runs the operation until the calls of the storage match the condition, and then drops it.
/// Gives the output of the operation when it ends first.
async fn drop_when<T>(
    storage: &ScriptedBlobStorage,
    condition: impl Fn(&[(&'static str, String)]) -> bool,
    operation: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        output = operation => Some(output),
        _ = eventually(|| condition(&storage.calls())) => None,
    }
}

fn is_storage(error: &SnapshotStoreError, expected_retryable: bool) -> bool {
    matches!(error, SnapshotStoreError::Storage { retryable, .. } if *retryable == expected_retryable)
}

#[test_gen]
fn rustic_store_keeps_the_contract(r: &mut DynamicTestRegistration) {
    contract_tests::register(r, || {
        let storage: Arc<dyn BlobStorage> = Arc::new(InMemoryBlobStorage::new());
        let open: OpenStore = Arc::new(move || {
            Arc::new(RusticSnapshotStore::new(storage.clone(), &config()))
                as Arc<dyn FilesystemSnapshotStore>
        });
        open
    });
}

#[test]
fn the_policy_takes_the_configured_values_and_the_options_are_strict() {
    let policy = StorePolicy::from_config(&config());
    let backup = store_backup_options(&policy, None);
    let restore = store_restore_options(&policy);

    assert_eq!(
        (
            policy.deadline,
            policy.save_threads.map(NonZeroUsize::get),
            policy.restore_reader_threads.get(),
            policy.prune.keep_delete,
            policy.prune.fast_repack,
            policy.prune.repack,
            policy.prune_threshold,
        ),
        (
            Duration::from_secs(30),
            Some(3),
            4,
            Duration::from_secs(15 * 60),
            true,
            RepackLimits::Rustic,
            Percent(10),
        )
    );
    assert_eq!(
        (
            backup.fail_on_read_error,
            backup.threads.map(NonZeroUsize::get),
            backup.as_path.as_deref().map(Path::to_path_buf),
            restore.fail_on_metadata_error,
            restore.no_ownership,
            restore.reader_threads.map(NonZeroUsize::get),
        ),
        (
            true,
            Some(3),
            Some(Path::new("/").to_path_buf()),
            true,
            true,
            Some(4),
        )
    );
}

#[test]
fn a_save_time_is_the_first_whole_millisecond_that_is_not_before_the_call() {
    let now = golem_common::model::Timestamp::now_utc();
    let whole = golem_common::model::Timestamp::from(5_000);
    let rounded = whole_millis_from(now);

    assert_eq!(
        (
            whole_millis_from(whole),
            rounded >= now,
            rounded.to_millis() - now.to_millis() <= 1,
            golem_common::model::Timestamp::from(rounded.to_millis()) == rounded,
        ),
        (whole, true, true, true)
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn a_tree_saved_through_a_proc_self_fd_path_is_stored_below_the_root() {
    use std::os::fd::AsRawFd;
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let directory = std::fs::File::open(tree.path()).unwrap();
    let through_fd = format!("/proc/self/fd/{}", directory.as_raw_fd());

    store
        .save(&scope, &name("p-fd"), Path::new(&through_fd), None)
        .await
        .unwrap();
    let namespace = scope.0.clone();
    let paths = tokio::task::spawn_blocking(move || {
        let backend = super::super::backend::BlobBackend::new(
            storage,
            namespace,
            tokio::runtime::Handle::current(),
            LONG_DEADLINE,
        );
        let repository = open_existing(Arc::new(backend), &key()).unwrap().unwrap();
        scope_snapshots(&repository)
            .unwrap()
            .readable
            .iter()
            .map(|snapshot| snapshot.paths.to_string())
            .collect::<Vec<_>>()
    })
    .await
    .unwrap();

    assert_eq!(
        (
            paths,
            restored_listing(&store, &scope, &name("p-fd"))
                .await
                .unwrap()
        ),
        (vec!["/".to_string()], listing(tree.path()))
    );
}

#[test]
async fn a_save_whose_index_write_fails_publishes_nothing_and_leaves_the_name_free() {
    let refuse = Arc::new(AtomicBool::new(true));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst) && op_label == "write" && path.starts_with("index") {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();

    let failed = store.save(&scope, &name("p-1"), tree.path(), None).await;
    let stat = store.stat(&scope, &name("p-1")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let restore = restored_listing(&store, &scope, &name("p-1")).await;
    refuse.store(false, Ordering::SeqCst);
    let saved_again = store.save(&scope, &name("p-1"), tree.path(), None).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(
        matches!(restore, Err(SnapshotStoreError::NotFound)),
        "{restore:?}"
    );
    assert_eq!(
        (
            stat,
            names,
            saved_again.is_ok(),
            restored_listing(&store, &scope, &name("p-1"))
                .await
                .unwrap()
        ),
        (None, Vec::<String>::new(), true, listing(tree.path()))
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_that_reaches_the_deadline_and_lands_late_publishes_nothing() {
    let hang = Arc::new(AtomicBool::new(true));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let hang = hang.clone();
        move |op_label, _| {
            if hang.load(Ordering::SeqCst) && op_label == "publish" {
                Script::NeverAnswer
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(Duration::from_secs(1), NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("late");

    let failed = store.save(&scope, &name("p-late"), tree.path(), None).await;
    hang.store(false, Ordering::SeqCst);
    let stat = store.stat(&scope, &name("p-late")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let saved_again = store.save(&scope, &name("p-late"), tree.path(), None).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert_eq!(
        (
            stat,
            names,
            saved_again.is_ok(),
            storage
                .calls()
                .iter()
                .filter(|(op_label, _)| *op_label == "retract")
                .count()
        ),
        (None, Vec::<String>::new(), true, 1)
    );
}

#[test]
async fn a_second_save_of_an_unchanged_tree_writes_no_pack() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();

    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    let first = storage.calls().len();
    store
        .save(&scope, &name("p-2"), tree.path(), None)
        .await
        .unwrap();
    let writes = storage.calls()[first..]
        .iter()
        .filter(|(op_label, _)| *op_label == "write" || *op_label == "publish")
        .map(|(op_label, path)| {
            (
                *op_label,
                path.split('/').next().unwrap_or_default().to_string(),
            )
        })
        .collect::<Vec<_>>();

    let restored = restored_listing(&store, &scope, &name("p-2")).await.ok();

    assert_eq!(
        (writes, restored),
        (
            vec![("publish", "snapshots".to_string())],
            Some(listing(tree.path()))
        )
    );
}

#[test]
#[timeout("60s")]
async fn two_stores_that_create_one_repository_at_the_same_time_both_save() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path == Path::new("config") {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        });
    let policy = policy(LONG_DEADLINE, NEVER, Duration::ZERO);
    let (first, second) = (
        store(storage.clone(), policy),
        store(storage.clone(), policy),
    );
    let scope = new_scope();
    let (first_tree, second_tree) = (one_file_tree("first"), one_file_tree("second"));
    let config_writes = || {
        storage
            .calls()
            .iter()
            .filter(|(op_label, path)| *op_label == "write" && path == "config")
            .count()
    };

    let (first_name, second_name) = (name("p-first"), name("p-second"));
    let (first_saved, second_saved, both_waited) = tokio::join!(
        first.save(&scope, &first_name, first_tree.path(), None),
        second.save(&scope, &second_name, second_tree.path(), None),
        async {
            let both = eventually(|| config_writes() == 2).await;
            storage.open_gate();
            both
        }
    );
    let mut names = listed_names(&first, &scope).await;
    names.sort();

    assert_eq!(
        (
            both_waited,
            first_saved.map(|_| ()).map_err(|error| error.to_string()),
            second_saved.map(|_| ()).map_err(|error| error.to_string()),
            names,
            restored_listing(&second, &scope, &name("p-first"))
                .await
                .unwrap(),
            restored_listing(&first, &scope, &name("p-second"))
                .await
                .unwrap(),
        ),
        (
            true,
            Ok(()),
            Ok(()),
            vec!["p-first".to_string(), "p-second".to_string()],
            listing(first_tree.path()),
            listing(second_tree.path()),
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_during_a_save_keeps_the_packs_of_the_save() {
    // The first index write after the arm waits at the gate. That is the index write of the
    // second save, so its packs are in no index while the delete prunes.
    let hold_next_index = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let hold_next_index = hold_next_index.clone();
        move |op_label, path| {
            if op_label == "write"
                && path.starts_with("index")
                && hold_next_index.swap(false, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (old_tree, new_tree) = (one_file_tree("old"), fixture_tree());
    store
        .save(&scope, &name("p-old"), old_tree.path(), None)
        .await
        .unwrap();
    hold_next_index.store(true, Ordering::SeqCst);
    let index_writes = || {
        storage
            .calls()
            .iter()
            .filter(|(op_label, path)| *op_label == "write" && path.starts_with("index"))
            .count()
    };
    let before = index_writes();

    let saving = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = new_tree.path().to_path_buf();
        async move { store.save(&scope, &name("p-new"), &path, None).await }
    });
    let held = eventually(|| index_writes() > before).await;
    let deleted = store.delete(&scope, &name("p-old")).await;
    let pruned_while_held = ledger(&storage, &scope).await.last_prune.is_some();
    storage.open_gate();
    let saved = saving.await.unwrap();
    let pruned_again = store.delete(&scope, &name("p-none")).await;

    assert_eq!(
        (
            held,
            deleted.is_ok(),
            pruned_while_held,
            saved.is_ok(),
            pruned_again.is_ok(),
            restored_listing(&store, &scope, &name("p-new")).await.ok(),
        ),
        (true, true, true, true, true, Some(listing(new_tree.path())))
    );
}

#[test]
async fn a_restore_whose_pack_reads_fail_gives_a_retryable_storage_error() {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst)
                && matches!(op_label, "read" | "read_range")
                && path.starts_with("data")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(
        restored
            .as_ref()
            .is_err_and(|error| is_storage(error, true)),
        "{restored:?}"
    );
}

#[cfg(unix)]
#[test]
async fn a_save_of_a_file_without_read_permission_gives_source_with_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: `geteuid` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert_ne!(
        uid, 0,
        "this test needs a user other than root, because permissions do not stop root from a read"
    );
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("readable");
    let locked = tree.path().join("locked.txt");
    std::fs::write(&locked, b"locked").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let saved = store
        .save(&scope, &name("p-locked"), tree.path(), None)
        .await;

    assert!(
        matches!(&saved, Err(SnapshotStoreError::Source(error)) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "{saved:?}"
    );
    assert_eq!(
        blobs(&*storage, &scope.0, "snapshots/").await,
        Vec::<String>::new()
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn a_restore_that_cannot_set_an_extended_attribute_gives_destination() {
    // A file on tmpfs takes a user attribute of 6,000 bytes. A file on ext4 with 4 KiB blocks
    // does not, so the restore cannot set it. The test checks nothing on a host where the source
    // does not take the attribute or where the destination takes it.
    let value = vec![b'a'; 6000];
    let set = |path: &Path| xattr_set(path, "user.golem-test", &value);
    let Ok(source) = tempfile::tempdir_in("/dev/shm") else {
        println!("SKIPPED: /dev/shm has no directory for the source of the test");
        return;
    };
    let file = source.path().join("file.txt");
    std::fs::write(&file, b"content").unwrap();
    let probe = Scratch::new();
    let probe_file = probe.path().join("probe");
    std::fs::write(&probe_file, b"probe").unwrap();
    if let Err(error) = set(&file) {
        println!("SKIPPED: /dev/shm does not take a user attribute of 6,000 bytes: {error}");
        return;
    }
    if set(&probe_file).is_ok() {
        println!(
            "SKIPPED: the destination {} takes a user attribute of 6,000 bytes",
            probe.path().display()
        );
        return;
    }
    let store = store(
        Arc::new(InMemoryBlobStorage::new()),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    store
        .save(&scope, &name("p-xattr"), source.path(), None)
        .await
        .unwrap();

    let restored = restored_listing(&store, &scope, &name("p-xattr")).await;

    assert!(
        matches!(&restored, Err(SnapshotStoreError::Destination(_))),
        "{restored:?}"
    );
}

#[cfg(target_os = "linux")]
fn xattr_set(path: &Path, name: &str, value: &[u8]) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let name = std::ffi::CString::new(name)?;
    // SAFETY: the path and the name are NUL-terminated strings, and the value lives for the call.
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[test]
async fn a_snapshot_file_that_fails_its_check_is_left_out_of_list_and_makes_an_unknown_name_corrupt()
 {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path(), None)
        .await
        .unwrap();
    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new(&format!("snapshots/{}", "ab".repeat(32))),
            b"not a snapshot",
        )
        .await
        .unwrap();

    let names = listed_names(&store, &scope).await;
    let kept = store.stat(&scope, &name("p-kept")).await;
    let unknown = store.stat(&scope, &name("p-unknown")).await;
    let restore_unknown = restored_listing(&store, &scope, &name("p-unknown")).await;

    assert!(
        matches!(unknown, Err(SnapshotStoreError::Corrupt(_))),
        "{unknown:?}"
    );
    assert!(
        matches!(restore_unknown, Err(SnapshotStoreError::Corrupt(_))),
        "{restore_unknown:?}"
    );
    assert_eq!(
        (names, kept.map(|info| info.is_some()).ok()),
        (vec!["p-kept".to_string()], Some(true))
    );
}

#[test]
async fn a_delete_past_the_threshold_prunes_and_the_packs_go_after_the_grace_period() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    let packs_before = blobs(&*storage, &scope.0, "data/").await;

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let after_first = ledger(&storage, &scope).await;
    let after_first_freed = freed(&storage, &scope).await;
    // The margin for clock skew keeps the next prune back, so the ledger moves back.
    age_ledger(&storage, &scope, &after_first).await;
    store.delete(&scope, &name("p-none")).await.unwrap();
    let packs_after = blobs(&*storage, &scope.0, "data/").await;

    assert_eq!(
        (
            after_first_freed,
            after_first.last_prune.is_some(),
            after_first.awaiting_removal,
            packs_after.len() < packs_before.len(),
            packs_after.iter().all(|pack| packs_before.contains(pack)),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (0, true, true, true, true, Some(listing(kept_tree.path())))
    );
}

/// Counts the listings of the packs among the recorded calls.
fn data_listings(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == "list_data")
        .count()
}

/// Makes the ledger one entry with no marked packs and a last prune at the time, and writes a
/// record of one freed byte.
async fn set_last_prune<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &SnapshotScope,
    last_prune: golem_common::model::Timestamp,
) {
    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-freed/1-test"),
            GONE_SNAPSHOT.as_bytes(),
        )
        .await
        .unwrap();
    storage
        .delete_dir("test", "test", scope.0.clone(), Path::new(LEDGERS_PATH))
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
async fn age_ledger<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &SnapshotScope,
    ledger: &PruneLedger,
) {
    let back = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap() + 2 * 3_600_000;
    let aged = ledger
        .last_prune
        .map_or(0, |last| last.to_millis().saturating_sub(back));
    storage
        .delete_dir("test", "test", scope.0.clone(), Path::new(LEDGERS_PATH))
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
async fn put_ledger_entry<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    scope: &SnapshotScope,
    name: &str,
) {
    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            &Path::new(LEDGERS_PATH).join(name),
            b"",
        )
        .await
        .unwrap();
}

#[test]
async fn a_delete_within_the_grace_period_does_not_list_the_packs() {
    // The ledger has no marked packs, so only the grace period keeps the first delete from a
    // listing. The second delete comes after the grace period and lists the packs one time.
    let grace = Duration::from_secs(3600);
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(storage.clone(), policy(LONG_DEADLINE, NEVER, grace));
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), one_file_tree("second"));
    store
        .save(&scope, &name("p-1"), first.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), second.path(), None)
        .await
        .unwrap();
    let now = golem_common::model::Timestamp::now_utc();
    set_last_prune(&storage, &scope, now).await;
    let before_first = storage.calls().len();

    store.delete(&scope, &name("p-1")).await.unwrap();
    let before_second = storage.calls().len();
    set_last_prune(
        &storage,
        &scope,
        golem_common::model::Timestamp::from(now.to_millis().saturating_sub(2 * 3_600_000)),
    )
    .await;
    store.delete(&scope, &name("p-2")).await.unwrap();
    let calls = storage.calls();

    assert_eq!(
        (
            data_listings(&calls[before_first..before_second]),
            data_listings(&calls[before_second..]),
        ),
        (0, 1)
    );
}

#[test]
async fn a_failed_listing_of_the_packs_gives_storage_and_records_no_prune() {
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "list_data" {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), one_file_tree("kept"));
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();

    let deleted = store.delete(&scope, &name("p-deleted")).await;
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert!(
        deleted.as_ref().is_err_and(|error| is_storage(error, true)),
        "{deleted:?}"
    );
    assert_eq!((after_freed > 0, after.last_prune), (true, None));
}

/// Counts the prunes among the recorded calls. A prune lists the packs, and no other step of a
/// delete makes that call.
fn prunes(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, path)| *op_label == "list" && path == "data")
        .count()
}

/// Saves a tree of one file with the content under each name.
async fn save_each(store: &RusticSnapshotStore, scope: &SnapshotScope, names: &[&str]) {
    futures::stream::iter(names)
        .for_each(|text| async move {
            let tree = one_file_tree(text);
            store
                .save(scope, &name(text), tree.path(), None)
                .await
                .unwrap();
        })
        .await;
}

#[test]
#[timeout("60s")]
async fn a_delete_that_paused_after_its_ledger_read_does_not_put_back_the_old_ledger() {
    // The gate holds the first delete after its record write and its ledger read. The second
    // delete prunes to its end. The first delete then goes on with the ledger that it read.
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let held = held.clone();
        move |op_label, _| {
            if op_label == "list_freed" && !held.swap(true, Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let paused = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let paused_held = eventually(|| held.load(Ordering::SeqCst)).await;

    let pruned = store.delete(&scope, &name("p-2")).await;
    let after_prune = ledger(&storage, &scope).await;
    storage.open_gate();
    let paused = tokio::time::timeout(LIMIT, paused).await;
    let after_all = ledger(&storage, &scope).await;

    assert!(pruned.is_ok(), "{pruned:?}");
    assert!(matches!(paused, Ok(Ok(Ok(())))), "{paused:?}");
    assert_eq!(
        (
            paused_held,
            prunes(&storage.calls()),
            after_prune.last_prune.is_some(),
            after_all,
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, 1, true, after_prune, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_record_that_a_delete_adds_during_a_prune_stays_for_the_next_prune() {
    // The gate holds the prune at its listing of the packs, and a record comes in meanwhile.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let prune_held = eventually(|| held.load(Ordering::SeqCst)).await;

    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-freed/7-late"),
            GONE_SNAPSHOT.as_bytes(),
        )
        .await
        .unwrap();
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            blobs(&*storage, &scope.0, "golem/prune-freed/").await,
            freed(&storage, &scope).await,
        ),
        (true, vec!["golem/prune-freed/7-late".to_string()], 7)
    );
}

#[test]
async fn a_late_older_ledger_entry_does_not_win() {
    // The newer entry is inside the grace period, so a delete does not prune.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let now = golem_common::model::Timestamp::now_utc().to_millis();
    let newer = now.saturating_sub(60_000);
    put_ledger_entry(&storage, &scope, &format!("{newer}-0-newer")).await;
    put_ledger_entry(
        &storage,
        &scope,
        &format!("{}-1-older", now.saturating_sub(2 * 3_600_000)),
    )
    .await;

    let read = ledger(&storage, &scope).await;
    store.delete(&scope, &name("p-1")).await.unwrap();

    assert_eq!(
        (
            read.last_prune.map(|last| last.to_millis()),
            read.awaiting_removal,
            prunes(&storage.calls()),
        ),
        (Some(newer), false, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_the_older_ledger_entries_and_keeps_a_newer_one() {
    // The gate holds the prune at its listing of the packs. An older and a newer entry come in
    // meanwhile.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let prune_held = eventually(|| held.load(Ordering::SeqCst)).await;

    let newer = format!(
        "{}-0-newer",
        golem_common::model::Timestamp::now_utc().to_millis() + 60_000
    );
    put_ledger_entry(&storage, &scope, "1000-0-older").await;
    put_ledger_entry(&storage, &scope, &newer).await;
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;
    let entries = blobs(&*storage, &scope.0, "golem/prune-ledgers/").await;

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            entries.len(),
            entries.contains(&format!("golem/prune-ledgers/{newer}")),
            entries.contains(&"golem/prune-ledgers/1000-0-older".to_string()),
        ),
        (true, 2, true, false)
    );
}

/// Gives the time of the newest marker of the claims of the scope.
async fn claim_time(storage: &ScriptedBlobStorage, scope: &SnapshotScope) -> Option<u64> {
    blobs(storage, &scope.0, "golem/prune-claims/")
        .await
        .iter()
        .filter_map(
            |path| match parse_claim_entry(Path::new(path).file_name()?.to_str()?)? {
                ClaimEntry::Marker(_, at) => Some(at.to_millis()),
                ClaimEntry::Claim(_) => None,
            },
        )
        .max()
}

/// Moves each marker of the claims of the scope back by two hours and the margin, so no marker
/// holds its ledger.
async fn age_claims<S: BlobStorage + 'static>(storage: &Arc<S>, scope: &SnapshotScope) {
    let stale = golem_common::model::Timestamp::now_utc()
        .to_millis()
        .saturating_sub(2 * 3_600_000 + 2 * 60_000);
    let markers = storage
        .list_blobs_below(
            "test",
            "test",
            scope.0.clone(),
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
                    .delete("test", "test", scope.0.clone(), &path)
                    .await
                    .unwrap();
                let aged = path
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(format!("{number}@{stale}-aged"));
                storage
                    .put_raw("test", "test", scope.0.clone(), &aged, b"")
                    .await
                    .unwrap();
            }
        })
        .await;
}

#[test]
#[timeout("60s")]
async fn a_prune_slower_than_the_grace_period_keeps_its_claim_fresh() {
    // The gate holds the prune at its listing of the packs for longer than the grace period.
    let grace = Duration::from_millis(400);
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let prune_held = eventually(|| held.load(Ordering::SeqCst)).await;
    let first = claim_time(&storage, &scope).await.unwrap_or(u64::MAX);
    let wanted = first.saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX));

    let refreshed = tokio::time::timeout(
        LIMIT,
        futures::stream::repeat(())
            .then(|()| async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                claim_time(&storage, &scope).await
            })
            .filter(|time| std::future::ready(time.is_some_and(|time| time >= wanted)))
            .boxed()
            .next(),
    )
    .await
    .is_ok();
    let second = store.delete(&scope, &name("p-2")).await;
    let prunes_while_held = prunes(&storage.calls());
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;

    assert!(second.is_ok(), "{second:?}");
    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            refreshed,
            prunes_while_held,
            prunes(&storage.calls()),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, true, 1, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_the_claims_of_old_ledgers() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    futures::stream::iter(["golem/prune-claims/100/0", "golem/prune-claims/200/4"])
        .for_each(|path| {
            let storage = storage.clone();
            let scope = scope.clone();
            async move {
                storage
                    .put_raw("test", "test", scope.0.clone(), Path::new(path), b"100")
                    .await
                    .unwrap();
            }
        })
        .await;

    store.delete(&scope, &name("p-1")).await.unwrap();

    assert_eq!(
        (
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_an_empty_claim_directory_of_an_old_ledger() {
    // A listing of the blobs does not find an empty directory, so only a listing of the
    // directories finds it.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    storage
        .create_dir(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-claims/100"),
        )
        .await
        .unwrap();
    let list_claims = || {
        storage.list_dir(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-claims"),
        )
    };
    let before = list_claims().await.unwrap();

    store.delete(&scope, &name("p-1")).await.unwrap();

    assert_eq!(
        (
            before,
            ledger(&storage, &scope).await.last_prune.is_some(),
            list_claims().await.unwrap(),
        ),
        (
            vec![std::path::PathBuf::from("golem/prune-claims/100")],
            true,
            Vec::<std::path::PathBuf>::new()
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_ledger_write_after_a_prune_keeps_the_claim_so_no_second_prune_runs() {
    // The first prune runs and its ledger write fails. A delete right after it finds the claim
    // and does not prune. When the claim is older than the grace period and the margin, the next
    // delete prunes.
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refused = refused.clone();
        move |op_label, _| {
            if op_label == "write_ledger" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2", "p-3"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    let second = store.delete(&scope, &name("p-2")).await;
    let prunes_after_second = prunes(&storage.calls());
    age_claims(&storage, &scope).await;
    let third = store.delete(&scope, &name("p-3")).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(second.is_ok(), "{second:?}");
    assert!(third.is_ok(), "{third:?}");
    assert_eq!(
        (
            claims_after_failure
                .iter()
                .filter(|path| !path.contains('@'))
                .count(),
            prunes_after_second,
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (1, 1, 2, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_backend_that_does_not_build_after_the_claim_releases_it() {
    // The first claim write makes the next backend build fail, and that build is the one of the
    // prune.
    let refuse_backends = Arc::new(AtomicBool::new(false));
    let armed = Arc::new(AtomicBool::new(true));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse_backends = refuse_backends.clone();
        move |op_label, _| {
            if op_label == "write_claim" && armed.swap(false, Ordering::SeqCst) {
                refuse_backends.store(true, Ordering::SeqCst);
            }
            Script::Pass
        }
    });
    let store = Arc::new(RusticSnapshotStore {
        refuse_backends: refuse_backends.clone(),
        ..RusticSnapshotStore::with_policy(
            storage.clone(),
            key(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        )
    });
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    refuse_backends.store(false, Ordering::SeqCst);
    let retried = store.delete(&scope, &name("p-2")).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(
        (
            claims_after_failure,
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (Vec::<String>::new(), true)
    );
}

#[test]
async fn a_forget_that_fails_after_the_record_write_leaves_the_record() {
    // The forget deletes the snapshot file, and the storage refuses that call.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "delete" && path.starts_with("snapshots") {
                Script::Refuse
            } else {
                Script::Pass
            }
        });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;

    let deleted = store.delete(&scope, &name("p-1")).await;

    assert!(
        deleted.as_ref().is_err_and(|error| is_storage(error, true)),
        "{deleted:?}"
    );
    assert_eq!(
        (
            freed(&storage, &scope).await > 0,
            listed_names(&store, &scope).await
        ),
        (true, vec!["p-1".to_string()])
    );
}

/// Tells whether the call is the forget of a delete: the delete of a snapshot file.
fn is_forget(op_label: &str, path: &Path) -> bool {
    op_label == "delete" && path.starts_with("snapshots")
}

/// Gives the path of each record of freed bytes of the scope.
async fn records(storage: &InMemoryBlobStorage, scope: &SnapshotScope) -> Vec<String> {
    blobs(storage, &scope.0, "golem/prune-freed/").await
}

#[test]
#[timeout("60s")]
async fn a_prune_keeps_the_record_of_a_delete_that_has_not_forgotten_its_snapshot() {
    // The first delete writes its record and waits at its forget. The second delete prunes and
    // must not count that record. After the forget, the next due prune counts it.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let plain = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&plain, &scope, &["p-1", "p-2"]).await;
    let held = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if is_forget(op_label, path) {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let pausing = store(
        held.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let paused = tokio::spawn({
        let scope = scope.clone();
        async move { pausing.delete(&scope, &name("p-1")).await }
    });
    let at_forget = eventually(|| {
        held.calls()
            .iter()
            .any(|(op_label, path)| is_forget(op_label, Path::new(path)))
    })
    .await;
    let paused_record = records(&inner, &scope).await;

    plain.delete(&scope, &name("p-2")).await.unwrap();
    let after_prune = records(&inner, &scope).await;
    let pruned = ledger(&inner, &scope).await;
    age_ledger(&inner, &scope, &pruned).await;
    held.open_gate();
    let resumed = tokio::time::timeout(LIMIT, paused).await;

    assert!(matches!(resumed, Ok(Ok(Ok(())))), "{resumed:?}");
    assert_eq!(
        (
            at_forget,
            paused_record.len(),
            after_prune == paused_record,
            pruned.last_prune.is_some(),
            records(&inner, &scope).await,
            ledger(&inner, &scope).await.last_prune > pruned.last_prune,
        ),
        (true, 1, true, true, Vec::<String>::new(), true)
    );
}

#[test]
#[timeout("60s")]
async fn a_forget_that_lands_while_a_prune_runs_keeps_its_record_for_the_next_prune() {
    // The prune of the second delete checks the records before the first delete forgets. The
    // forget lands while that prune runs, so the record was not settled at the check, and it must
    // stay for the next prune.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let plain = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&plain, &scope, &["p-1", "p-2", "p-3"]).await;
    let forgetting = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if is_forget(op_label, path) {
            Script::Step {
                refuse: false,
                late: false,
            }
        } else {
            Script::Pass
        }
    });
    let claimed = Arc::new(AtomicBool::new(false));
    let pruning = ScriptedBlobStorage::new(inner.clone(), {
        let claimed = claimed.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list" && path == Path::new("data") && claimed.load(Ordering::SeqCst) {
                Script::Step {
                    refuse: false,
                    late: false,
                }
            } else {
                Script::Pass
            }
        }
    });
    let first = tokio::spawn({
        let deleting = store(
            forgetting.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = scope.clone();
        async move { deleting.delete(&scope, &name("p-1")).await }
    });
    let first_at_forget = eventually(|| forgetting.waiting_steps() > 0).await;
    let first_record = records(&inner, &scope).await;
    let second = tokio::spawn({
        let deleting = store(
            pruning.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = scope.clone();
        async move { deleting.delete(&scope, &name("p-2")).await }
    });
    let second_at_prune = eventually(|| pruning.waiting_steps() > 0).await;

    forgetting.step();
    let first = tokio::time::timeout(LIMIT, first).await;
    pruning.step();
    let second = tokio::time::timeout(LIMIT, second).await;
    let after_prune = records(&inner, &scope).await;
    let pruned = ledger(&inner, &scope).await;
    age_ledger(&inner, &scope, &pruned).await;
    plain.delete(&scope, &name("p-3")).await.unwrap();

    assert!(matches!(first, Ok(Ok(Ok(())))), "{first:?}");
    assert!(matches!(second, Ok(Ok(Ok(())))), "{second:?}");
    assert_eq!(
        (
            first_at_forget,
            second_at_prune,
            first_record.len(),
            after_prune == first_record,
            pruned.last_prune.is_some(),
            records(&inner, &scope).await,
        ),
        (true, true, 1, true, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_record_whose_snapshot_still_exists_counts_nothing_and_does_not_make_a_prune_due() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    let snapshot = snapshot_files(inner.clone(), &scope)
        .await
        .first()
        .map(|snapshot| snapshot.id.to_hex().to_string())
        .unwrap_or_default();
    inner
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-freed/1000000000-kept"),
            snapshot.as_bytes(),
        )
        .await
        .unwrap();

    store.delete(&scope, &name("p-unknown")).await.unwrap();
    store.delete(&scope, &name("p-unknown")).await.unwrap();

    assert_eq!(
        (
            snapshot.len(),
            ledger(&inner, &scope).await.last_prune,
            records(&inner, &scope).await,
        ),
        (
            64,
            None,
            vec!["golem/prune-freed/1000000000-kept".to_string()]
        )
    );
}

#[test]
async fn a_record_write_that_answers_already_exists_counts_as_written() {
    // A new try of a record write whose first answer was lost finds the record of the first try.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "write_freed" {
            Script::AnswerAlreadyExists
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;

    let deleted = store.delete(&scope, &name("p-1")).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            freed(&storage, &scope).await > 0,
            listed_names(&store, &scope).await
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_sees_the_marker_of_a_claim_that_is_not_taken_yet_and_does_not_prune() {
    // The gate holds the second of the two writes of the first delete: its marker and its claim.
    // The second delete runs meanwhile.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let first = ScriptedBlobStorage::new(inner.clone(), {
        let writes = writes.clone();
        move |op_label, _| {
            if matches!(op_label, "write_marker" | "write_claim")
                && writes.fetch_add(1, Ordering::SeqCst) == 1
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let second = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let grace = Duration::from_secs(3600);
    let scope = new_scope();
    save_each(
        &store(inner.clone(), policy(LONG_DEADLINE, NEVER, grace)),
        &scope,
        &["p-1", "p-2"],
    )
    .await;
    let holding = tokio::spawn({
        let deleting = store(first.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
        let scope = scope.clone();
        async move { deleting.delete(&scope, &name("p-1")).await }
    });
    let held = eventually(|| writes.load(Ordering::SeqCst) >= 2).await;

    let seen = store(second.clone(), policy(LONG_DEADLINE, ALWAYS, grace))
        .delete(&scope, &name("p-2"))
        .await;
    first.open_gate();
    let holding = tokio::time::timeout(LIMIT, holding).await;

    assert!(seen.is_ok(), "{seen:?}");
    assert!(matches!(holding, Ok(Ok(Ok(())))), "{holding:?}");
    assert_eq!(
        (held, prunes(&second.calls()), prunes(&first.calls())),
        (true, 0, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_claim_without_a_marker_does_not_unblock_a_live_holder() {
    // The first delete holds claim 0 and waits at the start of its prune. A claim 1 without a
    // marker is in the same directory.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let claimed = Arc::new(AtomicBool::new(false));
    let first = ScriptedBlobStorage::new(inner.clone(), {
        let claimed = claimed.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list" && path == Path::new("data") && claimed.load(Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let second = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let grace = Duration::from_secs(3600);
    let scope = new_scope();
    save_each(
        &store(inner.clone(), policy(LONG_DEADLINE, NEVER, grace)),
        &scope,
        &["p-1", "p-2"],
    )
    .await;
    let holding = tokio::spawn({
        let deleting = store(first.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
        let scope = scope.clone();
        async move { deleting.delete(&scope, &name("p-1")).await }
    });
    let held = eventually(|| {
        first
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "list" && path == "data")
    })
    .await;
    inner
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-claims/none/1"),
            b"",
        )
        .await
        .unwrap();

    let seen = store(second.clone(), policy(LONG_DEADLINE, ALWAYS, grace))
        .delete(&scope, &name("p-2"))
        .await;
    first.open_gate();
    let holding = tokio::time::timeout(LIMIT, holding).await;

    assert!(seen.is_ok(), "{seen:?}");
    assert!(matches!(holding, Ok(Ok(Ok(())))), "{holding:?}");
    assert_eq!((held, prunes(&second.calls())), (true, 0));
}

#[test]
#[timeout("60s")]
async fn two_deletes_that_read_the_same_ledger_make_one_prune() {
    // The first read after the first claim is the start of the first prune. The gate holds it,
    // so the second delete reads the ledger that the first delete read.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, _| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "read"
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let first = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let first_held = eventually(|| held.load(Ordering::SeqCst)).await;

    let second = store.delete(&scope, &name("p-2")).await;
    storage.open_gate();
    let first = tokio::time::timeout(LIMIT, first).await;
    let calls = storage.calls();

    assert!(matches!(first, Ok(Ok(Ok(())))), "{first:?}");
    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        (
            first_held,
            prunes(&calls),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_that_claims_after_another_prune_removed_the_claims_does_not_prune() {
    // The gate holds the first delete after its ledger read and before its listing of the claims.
    // The second delete prunes to its end, so the first delete claims in a removed directory.
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let held = held.clone();
        move |op_label, _| {
            if op_label == "list_claims" && !held.swap(true, Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let late = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let late_held = eventually(|| held.load(Ordering::SeqCst)).await;

    let pruned = store.delete(&scope, &name("p-2")).await;
    storage.open_gate();
    let late = tokio::time::timeout(LIMIT, late).await;

    assert!(pruned.is_ok(), "{pruned:?}");
    assert!(matches!(late, Ok(Ok(Ok(())))), "{late:?}");
    assert_eq!(
        (
            late_held,
            prunes(&storage.calls()),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_second_read_of_the_ledger_deletes_the_claim_and_a_retry_of_the_delete_prunes() {
    // The first call after the first claim is the second read of the ledger, and it fails. So the
    // first delete does not prune, and only the retry counts as a prune.
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, refused) = (claimed.clone(), refused.clone());
        move |op_label, _| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
                Script::Pass
            } else if claimed.load(Ordering::SeqCst) && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    let retried = store.delete(&scope, &name("p-1")).await;
    let after = ledger(&storage, &scope).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(
        (
            claims_after_failure,
            after.last_prune.is_some(),
            prunes(&storage.calls())
        ),
        (Vec::<String>::new(), true, 1)
    );
}

/// A storage that refuses the first call after the first claim write, which is the second read of
/// the ledger. It holds each claim delete, and each ledger read after the claim when `hold_read` is
/// true, until the gate opens.
fn failing_after_the_claim(hold_read: bool) -> Arc<ScriptedBlobStorage> {
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
        if op_label == "write_claim" {
            claimed.store(true, Ordering::SeqCst);
            Script::Pass
        } else if op_label == "delete_claim"
            || (hold_read && op_label == "read_ledger" && claimed.load(Ordering::SeqCst))
        {
            Script::WaitForGate
        } else if !hold_read
            && claimed.load(Ordering::SeqCst)
            && !refused.swap(true, Ordering::SeqCst)
        {
            Script::Refuse
        } else {
            Script::Pass
        }
    })
}

#[test]
#[timeout("60s")]
async fn a_delete_dropped_after_a_failed_second_read_still_releases_its_claim() {
    // The second read of the ledger fails, and the gate holds the delete of the claim. The test
    // drops the delete there, and then opens the gate.
    let storage = failing_after_the_claim(false);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "delete_claim")
    })
    .await;

    deleting.abort();
    let dropped = deleting.await;
    let claims_at_drop = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    storage.open_gate();
    let ended = eventually(|| store.work_in_flight() == 0).await;

    assert!(
        dropped.as_ref().is_err_and(|error| error.is_cancelled()),
        "{dropped:?}"
    );
    assert_eq!(
        (
            held,
            claims_at_drop.is_empty(),
            ended,
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, false, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_shut_down_after_the_claim_still_releases_it() {
    // The gate holds the second read of the ledger, and the shut down cancels that read. The
    // gate stays shut for the read, and it opens only for the delete of the claim.
    let storage = failing_after_the_claim(true);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &name("p-1")).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .skip_while(|(op_label, _)| *op_label != "write_claim")
            .any(|(op_label, _)| *op_label == "read_ledger")
    })
    .await;

    let shutting_down = tokio::spawn({
        let store = store.clone();
        async move { store.shut_down().await }
    });
    let reached_release = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "delete_claim")
    })
    .await;
    let claims_at_release = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    storage.open_gate();
    let shut_down = tokio::time::timeout(LIMIT, shutting_down).await;
    let deleted = tokio::time::timeout(LIMIT, deleting).await;

    assert!(matches!(shut_down, Ok(Ok(()))), "{shut_down:?}");
    assert!(matches!(deleted, Ok(Ok(Err(_)))), "{deleted:?}");
    assert_eq!(
        (
            held,
            reached_release,
            claims_at_release.is_empty(),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (true, true, false, Vec::<String>::new())
    );
}

/// A storage that refuses the first call after the first claim write, which is the second read of
/// the ledger, and each call of the release with the operation label.
fn refusing_the_release_call(release_label: &'static str) -> Arc<ScriptedBlobStorage> {
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
        if op_label == "write_claim" {
            claimed.store(true, Ordering::SeqCst);
            Script::Pass
        } else if op_label == release_label
            || (claimed.load(Ordering::SeqCst) && !refused.swap(true, Ordering::SeqCst))
        {
            Script::Refuse
        } else {
            Script::Pass
        }
    })
}

#[test]
#[timeout("60s")]
async fn a_release_whose_claim_delete_is_refused_still_deletes_its_marker_and_the_next_delete_prunes()
 {
    let storage = refusing_the_release_call("delete_claim");
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    let retried = store.delete(&scope, &name("p-1")).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(
        (
            claims_after_failure,
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (vec!["golem/prune-claims/none/0".to_string()], 1, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_release_whose_marker_delete_is_refused_still_deletes_the_claim() {
    let storage = refusing_the_release_call("delete_marker");
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert_eq!(
        claims_after_failure
            .iter()
            .map(|path| path.starts_with("golem/prune-claims/none/0@"))
            .collect::<Vec<_>>(),
        vec![true]
    );
}

/// A storage that gives no blob to the first `vanishing` reads of a snapshot file after the first
/// claim write, as a forget of another delete between the listing and the read does. It counts
/// those reads.
fn vanishing_snapshots_after_the_claim(
    vanishing: usize,
) -> (Arc<ScriptedBlobStorage>, Arc<AtomicUsize>) {
    let claimed = Arc::new(AtomicBool::new(false));
    let vanished = Arc::new(AtomicUsize::new(0));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let vanished = vanished.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            let snapshot_read = claimed.load(Ordering::SeqCst)
                && path.starts_with("snapshots")
                && !is_forget(op_label, path)
                && op_label != "list";
            if snapshot_read
                && vanished
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                        (count < vanishing).then_some(count + 1)
                    })
                    .is_ok()
            {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    (storage, vanished)
}

#[test]
#[timeout("60s")]
async fn a_prune_plans_again_when_a_snapshot_file_is_gone_at_its_read_and_prunes_once() {
    let (storage, vanished) = vanishing_snapshots_after_the_claim(1);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &name("p-1")).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            vanished.load(Ordering::SeqCst),
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (1, 1, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_finds_a_snapshot_file_gone_at_each_attempt_releases_its_claim_and_gives_a_retryable_error()
 {
    let (storage, vanished) = vanishing_snapshots_after_the_claim(usize::MAX);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert_eq!(
        (
            vanished.load(Ordering::SeqCst),
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
        ),
        (3, 0, false, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_fails_keeps_its_claim_so_no_second_prune_runs_within_the_hold() {
    // The first listing of the packs by a prune after the first claim fails. Only a prune lists
    // the packs with that call, so the second read of the ledger passes and the prune fails after
    // it started. Its claim stays, so a retry within the hold does not prune, and a retry after it
    // does.
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, refused) = (claimed.clone(), refused.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !refused.swap(true, Ordering::SeqCst)
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &name("p-1")).await;
    let claims_after_failure = blobs(&*storage, &scope.0, "golem/prune-claims/").await;
    let retried = store.delete(&scope, &name("p-1")).await;
    let after_retry = ledger(&storage, &scope).await;
    age_claims(&storage, &scope).await;
    let later = store.delete(&scope, &name("p-1")).await;
    let after = ledger(&storage, &scope).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert!(retried.is_ok(), "{retried:?}");
    assert!(later.is_ok(), "{later:?}");
    assert_eq!(
        (
            claims_after_failure
                .iter()
                .filter(|path| !path.contains('@'))
                .count(),
            refused.load(Ordering::SeqCst),
            after_retry.last_prune,
            after.last_prune.is_some()
        ),
        (1, true, None, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_claim_older_than_the_grace_period_does_not_block_a_prune() {
    let grace = Duration::from_secs(3600);
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let old = golem_common::model::Timestamp::now_utc()
        .to_millis()
        .saturating_sub(2 * 3_600_000);
    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new("golem/prune-claims/none/0"),
            old.to_string().as_bytes(),
        )
        .await
        .unwrap();

    store.delete(&scope, &name("p-1")).await.unwrap();

    assert!(ledger(&storage, &scope).await.last_prune.is_some());
}

#[test]
#[timeout("60s")]
async fn a_prune_that_succeeds_deletes_the_claims_and_the_counted_records_of_freed_bytes() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    store.delete(&scope, &name("p-1")).await.unwrap();

    assert_eq!(
        (
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, &scope.0, "golem/prune-claims/").await,
            blobs(&*storage, &scope.0, "golem/prune-freed/").await,
        ),
        (true, Vec::<String>::new(), Vec::<String>::new())
    );
}

#[test]
async fn a_delete_below_the_threshold_does_not_prune() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), one_file_tree("kept"));
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    let packs_before = blobs(&*storage, &scope.0, "data/").await;

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_freed > 0,
            after.last_prune,
            blobs(&*storage, &scope.0, "data/").await,
        ),
        (true, None, packs_before)
    );
}

#[test]
async fn no_second_prune_runs_within_the_grace_period() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    let trees = [one_file_tree("a"), one_file_tree("b"), one_file_tree("c")];
    futures::stream::iter(["p-a", "p-b", "p-c"].into_iter().zip(&trees))
        .for_each(|(text, tree)| {
            let store = store.clone();
            let scope = scope.clone();
            async move {
                store
                    .save(&scope, &name(text), tree.path(), None)
                    .await
                    .unwrap();
            }
        })
        .await;

    store.delete(&scope, &name("p-a")).await.unwrap();
    let after_first = ledger(&storage, &scope).await;
    store.delete(&scope, &name("p-b")).await.unwrap();
    let after_second = ledger(&storage, &scope).await;
    let after_second_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_first.last_prune.is_some(),
            after_second.last_prune == after_first.last_prune,
            after_second_freed > 0,
        ),
        (true, true, true)
    );
}

#[test]
async fn a_delete_whose_prune_fails_gives_storage_and_a_retry_prunes() {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst)
                && matches!(op_label, "read" | "read_range")
                && path.starts_with("data")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let failed = store.delete(&scope, &name("p-deleted")).await;
    let after_failure = ledger(&storage, &scope).await;
    let after_failure_freed = freed(&storage, &scope).await;
    refuse.store(false, Ordering::SeqCst);
    // The prune started, so its claim stays until the hold passed.
    age_claims(&storage, &scope).await;
    let retried = store.delete(&scope, &name("p-deleted")).await;
    let after_retry = ledger(&storage, &scope).await;
    let after_retry_freed = freed(&storage, &scope).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert_eq!(
        (
            after_failure_freed > 0,
            after_failure.last_prune,
            retried.is_ok(),
            after_retry_freed,
            after_retry.last_prune.is_some(),
        ),
        (true, None, true, 0, true)
    );
}

#[test]
async fn a_deleted_scope_holds_no_blob() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), one_file_tree("second"));
    store
        .save(&scope, &name("p-1"), first.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), second.path(), None)
        .await
        .unwrap();
    store.delete(&scope, &name("p-1")).await.unwrap();
    let before = blobs(&*storage, &scope.0, "").await;

    store.delete_scope(&scope).await.unwrap();

    assert_eq!(
        (
            before
                .iter()
                .any(|path| path.starts_with("golem/prune-ledgers/")),
            blobs(&*storage, &scope.0, "").await
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_save_dropped_at_any_storage_call_publishes_nothing_and_leaves_the_name_free() {
    // The first save counts the calls of a save. Each later round holds one of these calls: the
    // call reaches the storage and never answers, as a write that S3 received and completes after
    // the caller left. The round drops the save there, and waits until the store has no work.
    let tree = one_file_tree("dropped");
    let other = one_file_tree("saved later");
    let counted =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    store(
        counted.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    )
    .save(&new_scope(), &name("p-dropped"), tree.path(), None)
    .await
    .unwrap();
    let calls = counted.calls().len();

    let rounds = futures::stream::iter(1..=calls)
        .then(|held| {
            let (tree, other) = (tree.path().to_path_buf(), other.path().to_path_buf());
            async move {
                let inner = Arc::new(InMemoryBlobStorage::new());
                let seen = Arc::new(AtomicUsize::new(0));
                let storage = ScriptedBlobStorage::new(inner.clone(), move |_, _| {
                    if seen.fetch_add(1, Ordering::SeqCst) + 1 == held {
                        Script::NeverAnswer
                    } else {
                        Script::Pass
                    }
                });
                let policy = policy(LONG_DEADLINE, NEVER, Duration::ZERO);
                let dropping = store(storage.clone(), policy);
                let scope = new_scope();
                let ended = drop_when(
                    &storage,
                    |calls| calls.len() >= held,
                    dropping.save(&scope, &name("p-dropped"), &tree, None),
                )
                .await;
                let stopped = tokio::time::timeout(LIMIT, dropping.shut_down())
                    .await
                    .is_ok();
                let later = store(inner, policy);
                let stat = later.stat(&scope, &name("p-dropped")).await.ok().flatten();
                let names = listed_names(&later, &scope).await;
                let saved_again = later.save(&scope, &name("p-dropped"), &other, None).await;
                let restored = restored_listing(&later, &scope, &name("p-dropped"))
                    .await
                    .ok();
                (
                    held,
                    ended.is_none(),
                    stopped,
                    stat,
                    names,
                    saved_again.is_ok(),
                    restored == Some(listing(&other)),
                )
            }
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        rounds,
        (1..=calls)
            .map(|held| (held, true, true, None, Vec::new(), true, true))
            .collect::<Vec<(
                usize,
                bool,
                bool,
                Option<SnapshotInfo>,
                Vec<String>,
                bool,
                bool
            )>>()
    );
}

#[test]
async fn a_blob_call_of_a_cancelled_operation_does_not_start() {
    // The in-memory storage answers at the first poll, so only the check before the call keeps
    // the call from the storage.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let files = SnapshotFiles {
        storage: storage.clone(),
        namespace: new_scope().0,
        deadline: LONG_DEADLINE,
        cancel,
        tracker: tokio_util::task::TaskTracker::new(),
    };

    let read = files
        .get("read_ledger", Path::new("golem/prune-ledger"))
        .await;

    assert!(read.is_err(), "{read:?}");
    assert_eq!(storage.calls(), Vec::new());
}

#[test]
#[timeout("60s")]
async fn shut_down_waits_for_a_blob_call_of_the_store_that_is_not_polled() {
    // The test polls the scope delete one time, so its first blob call waits at the gate, and
    // then the test does not poll it again. Only the tracker makes the shut down wait for it.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "delete_scope" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let deleting = store.delete_scope(&scope);
    tokio::pin!(deleting);
    let pending = futures::poll!(&mut deleting).is_pending();

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    let deleted = tokio::time::timeout(LIMIT, &mut deleting).await;
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();
    storage.open_gate();

    assert!(
        matches!(&deleted, Ok(Err(error)) if is_storage(error, true)),
        "{deleted:?}"
    );
    assert_eq!(
        (pending, waited, stopped, store.work_in_flight()),
        (true, true, true, 0)
    );
}

#[test]
#[timeout("60s")]
async fn shut_down_waits_for_the_step_of_a_prune_and_its_refresh_that_is_not_polled() {
    // The test drives the delete until its prune waits at the gate and its refresh runs, and then
    // does not poll it. Only the tracker of the step makes the shut down wait for it.
    let grace = Duration::from_millis(400);
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleted_name = name("p-1");
    let deleting = store.delete(&scope, &deleted_name);
    tokio::pin!(deleting);
    let reached = tokio::select! {
        biased;
        () = async {
            eventually(|| held.load(Ordering::SeqCst)).await;
        } => true,
        _ = &mut deleting => false,
    };

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    let deleted = tokio::time::timeout(LIMIT, &mut deleting).await;
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();
    let refreshes = |calls: &[(&'static str, String)]| {
        calls
            .iter()
            .filter(|(op_label, _)| *op_label == "refresh_claim")
            .count()
    };
    let at_stop = refreshes(&storage.calls());
    tokio::time::sleep(grace).await;
    storage.open_gate();

    assert!(
        matches!(&deleted, Ok(Err(error)) if is_storage(error, true)),
        "{deleted:?}"
    );
    assert_eq!(
        (
            reached,
            waited,
            stopped,
            store.work_in_flight(),
            refreshes(&storage.calls()) - at_stop,
        ),
        (true, true, true, 0, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_fails_when_a_prune_removed_an_index_file_that_it_listed() {
    // The gate holds the read of the listed index file, and the test deletes that file meanwhile,
    // as a prune does.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if op_label == "copy_read" && path.starts_with("index") {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let (from, to) = (new_scope(), new_scope());
    save_each(&store, &from, &["p-1"]).await;
    let copying = tokio::spawn({
        let store = store.clone();
        let (from, to) = (from.clone(), to.clone());
        async move { store.copy_scope(&from, &to).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "copy_read" && path.starts_with("index"))
    })
    .await;
    let index_files = blobs(&*inner, &from.0, "index/").await;

    futures::stream::iter(&index_files)
        .for_each(|path| {
            let (inner, from) = (&inner, &from);
            async move {
                inner
                    .delete("test", "test", from.0.clone(), Path::new(path))
                    .await
                    .unwrap();
            }
        })
        .await;
    storage.open_gate();
    let copied = tokio::time::timeout(LIMIT, copying).await;

    assert!(
        matches!(&copied, Ok(Ok(Err(error))) if is_storage(error, true)),
        "{copied:?}"
    );
    assert_eq!(
        (
            held,
            index_files.is_empty(),
            blobs(&*inner, &to.0, "config").await
        ),
        (true, false, Vec::<String>::new())
    );
}

#[test]
async fn a_copy_leaves_out_a_snapshot_file_that_a_delete_removed_after_the_listing() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "copy_read" && path.starts_with("snapshots") {
                Script::Vanish
            } else {
                Script::Pass
            }
        });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let (from, to) = (new_scope(), new_scope());
    save_each(&store, &from, &["p-1"]).await;

    let copied = store.copy_scope(&from, &to).await;

    assert!(copied.is_ok(), "{copied:?}");
    assert_eq!(
        (
            blobs(&*storage, &to.0, "config").await.len(),
            blobs(&*storage, &to.0, "snapshots/").await
        ),
        (1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_held_at_a_storage_call_stops_at_shut_down_and_makes_no_later_call() {
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "copy_list" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let (from, to) = (new_scope(), new_scope());
    let tree = one_file_tree("copied");
    store
        .save(&from, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    let mut copying = tokio::spawn({
        let store = store.clone();
        let (from, to) = (from.clone(), to.clone());
        async move { store.copy_scope(&from, &to).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "copy_list")
    })
    .await;

    // The gate stays closed until the end, so only the cancel can end the held call.
    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let calls_at_stop = storage.calls().len();
    let copied = tokio::time::timeout(LIMIT, &mut copying).await;
    let calls_after_copy = storage.calls().len();
    storage.open_gate();

    assert!(
        matches!(&copied, Ok(Ok(Err(error))) if is_storage(error, true)),
        "{copied:?}"
    );
    assert_eq!(
        (held, stopped, calls_after_copy),
        (true, true, calls_at_stop)
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_held_at_its_storage_call_keeps_shut_down_waiting_until_it_ends() {
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "publish" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("published");
    let saving = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = tree.path().to_path_buf();
        async move { store.save(&scope, &name("p-held"), &path, None).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "publish")
    })
    .await;

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    storage.open_gate();
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();
    let saved = tokio::time::timeout(LIMIT, saving).await;

    assert!(matches!(&saved, Ok(Ok(Ok(_)))), "{saved:?}");
    assert_eq!((held, waited, stopped), (true, true, true));
}

#[test]
#[timeout("60s")]
async fn a_save_that_reaches_its_publish_after_shut_down_publishes_nothing() {
    // The gate holds the save after its blocking work, so the tracker is empty and `shut_down`
    // returns before the publish starts.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let gate = Arc::new(PublishGate::default());
    let store = Arc::new(RusticSnapshotStore {
        publish_gate: Some(gate.clone()),
        ..RusticSnapshotStore::with_policy(
            storage.clone(),
            key(),
            policy(LONG_DEADLINE, NEVER, Duration::ZERO),
        )
    });
    let scope = new_scope();
    let tree = one_file_tree("late");
    let saving = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = tree.path().to_path_buf();
        async move { store.save(&scope, &name("p-late"), &path, None).await }
    });
    let reached = tokio::time::timeout(LIMIT, gate.reached.notified())
        .await
        .is_ok();

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    gate.open.notify_one();
    let saved = tokio::time::timeout(LIMIT, saving).await;

    assert!(
        matches!(&saved, Ok(Ok(Err(error))) if is_storage(error, false)),
        "{saved:?}"
    );
    assert_eq!(
        (
            reached,
            stopped,
            blobs(&*storage, &scope.0, "snapshots/").await,
            store.work_in_flight(),
        ),
        (true, true, Vec::<String>::new(), 0)
    );
}

#[test]
async fn delete_scope_and_copy_scope_after_shut_down_give_storage() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let (scope, other) = (new_scope(), new_scope());
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    store.shut_down().await;

    let deleted = store.delete_scope(&scope).await;
    let copied = store.copy_scope(&scope, &other).await;

    assert!(
        deleted
            .as_ref()
            .is_err_and(|error| is_storage(error, false)),
        "{deleted:?}"
    );
    assert!(
        copied.as_ref().is_err_and(|error| is_storage(error, false)),
        "{copied:?}"
    );
}

#[test]
#[timeout("60s")]
async fn shut_down_ends_running_operations_before_it_returns() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path.starts_with("data") {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let saving = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = tree.path().to_path_buf();
        async move { store.save(&scope, &name("p-held"), &path, None).await }
    });
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "write" && path.starts_with("data"))
    })
    .await;

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let saved = tokio::time::timeout(LIMIT, saving).await;
    let later = store.stat(&scope, &name("p-held")).await;
    storage.open_gate();

    assert!(
        matches!(&saved, Ok(Ok(Err(error))) if is_storage(error, true)),
        "{saved:?}"
    );
    assert!(
        later.as_ref().is_err_and(|error| is_storage(error, false)),
        "{later:?}"
    );
    assert_eq!((held, stopped, store.work_in_flight()), (true, true, 0));
}

/// The operation of the store that a test drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dropped {
    Restore,
    Stat,
    List,
    Delete,
}

#[test]
#[timeout("60s")]
async fn a_dropped_operation_stops_its_blocking_work() {
    let outcomes = futures::stream::iter([
        Dropped::Restore,
        Dropped::Stat,
        Dropped::List,
        Dropped::Delete,
    ])
    .then(|dropped| async move {
        let held_call = move |op_label: &str, path: &Path| match dropped {
            Dropped::Restore => op_label == "read_range" && path.starts_with("data"),
            Dropped::Stat | Dropped::List => op_label == "read" && path.starts_with("snapshots"),
            Dropped::Delete => op_label == "delete" && path.starts_with("snapshots"),
        };
        let hold = Arc::new(AtomicBool::new(false));
        let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
            let hold = hold.clone();
            move |op_label, path| {
                if held_call(op_label, path) && hold.load(Ordering::SeqCst) {
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
        let scope = new_scope();
        let tree = fixture_tree();
        store
            .save(&scope, &name("p-1"), tree.path(), None)
            .await
            .unwrap();
        hold.store(true, Ordering::SeqCst);
        let before = storage.calls().len();
        let reached = |calls: &[(&'static str, String)]| {
            calls[before..]
                .iter()
                .any(|(op_label, path)| held_call(op_label, Path::new(path)))
        };
        let into = Scratch::new();
        let ended = match dropped {
            Dropped::Restore => {
                drop_when(
                    &storage,
                    reached,
                    store.restore(&scope, &name("p-1"), into.path()).map(|_| ()),
                )
                .await
            }
            Dropped::Stat => {
                drop_when(
                    &storage,
                    reached,
                    store.stat(&scope, &name("p-1")).map(|_| ()),
                )
                .await
            }
            Dropped::List => drop_when(&storage, reached, store.list(&scope).map(|_| ())).await,
            Dropped::Delete => {
                drop_when(
                    &storage,
                    reached,
                    store.delete(&scope, &name("p-1")).map(|_| ()),
                )
                .await
            }
        };
        let held = reached(&storage.calls());
        let stopped = eventually(|| store.work_in_flight() == 0).await;
        storage.open_gate();
        (dropped, held, ended.is_none(), stopped)
    })
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        outcomes,
        [
            Dropped::Restore,
            Dropped::Stat,
            Dropped::List,
            Dropped::Delete
        ]
        .map(|dropped| (dropped, true, true, true))
    );
}

/// Gives the snapshot files of the scope that the bridge reads, on a blocking thread.
async fn snapshot_files(
    storage: Arc<dyn BlobStorage>,
    scope: &SnapshotScope,
) -> Vec<rustic_core::repofile::SnapshotFile> {
    let namespace = scope.0.clone();
    tokio::task::spawn_blocking(move || {
        let backend = super::super::backend::BlobBackend::new(
            storage,
            namespace,
            tokio::runtime::Handle::current(),
            LONG_DEADLINE,
        );
        let repository = open_existing(Arc::new(backend), &key()).unwrap().unwrap();
        scope_snapshots(&repository).unwrap().readable
    })
    .await
    .unwrap()
}

/// Gives, for the snapshot with the name, the numbers of new, changed and unmodified files that its
/// save counted, and the id of its parent.
fn read_counts(
    files: &[rustic_core::repofile::SnapshotFile],
    name: &str,
) -> Option<((u64, u64, u64), Option<SnapshotId>)> {
    let snapshot = files.iter().find(|snapshot| snapshot.label == name)?;
    let summary = snapshot.summary.as_ref()?;
    Some((
        (
            summary.files_new,
            summary.files_changed,
            summary.files_unmodified,
        ),
        snapshot.parent,
    ))
}

fn id_of(files: &[rustic_core::repofile::SnapshotFile], name: &str) -> Option<SnapshotId> {
    files
        .iter()
        .find(|snapshot| snapshot.label == name)
        .map(|snapshot| snapshot.id)
}

#[test]
async fn a_size_and_mtime_save_of_a_copied_tree_reads_no_unchanged_file() {
    // A copy gives each file a new inode and a new change time, and keeps its size and its
    // modification time, as a capture does.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();
    let copy = Scratch::new();
    wait_past_change_times(&entries(tree.path()));
    copy_flat_tree(tree.path(), copy.path());

    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-2"),
            copy.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
        )
        .await
        .unwrap();
    let files = snapshot_files(storage, &scope).await;

    assert_eq!(
        read_counts(&files, "p-2"),
        Some(((0, 0, 3), id_of(&files, "p-1")))
    );
}

#[test]
async fn a_full_save_reads_each_file() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::Full)),
        )
        .await
        .unwrap();
    let files = snapshot_files(storage, &scope).await;

    assert_eq!(read_counts(&files, "p-2"), Some(((3, 0, 0), None)));
}

#[test]
async fn the_parent_of_a_save_is_the_named_snapshot_also_when_a_newer_snapshot_exists() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-3"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
        )
        .await
        .unwrap();
    let files = snapshot_files(storage, &scope).await;

    assert_eq!(
        (
            read_counts(&files, "p-3"),
            id_of(&files, "p-1") == id_of(&files, "p-2")
        ),
        (Some(((0, 0, 3), id_of(&files, "p-1"))), false)
    );
}

#[test]
async fn a_save_without_a_parent_or_with_a_parent_that_the_scope_does_not_hold_reads_each_file() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), tree.path(), None)
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-3"),
            tree.path(),
            Some((&name("p-missing"), ChangeDetection::SizeMtime)),
        )
        .await
        .unwrap();
    let files = snapshot_files(storage, &scope).await;

    assert_eq!(
        (read_counts(&files, "p-2"), read_counts(&files, "p-3")),
        (Some(((3, 0, 0), None)), Some(((3, 0, 0), None)))
    );
}

#[test]
async fn a_save_whose_read_of_the_snapshot_files_fails_while_it_finds_the_parent_gives_storage_and_publishes_nothing()
 {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst) && op_label == "read" && path.starts_with("snapshots")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("parent");
    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);
    let before = storage.calls().len();

    let saved = store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
        )
        .await;
    let publishes = storage.calls()[before..]
        .iter()
        .filter(|(op_label, _)| *op_label == "publish")
        .count();
    refuse.store(false, Ordering::SeqCst);

    assert!(
        saved.as_ref().is_err_and(|error| is_storage(error, true)),
        "{saved:?}"
    );
    assert_eq!(
        (publishes, listed_names(&store, &scope).await),
        (0, vec!["p-1".to_string()])
    );
}

#[test]
async fn a_failed_read_of_a_snapshot_file_fails_stat_and_list_with_a_retryable_storage_error() {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst) && op_label == "read" && path.starts_with("snapshots")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path(), None)
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let stat = store.stat(&scope, &name("p-kept")).await;
    let list = store.list(&scope).await;

    assert!(
        stat.as_ref().is_err_and(|error| is_storage(error, true)),
        "{stat:?}"
    );
    assert!(
        list.as_ref().is_err_and(|error| is_storage(error, true)),
        "{list:?}"
    );
}

#[test]
async fn a_snapshot_file_that_is_gone_after_the_listing_is_left_out() {
    let gone = format!("snapshots/{}", "cd".repeat(32));
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let gone = gone.clone();
        move |op_label, path| {
            if op_label == "read" && path == Path::new(&gone) {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path(), None)
        .await
        .unwrap();
    inner
        .put_raw("test", "test", scope.0.clone(), Path::new(&gone), b"listed")
        .await
        .unwrap();

    let unknown = store.stat(&scope, &name("p-unknown")).await;
    let names = listed_names(&store, &scope).await;

    assert!(matches!(unknown, Ok(None)), "{unknown:?}");
    assert_eq!(names, vec!["p-kept".to_string()]);
}

#[test]
async fn a_delete_that_frees_nothing_writes_no_ledger() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path(), None)
        .await
        .unwrap();

    store.delete(&scope, &name("p-unknown")).await.unwrap();

    assert_eq!(
        blobs(&*storage, &scope.0, "golem/").await,
        Vec::<String>::new()
    );
}

#[test]
fn a_prune_that_marks_only_a_pack_that_no_index_lists_leaves_marked_packs() {
    assert!(leaves_marked_packs(&PruneReport {
        packs_used: 3,
        packs_unindexed: 1,
        ..PruneReport::default()
    }));
}

#[test]
async fn a_due_prune_that_marks_a_pack_that_no_index_lists_records_the_marked_pack() {
    // The pack of the kept snapshot stays in use, so the pack that no index lists is the only pack
    // that the prune marks. The ledger holds freed bytes from an earlier delete, so a delete of an
    // unknown name prunes.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path(), None)
        .await
        .unwrap();
    let unindexed = format!("data/ab/{}", "ab".repeat(32));
    storage
        .put_raw(
            "test",
            "test",
            scope.0.clone(),
            Path::new(&unindexed),
            b"a pack that no index lists",
        )
        .await
        .unwrap();
    set_last_prune(&storage, &scope, golem_common::model::Timestamp::from(0)).await;

    store.delete(&scope, &name("p-unknown")).await.unwrap();
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_freed,
            after.last_prune.is_some_and(|last| last.to_millis() > 0),
            after.awaiting_removal,
            blobs(&*storage, &scope.0, "data/")
                .await
                .contains(&unindexed),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (0, true, true, true, Some(listing(tree.path())))
    );
}

#[test]
fn a_prune_leaves_marked_packs_when_it_marks_repacks_or_keeps_marked_packs() {
    let report = |packs_unused, packs_repacked, marked_packs_kept| PruneReport {
        packs_unused,
        packs_repacked,
        marked_packs_kept,
        ..PruneReport::default()
    };

    assert_eq!(
        [
            leaves_marked_packs(&report(0, 0, 0)),
            leaves_marked_packs(&report(1, 0, 0)),
            leaves_marked_packs(&report(0, 1, 0)),
            leaves_marked_packs(&report(0, 0, 1)),
            leaves_marked_packs(&PruneReport {
                packs_used: 3,
                marked_packs_deleted: 2,
                ..PruneReport::default()
            }),
        ],
        [false, true, true, true, false]
    );
}

#[test]
async fn a_save_of_a_relative_directory_path_gives_source_and_writes_nothing() {
    // Cargo runs the tests in the directory of the crate, so the path names a directory.
    let relative = Path::new("src/filesystem_snapshot/contract_tests");
    assert!(
        relative.is_dir(),
        "the test runs in the directory of the crate"
    );
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();

    let saved = store
        .save(&scope, &name("p-relative"), relative, None)
        .await;

    assert!(
        matches!(saved, Err(SnapshotStoreError::Source(_))),
        "{saved:?}"
    );
    assert_eq!(blobs(&*storage, &scope.0, "").await, Vec::<String>::new());
}

#[test]
async fn a_save_of_a_regular_file_gives_source_and_writes_nothing() {
    // The store refuses the tree before it makes a repository, so the scope stays unused.
    let tree = one_file_tree("a file, not a tree");
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();

    let saved = store
        .save(&scope, &name("p-file"), &tree.path().join("file.txt"), None)
        .await;

    assert!(
        matches!(saved, Err(SnapshotStoreError::Source(_))),
        "{saved:?}"
    );
    assert_eq!(blobs(&*storage, &scope.0, "").await, Vec::<String>::new());
}

#[test]
async fn the_ledger_counts_the_packed_bytes_that_the_deleted_snapshot_added() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(&scope, &name("p-deleted"), tree.path(), None)
        .await
        .unwrap();
    let added = snapshot_files(storage.clone(), &scope)
        .await
        .iter()
        .filter_map(|snapshot| snapshot.summary.as_ref())
        .map(|summary| summary.data_added_packed)
        .sum::<u64>();

    store.delete(&scope, &name("p-deleted")).await.unwrap();

    assert_eq!((added > 1, freed(&storage, &scope).await), (true, added));
}

#[test]
async fn a_config_write_that_fails_gives_a_storage_error_with_that_failure() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path == Path::new("config") {
                Script::Refuse
            } else {
                Script::Pass
            }
        });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("never saved");

    let saved = store.save(&scope, &name("p-1"), tree.path(), None).await;

    assert!(
        matches!(
            &saved,
            Err(SnapshotStoreError::Storage { retryable: true, source })
                if format!("{source:#}").contains("the storage refused the call")
        ),
        "{saved:?}"
    );
}

#[test]
async fn a_prune_that_fails_without_a_storage_failure_gives_storage_that_is_not_retryable() {
    // Packs of zeros with the sizes of the index give the prune a decryption error, not a failed
    // storage call. The forget before the prune has succeeded, so the delete gives `Storage`.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    let packs = storage
        .list_blobs_below("test", "test", scope.0.clone(), Path::new("data"))
        .await
        .unwrap();
    futures::future::join_all(packs.iter().map(|pack| {
        let zeros = vec![0; usize::try_from(pack.size).unwrap()];
        let storage = storage.clone();
        let namespace = scope.0.clone();
        async move {
            storage
                .put_raw("test", "test", namespace, &pack.path, &zeros)
                .await
        }
    }))
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();

    let deleted = store.delete(&scope, &name("p-deleted")).await;

    assert!(
        deleted
            .as_ref()
            .is_err_and(|error| is_storage(error, false)),
        "{deleted:?}"
    );
}

/// The operation label, the path, and the name and the nice value of the calling thread of each
/// storage call.
#[cfg(target_os = "linux")]
type NiceCalls = Arc<std::sync::Mutex<Vec<(String, String, String, i32)>>>;

/// A storage that records the nice value of the thread of each call.
#[cfg(target_os = "linux")]
fn nice_recording_storage() -> (Arc<ScriptedBlobStorage>, NiceCalls) {
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
                    super::super::priority::own_nice(),
                ));
            Script::Pass
        }
    });
    (storage, calls)
}

/// Takes the recorded calls as the operation label, the path and the nice value.
#[cfg(target_os = "linux")]
fn taken_calls(calls: &NiceCalls) -> Vec<(String, String, i32)> {
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
fn taken_threads(calls: &NiceCalls, op_label: &str, directory: &str) -> Vec<(String, String)> {
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

#[cfg(target_os = "linux")]
#[test]
async fn the_forget_of_a_delete_runs_its_storage_calls_in_a_rayon_pool_of_its_own() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    taken_calls(&calls);

    store.delete(&scope, &name("p-1")).await.unwrap();
    let forgets = taken_threads(&calls, "delete", "snapshots/");

    assert_eq!(
        (
            forgets.len(),
            forgets
                .iter()
                .filter(|(_, thread)| !thread.starts_with("fs-snap-delete-"))
                .collect::<Vec<_>>()
        ),
        (1, Vec::<&(String, String)>::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_index_load_of_a_restore_runs_its_storage_calls_in_a_rayon_pool_of_its_own() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    taken_calls(&calls);

    let into = Scratch::new();
    store
        .restore(&scope, &name("p-1"), into.path())
        .await
        .unwrap();
    let index_reads = taken_threads(&calls, "read", "index/");

    assert_eq!(
        (
            index_reads.is_empty(),
            index_reads
                .iter()
                .filter(|(_, thread)| !thread.starts_with("fs-snap-restore-"))
                .collect::<Vec<_>>()
        ),
        (false, Vec::<&(String, String)>::new())
    );
}

/// Gives the operation labels of the calls, and each call that does not run at nice 19.
#[cfg(target_os = "linux")]
fn labels_and_calls_not_at_nice_19(
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

#[cfg(target_os = "linux")]
#[test]
async fn the_storage_calls_of_a_save_run_at_nice_19() {
    // The publish of the snapshot file runs on the async runtime after the work, so it keeps
    // the normal priority. The second save reads the config, the index, the snapshot files and
    // the trees of its parent, and writes the added file.
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    std::fs::write(tree.path().join("added.txt"), b"added").unwrap();
    taken_calls(&calls);

    store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
        )
        .await
        .unwrap();
    let work = taken_calls(&calls)
        .into_iter()
        .filter(|(op_label, _, _)| op_label != "publish")
        .collect::<Vec<_>>();

    assert_eq!(
        labels_and_calls_not_at_nice_19(&work),
        (vec!["list", "read", "stat", "write"], Vec::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_storage_calls_of_a_prune_run_at_nice_19() {
    // The forget of a delete runs before the ledger read at the normal priority. The ledger calls,
    // the calls of the freed records, the listing of the packs and the claim calls run on the
    // async runtime.
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    taken_calls(&calls);

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let prune = taken_calls(&calls)
        .into_iter()
        .skip_while(|(op_label, _, _)| op_label != "read_ledger")
        .filter(|(op_label, _, _)| {
            ![
                "read_ledger",
                "write_ledger",
                "list_data",
                "list_claims",
                "write_marker",
                "write_claim",
                "final_marker",
                "delete_marker",
                "delete_claims",
                "write_freed",
                "list_freed",
                "delete_freed",
                "list_ledgers",
                "delete_ledger",
                "refresh_claim",
                "list_claim_directories",
                "list_claim_blobs",
                "read_freed",
                "list_snapshots",
            ]
            .contains(&op_label.as_str())
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels_and_calls_not_at_nice_19(&prune),
        (vec!["delete", "list", "read", "stat", "write"], Vec::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_storage_calls_of_a_restore_run_at_the_nice_value_of_the_process() {
    let process_nice = super::super::priority::own_nice();
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(&scope, &name("p-1"), tree.path(), None)
        .await
        .unwrap();
    std::mem::take(&mut *calls.lock().unwrap());

    let restored = restored_listing(&store, &scope, &name("p-1")).await;
    let recorded = std::mem::take(&mut *calls.lock().unwrap());

    assert_eq!(
        (
            restored.ok(),
            recorded.is_empty(),
            recorded
                .iter()
                .filter(|(_, _, _, nice)| *nice != process_nice)
                .collect::<Vec<_>>()
        ),
        (
            Some(listing(tree.path())),
            false,
            Vec::<&(String, String, String, i32)>::new()
        )
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn after_saves_and_prunes_the_pools_keep_the_nice_value_of_the_process() {
    // All tasks wait for each other, so each runs on its own thread of the blocking pool, and the
    // idle threads that ran the saves and the prune are among them.
    const TASKS: usize = 16;
    let process_nice = super::super::priority::own_nice();
    let store = store(
        Arc::new(InMemoryBlobStorage::new()),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path(), None)
        .await
        .unwrap();
    store.delete(&scope, &name("p-deleted")).await.unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(TASKS));
    let blocking = futures::future::join_all((0..TASKS).map(|_| {
        let barrier = barrier.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            super::super::priority::own_nice()
        })
    }))
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect::<Vec<_>>();
    let rayon = rayon::broadcast(|_| super::super::priority::own_nice());

    assert_eq!(
        (
            blocking.iter().all(|nice| *nice == process_nice),
            rayon.iter().all(|nice| *nice == process_nice),
        ),
        (true, true),
        "{blocking:?} {rayon:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_storage_calls_of_the_rayon_workers_of_a_prune_that_repacks_run_at_nice_19() {
    // The deleted snapshot shares a pack with the kept one, so the prune repacks that pack. The
    // prune reads the index files and repacks with rayon, on the workers of the pool of the prune.
    let (storage, calls) = nice_recording_storage();
    let base = policy(LONG_DEADLINE, ALWAYS, Duration::ZERO);
    let store = store(
        storage,
        StorePolicy {
            prune: PruneSettings {
                repack: RepackLimits::Unlimited,
                ..base.prune
            },
            ..base
        },
    );
    let scope = new_scope();
    let file = |content: &[u8]| Spec::File {
        content: Box::from(content),
        mode: 0o644,
    };
    let both = Scratch::new();
    write_tree(
        both.path(),
        &[
            ("kept.txt", file(b"kept content")),
            ("deleted.txt", file(b"deleted content")),
        ],
    );
    let kept = Scratch::new();
    write_tree(kept.path(), &[("kept.txt", file(b"kept content"))]);
    store
        .save(&scope, &name("p-both"), both.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept.path(), None)
        .await
        .unwrap();
    std::mem::take(&mut *calls.lock().unwrap());

    store.delete(&scope, &name("p-both")).await.unwrap();
    let recorded = std::mem::take(&mut *calls.lock().unwrap());
    let from_workers = recorded
        .iter()
        .filter(|(_, _, thread, _)| thread.starts_with("fs-snap-prune-"))
        .collect::<Vec<_>>();

    assert_eq!(
        (
            recorded
                .iter()
                .any(|(op, path, _, _)| op == "write" && path.starts_with("data/")),
            from_workers.is_empty(),
            from_workers
                .iter()
                .filter(|(_, _, _, nice)| *nice != 19)
                .count(),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (true, false, 0, Some(listing(kept.path())))
    );
}

/// A pool builder that cannot start a thread.
#[cfg(target_os = "linux")]
fn no_pool(
    _: &str,
    _: Option<NonZeroUsize>,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .spawn_handler(|_| Err(std::io::Error::other("no thread can start here")))
        .build()
}

#[cfg(target_os = "linux")]
#[test]
async fn the_global_rayon_pool_keeps_the_nice_value_of_the_process_after_saves_without_their_pool()
{
    // Without its own pool, the rayon work of a save goes to the global pool from a thread at
    // nice 19. The store starts the global pool when it is made, so its threads keep the normal
    // priority also when a save is the first rayon work of the process.
    let process_nice = super::super::priority::own_nice();
    let store = RusticSnapshotStore {
        low_priority: super::super::priority::LowPriority {
            build_pool: no_pool,
            ..super::super::priority::LowPriority::new(NonZeroUsize::new(2))
        },
        ..RusticSnapshotStore::new(Arc::new(InMemoryBlobStorage::new()), &config())
    };
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), fixture_tree());
    store
        .save(&scope, &name("p-1"), first.path(), None)
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), second.path(), None)
        .await
        .unwrap();

    let global = rayon::broadcast(|_| super::super::priority::own_nice());

    assert!(
        global.iter().all(|nice| *nice == process_nice),
        "{global:?}"
    );
}

mod sweep;

/// The largest number of steps of one turn. A delete takes at most 21 steps of the protocol, and
/// its prune writes at least one new marker of its claim, so a turn of 22 steps runs a delete to
/// its end when its prune writes one new marker.
const SWEEP_TURN: usize = 22;

/// The number of random orders that the property test tries.
const SWEEP_CASES: u32 = 1000;

/// Saves the two snapshots that each case deletes, in a scope that each case copies.
async fn prepared_scope() -> (Arc<InMemoryBlobStorage>, SnapshotScope) {
    let shared = Arc::new(InMemoryBlobStorage::new());
    let prepared = new_scope();
    save_each(
        &store(shared.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO)),
        &prepared,
        &["p-1", "p-2"],
    )
    .await;
    (shared, prepared)
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_in_each_order_with_up_to_two_switches() {
    // Each order where one delete runs some steps, the other runs some steps, and then each runs
    // to its end. This holds each pause of one delete while the other runs.
    let (shared, prepared) = prepared_scope().await;
    let schedules = (0..2).flat_map(|first| {
        (0..=SWEEP_TURN).flat_map(move |one| {
            (0..=SWEEP_TURN).map(move |two| sweep::Schedule {
                first,
                turns: vec![one, two],
                fail: None,
                late: None,
            })
        })
    });
    let started = std::time::Instant::now();

    let cases = futures::stream::iter(schedules)
        .then(|schedule| {
            let (shared, prepared) = (&shared, &prepared);
            async move { sweep::run_case(shared, prepared, &schedule).await }
        })
        .try_fold(0usize, |cases, _| async move { Ok(cases + 1) })
        .await;

    println!("{cases:?} orders in {:?}", started.elapsed());
    assert!(cases.is_ok(), "{cases:?}");
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_in_random_orders_with_a_failed_call() {
    // The test generates the order of the steps, a call that fails, and a call that gets no answer
    // and reaches the storage later, and shrinks a failing case to the shortest order. The seed is fixed, and no file keeps a failing case.
    use proptest::prelude::{Strategy, prop};
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    let (shared, prepared) = prepared_scope().await;
    let runtime = tokio::runtime::Handle::current();
    let strategy = (
        0usize..2,
        prop::collection::vec(0usize..=SWEEP_TURN, 0..=8),
        prop::option::of((0usize..2, 0usize..SWEEP_TURN)),
        prop::option::of((0usize..2, 0usize..SWEEP_TURN, 0usize..8)),
    )
        .prop_map(|(first, turns, fail, late)| sweep::Schedule {
            first,
            turns,
            fail,
            late,
        });
    let started = std::time::Instant::now();

    let outcome = tokio::task::spawn_blocking(move || {
        let mut runner = TestRunner::new_with_rng(
            Config {
                cases: SWEEP_CASES,
                failure_persistence: None,
                ..Config::default()
            },
            TestRng::deterministic_rng(RngAlgorithm::ChaCha),
        );
        runner
            .run(&strategy, |schedule| {
                runtime
                    .block_on(sweep::run_case(&shared, &prepared, &schedule))
                    .map(|_| ())
                    .map_err(proptest::test_runner::TestCaseError::fail)
            })
            .map_err(|error| error.to_string())
    })
    .await;

    println!("{SWEEP_CASES} random orders in {:?}", started.elapsed());
    assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
}
