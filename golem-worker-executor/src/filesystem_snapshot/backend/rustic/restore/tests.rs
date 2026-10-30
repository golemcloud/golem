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

use super::destination::ClearHook;
use crate::filesystem_snapshot::backend::rustic::RusticSnapshotStore;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::holding::holding_storage;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    ALWAYS, LIMIT, LONG_DEADLINE, NEVER, REACH_LIMIT, SHORT_DEADLINE, SharedSlots, blobs,
    data_packs, dropped_within_limit, eventually, failed_at_deadline, fixture_tree,
    holding_index_reads, is_failed, key, ledger, mark_the_packs_of_a_hidden_snapshot, name,
    one_file_tree, policy, repository_fixture_tree, restored_listing, runs_policy, save_each,
    scope_with_a_prune_to_come, store, store_over, store_with_deadline,
};

use crate::filesystem_snapshot::clock::SystemClock;
use crate::filesystem_snapshot::test_support::fixture::{Scratch, Spec, fixture, listing};
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    AgentSnapshots, FilesystemSnapshotStore, RestoreFailure, SnapshotInfo, Unlimited,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::task::AbortOnDropHandle;

use golem_service_base::storage::blob::BlobStorage;
#[test]
#[timeout("120s")]
async fn a_restore_whose_data_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = repository_fixture_tree();
    store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE)
        .save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
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
    let store_with_deadline = store_with_deadline(storage, SHORT_DEADLINE);
    let into = Scratch::new();
    let directories = fixture()
        .into_iter()
        .filter(|(_, spec)| matches!(spec, Spec::Directory { .. }))
        .map(|(path, _)| path)
        .collect::<Vec<_>>();

    let restored = tokio::time::timeout(
        REACH_LIMIT,
        store_with_deadline.restore(&scope, &name("p-first"), into.path(), &Unlimited),
    )
    .await;
    drop(store_with_deadline);
    let stopped = dropped_within_limit(dropped).await;
    // The plan phase of a restore makes each directory before the content phase reads data. A
    // run that fails after its first write removes what it wrote.
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
                .map(|path| (*path, false))
                .collect::<Vec<_>>()
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_pack_reads_fail_gives_failed() {
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
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(restored.as_ref().is_err_and(is_failed), "{restored:?}");
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
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
        .save(
            &scope,
            &name("p-xattr"),
            source.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();

    let restored = restored_listing(&store, &scope, &name("p-xattr")).await;

    assert!(
        matches!(&restored, Err(RestoreFailure::Destination(_))),
        "{restored:?}"
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_index_file_a_prune_deleted_after_the_listing_loads_again_and_gives_the_whole_tree()
 {
    let shared = Arc::new(InMemoryBlobStorage::new());
    let (scope, pruning, kept) = scope_with_a_prune_to_come(&shared).await;
    let (restoring, storage, hold, held) = holding_index_reads(&shared);
    hold.store(true, Ordering::SeqCst);
    let first = AbortOnDropHandle::new(tokio::spawn({
        let (restoring, scope) = (restoring.clone(), scope.clone());
        async move {
            let into = Scratch::new();
            restoring
                .restore(&scope, &name("p-1"), into.path(), &Unlimited)
                .await
        }
    }));
    let reached = eventually(|| held.load(Ordering::SeqCst) > 0).await;

    let deleted = pruning.delete(&scope, &[name("p-2")], &Unlimited).await;
    let pruned = ledger(&shared, &scope).await.last_prune.is_some();
    hold.store(false, Ordering::SeqCst);
    storage.open_gate();
    let first = tokio::time::timeout(LIMIT, first).await;
    let again = restored_listing(&restoring, &scope, &name("p-1")).await;

    assert!(matches!(&first, Ok(Ok(Ok(_)))), "{first:?}");
    assert_eq!(
        (reached, deleted.is_ok(), pruned, again.ok()),
        (true, true, true, Some(listing(kept.path())))
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_builds_its_pool_with_the_restore_reader_threads() {
    // The save threads and the restore reader threads differ, so the count of the pool tells
    // which setting the restore took.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let policy = StorePolicy {
        save_threads: NonZeroUsize::new(2),
        restore_reader_threads: NonZeroUsize::new(3).unwrap(),
        ..policy(LONG_DEADLINE, NEVER, Duration::ZERO)
    };
    let store = RusticSnapshotStore {
        low_priority: crate::filesystem_snapshot::backend::rustic::cpu_priority::LowPriority {
            build_pool: recording_pool,
            ..crate::filesystem_snapshot::backend::rustic::cpu_priority::LowPriority::new(
                policy.save_threads,
            )
        },
        ..RusticSnapshotStore::with_policy(storage, key(), policy, Arc::new(SystemClock))
    };
    let scope = new_scope();
    let tree = one_file_tree("restored");
    store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();

    let restored = restored_listing(&store, &scope, &name("p-1")).await;
    let restore_pools = BUILT_POOLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(name, _)| *name == "fs-snap-restore")
        .map(|(_, threads)| *threads)
        .collect::<Vec<_>>();

    assert_eq!(
        (restored.ok(), restore_pools),
        (Some(listing(tree.path())), vec![NonZeroUsize::new(3)])
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_of_a_name_whose_index_is_gone_gives_corrupt_after_a_check_that_wrote_nothing() {
    // No index file lists the blobs of the snapshot, and no index file marks them. The check
    // fails before a write, a new listing of the index files gives no new name, the snapshot file
    // is there, and no marked pack holds the blobs.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store_over(&inner);
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    let index_files = blobs(&*inner, scope.namespace(), "index/").await;
    futures::future::join_all(
        index_files
            .iter()
            .map(|path| inner.delete("test", "test", scope.namespace().clone(), Path::new(path))),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
    let into = Scratch::new();

    let restored = store
        .restore(&scope, &name("p-1"), into.path(), &Unlimited)
        .await;

    assert!(
        matches!(restored, Err(RestoreFailure::Corrupt(_))),
        "{restored:?}"
    );
    assert_eq!(listing(into.path()), Vec::new());
}

#[test]
#[timeout("60s")]
async fn a_restore_of_a_name_whose_deduped_pack_a_prune_marked_gives_the_whole_tree_in_its_first_run_and_after_the_next_prune()
 {
    let (inner, scope, tree, before, unused) = marked_dedup().await;
    let saving = store(
        inner.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::from_secs(3600)),
    );
    let first_slots = SharedSlots::new(1);
    let first_into = Scratch::new();
    let first = saving
        .restore(&scope, &name("p-2"), first_into.path(), &first_slots)
        .await;
    let pruning = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    save_each(&pruning, &scope, &["p-3"]).await;
    pruning
        .delete(&scope, &[name("p-3")], &Unlimited)
        .await
        .unwrap();
    let after_the_next_prune = restored_listing(&saving, &scope, &name("p-2")).await;

    assert_eq!((before.len(), unused), (2, true));
    assert!(first.is_ok(), "{first:?}");
    assert_eq!(after_the_next_prune.ok(), Some(listing(tree.path())));
    // The first restore answered in its first run, with no race run.
    assert_eq!(
        (first_slots.counts().0, listing(first_into.path())),
        (1, listing(tree.path()))
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_marked_pack_is_gone_while_an_index_file_marks_it_gives_corrupt_in_the_first_run()
 {
    // The packs of `p-2` are gone, and an index file still marks them. A load of the index again
    // lists each pack that was gone, as a marked pack, so the run did not race a prune, and the
    // answer is `Corrupt`.
    let (inner, scope, _tree, _, unused) = marked_dedup().await;
    let packs = blobs(&*inner, scope.namespace(), "data/").await;
    futures::future::join_all(
        packs
            .iter()
            .map(|path| inner.delete("test", "test", scope.namespace().clone(), Path::new(path))),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
    let store = store(
        inner.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::from_secs(3600)),
    );
    let slots = SharedSlots::new(1);
    let into = Scratch::new();

    let restored = store
        .restore(&scope, &name("p-2"), into.path(), &slots)
        .await;

    assert!(
        matches!(restored, Err(RestoreFailure::Corrupt(_))),
        "{restored:?}"
    );
    assert_eq!((unused, slots.counts().0), (true, 1));
}

#[test]
#[timeout("60s")]
async fn a_marked_pack_after_a_load_that_missed_an_index_file_gives_the_whole_tree_in_the_first_run()
 {
    // The first listing of the restore holds an index file that is gone at its read, and the
    // later listings do not hold it. The load after it finds the name, and its index holds the
    // pack that a prune marked.
    let (inner, scope, tree, _, unused) = marked_dedup().await;
    let gone = Path::new("index").join("00".repeat(32));
    inner
        .put_raw("test", "test", scope.namespace().clone(), &gone, b"gone")
        .await
        .unwrap();
    let listings = AtomicUsize::new(0);
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let gone = gone.clone();
        move |op_label, path| match op_label {
            "read" if path == gone => Script::Vanish,
            "list" if path == Path::new("index") => {
                if listings.fetch_add(1, Ordering::SeqCst) == 0 {
                    Script::Pass
                } else {
                    Script::Torn
                }
            }
            _ => Script::Pass,
        }
    });
    storage.hide([gone.clone().into_boxed_path()]);
    storage.open_gate();
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::from_secs(3600)),
    );
    let slots = SharedSlots::new(1);
    let into = Scratch::new();

    let restored = store
        .restore(&scope, &name("p-2"), into.path(), &slots)
        .await;

    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(
        (
            unused,
            listing(into.path()),
            slots.counts().0,
            storage
                .calls()
                .iter()
                .filter(|(op_label, path)| *op_label == "read" && Path::new(path) == gone)
                .count()
        ),
        (true, listing(tree.path()), 1, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_ghost_index_file_ends_the_run_after_two_loads() {
    // A listed index file is never there at its read, in each load of the run.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let scope = new_scope();
    save_each(&saving, &scope, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if op_label == "read" && path.starts_with("index") {
            Script::Vanish
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(
        matches!(restored, Err(RestoreFailure::Failed(_))),
        "{restored:?}"
    );
    assert_eq!(
        storage
            .calls()
            .iter()
            .filter(|(op_label, path)| *op_label == "read" && path.starts_with("index"))
            .count(),
        2
    );
}

#[test]
#[timeout("60s")]
async fn a_restore_of_a_name_deleted_during_it_gives_not_found() {
    // A delete removes the snapshot file after the first read of the snapshots, and a prune the
    // packs: the check misses a pack, and the snapshots read again have no such name.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let scope = new_scope();
    save_each(&saving, &scope, &["p-1"]).await;
    let snapshot_reads = Arc::new(AtomicUsize::new(0));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let snapshot_reads = snapshot_reads.clone();
        move |op_label, path| {
            let snapshot = op_label == "read" && path.starts_with("snapshots");
            if snapshot && snapshot_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                Script::Pass
            } else if snapshot || path.starts_with("data") {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(
        matches!(restored, Err(RestoreFailure::NotFound)),
        "{restored:?}"
    );
}

#[cfg(unix)]
#[test]
#[timeout("60s")]
async fn a_run_after_a_clear_that_failed_partway_clears_before_it_loads() {
    // The first run fails after its first write, and its clear fails after one entry. The second
    // run finds the directory not empty, clears it first, and gives the whole tree.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let scope = new_scope();
    let tree = fixture_tree();
    saving
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let refused = AtomicBool::new(false);
        move |op_label, path| {
            if op_label == "read_range"
                && path.starts_with("data")
                && !refused.swap(true, Ordering::SeqCst)
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let hook = Arc::new(FailingOnce::default());
    let store = Arc::new(
        RusticSnapshotStore::with_policy(
            storage.clone(),
            key(),
            runs_policy(LONG_DEADLINE, 1),
            Arc::new(SystemClock),
        )
        .with_clear_hook(hook.clone()),
    );
    let into = Scratch::new();

    let restored = store
        .restore(&scope, &name("p-1"), into.path(), &Unlimited)
        .await;

    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(
        (hook.0.load(Ordering::SeqCst), listing(into.path())),
        (true, listing(tree.path()))
    );
}

#[cfg(unix)]
#[test]
#[timeout("60s")]
async fn a_restore_whose_run_failed_after_it_made_a_directory_read_only_clears_into_and_gives_the_whole_tree()
 {
    use std::os::unix::fs::PermissionsExt;
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let scope = new_scope();
    let tree = Scratch::new();
    // A save reads each directory, so the saved directories can be read and searched; the
    // directory at mode 0o444 is empty. The clear of a directory at mode 0o000 has its own test
    // in `clear`.
    let modes = [0o555, 0o444];
    modes.iter().for_each(|mode| {
        let directory = tree.path().join(format!("d{mode:o}"));
        std::fs::create_dir(&directory).unwrap();
        if *mode == 0o555 {
            std::fs::write(directory.join("file"), format!("{mode:o}")).unwrap();
        }
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(*mode)).unwrap();
    });
    saving
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let expected = listing(tree.path());
    // The first restore writes the whole tree with its modes; a refused read of the second makes
    // its first run fail after it wrote into the same directory.
    let first = Scratch::new();
    saving
        .restore(&scope, &name("p-1"), first.path(), &Unlimited)
        .await
        .unwrap();
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let refused = AtomicBool::new(false);
        move |op_label, path| {
            if op_label == "read_range"
                && path.starts_with("data")
                && !refused.swap(true, Ordering::SeqCst)
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let into = Scratch::new();

    let restored = store
        .restore(&scope, &name("p-1"), into.path(), &Unlimited)
        .await;
    let restored_first = listing(first.path());
    modes.iter().for_each(|mode| {
        let _ = std::fs::set_permissions(
            tree.path().join(format!("d{mode:o}")),
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::set_permissions(
            first.path().join(format!("d{mode:o}")),
            std::fs::Permissions::from_mode(0o755),
        );
    });
    let restored_listing = listing(into.path());
    modes.iter().for_each(|mode| {
        let _ = std::fs::set_permissions(
            into.path().join(format!("d{mode:o}")),
            std::fs::Permissions::from_mode(0o755),
        );
    });

    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(
        (restored_first, restored_listing),
        (expected.clone(), expected)
    );
}

#[cfg(unix)]
#[test]
#[timeout("60s")]
async fn a_restore_into_a_directory_whose_path_is_not_utf8_gives_destination() {
    use std::os::unix::ffi::OsStrExt;
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store_over(&inner);
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    let parent = Scratch::new();
    let into = parent
        .path()
        .join(std::ffi::OsStr::from_bytes(b"into-\xff"));
    std::fs::create_dir(&into).unwrap();

    let restored = store.restore(&scope, &name("p-1"), &into, &Unlimited).await;

    assert!(
        matches!(
            &restored,
            Err(RestoreFailure::Destination(error))
                if error.kind() == std::io::ErrorKind::InvalidInput
        ),
        "{restored:?}"
    );
    assert_eq!(listing(&into), Vec::new());
}

#[cfg(unix)]
#[test]
#[timeout("60s")]
async fn a_local_io_error_of_a_restore_gives_destination_with_its_kind() {
    // The directory of the restore loses its permissions while the run loads the repository, so
    // the write cannot open the first file. rustic gives that error without its `io::Error` as a
    // source.
    use std::os::unix::fs::PermissionsExt;
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let first_read = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(inner, {
        let first_read = first_read.clone();
        move |op_label, path| {
            if op_label == "read"
                && path == Path::new("config")
                && !first_read.swap(true, Ordering::SeqCst)
            {
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
    let parent = Scratch::new();
    let into = parent.path().join("into");
    std::fs::create_dir(&into).unwrap();
    let restored_name = name("p-1");
    let restoring = store.restore(&scope, &restored_name, &into, &Unlimited);
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| first_read.load(Ordering::SeqCst)) => reached,
        restored = &mut restoring => panic!("the restore ended before its load: {restored:?}"),
    };
    std::fs::set_permissions(&into, std::fs::Permissions::from_mode(0o000)).unwrap();
    let privileged = std::fs::read_dir(&into).is_ok();
    storage.open_gate();
    let restored = tokio::time::timeout(LIMIT, restoring).await.unwrap();
    std::fs::set_permissions(&into, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(reached);
    assert!(
        privileged
            || matches!(
                &restored,
                Err(RestoreFailure::Destination(error))
                    if error.kind() == std::io::ErrorKind::PermissionDenied
            ),
        "{restored:?}"
    );
    assert_eq!(listing(&into), Vec::new());
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

/// The name and the thread count of each pool that [`recording_pool`] built.
static BUILT_POOLS: std::sync::Mutex<Vec<(&'static str, Option<NonZeroUsize>)>> =
    std::sync::Mutex::new(Vec::new());

/// Builds a rayon pool with the name and the thread count, and records both. Only the store of
/// one test uses it, so the record holds only the pools of that store.
fn recording_pool(
    name: &'static str,
    threads: Option<NonZeroUsize>,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    BUILT_POOLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((name, threads));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.map_or(0, NonZeroUsize::get))
        .build()
}

/// Saves `p-1` and then `p-2`, which reuses the packs of `p-1`. A prune that does not see the
/// snapshot file of `p-2` marks those packs after the delete of `p-1`, so no index file lists
/// them as packs. Gives the storage, the scope, the tree of `p-2`, the snapshot files before the
/// delete, and whether the prune found unused packs.
async fn marked_dedup() -> (
    Arc<InMemoryBlobStorage>,
    AgentSnapshots,
    Scratch,
    Vec<String>,
    bool,
) {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store(
        inner.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    save_each(&saving, &scope, &["p-1"]).await;
    saving
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let (before, unused) =
        mark_the_packs_of_a_hidden_snapshot(inner.clone(), &scope, Duration::from_secs(3600)).await;

    (inner, scope, tree, before, unused)
}

/// The points of the clear of a restore where a test acts: the first clear fails after one entry.
#[derive(Default)]
struct FailingOnce(AtomicBool);

impl ClearHook for FailingOnce {
    fn after_entry(&self) -> std::io::Result<()> {
        if self.0.swap(true, Ordering::SeqCst) {
            Ok(())
        } else {
            Err(std::io::Error::other("the clear failed after one entry"))
        }
    }
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_repository_a_delete_all_removed_gives_not_found() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store_over(&inner);
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    store.delete_all(&scope, &Unlimited).await.unwrap();

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(
        matches!(restored, Err(RestoreFailure::NotFound)),
        "{restored:?}"
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_of_all_snapshots_before_the_index_is_loaded_again_gives_not_found() {
    // The first read of a pack finds no blob, so the snapshots are read again and hold the name. A
    // delete of all snapshots then removes the repository before the index is loaded again.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let vanished = AtomicBool::new(false);
    let storage = holding_the_third_open(inner.clone(), move |op_label, path| {
        op_label == "read" && path.starts_with("data") && !vanished.swap(true, Ordering::SeqCst)
    });

    let restored = restore_through_a_delete_of_all(&storage, &inner, &scope).await;

    assert!(
        matches!(restored, Err(RestoreFailure::NotFound)),
        "{restored:?}"
    );
}

/// Deletes each blob of the scope, as a delete of all snapshots does.
async fn delete_everything(inner: &InMemoryBlobStorage, scope: &AgentSnapshots) {
    let paths = blobs(inner, scope.namespace(), "").await;
    futures::future::join_all(
        paths
            .iter()
            .map(|path| inner.delete("test", "test", scope.namespace().clone(), Path::new(path))),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
}

/// A storage over `inner` that holds the first look at the config of the third open of the
/// repository at its gate, and that gives no blob to the reads that `vanishing` picks.
/// The number of the first look at the config in the third open of the repository: an open looks
/// at the config twice and reads it once.
const THIRD_OPEN_STAT: usize = 5;

fn holding_the_third_open(
    inner: Arc<InMemoryBlobStorage>,
    vanishing: impl Fn(&str, &Path) -> bool + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::with_state(inner, 0usize, move |config_calls, op_label, path| {
        if op_label == "stat" && path == Path::new("config") {
            let number = config_calls + 1;
            (
                if number == THIRD_OPEN_STAT {
                    Script::WaitForGate
                } else {
                    Script::Pass
                },
                number,
            )
        } else if vanishing(op_label, path) {
            (Script::Vanish, *config_calls)
        } else {
            (Script::Pass, *config_calls)
        }
    })
}

/// Restores `p-1` while the storage holds the first look at the config of the third open, deletes
/// every blob of the scope while it is held, and then opens the gate.
async fn restore_through_a_delete_of_all(
    storage: &Arc<ScriptedBlobStorage>,
    inner: &InMemoryBlobStorage,
    scope: &AgentSnapshots,
) -> Result<SnapshotInfo, RestoreFailure> {
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let into = Scratch::new();
    let restored_name = name("p-1");
    let restoring = store.restore(scope, &restored_name, into.path(), &Unlimited);
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| {
            storage
                .calls()
                .iter()
                .filter(|(op_label, path)| *op_label == "stat" && path == "config")
                .count()
                == THIRD_OPEN_STAT
        }) => reached,
        restored = &mut restoring => panic!("the restore ended before its third open: {restored:?}"),
    };
    assert!(reached);
    delete_everything(inner, scope).await;
    storage.open_gate();
    tokio::time::timeout(LIMIT, restoring).await.unwrap()
}

mod failure_mapping {
    use super::super::{Phase, RestoreError, restore_error};
    use crate::filesystem_snapshot::backend::rustic::failure_classification::OperationCancelled;
    use crate::filesystem_snapshot::backend::rustic::test_support::{
        mapping_failed_call, mapping_local_io_without_source, mapping_missing, mapping_refused,
        mapping_rustic,
    };

    use golem_service_base::storage::blob::{BlobNameError, BlobRangeError};
    use pretty_assertions::assert_eq;
    use rustic_core::{ErrorKind, Id, RusticError};
    use std::io;
    use std::path::Path;
    use test_r::test;
    #[test]
    fn a_restore_error_is_read_from_the_storage_first_and_then_from_its_phase() {
        let each_phase = |error: fn() -> anyhow::Error| {
            [Phase::Load, Phase::Check, Phase::Write].map(|phase| restore_error(&error(), phase))
        };
        let pack = PACK.parse::<Id>().unwrap();

        assert_eq!(
            [
                each_phase(|| mapping_missing(&format!("data/01/{PACK}"))),
                each_phase(|| mapping_missing("data/01/not-an-id")),
                each_phase(|| mapping_missing("config")),
                each_phase(|| mapping_missing("index/0123")),
                each_phase(|| mapping_failed_call(anyhow::Error::new(OperationCancelled))),
                each_phase(|| mapping_failed_call(anyhow::Error::new(BlobRangeError {
                    start: 4,
                    end: 9
                }))),
                each_phase(|| {
                    mapping_failed_call(anyhow::Error::new(BlobNameError::NotUtf8 {
                        path: "x".into(),
                    }))
                }),
                each_phase(|| mapping_failed_call(anyhow::anyhow!("the bucket gave 503"))),
                each_phase(|| mapping_rustic(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "no space"
                ))),
                each_phase(|| {
                    anyhow::Error::new(RusticError::with_source(
                        ErrorKind::InputOutput,
                        "The restore cannot set the extended attributes of `file.txt`.",
                        SettingXattrFailed(io::Error::from_raw_os_error(28)),
                    ))
                }),
                each_phase(mapping_local_io_without_source),
                each_phase(mapping_refused),
            ],
            [
                [(); 3].map(|()| RestoreError::PackMissing(Some(pack))),
                [
                    RestoreError::PackMissing(None),
                    RestoreError::PackMissing(None),
                    RestoreError::PackMissing(None)
                ],
                [
                    RestoreError::ConfigMissing,
                    RestoreError::ConfigMissing,
                    RestoreError::ConfigMissing
                ],
                [
                    RestoreError::FileMissing(Path::new("index/0123").into()),
                    RestoreError::FileMissing(Path::new("index/0123").into()),
                    RestoreError::FileMissing(Path::new("index/0123").into()),
                ],
                [
                    RestoreError::Cancelled,
                    RestoreError::Cancelled,
                    RestoreError::Cancelled
                ],
                [(); 3].map(|()| RestoreError::Permanent { range: true }),
                [(); 3].map(|()| RestoreError::Permanent { range: false }),
                [
                    RestoreError::CallFailed,
                    RestoreError::CallFailed,
                    RestoreError::CallFailed
                ],
                [
                    RestoreError::Corrupt,
                    RestoreError::Destination(io::ErrorKind::StorageFull),
                    RestoreError::Destination(io::ErrorKind::StorageFull),
                ],
                [
                    RestoreError::Corrupt,
                    RestoreError::Destination(io::ErrorKind::StorageFull),
                    RestoreError::Destination(io::ErrorKind::StorageFull),
                ],
                [
                    RestoreError::Corrupt,
                    RestoreError::Destination(io::ErrorKind::PermissionDenied),
                    RestoreError::Destination(io::ErrorKind::PermissionDenied),
                ],
                [
                    RestoreError::Corrupt,
                    RestoreError::CheckFailed,
                    RestoreError::Corrupt
                ],
            ]
        );
    }

    /// The error below the rustic error of a restore that cannot set an extended attribute, as the
    /// fork gives it on ext4 for a user attribute of 6,000 bytes.
    #[derive(Debug)]
    struct SettingXattrFailed(io::Error);

    impl std::fmt::Display for SettingXattrFailed {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                formatter,
                "setting xattr `user.golem-test` on `\"/restore/file.txt\"` with `{:?}`",
                self.0
            )
        }
    }

    impl std::error::Error for SettingXattrFailed {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    const PACK: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
}

mod missing_paths {
    use super::super::is_pack_missing;
    use crate::filesystem_snapshot::backend::rustic::failure_classification::{
        is_index_missing, missing_file_path,
    };
    use crate::filesystem_snapshot::backend::rustic::test_support::mapping_missing as missing;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use test_r::test;
    #[test]
    fn a_missing_file_gives_its_path_and_its_directory() {
        let index = missing("index/ab12");
        let pack = missing("data/ab/ab12");

        assert_eq!(
            (
                missing_file_path(index.as_ref()),
                is_index_missing(index.as_ref()),
                is_pack_missing(index.as_ref()),
                missing_file_path(pack.as_ref()),
                is_index_missing(pack.as_ref()),
                is_pack_missing(pack.as_ref()),
                missing_file_path(anyhow::anyhow!("other").as_ref()),
            ),
            (
                Some(Path::new("index/ab12")),
                true,
                false,
                Some(Path::new("data/ab/ab12")),
                false,
                true,
                None,
            )
        );
    }
}
