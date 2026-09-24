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
use super::super::prune::{PruneLedger, read_ledger};
use super::super::scripted::{Script, ScriptedBlobStorage};
use super::super::{PruneReport, PruneSettings, RepackLimits, RepositoryKey, open_existing};
use super::{
    RusticSnapshotStore, StorePolicy, leaves_marked_packs, scope_snapshots, store_backup_options,
    store_restore_options, whole_millis_from,
};
use crate::filesystem_snapshot::contract_tests::fixture::{
    Listed, Scratch, Spec, fixture, listing, write_tree,
};
use crate::filesystem_snapshot::contract_tests::{self, OpenStore, new_scope};
use crate::filesystem_snapshot::{
    FilesystemSnapshotStore, SnapshotInfo, SnapshotName, SnapshotScope, SnapshotStoreError,
};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use futures::{FutureExt, StreamExt};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use std::future::Future;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::core::DynamicTestRegistration;
use test_r::{test, test_gen};

/// The longest time that a test waits for an operation or for the work of a store to end.
const LIMIT: Duration = Duration::from_secs(10);

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
fn policy(deadline: Duration, prune_threshold: u64, grace: Duration) -> StorePolicy {
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
    })
    .await
    .unwrap()
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
    let backup = store_backup_options(&policy);
    let restore = store_restore_options(&policy);

    assert_eq!(
        (
            policy.deadline,
            policy.save.threads.map(NonZeroUsize::get),
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
            Duration::from_secs(3600),
            false,
            RepackLimits::Rustic,
            64 * 1024 * 1024,
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let directory = std::fs::File::open(tree.path()).unwrap();
    let through_fd = format!("/proc/self/fd/{}", directory.as_raw_fd());

    store
        .save(&scope, &name("p-fd"), Path::new(&through_fd))
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
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();

    let failed = store.save(&scope, &name("p-1"), tree.path()).await;
    let stat = store.stat(&scope, &name("p-1")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let restore = restored_listing(&store, &scope, &name("p-1")).await;
    refuse.store(false, Ordering::SeqCst);
    let saved_again = store.save(&scope, &name("p-1"), tree.path()).await;

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
        policy(Duration::from_secs(1), u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("late");

    let failed = store.save(&scope, &name("p-late"), tree.path()).await;
    hang.store(false, Ordering::SeqCst);
    let stat = store.stat(&scope, &name("p-late")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let saved_again = store.save(&scope, &name("p-late"), tree.path()).await;

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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();

    store.save(&scope, &name("p-1"), tree.path()).await.unwrap();
    let first = storage.calls().len();
    store.save(&scope, &name("p-2"), tree.path()).await.unwrap();
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
async fn two_stores_that_create_one_repository_at_the_same_time_both_save() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path == Path::new("config") {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        });
    let policy = policy(LONG_DEADLINE, u64::MAX, Duration::ZERO);
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
        first.save(&scope, &first_name, first_tree.path()),
        second.save(&scope, &second_name, second_tree.path()),
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
    let store = store(storage.clone(), policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (old_tree, new_tree) = (one_file_tree("old"), fixture_tree());
    store
        .save(&scope, &name("p-old"), old_tree.path())
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
        async move { store.save(&scope, &name("p-new"), &path).await }
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
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store.save(&scope, &name("p-1"), tree.path()).await.unwrap();
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("readable");
    let locked = tree.path().join("locked.txt");
    std::fs::write(&locked, b"locked").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let saved = store.save(&scope, &name("p-locked"), tree.path()).await;

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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    store
        .save(&scope, &name("p-xattr"), source.path())
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path())
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
    let store = store(storage.clone(), policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
        .await
        .unwrap();
    let packs_before = blobs(&*storage, &scope.0, "data/").await;

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let after_first = ledger(&storage, &scope).await;
    store.delete(&scope, &name("p-none")).await.unwrap();
    let packs_after = blobs(&*storage, &scope.0, "data/").await;

    assert_eq!(
        (
            after_first.freed_bytes,
            after_first.last_prune.is_some(),
            after_first.awaiting_removal,
            packs_after.len() < packs_before.len(),
            packs_after.iter().all(|pack| packs_before.contains(pack)),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (0, true, true, true, true, Some(listing(kept_tree.path())))
    );
}

#[test]
async fn a_delete_below_the_threshold_does_not_prune() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), one_file_tree("kept"));
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
        .await
        .unwrap();
    let packs_before = blobs(&*storage, &scope.0, "data/").await;

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let after = ledger(&storage, &scope).await;

    assert_eq!(
        (
            after.freed_bytes > 0,
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
        policy(LONG_DEADLINE, 1, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    let trees = [one_file_tree("a"), one_file_tree("b"), one_file_tree("c")];
    futures::stream::iter(["p-a", "p-b", "p-c"].into_iter().zip(&trees))
        .for_each(|(text, tree)| {
            let store = store.clone();
            let scope = scope.clone();
            async move {
                store.save(&scope, &name(text), tree.path()).await.unwrap();
            }
        })
        .await;

    store.delete(&scope, &name("p-a")).await.unwrap();
    let after_first = ledger(&storage, &scope).await;
    store.delete(&scope, &name("p-b")).await.unwrap();
    let after_second = ledger(&storage, &scope).await;

    assert_eq!(
        (
            after_first.last_prune.is_some(),
            after_second.last_prune == after_first.last_prune,
            after_second.freed_bytes > 0,
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
    let store = store(storage.clone(), policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let failed = store.delete(&scope, &name("p-deleted")).await;
    let after_failure = ledger(&storage, &scope).await;
    refuse.store(false, Ordering::SeqCst);
    let retried = store.delete(&scope, &name("p-deleted")).await;
    let after_retry = ledger(&storage, &scope).await;

    assert!(
        failed.as_ref().is_err_and(|error| is_storage(error, true)),
        "{failed:?}"
    );
    assert_eq!(
        (
            after_failure.freed_bytes > 0,
            after_failure.last_prune,
            retried.is_ok(),
            after_retry.freed_bytes,
            after_retry.last_prune.is_some(),
        ),
        (true, None, true, 0, true)
    );
}

#[test]
async fn a_deleted_scope_holds_no_blob() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(storage.clone(), policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), one_file_tree("second"));
    store
        .save(&scope, &name("p-1"), first.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), second.path())
        .await
        .unwrap();
    store.delete(&scope, &name("p-1")).await.unwrap();
    let before = blobs(&*storage, &scope.0, "").await;

    store.delete_scope(&scope).await.unwrap();

    assert_eq!(
        (
            before.contains(&"golem/prune-ledger".to_string()),
            blobs(&*storage, &scope.0, "").await
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    )
    .save(&new_scope(), &name("p-dropped"), tree.path())
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
                let policy = policy(LONG_DEADLINE, u64::MAX, Duration::ZERO);
                let dropping = store(storage.clone(), policy);
                let scope = new_scope();
                let ended = drop_when(
                    &storage,
                    |calls| calls.len() >= held,
                    dropping.save(&scope, &name("p-dropped"), &tree),
                )
                .await;
                let stopped = tokio::time::timeout(LIMIT, dropping.shut_down())
                    .await
                    .is_ok();
                let later = store(inner, policy);
                let stat = later.stat(&scope, &name("p-dropped")).await.ok().flatten();
                let names = listed_names(&later, &scope).await;
                let saved_again = later.save(&scope, &name("p-dropped"), &other).await;
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let saving = tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = tree.path().to_path_buf();
        async move { store.save(&scope, &name("p-held"), &path).await }
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
            policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
        );
        let scope = new_scope();
        let tree = fixture_tree();
        store.save(&scope, &name("p-1"), tree.path()).await.unwrap();
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
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path())
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
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path())
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(&scope, &name("p-kept"), tree.path())
        .await
        .unwrap();

    store.delete(&scope, &name("p-unknown")).await.unwrap();

    assert_eq!(
        blobs(&*storage, &scope.0, "golem/").await,
        Vec::<String>::new()
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();

    let saved = store.save(&scope, &name("p-relative"), relative).await;

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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();

    let saved = store
        .save(&scope, &name("p-file"), &tree.path().join("file.txt"))
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
        policy(LONG_DEADLINE, u64::MAX, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(&scope, &name("p-deleted"), tree.path())
        .await
        .unwrap();
    let added = snapshot_files(storage.clone(), &scope)
        .await
        .iter()
        .filter_map(|snapshot| snapshot.summary.as_ref())
        .map(|summary| summary.data_added_packed)
        .sum::<u64>();

    store.delete(&scope, &name("p-deleted")).await.unwrap();

    assert_eq!(
        (added > 1, ledger(&storage, &scope).await.freed_bytes),
        (true, added)
    );
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
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("never saved");

    let saved = store.save(&scope, &name("p-1"), tree.path()).await;

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
    let store = store(storage.clone(), policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
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

/// Takes the recorded calls with the operation label.
#[cfg(target_os = "linux")]
fn taken_calls(calls: &NiceCalls, op_label: &str) -> Vec<(String, i32)> {
    std::mem::take(
        &mut *calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_iter()
    .filter(|(op, _, _, _)| op == op_label)
    .map(|(_, path, _, nice)| (path, nice))
    .collect()
}

#[cfg(target_os = "linux")]
#[test]
async fn the_writes_of_a_save_run_at_nice_19() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();

    store.save(&scope, &name("p-1"), tree.path()).await.unwrap();
    let writes = taken_calls(&calls, "write");

    assert_eq!(
        (
            writes.is_empty(),
            writes
                .iter()
                .filter(|(_, nice)| *nice != 19)
                .collect::<Vec<_>>()
        ),
        (false, Vec::<&(String, i32)>::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_writes_of_a_prune_run_at_nice_19() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, 1, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
        .await
        .unwrap();
    taken_calls(&calls, "write");

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let writes = taken_calls(&calls, "write");

    assert_eq!(
        (
            writes.is_empty(),
            writes
                .iter()
                .filter(|(_, nice)| *nice != 19)
                .collect::<Vec<_>>()
        ),
        (false, Vec::<&(String, i32)>::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn the_storage_calls_of_a_restore_run_at_the_nice_value_of_the_process() {
    let process_nice = super::super::priority::own_nice();
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, u64::MAX, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store.save(&scope, &name("p-1"), tree.path()).await.unwrap();
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
async fn after_saves_and_prunes_the_pools_keep_the_nice_value_of_the_process() {
    // All tasks wait for each other, so each runs on its own thread of the blocking pool, and the
    // idle threads that ran the saves and the prune are among them.
    const TASKS: usize = 16;
    let process_nice = super::super::priority::own_nice();
    let store = store(
        Arc::new(InMemoryBlobStorage::new()),
        policy(LONG_DEADLINE, 1, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(&scope, &name("p-deleted"), deleted_tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept_tree.path())
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
    let base = policy(LONG_DEADLINE, 1, Duration::ZERO);
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
        .save(&scope, &name("p-both"), both.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), kept.path())
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
        .save(&scope, &name("p-1"), first.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-2"), second.path())
        .await
        .unwrap();

    let global = rayon::broadcast(|_| super::super::priority::own_nice());

    assert!(
        global.iter().all(|nice| *nice == process_nice),
        "{global:?}"
    );
}
