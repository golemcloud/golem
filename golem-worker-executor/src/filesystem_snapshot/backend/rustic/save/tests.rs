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

use super::{
    RunLate, SaveExit, backup_options, ran_at_shutdown, save_failure, save_ran, settle_may_land,
    store_backup_options, whole_millis_from,
};
use crate::filesystem_snapshot::backend::rustic::blob_io::LateWrites;
use crate::filesystem_snapshot::backend::rustic::call_execution::{
    OwnFile, Ran, RunEnd, Settle, Settled,
};
use crate::filesystem_snapshot::backend::rustic::failure_classification::OperationCancelled;
use crate::filesystem_snapshot::backend::rustic::pruning::PruneSettings;
use crate::filesystem_snapshot::backend::rustic::publication::PublishBound;
use crate::filesystem_snapshot::backend::rustic::repository::open_existing;
use crate::filesystem_snapshot::backend::rustic::snapshot_catalog::repository_snapshots;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::holding::holding_storage;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    LIMIT, LONG_DEADLINE, NEVER, REACH_LIMIT, SHORT_DEADLINE, SHORT_WRITE_DEADLINE, SharedSlots,
    backend_of, blobs, calls_of,
};
use std::path::PathBuf;

use crate::filesystem_snapshot::backend::rustic::test_support::{
    drop_when, dropped_within_limit, eventually, failed_at_deadline, fixture_tree,
    holding_index_reads, is_failed, key, ledger, listed_names, mapping_failed_call,
    mapping_local_io_without_source, mapping_missing, mapping_refused, mapping_rustic, name,
    one_file_tree, policy, polled_until, refusing, repository_fixture_tree, restored_listing,
    runs_policy, save_each, save_numbered, scope_with_a_prune_to_come, scripted_first_pack,
    snapshot_files, store, store_over, store_with_deadline, stored_paths,
};

use crate::filesystem_snapshot::backend::rustic::test_support::with_existing_repository;

use crate::filesystem_snapshot::test_support::fixture::{Scratch, Spec, listing, write_tree};
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    CallError, ChangeDetection, ChangeDetection as StoreChangeDetection, FilesystemSnapshotStore,
    RestoreFailure, RunSlots, SaveError, Slot, SnapshotInfo, Unlimited, Withdrawal,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use anyhow::Context;
use futures::StreamExt;
use futures::future::BoxFuture;
use golem_common::model::RetryConfig;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobNameError, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use rustic_core::repofile::SnapshotId;
use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

#[test]
fn the_off_side_of_each_save_setting_goes_into_its_rustic_option() {
    assert_eq!(backup_options(None).threads, None);
}

#[test]
fn each_save_setting_goes_into_its_rustic_option() {
    let backup = backup_options(NonZeroUsize::new(2));
    assert_eq!(
        (
            backup.threads,
            backup.parent_opts.group_by.is_some(),
            backup.as_path
        ),
        (NonZeroUsize::new(2), true, Some(PathBuf::from("/")))
    );
}

#[test]
fn backup_options_are_strict_and_use_the_save_thread_count() {
    let settings = StorePolicy::from_config(
        &crate::filesystem_snapshot::backend::rustic::test_support::config(),
    )
    .save_settings();
    let backup = store_backup_options(&settings, None);
    assert_eq!(
        (
            backup.fail_on_read_error,
            backup.threads.map(NonZeroUsize::get),
            backup.as_path.as_deref().map(Path::to_path_buf)
        ),
        (true, Some(3), Some(Path::new("/").to_path_buf()))
    );
}

#[test]
fn a_size_and_mtime_save_names_its_parent_and_ignores_the_change_time_and_a_full_save_forces_a_read()
 {
    let settings = StorePolicy::from_config(
        &crate::filesystem_snapshot::backend::rustic::test_support::config(),
    )
    .save_settings();
    let parent = SnapshotId::default();
    let modes = [
        Some((parent, ChangeDetection::SizeMtime)),
        Some((parent, ChangeDetection::Full)),
        None,
    ]
    .map(|mode| {
        let options = store_backup_options(&settings, mode).parent_opts;
        (
            options.parents,
            options.ignore_ctime,
            options.ignore_inode,
            options.force,
        )
    });
    assert_eq!(
        modes,
        [
            (vec![parent.to_hex().to_string()], true, false, false),
            (Vec::new(), false, false, true),
            (Vec::new(), false, false, true)
        ]
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_run_whose_pack_try_was_lost_waits_for_it_and_runs_again_without_a_check_of_its_name()
 {
    // The first pack try loses its answer and its next try writes the pack. Each try of the first
    // index write is refused, so the backup fails before the publish. The listing of the check
    // of the own name would fail.
    let deadline = Duration::from_secs(4);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let index_writes = AtomicUsize::new(0);
    let storage = scripted_first_pack(
        inner.clone(),
        Script::LandAfter(Duration::from_secs(1)),
        move |op_label, path| match op_label {
            "write"
                if path.starts_with("index") && index_writes.fetch_add(1, Ordering::SeqCst) < 3 =>
            {
                Script::Refuse
            }
            "check_name" => Script::Refuse,
            _ => Script::Pass,
        },
    );
    let store = store(storage.clone(), runs_policy(deadline, 3));
    let started = std::time::Instant::now();
    let saved = store
        .save(
            &new_scope(),
            &name("p-own-name"),
            fixture_tree().path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let took = started.elapsed();
    assert!(saved.is_ok(), "{saved:?}");
    assert!(took >= deadline, "the second run started after {took:?}");
    assert_eq!(calls_of(&storage, "check_name"), 0);
}

#[test]
#[timeout("120s")]
async fn a_save_whose_pack_write_gets_no_answer_fails_with_no_snapshot_and_its_threads_stop() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let (storage, _gate, dropped) = holding_storage(inner.clone(), |op_label, path| {
        op_label == "write" && path.starts_with("data")
    });
    let store_with_deadline = store_with_deadline(storage, SHORT_DEADLINE);
    let tree = repository_fixture_tree();

    // The held pack write never answers, so only the cut of its try at the deadline lets the save
    // answer. The limit only tells a save that waits for the write from one that answers.
    let saved = tokio::time::timeout(
        REACH_LIMIT,
        store_with_deadline.save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        ),
    )
    .await;
    drop(store_with_deadline);
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
async fn a_save_cancelled_before_its_publish_publishes_nothing() {
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
    // The second save of the same tree deduplicates all its content. The third call on the
    // snapshot files is the listing after its backup, and it waits until the cancel ends it.
    let calls = Arc::new(AtomicUsize::new(0));
    let held = Arc::new(Notify::new());
    let (storage, _gate, _dropped) = holding_storage(inner.clone(), {
        let (calls, held) = (calls.clone(), held.clone());
        move |_, path| {
            let selected =
                path.starts_with("snapshots") && calls.fetch_add(1, Ordering::SeqCst) == 2;
            if selected {
                held.notify_one();
            }
            selected
        }
    });
    let store_with_deadline = store_with_deadline(storage, STORAGE_CALL_DEADLINE);
    let cancel = CancellationToken::new();

    let second = name("p-second");
    let saving = store_with_deadline.save(&scope, &second, tree.path(), None, &cancel, &Unlimited);
    let cancelling = async {
        held.notified().await;
        cancel.cancel();
    };
    let (saved, ()) = tokio::join!(saving, cancelling);
    let listed = store_with_deadline
        .list(&scope, &Unlimited)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<Vec<_>>();
    let snapshot_files = stored_paths(&inner, &scope)
        .await
        .into_iter()
        .filter(|path| path.starts_with("snapshots/"))
        .count();

    assert!(
        matches!(saved, Err(SaveError::Stopped(Withdrawal::Stopped))),
        "{saved:?}"
    );
    assert_eq!((listed, snapshot_files), (vec!["p-first".to_string()], 1));
}

#[test]
#[timeout("120s")]
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
    let store_with_deadline = store_with_deadline(storage, Duration::from_secs(2));
    let tree = repository_fixture_tree();
    let into = Scratch::new();
    let opener = tokio::spawn(async move {
        held.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(gate);
    });

    let saved = tokio::time::timeout(
        REACH_LIMIT,
        store_with_deadline.save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        ),
    )
    .await
    .map(|result| result.map(|_| ()).map_err(|error| format!("{error:#}")));
    let opened = tokio::time::timeout(REACH_LIMIT, opener)
        .await
        .is_ok_and(|joined| joined.is_ok());
    let restored = store_with_deadline
        .restore(&scope, &name("p-first"), into.path(), &Unlimited)
        .await;

    assert_eq!(
        (saved, opened, restored.is_ok(), listing(into.path())),
        (Ok(Ok(())), true, true, listing(tree.path()))
    );
}

#[cfg(unix)]
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
        let store_with_deadline = store_with_deadline(storage.clone(), STORAGE_CALL_DEADLINE);
        store_with_deadline
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
        rewrite(tree.path());
        store_with_deadline
            .save(
                &scope,
                &name("p-second"),
                tree.path(),
                Some((&name("p-first"), detection)),
                never_cancelled(),
                &Unlimited,
            )
            .await
            .unwrap();
        let into = Scratch::new();
        store_with_deadline
            .restore(&scope, &name("p-second"), into.path(), &Unlimited)
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
#[timeout("120s")]
async fn a_save_reads_all_snapshot_files_once_and_then_only_the_new_ones() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_numbered(
        &store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE),
        &scope,
        0..5,
    )
    .await;
    let counting = Arc::new(
        crate::filesystem_snapshot::backend::rustic::test_support::counting::CountingBlobStorage::new(
            inner,
            Duration::ZERO,
        ),
    );
    let store_with_deadline = store_with_deadline(counting.clone(), STORAGE_CALL_DEADLINE);

    save_numbered(&store_with_deadline, &scope, 5..6).await;

    assert_eq!(
        (
            counting.count("read", "snapshots"),
            counting.count("list", "snapshots")
        ),
        (5, 2)
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
#[timeout("60s")]
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
        .save(
            &scope,
            &name("p-fd"),
            Path::new(&through_fd),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let backend = backend_of(storage, &scope, LONG_DEADLINE);
    let paths = tokio::task::spawn_blocking(move || {
        let repository = open_existing(backend.clone(), &key()).unwrap().unwrap();
        repository_snapshots(&repository, &backend)
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
#[timeout("60s")]
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
    let store = store(storage, policy(SHORT_WRITE_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();

    let failed = store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let stat = store.stat(&scope, &name("p-1")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let restore = restored_listing(&store, &scope, &name("p-1")).await;
    refuse.store(false, Ordering::SeqCst);
    let saved_again = store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(failed.as_ref().is_err_and(is_failed), "{failed:?}");
    assert!(
        matches!(restore, Err(RestoreFailure::NotFound)),
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
async fn a_save_whose_publish_landed_late_stays_saved() {
    // The publish reaches its deadline, and the storage writes the file without an answer. The
    // store waits one deadline after the try, finds its own file, and answers with the info. No
    // delete of the file is sent.
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

    let saved = store
        .save(
            &scope,
            &name("p-late"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    hang.store(false, Ordering::SeqCst);
    let stat = store.stat(&scope, &name("p-late")).await.unwrap();
    let names = listed_names(&store, &scope).await;
    let saved_again = store
        .save(
            &scope,
            &name("p-late"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert!(
        matches!(saved_again, Err(SaveError::NameInUse)),
        "{saved_again:?}"
    );
    assert_eq!(
        (
            stat,
            names,
            blobs(&*storage, scope.namespace(), "snapshots/")
                .await
                .len(),
            storage
                .calls()
                .iter()
                .filter(
                    |(op_label, path)| op_label.contains("delete") && path.starts_with("snapshots")
                )
                .count()
        ),
        (saved.ok(), vec!["p-late".to_string()], 1, 0)
    );
}

#[test]
#[timeout("60s")]
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
    let first = storage.calls().len();
    store
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

#[cfg(unix)]
#[test]
#[timeout("60s")]
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
        .save(
            &scope,
            &name("p-locked"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(
        matches!(&saved, Err(SaveError::Source(error)) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "{saved:?}"
    );
    assert_eq!(
        blobs(&*storage, scope.namespace(), "snapshots/").await,
        Vec::<String>::new()
    );
}

#[test]
#[timeout("60s")]
async fn a_save_dropped_at_any_storage_call_before_its_publish_publishes_nothing_and_leaves_the_name_free()
 {
    // The first save counts the calls of a save. Each later round holds one of these calls: the
    // call reaches the storage and never answers, as a write that S3 received and completes after
    // the caller left. The round drops the save there, and waits until the store has no work. A
    // save that is dropped in its publish is the one exception that the doc of `save` names: the
    // publish that the storage received lands, and the name resolves to the whole tree.
    let tree = one_file_tree("dropped");
    let other = one_file_tree("saved later");
    let counted =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    store(
        counted.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    )
    .save(
        &new_scope(),
        &name("p-dropped"),
        tree.path(),
        None,
        never_cancelled(),
        &Unlimited,
    )
    .await
    .unwrap();
    let labels = counted
        .calls()
        .into_iter()
        .map(|(op_label, _)| op_label)
        .collect::<Vec<_>>();
    let calls = labels.len();

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
                let dropping = store(storage.clone(), policy.clone());
                let scope = new_scope();
                let ended = drop_when(
                    &storage,
                    |calls| calls.len() >= held,
                    dropping.save(
                        &scope,
                        &name("p-dropped"),
                        &tree,
                        None,
                        never_cancelled(),
                        &Unlimited,
                    ),
                )
                .await;
                let stopped = tokio::time::timeout(LIMIT, dropping.shut_down())
                    .await
                    .is_ok();
                let later = store(inner, policy);
                let stat = later.stat(&scope, &name("p-dropped")).await.ok().flatten();
                let names = listed_names(&later, &scope).await;
                let saved_again = later
                    .save(
                        &scope,
                        &name("p-dropped"),
                        &other,
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await;
                let restored = restored_listing(&later, &scope, &name("p-dropped"))
                    .await
                    .ok();
                (
                    held,
                    ended.is_none(),
                    stopped,
                    stat.is_some(),
                    names,
                    saved_again.is_ok(),
                    restored.map(|restored| {
                        if restored == listing(&tree) {
                            "the dropped tree"
                        } else if restored == listing(&other) {
                            "the later tree"
                        } else {
                            "a part of a tree"
                        }
                    }),
                )
            }
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        rounds,
        (1..=calls)
            .map(|held| match labels[held - 1] {
                "publish" => (
                    held,
                    true,
                    true,
                    true,
                    vec!["p-dropped".to_string()],
                    false,
                    Some("the dropped tree")
                ),
                _ => (
                    held,
                    true,
                    true,
                    false,
                    Vec::new(),
                    true,
                    Some("the later tree")
                ),
            })
            .collect::<Vec<(usize, bool, bool, bool, Vec<String>, bool, Option<&str>)>>()
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_held_at_its_storage_call_ends_at_shut_down_and_the_save_answers_stopped() {
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
    let saving = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = tree.path().to_path_buf();
        async move {
            store
                .save(
                    &scope,
                    &name("p-held"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "publish")
    })
    .await;

    // The shutdown ends the held publish, so it returns without a wait for the gate.
    let shut_down = tokio::time::timeout(Duration::from_millis(500), store.shut_down())
        .await
        .is_ok();
    let saved = tokio::time::timeout(LIMIT, saving).await;
    storage.open_gate();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let published = blobs(&*storage, scope.namespace(), "snapshots/").await;

    assert!(
        matches!(&saved, Ok(Ok(Err(SaveError::Stopped(_))))),
        "{saved:?}"
    );
    assert_eq!(
        (held, shut_down, published),
        (true, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
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
    #[cfg(unix)]
    wait_past_change_times(&entries(tree.path()));
    copy_flat_tree(tree.path(), copy.path());

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
    store
        .save(
            &scope,
            &name("p-2"),
            copy.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
            never_cancelled(),
            &Unlimited,
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
#[timeout("60s")]
async fn a_full_save_reads_each_file() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

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
    store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::Full)),
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let files = snapshot_files(storage, &scope).await;

    assert_eq!(read_counts(&files, "p-2"), Some(((3, 0, 0), None)));
}

#[test]
#[timeout("60s")]
async fn the_parent_of_a_save_is_the_named_snapshot_also_when_a_newer_snapshot_exists() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

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
    store
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
    store
        .save(
            &scope,
            &name("p-3"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
            never_cancelled(),
            &Unlimited,
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
#[timeout("60s")]
async fn a_save_without_a_parent_or_with_a_parent_that_the_scope_does_not_hold_reads_each_file() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = three_file_tree();

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
    store
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
    store
        .save(
            &scope,
            &name("p-3"),
            tree.path(),
            Some((&name("p-missing"), ChangeDetection::SizeMtime)),
            never_cancelled(),
            &Unlimited,
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
#[timeout("60s")]
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
    let before = storage.calls().len();

    let saved = store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let publishes = storage.calls()[before..]
        .iter()
        .filter(|(op_label, _)| *op_label == "publish")
        .count();
    refuse.store(false, Ordering::SeqCst);

    assert!(saved.as_ref().is_err_and(is_failed), "{saved:?}");
    assert_eq!(
        (publishes, listed_names(&store, &scope).await),
        (0, vec!["p-1".to_string()])
    );
}

#[test]
#[timeout("60s")]
async fn a_save_of_a_relative_directory_path_gives_source_and_writes_nothing() {
    // Cargo runs the tests in the directory of the crate, so the path names a directory.
    let relative = Path::new("src/filesystem_snapshot/test_support");
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
        .save(
            &scope,
            &name("p-relative"),
            relative,
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert_eq!(
        blobs(&*storage, scope.namespace(), "").await,
        Vec::<String>::new()
    );
}

#[test]
#[timeout("60s")]
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
        .save(
            &scope,
            &name("p-file"),
            &tree.path().join("file.txt"),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert_eq!(
        blobs(&*storage, scope.namespace(), "").await,
        Vec::<String>::new()
    );
}

#[test]
fn a_save_run_waits_for_its_latest_late_try_and_checks_its_name_only_after_a_lost_publish_try() {
    let base = std::time::Instant::now();
    let second = Duration::from_secs(1);
    let deadline = Duration::from_secs(10);
    let lates = [
        RunLate::default(),
        RunLate {
            backend: Some(base + second),
            publish: None,
        },
        RunLate {
            backend: None,
            publish: Some(base + second),
        },
        RunLate {
            backend: Some(base + 2 * second),
            publish: Some(base + second),
        },
    ];

    let shapes = lates
        .iter()
        .flat_map(|late| {
            exits()
                .into_iter()
                .map(move |exit| ran_shape(save_ran(exit, *late, deadline), base))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        shapes,
        [
            "answered true",
            "ended CallFailed",
            "ended CallFailed",
            "answered true after 11s",
            "ended CallFailed after 11s own false",
            "ended CallFailed after 11s own false",
            "answered true after 11s",
            "ended CallFailed after 11s own false",
            "ended CallFailed after 11s own true",
            "answered true after 12s",
            "ended CallFailed after 12s own false",
            "ended CallFailed after 12s own true",
        ]
    );
}

#[test]
fn a_shutdown_in_the_settle_keeps_an_answer_and_cancels_every_other_run() {
    let base = std::time::Instant::now();

    assert_eq!(
        [
            SaveExit::Answered(Ok(own_file().info)),
            SaveExit::Answered(Err(SaveError::NameInUse)),
            SaveExit::Ended {
                end: RunEnd::CallFailed,
                failure: anyhow::anyhow!("failed"),
                staged: Some(own_file()),
            },
        ]
        .map(|exit| ran_shape(ran_at_shutdown(exit), base)),
        ["answered true", "answered false", "ended Cancelled"]
    );
}

#[test]
#[timeout("60s")]
async fn a_run_that_fails_frees_its_slot_its_pool_and_its_index_and_a_new_run_gives_the_answer() {
    let storage = refusing(1, |op_label, path| {
        op_label == "write" && path.starts_with("data")
    });
    let store = store(storage.clone(), runs_policy(SHORT_WRITE_DEADLINE, 1));
    let scope = new_scope();
    let tree = fixture_tree();
    let slots = SharedSlots::new(1);

    let saved = store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &slots,
        )
        .await;
    let freed = eventually(|| store.work_in_flight() == 0).await;
    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (slots.counts(), freed, restored.ok()),
        ((2, 0, 1, 1), true, Some(listing(tree.path())))
    );
}

#[test]
#[timeout("60s")]
async fn a_call_that_the_storage_refuses_answers_failed_after_one_try_of_one_run() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner, |op_label, path| {
        if op_label == "list" && path.starts_with("snapshots") {
            Script::Deny
        } else {
            Script::Pass
        }
    });
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 3));
    let slots = SharedSlots::new(1);

    let listed = store.list(&scope, &slots).await;

    assert!(matches!(listed, Err(CallError::Failed(_))), "{listed:?}");
    assert_eq!(
        (
            slots.counts().0,
            storage
                .calls()
                .iter()
                .filter(|(op_label, path)| *op_label == "list" && path.starts_with("snapshots"))
                .count()
        ),
        (1, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_call_answers_failed_after_its_runs_and_each_run_tells_the_limiter_before_its_wait()
 {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner, |op_label, path| {
        if op_label == "list" && path.starts_with("snapshots") {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let slots = SharedSlots::new(1);

    let listed = store.list(&scope, &slots).await;

    assert!(matches!(listed, Err(CallError::Failed(_))), "{listed:?}");
    assert_eq!(slots.counts(), (3, 0, 1, 2));
}

#[test]
#[timeout("60s")]
async fn the_live_runs_never_pass_the_slots() {
    // Six saves and then six restores, each of its own agent, run at one time under a limiter of
    // two slots. The first write of a pack and the first read of a pack fail, so two calls run
    // again after a wait. The storage, not the limiter, judges: a run makes storage calls only for
    // its own agent, so the namespaces with a call in flight at one time are the runs that live.
    let refused_write = Arc::new(AtomicBool::new(false));
    let refused_read = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (refused_write, refused_read) = (refused_write.clone(), refused_read.clone());
        move |op_label, path| {
            let pack = path.starts_with("data");
            let refused = if op_label == "write" {
                &refused_write
            } else {
                &refused_read
            };
            if pack && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else if pack {
                Script::Delay(Duration::from_millis(20))
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), runs_policy(SHORT_WRITE_DEADLINE, 1));
    let slots = SharedSlots::new(2);
    let trees = (0..6).map(|_| one_file_tree("tree")).collect::<Vec<_>>();
    let scopes = (0..6).map(|_| new_scope()).collect::<Vec<_>>();

    let saved = futures::future::join_all(trees.iter().zip(&scopes).map(|(tree, scope)| {
        let (store, slots) = (&store, &slots);
        async move {
            store
                .save(
                    scope,
                    &name("p-1"),
                    tree.path(),
                    None,
                    never_cancelled(),
                    slots,
                )
                .await
        }
    }))
    .await;
    let restored = futures::future::join_all(scopes.iter().map(|scope| {
        let (store, slots) = (&store, &slots);
        async move {
            let into = Scratch::new();
            store
                .restore(scope, &name("p-1"), into.path(), slots)
                .await
                .map(|_| listing(into.path()))
        }
    }))
    .await;

    assert!(saved.iter().all(Result::is_ok), "{saved:?}");
    assert!(
        restored
            .iter()
            .all(|restored| matches!(restored, Ok(tree) if *tree == listing(trees[0].path()))),
        "{restored:?}"
    );
    assert_eq!(
        (
            refused_write.load(Ordering::SeqCst),
            refused_read.load(Ordering::SeqCst),
            storage.most_namespaces_at_once(),
            slots.counts().3,
        ),
        (true, true, 2, 2)
    );
}

#[test]
#[timeout("60s")]
async fn a_new_run_after_a_publish_began_answers_saved_when_the_publish_landed_late() {
    // The first try lands and loses its answer, and the later tries are refused, so the publish
    // ends without an answer. The check after the wait finds the own file.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |tried| match tried {
        1 => Script::LoseTheAnswer,
        _ => Script::Refuse,
    });
    let store = store(storage.clone(), late_policy());
    let scope = new_scope();
    let tree = fixture_tree();
    let slots = SharedSlots::new(1);

    let saved = store
        .save(
            &scope,
            &name("p-late"),
            tree.path(),
            None,
            never_cancelled(),
            &slots,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            slots.counts(),
            blobs(&*inner, scope.namespace(), "snapshots/").await.len(),
            restored_listing(&store, &scope, &name("p-late")).await.ok(),
        ),
        (3, 1, (1, 0, 1, 0), 1, Some(listing(tree.path())))
    );
}

#[test]
#[timeout("60s")]
async fn a_save_reports_its_wait_for_a_late_publish_and_a_clean_save_reports_nothing() {
    // The first publish try lands and loses its answer, and the later tries are refused, so the
    // call waits for the try before it checks its own name. The limiter hears of that wait, so the
    // service can replace the job of the call while it only waits.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |tried| match tried {
        1 => Script::LoseTheAnswer,
        _ => Script::Refuse,
    });
    let store = store(storage.clone(), late_policy());
    let (late_scope, clean_scope) = (new_scope(), new_scope());
    let tree = fixture_tree();
    let (late_slots, clean_slots) = (SharedSlots::new(1), SharedSlots::new(1));

    let late_saved = store
        .save(
            &late_scope,
            &name("p-late"),
            tree.path(),
            None,
            never_cancelled(),
            &late_slots,
        )
        .await;
    let clean_store = crate::filesystem_snapshot::backend::rustic::test_support::store(
        Arc::new(InMemoryBlobStorage::new()),
        late_policy(),
    );
    let clean_saved = clean_store
        .save(
            &clean_scope,
            &name("p-clean"),
            tree.path(),
            None,
            never_cancelled(),
            &clean_slots,
        )
        .await;

    assert!(late_saved.is_ok(), "{late_saved:?}");
    assert!(clean_saved.is_ok(), "{clean_saved:?}");
    // The settle of the run whose publish try was lost, then the wait before the check.
    assert_eq!((late_slots.late_waits(), clean_slots.late_waits()), (2, 0));
}

#[test]
#[timeout("60s")]
async fn a_new_run_after_a_publish_began_saves_again_when_it_never_landed() {
    // The tries of the first run are refused and land nothing. The check finds no file, so the
    // call waits after the failed run and runs again, and the second run publishes.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |tried| {
        if tried <= 3 {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store = store(storage.clone(), late_policy());
    let scope = new_scope();
    let tree = fixture_tree();
    let slots = SharedSlots::new(1);

    let saved = store
        .save(
            &scope,
            &name("p-again"),
            tree.path(),
            None,
            never_cancelled(),
            &slots,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            slots.counts(),
            blobs(&*inner, scope.namespace(), "snapshots/").await.len(),
            restored_listing(&store, &scope, &name("p-again"))
                .await
                .ok(),
        ),
        (4, 1, (2, 0, 1, 1), 1, Some(listing(tree.path())))
    );
}

#[test]
#[timeout("120s")]
async fn the_own_name_check_gives_name_in_use_for_another_file_with_the_label() {
    // The publish of the first save ends without an answer and lands nothing. During the wait,
    // another writer saves the same name, so the check finds a file with the name that is not
    // the own one. The listing of the check waits at the gate until the other save returned, so
    // the order does not depend on the speed of the host. The listing is cut one deadline after
    // it began, and it begins one deadline after the last publish try, so the other save has two
    // deadlines, 20 s, to save a tree of one file.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), |op_label, _| match op_label {
        "publish" => Script::Refuse,
        "check_name" => Script::WaitForGate,
        _ => Script::Pass,
    });
    let store = store(storage.clone(), runs_policy(Duration::from_secs(10), 3));
    let other = store_over(&inner);
    let scope = new_scope();
    let (tree, other_tree) = (fixture_tree(), one_file_tree("other writer"));

    let saving = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope, path) = (store.clone(), scope.clone(), tree.path().to_path_buf());
        async move {
            store
                .save(
                    &scope,
                    &name("p-shared"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let refused = polled_until(REACH_LIMIT, || calls_of(&storage, "publish") == 3).await;
    let other_saved = other
        .save(
            &scope,
            &name("p-shared"),
            other_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    storage.open_gate();
    let saved = tokio::time::timeout(REACH_LIMIT, saving).await;

    assert!(other_saved.is_ok(), "{other_saved:?}");
    assert!(
        matches!(saved, Ok(Ok(Err(SaveError::NameInUse)))),
        "{saved:?}"
    );
    assert_eq!(
        (
            refused,
            calls_of(&storage, "publish"),
            restored_listing(&other, &scope, &name("p-shared"))
                .await
                .ok()
        ),
        (true, 3, Some(listing(other_tree.path())))
    );
}

#[test]
#[timeout("60s")]
async fn a_lost_shard_cancel_after_the_publish_began_waits_and_checks() {
    // The first try lands and loses its answer, and the cancel of the save fires before the next
    // try. The call waits for the write and checks the own name before it answers.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let cancel = CancellationToken::new();
    let storage = scripted_publish(inner.clone(), {
        let cancel = cancel.clone();
        move |tried| {
            if tried == 1 {
                cancel.cancel();
                Script::LoseTheAnswer
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), late_policy());
    let scope = new_scope();
    let tree = fixture_tree();

    let saved = store
        .save(
            &scope,
            &name("p-cancelled"),
            tree.path(),
            None,
            &cancel,
            &Unlimited,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            blobs(&*inner, scope.namespace(), "snapshots/").await.len()
        ),
        (1, 1, 1)
    );
}

#[test]
#[timeout("120s")]
async fn a_shutdown_during_the_wait_for_a_late_publish_answers_stopped_at_once() {
    // The publish ends without an answer, and the wait for it lasts one deadline of a minute. The
    // shutdown ends the wait at once: the save answers and the shutdown returns within half the
    // deadline, which a wait that the shutdown did not end cannot do.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |_| Script::Refuse);
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let scope = new_scope();
    let tree = fixture_tree();
    let saving = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope, path) = (store.clone(), scope.clone(), tree.path().to_path_buf());
        async move {
            store
                .save(
                    &scope,
                    &name("p-waiting"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let refused = polled_until(REACH_LIMIT, || calls_of(&storage, "publish") == 1).await;

    let stopped = tokio::time::timeout(LONG_DEADLINE / 2, store.shut_down())
        .await
        .is_ok();
    let saved = tokio::time::timeout(LONG_DEADLINE / 2, saving).await;

    assert!(
        matches!(saved, Ok(Ok(Err(SaveError::Stopped(Withdrawal::Stopped))))),
        "{saved:?}"
    );
    assert_eq!(
        (refused, stopped, calls_of(&storage, "check_name")),
        (true, true, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_save_never_publishes_two_files_with_its_label() {
    // Each of three runs loses the answer of its first try and its later tries are refused. The
    // first check finds the file of the first run.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |tried| match tried % 3 {
        1 => Script::LoseTheAnswer,
        _ => Script::Refuse,
    });
    let store = store(storage.clone(), late_policy());
    let scope = new_scope();
    let tree = fixture_tree();

    let saved = store
        .save(
            &scope,
            &name("p-once"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        blobs(&*inner, scope.namespace(), "snapshots/").await.len(),
        1
    );
}

#[test]
#[timeout("60s")]
async fn a_save_whose_backup_passes_its_end_answers_failed_and_publishes_nothing() {
    // A grace period of 5.7 s and a deadline of 1 s give a refresh period of about 1.175 s, the
    // earliest index read of a prune about 2.525 s after the first slot, and the end of the
    // backup two deadlines before it, at about 0.525 s. Each pack write takes 0.8 s, so the backup
    // does not end in time.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path.starts_with("data") {
                Script::Delay(Duration::from_millis(800))
            } else {
                Script::Pass
            }
        });
    let store = store(
        storage.clone(),
        StorePolicy {
            publish_bound: PublishBound::On,
            ..policy(Duration::from_secs(1), NEVER, Duration::from_millis(5700))
        },
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let started = std::time::Instant::now();

    let saved = store
        .save(
            &scope,
            &name("p-slow"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let answer = std::time::Instant::now();
    let answered = answer.duration_since(started);
    // Read before the test lists the blobs through the same storage.
    let started_after_the_answer = storage
        .events()
        .iter()
        .filter(|event| event.started > answer)
        .count();

    assert!(
        matches!(
            &saved,
            Err(SaveError::Failed(failed))
                if failed.cause().to_string().contains("longer than the filesystem snapshot store allows")
        ),
        "{saved:?}"
    );
    // The save answers no earlier than the end of its backup. How long the backup takes to stop
    // after the cut depends on the speed of the host, so the test does not bound the answer from
    // above; the save whose pack write is held past the end of its backup shows that the cut ends
    // the backup. No blob call of the save starts after its answer.
    assert!(answered >= Duration::from_millis(525), "{answered:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            blobs(&*storage, scope.namespace(), "snapshots/").await,
            started_after_the_answer
        ),
        (0, Vec::<String>::new(), 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_save_whose_publish_lands_after_its_try_answers_with_its_own_file_and_stays_saved() {
    // The publish gets no answer, and the file lands six tenths of a deadline after the try. The
    // store waits one deadline after the try, finds its own file, and answers with the info. Two
    // deadlines after the try the name still resolves, and no delete of a snapshot file was sent.
    let deadline = Duration::from_secs(1);
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
            if op_label == "publish" {
                Script::LandAfter(deadline * 6 / 10)
            } else {
                Script::Pass
            }
        });
    let store = store(storage.clone(), runs_policy(deadline, 1));
    let scope = new_scope();
    let tree = one_file_tree("landed late");

    let saved = store
        .save(
            &scope,
            &name("p-late"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let tried = storage
        .events()
        .iter()
        .find(|event| event.op_label == "publish" && !event.landed_late)
        .map(|event| event.ended)
        .unwrap();
    let answered = crate::filesystem_snapshot::backend::rustic::call_execution::now();
    tokio::time::sleep_until(tokio::time::Instant::from_std(tried + deadline * 2)).await;
    let restored = restored_listing(&store, &scope, &name("p-late")).await;

    assert!(saved.is_ok(), "{saved:?}");
    assert!(answered >= tried + deadline, "{:?}", answered - tried);
    assert_eq!(
        (
            restored.ok(),
            calls_of(&storage, "publish"),
            storage
                .calls()
                .iter()
                .filter(
                    |(op_label, path)| op_label.contains("delete") && path.starts_with("snapshots")
                )
                .count()
        ),
        (Some(listing(tree.path())), 1, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_withdrawal_during_the_wait_after_a_failed_run_ends_the_call_with_stopped() {
    // Each listing of the snapshot files is refused, and the wait after a failed run is 10 s. The
    // limiter withdraws the call 100 ms into the first wait.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner, |op_label, path| {
        if op_label == "list" && path.starts_with("snapshots") {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            retry: RetryConfig {
                max_attempts: 3,
                min_delay: Duration::from_secs(10),
                max_delay: Duration::from_secs(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            },
            ..runs_policy(LONG_DEADLINE, 1)
        },
    );
    let slots = WithdrawnSlots {
        withdraw: CancellationToken::new(),
        failed_waits: AtomicUsize::new(0),
    };
    let withdraw = slots.withdraw.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        withdraw.cancel();
    });
    let started = std::time::Instant::now();

    let listed = store.list(&scope, &slots).await;

    assert!(
        matches!(listed, Err(CallError::Stopped(Withdrawal::Stopped))),
        "{listed:?}"
    );
    assert_eq!(
        (
            slots.failed_waits.load(Ordering::SeqCst),
            calls_of(&storage, "list") >= 1,
            started.elapsed() < Duration::from_secs(5)
        ),
        (1, true, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_save_with_two_lost_publishes_checks_its_own_name_after_each_and_saves_in_its_third_run()
{
    // The first two publishes are refused, so each could still land; neither does. After each, the
    // call waits one deadline and finds no own file, and the next run publishes again.
    let deadline = Duration::from_millis(300);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner, |tried| match tried {
        1 | 2 => Script::Refuse,
        _ => Script::Pass,
    });
    let store = store(storage.clone(), runs_policy(deadline, 1));
    let scope = new_scope();
    let tree = one_file_tree("third run");

    let saved = store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let publishes = storage
        .events()
        .iter()
        .filter(|event| event.op_label == "publish")
        .map(|event| (event.started, event.ended))
        .collect::<Vec<_>>();
    let gaps = publishes
        .windows(2)
        .map(|pair| pair[1].0.duration_since(pair[0].1) >= deadline)
        .collect::<Vec<_>>();

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            gaps,
            restored_listing(&store, &scope, &name("p-1")).await.ok()
        ),
        (3, 2, vec![true, true], Some(listing(tree.path())))
    );
}

#[test]
#[timeout("240s")]
async fn a_save_whose_pack_write_is_held_past_the_end_of_its_backup_answers_failed_while_the_write_is_held()
 {
    // The storage holds each pack write at its gate until the test opens it, which is after the
    // save answered. A deadline of 40 s and a grace period of 220 s put the end of the backup 15 s
    // after the first slot: late enough that the backup reaches its first pack write also on a
    // busy host, and before half the deadline of a pack write that started after that slot. The
    // cut of the run at the end of its backup ends a held write; without it, the write would end
    // only at its own deadline. So each held write ends within half a deadline of its start,
    // whatever the speed of the host. The save answers one deadline after the cut, when the cut
    // write can no longer land.
    let deadline = Duration::from_secs(40);
    let grace = Duration::from_secs(220);
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
        StorePolicy {
            publish_bound: PublishBound::On,
            ..policy(deadline, NEVER, grace)
        },
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let started = std::time::Instant::now();
    let backup_end = crate::filesystem_snapshot::backend::rustic::publication::backup_end(
        crate::filesystem_snapshot::backend::rustic::publication::index_read_bound(
            started,
            grace,
            deadline,
            crate::filesystem_snapshot::backend::rustic::pruning::timing::refresh_period(
                grace, deadline,
            ),
        ),
        deadline,
    )
    .duration_since(started);

    let saved = store
        .save(
            &scope,
            &name("p-held"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let answered = started.elapsed();
    let held = storage
        .events()
        .iter()
        .filter(|event| event.op_label == "write" && event.path.starts_with("data"))
        .map(|event| event.ended.duration_since(event.started))
        .collect::<Vec<_>>();
    storage.open_gate();

    assert!(
        matches!(
            &saved,
            Err(SaveError::Failed(failed))
                if failed.cause().to_string().contains("longer than the filesystem snapshot store allows")
        ),
        "{saved:?}"
    );
    assert!(answered >= backup_end, "{answered:?} before {backup_end:?}");
    assert!(
        !held.is_empty() && held.iter().all(|took| *took < deadline / 2),
        "the held pack writes took {held:?}"
    );
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            blobs(&*storage, scope.namespace(), "snapshots/").await
        ),
        (0, Vec::<String>::new())
    );
}

#[test]
#[timeout("120s")]
async fn a_save_whose_first_pack_write_fails_while_the_backup_goes_on_runs_again_and_saves() {
    // The tree fills two data packs. The storage refuses the write of the first pack, while the
    // archiver still chunks the rest of the tree. The backup gives the error of the storage, so
    // the run ends as a failed storage call, and the next run saves the tree.
    let tree = Scratch::new();
    let mut content = vec![0u8; 40 << 20];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut content);
    write_tree(
        tree.path(),
        &[(
            "a.bin",
            Spec::File {
                content: content.into_boxed_slice(),
                mode: 0o644,
            },
        )],
    );
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_first_pack(inner, Script::Refuse, |_, _| Script::Pass);
    let store = store(storage.clone(), runs_policy(SHORT_WRITE_DEADLINE, 1));
    let scope = new_scope();
    let slots = SharedSlots::new(1);

    let saved = store
        .save(
            &scope,
            &name("p-packs"),
            tree.path(),
            None,
            never_cancelled(),
            &slots,
        )
        .await;
    // The second run starts with a look at the config after the refused write. It writes each
    // pack of the tree: two data packs and a tree pack. So the first pack went out while the
    // archiver still read the tree.
    let calls = storage.calls();
    let is_pack_write =
        |(op_label, path): &&(&str, String)| *op_label == "write" && path.starts_with("data");
    let refused = calls.iter().position(|call| is_pack_write(&call)).unwrap();
    let second_run = refused
        + calls[refused..]
            .iter()
            .position(|(op_label, path)| *op_label == "stat" && path == "config")
            .unwrap();
    let packs_of_the_second_run = calls[second_run..].iter().filter(is_pack_write).count();
    let restored = restored_listing(&store, &scope, &name("p-packs")).await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(slots.counts().0, 2);
    assert_eq!(restored.ok(), Some(listing(tree.path())));
    assert_eq!(packs_of_the_second_run, 3, "{calls:?}");
}

#[test]
#[timeout("300s")]
async fn a_save_that_meets_an_unreadable_file_after_a_lost_pack_try_answers_only_after_the_try() {
    // The first pack goes out while the archiver still chunks the rest of the file. Its try loses
    // its answer and lands 15 s later, and its next try writes it. The archiver then meets the
    // unreadable file, and the save answers `Source`.
    let Some(raced) = first_attempt_that_raced(Script::LandAfter(Duration::from_secs(15))).await
    else {
        return;
    };
    let (saved, left) = raced.expect("the archiver ended before the first pack in each attempt");

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert_eq!(left, Vec::<String>::new());
}

#[test]
#[timeout("300s")]
async fn a_save_whose_detached_pack_write_is_in_flight_when_the_backup_fails_answers_after_it() {
    // The first pack goes to the file writer, whose write never answers and lands 15 s after it
    // was sent. The archiver chunks the rest of the file meanwhile, meets the unreadable file and
    // returns while the write is in flight.
    let Some(raced) = first_attempt_that_raced(Script::HangThenLand(Duration::from_secs(15))).await
    else {
        return;
    };
    let (saved, left) = raced.expect("the archiver ended before the first pack in each attempt");

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert_eq!(left, Vec::<String>::new());
}

#[test]
#[timeout("300s")]
async fn a_save_whose_backup_meets_an_unreadable_file_answers_source_at_once() {
    // Every call passes. The last pack of the backup goes out after the run cancelled its token,
    // and that refused try sends nothing, so it holds the answer for no deadline. A refused try
    // that held the answer would hold it for one deadline after the try, so the answer would come
    // more than one deadline after the start. The deadline of 90 s is far above the time of the
    // backup, about 25 s when four copies of the tests run at once.
    let Some(tree) = large_tree_with_an_unreadable_file(16 << 20) else {
        return;
    };
    let deadline = Duration::from_secs(90);
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(storage, runs_policy(deadline, 3));
    let started = std::time::Instant::now();

    let saved = store
        .save(
            &new_scope(),
            &name("p-refused"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let took = started.elapsed();

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert!(took < deadline, "the save took {took:?}");
}

#[test]
#[timeout("120s")]
async fn no_blob_call_of_a_save_starts_after_it_answered() {
    // Each pack write never answers and lands later. The backup fails on the unreadable file.
    let Some(tree) = large_tree_with_an_unreadable_file(16 << 20) else {
        return;
    };
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path.starts_with("data") {
                Script::HangThenLand(Duration::from_millis(500))
            } else {
                Script::Pass
            }
        });
    let store = store(storage.clone(), runs_policy(Duration::from_secs(2), 1));

    let _ = store
        .save(
            &new_scope(),
            &name("p-quiet"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let at_the_answer = storage.calls().len();
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(storage.calls().len(), at_the_answer);
}

#[test]
#[timeout("60s")]
async fn a_save_reports_its_waits_for_a_lost_pack_try_in_the_settle_and_before_the_next_step() {
    let saved_after_a_lost_pack = {
        let storage = scripted_first_pack(
            Arc::new(InMemoryBlobStorage::new()),
            Script::LandAfter(Duration::from_millis(200)),
            |_, _| Script::Pass,
        );
        let store = store(storage, runs_policy(Duration::from_millis(500), 3));
        let slots = SharedSlots::new(1);
        let saved = store
            .save(
                &new_scope(),
                &name("p-reported"),
                fixture_tree().path(),
                None,
                never_cancelled(),
                &slots,
            )
            .await;
        (saved.is_ok(), slots.late_waits(), slots.counts().3)
    };
    let failed_after_a_lost_pack = {
        let index_writes = AtomicUsize::new(0);
        let storage = scripted_first_pack(
            Arc::new(InMemoryBlobStorage::new()),
            Script::LandAfter(Duration::from_millis(200)),
            move |op_label, path| {
                if op_label == "write"
                    && path.starts_with("index")
                    && index_writes.fetch_add(1, Ordering::SeqCst) < 3
                {
                    Script::Refuse
                } else {
                    Script::Pass
                }
            },
        );
        let store = store(storage, runs_policy(Duration::from_millis(500), 3));
        let slots = SharedSlots::new(1);
        let saved = store
            .save(
                &new_scope(),
                &name("p-reported-failure"),
                fixture_tree().path(),
                None,
                never_cancelled(),
                &slots,
            )
            .await;
        (saved.is_ok(), slots.late_waits(), slots.counts().3)
    };

    // The settle and the wait of row 0 after a success; the settle and the wait of row 3 after a
    // failed run, then the wait after the failure before the next run.
    assert_eq!(
        (saved_after_a_lost_pack, failed_after_a_lost_pack),
        ((true, 2, 0), (true, 2, 1))
    );
}

#[test]
#[timeout("60s")]
async fn the_settle_of_a_save_reads_its_late_writes_only_after_every_holder_of_its_blobs_dropped() {
    // A holder of the blobs of the run records a late try 200 ms after the settle began, and then
    // drops its token.
    let deadline = Duration::from_secs(2);
    let run_calls = tokio_util::task::TaskTracker::new();
    let backend_late = Arc::new(LateWrites::default());
    let holder = {
        let (token, late) = (run_calls.token(), backend_late.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let ended = crate::filesystem_snapshot::backend::rustic::call_execution::now();
            late.record(ended);
            drop(token);
            ended
        })
    };
    let Settle { wait, finish, .. } =
        crate::filesystem_snapshot::backend::rustic::save::save_settle(
            new_scope(),
            name("p-00000000-0000-4000-8000-0000000000ab"),
            crate::filesystem_snapshot::backend::rustic::save::SaveExit::Ended {
                end: RunEnd::CallFailed,
                failure: anyhow::anyhow!("the backup failed"),
                staged: None,
            },
            true,
            run_calls,
            backend_late,
            Arc::new(LateWrites::default()),
            deadline,
        );

    wait.await;
    // The run is built before the test waits for the holder, as the shell builds it after the wait.
    let until = match finish(Settled::Waited) {
        Ran::Ended(ended) => ended.late.map(|late| late.until),
        _ => None,
    };
    let recorded = holder.await.unwrap();

    assert_eq!(until, Some(recorded + deadline));
}

#[test]
#[timeout("60s")]
async fn the_settle_of_a_save_waits_for_a_holder_of_its_blobs_past_one_deadline_and_warns_with_its_name()
 {
    use tracing::instrument::WithSubscriber;

    let run_calls = tokio_util::task::TaskTracker::new();
    let _held = run_calls.token();
    let saved = name("p-00000000-0000-4000-8000-0000000000aa");
    let Settle { wait, .. } = crate::filesystem_snapshot::backend::rustic::save::save_settle(
        new_scope(),
        saved.clone(),
        crate::filesystem_snapshot::backend::rustic::save::SaveExit::Answered(Err(
            SaveError::NameInUse,
        )),
        true,
        run_calls,
        Arc::new(LateWrites::default()),
        Arc::new(LateWrites::default()),
        Duration::from_millis(100),
    );
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();

    let waited =
        tokio::time::timeout(Duration::from_millis(500), wait.with_subscriber(subscriber)).await;
    let logged = captured.text();

    assert_eq!(
        (
            waited.is_err(),
            logged.contains(
                "A save of a filesystem snapshot still waits for the threads of its backup"
            ),
            logged.contains(&format!("snapshot={saved}")),
        ),
        (true, true, true),
        "{logged}"
    );
}

#[test]
fn a_save_error_ends_the_run_by_its_storage_and_answers_source_for_a_local_io_error() {
    assert_eq!(
        [
            mapping_failed_call(anyhow::anyhow!("the bucket gave 503")),
            mapping_failed_call(anyhow::Error::new(OperationCancelled)),
            mapping_failed_call(anyhow::Error::new(BlobNameError::NotUtf8 {
                path: "x".into()
            })),
            mapping_missing("index/0123"),
            mapping_rustic(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "a file of the tree"
            )),
            mapping_local_io_without_source(),
            mapping_refused(),
        ]
        .map(|error| mapping_save_shape(save_failure(error))),
        [
            format!("ended {:?}", RunEnd::CallFailed),
            format!("ended {:?}", RunEnd::Cancelled),
            format!("ended {:?}", RunEnd::Permanent),
            format!("ended {:?}", RunEnd::CallFailed),
            format!("source {:?}", io::ErrorKind::PermissionDenied),
            format!("source {:?}", io::ErrorKind::PermissionDenied),
            format!("ended {:?}", RunEnd::Permanent),
        ]
    );
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

/// What a run of a save gave, with its instants as their distance after `base`.
fn ran_shape(ran: Ran<Result<SnapshotInfo, SaveError>>, base: std::time::Instant) -> String {
    let after = |until: std::time::Instant| until.saturating_duration_since(base);
    match ran {
        Ran::Answered(answer) => format!("answered {}", answer.is_ok()),
        Ran::AnsweredAfter(answer, late) => {
            format!("answered {} after {:?}", answer.is_ok(), after(late.until))
        }
        Ran::Ended(ended) => match ended.late {
            Some(late) => format!(
                "ended {:?} after {:?} own {}",
                ended.end,
                after(late.until),
                late.own.is_some()
            ),
            None => format!("ended {:?}", ended.end),
        },
        Ran::Settling(_) => "settling".to_string(),
    }
}

fn own_file() -> OwnFile {
    OwnFile {
        path: std::sync::Arc::from(std::path::Path::new("snapshots/0101")),
        info: SnapshotInfo {
            created_at: golem_common::model::Timestamp::from(7),
            files: 1,
            bytes: 2,
        },
    }
}

fn exits() -> [SaveExit; 3] {
    [
        SaveExit::Answered(Ok(own_file().info)),
        SaveExit::Ended {
            end: RunEnd::CallFailed,
            failure: anyhow::anyhow!("failed"),
            staged: None,
        },
        SaveExit::Ended {
            end: RunEnd::CallFailed,
            failure: anyhow::anyhow!("failed"),
            staged: Some(own_file()),
        },
    ]
}

/// The policy of the tests of a late publish: a deadline of 300 ms, three tries, and three runs.
fn late_policy() -> StorePolicy {
    runs_policy(Duration::from_millis(300), 3)
}

/// A limiter without a bound whose withdrawal comes when `withdraw` is cancelled, and that counts
/// the waits after a failed run that the store reports.
struct WithdrawnSlots {
    withdraw: CancellationToken,
    failed_waits: AtomicUsize,
}

impl RunSlots for WithdrawnSlots {
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        Unlimited.take(immediate)
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(async move {
            self.withdraw.cancelled().await;
            Withdrawal::Stopped
        })
    }

    fn waiting_after_failure(&self) {
        self.failed_waits.fetch_add(1, Ordering::SeqCst);
    }
}

/// A tree of one file of `size` bytes that do not compress, followed in the order of the archive
/// by a file that the process cannot read. Gives `None` when the process can read that file, as
/// root can.
fn large_tree_with_an_unreadable_file(size: usize) -> Option<Scratch> {
    let tree = Scratch::new();
    let mut content = vec![0u8; size];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut content);
    write_tree(
        tree.path(),
        &[
            (
                "a.bin",
                Spec::File {
                    content: content.into_boxed_slice(),
                    mode: 0o644,
                },
            ),
            (
                "z.txt",
                Spec::File {
                    content: Box::from(&b"unreadable"[..]),
                    mode: 0o000,
                },
            ),
        ],
    );
    std::fs::read(tree.path().join("z.txt"))
        .is_err()
        .then_some(tree)
}

/// Saves a tree of 63 MiB, then an unreadable file, on a storage whose first write of a pack
/// follows `first` and lands 15 s after it was sent, with a deadline of 20 s. Then deletes all
/// snapshots of the agent, and waits past the landing. Gives what the save gave and the blobs that
/// the agent holds then, or `None` when the archiver ended before the file writer sent the first
/// pack, so the attempt tested nothing. Gives `None` also when the process can read the
/// unreadable file.
async fn a_save_whose_first_pack_lands_after_the_backup_failed(
    first: Script,
) -> Option<(Result<SnapshotInfo, SaveError>, Vec<String>)> {
    let tree = large_tree_with_an_unreadable_file(63 << 20)?;
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_first_pack(inner.clone(), first, |_, _| Script::Pass);
    let store = store(storage.clone(), runs_policy(Duration::from_secs(20), 3));
    let scope = new_scope();
    let started = std::time::Instant::now();

    let saved = store
        .save(
            &scope,
            &name("p-source"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    if !wrote_a_pack(&storage) {
        return None;
    }
    store.delete_all(&scope, &Unlimited).await.unwrap();
    tokio::time::sleep(
        (started + Duration::from_secs(17)).saturating_duration_since(std::time::Instant::now())
            + Duration::from_secs(1),
    )
    .await;
    Some((saved, blobs(&*inner, scope.namespace(), "").await))
}

/// Runs [`a_save_whose_first_pack_lands_after_the_backup_failed`] until an attempt sends the first
/// pack before the answer, at most three times. The race of the file writer and the archiver is
/// lost in about one attempt of twenty.
async fn first_attempt_that_raced(
    first: Script,
) -> Option<Option<(Result<SnapshotInfo, SaveError>, Vec<String>)>> {
    // A process that can read the unreadable file, as root can, cannot run the test.
    large_tree_with_an_unreadable_file(1)?;
    let attempts = futures::stream::iter(0..3)
        .then(|_| a_save_whose_first_pack_lands_after_the_backup_failed(first))
        .filter_map(std::future::ready);
    Some(std::pin::pin!(attempts).next().await)
}

/// A writer for a test subscriber that keeps every line that it gets.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

/// Gives the end of the exit of a save run, or the kind of the error of its answer.
fn mapping_save_shape(exit: SaveExit) -> String {
    match exit {
        SaveExit::Ended { end, .. } => format!("ended {end:?}"),
        SaveExit::Answered(Err(SaveError::Source(error))) => format!("source {:?}", error.kind()),
        SaveExit::Answered(answer) => format!("answered {answer:?}"),
    }
}

mod bounds {
    //! The publish bound of a save and the lease of a prune, judged by the storage.
    //!
    //! The time is real, with a short grace period and a short deadline, because the backup of a save
    //! runs on blocking threads, where the paused time of tokio would move on by itself. So a case
    //! lasts more than one grace period, and the sweep runs few cases: 8 with the marking prune
    //! before the save and 8 with it during the save, in two tests that run at the same time.
    //!
    //! A case saves a name of an agent whose packs a prune of the same agent marks before or during
    //! the save. One grace period after the end of that prune, a second prune of the agent runs,
    //! which can delete each pack that the first one marked. The save and the prunes go through two
    //! stores over the same storage, as two executors do, so the record of each store holds only its
    //! own calls. The cases generate slow and refused pack writes, failed runs of the save, a late
    //! index write, late deletes of the marking prune, slow backend calls of the marking prune, and
    //! refresh writes of its claim that fail, are slow, or lose their answer.
    //!
    //! The judges read the record of the calls and the answers:
    //! - each write of the save lands before the first index read of the second prune, or the save
    //!   answers `Failed` and lands nothing after that read;
    //! - each write of the save lands before the bound of the formula;
    //! - after the second prune, a saved name restores the whole tree, and a failed save left no name;
    //! - the marking prune makes no storage call later than one lease span after the start of its
    //!   newest marker write that succeeded.
    //!
    //! The sweep does not crash the executor at a step; the shell tests of the runs cover a stop at
    //! each wait.

    use super::*;
    use crate::filesystem_snapshot::backend::rustic::pruning::timing::{
        lease_span, refresh_period,
    };
    use crate::filesystem_snapshot::backend::rustic::publication::index_read_bound;
    use crate::filesystem_snapshot::backend::rustic::test_support::scripted::CallEvent;
    use crate::filesystem_snapshot::backend::rustic::test_support::*;
    use crate::filesystem_snapshot::{RunSlots, Slot, Withdrawal};
    use futures::future::BoxFuture;
    use golem_common::model::RetryConfig;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use std::sync::Mutex;
    use std::time::Instant;
    use test_r::{test, timeout};

    /// The grace period of a marked pack in these tests.
    const GRACE: Duration = Duration::from_secs(10);

    /// The storage call deadline in these tests: the shortest that the configuration allows, because a
    /// publish needs one deadline of [`crate::filesystem_snapshot::backend::rustic::publication::MIN_PUBLISH_TRY`] or more.
    const DEADLINE: Duration = Duration::from_secs(1);

    /// The storage calls of the backend of rustic, which a prune makes inside its lease.
    const BACKEND_CALLS: [&str; 6] = ["stat", "list", "read", "read_range", "write", "delete"];

    /// The policy of these tests: the bound is on, each blob call has its tries, and a failed run
    /// runs again after a short wait.
    fn bound_policy() -> StorePolicy {
        StorePolicy {
            deadline: DEADLINE,
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: GRACE,
            },
            prune_threshold: ALWAYS,
            retry: RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(50),
                max_delay: Duration::from_millis(200),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            publish_bound: PublishBound::On,
            in_call_tries: crate::filesystem_snapshot::backend::rustic::blob_io::IN_CALL_TRIES,
            ..StorePolicy::from_config(&config())
        }
    }

    /// A limiter without a bound that keeps the instant of its first grant. The shell takes the
    /// instant of the first slot of a call after the grant, so this instant is not later.
    #[derive(Default)]
    struct FirstGrant(Mutex<Option<Instant>>);

    impl RunSlots for FirstGrant {
        fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
            Box::pin(async move {
                let slot = Unlimited.take(immediate).await?;
                self.0.lock().unwrap().get_or_insert_with(
                    crate::filesystem_snapshot::backend::rustic::call_execution::now,
                );
                Ok(slot)
            })
        }

        fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
            Box::pin(std::future::pending())
        }
    }

    /// What the refresh writes of the claim of the marking prune do.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Refresh {
        Pass,
        /// Each refresh write fails, and never lands.
        Refused,
        /// Each refresh write takes this time.
        Slow(Duration),
        /// Each refresh write lands and loses its answer.
        LostAnswer,
    }

    /// What a case generates.
    #[derive(Clone, Copy, Debug)]
    struct Case {
        /// The time of each pack write of the save.
        pack_delay: Duration,
        /// The pack writes of the save that the storage refuses first, so its first runs fail.
        refused_packs: usize,
        /// The first index write of the save gets no answer and lands after this time.
        late_index: Duration,
        /// Each index delete of the marking prune gets no answer and lands after this time.
        late_delete: Duration,
        /// The time between the start of the first of the save and the marking prune and the start of
        /// the other.
        offset: Duration,
        /// Whether the marking prune starts after the save.
        marking_after: bool,
        /// What the refresh writes of the marking prune do.
        refresh: Refresh,
        /// The time of each backend call of the marking prune.
        prune_call_delay: Duration,
    }

    fn case_strategy(marking_after: bool) -> impl Strategy<Value = Case> {
        (
            prop_oneof![Just(0u64), 100u64..1000, 1000u64..3000, 3000u64..7000],
            0usize..3,
            0u64..1000,
            0u64..1000,
            0u64..1000,
            prop_oneof![
                Just(Refresh::Pass),
                Just(Refresh::Refused),
                (0u64..1000).prop_map(|millis| Refresh::Slow(Duration::from_millis(millis))),
                Just(Refresh::LostAnswer),
            ],
            prop_oneof![Just(0u64), 50u64..300],
        )
            .prop_map(
                move |(
                    pack,
                    refused_packs,
                    late_index,
                    late_delete,
                    offset,
                    refresh,
                    prune_call,
                )| {
                    Case {
                        pack_delay: Duration::from_millis(pack),
                        refused_packs,
                        late_index: Duration::from_millis(late_index),
                        late_delete: Duration::from_millis(late_delete),
                        offset: Duration::from_millis(offset),
                        marking_after,
                        refresh,
                        prune_call_delay: Duration::from_millis(prune_call),
                    }
                },
            )
    }

    /// What the storage of the save saw of the writes of the save while the save and the marking
    /// prune race.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct SaveSeen {
        /// The pack writes of the save that got a script.
        packs: usize,
        /// Whether the first index write of the save got its script.
        first_index: bool,
    }

    /// Gives a script that passes after `delay`.
    fn delayed(delay: Duration) -> Script {
        if delay.is_zero() {
            Script::Pass
        } else {
            Script::Delay(delay)
        }
    }

    /// Gives the script of a call of the store of the save, and what the storage saw after it, when
    /// the save and the marking prune race when `racing`, and the storage saw `seen` before. The
    /// first `refused_packs` pack writes of the race are refused, and the first index write lands
    /// late.
    fn save_script(
        case: Case,
        racing: bool,
        seen: SaveSeen,
        op_label: &str,
        path: &Path,
    ) -> (Script, SaveSeen) {
        match op_label {
            "write" if racing && path.starts_with("data") => (
                if seen.packs < case.refused_packs {
                    Script::Refuse
                } else {
                    delayed(case.pack_delay)
                },
                SaveSeen {
                    packs: seen.packs + 1,
                    ..seen
                },
            ),
            "write" if racing && path.starts_with("index") && !seen.first_index => (
                Script::LandAfter(case.late_index),
                SaveSeen {
                    first_index: true,
                    ..seen
                },
            ),
            _ => (Script::Pass, seen),
        }
    }

    /// Gives the script of a call of the store of the prunes, when the save and the marking prune
    /// race when `racing`.
    fn prune_script(case: Case, racing: bool, op_label: &str, path: &Path) -> Script {
        if !racing {
            return Script::Pass;
        }
        match (op_label, case.refresh) {
            ("delete", _) if path.starts_with("index") => Script::LandAfter(case.late_delete),
            ("refresh_claim", Refresh::Refused) => Script::Refuse,
            ("refresh_claim", Refresh::Slow(delay)) => delayed(delay),
            ("refresh_claim", Refresh::LostAnswer) => Script::LoseTheAnswer,
            (label, _) if BACKEND_CALLS.contains(&label) => delayed(case.prune_call_delay),
            _ => Script::Pass,
        }
    }

    /// Each write of the save that started at or after `t0` and its late landings, judged against
    /// the bound of a save whose first slot was at `t0`: a call ends one deadline before the bound,
    /// and a late change lands before it.
    fn writes_after_the_bound(
        events: &[CallEvent],
        saving: &BlobStorageNamespace,
        t0: Instant,
    ) -> Vec<String> {
        let bound = index_read_bound(t0, GRACE, DEADLINE, refresh_period(GRACE, DEADLINE));
        events
            .iter()
            .filter(|event| {
                event.namespace == *saving
                    && event.started >= t0
                    && matches!(event.op_label, "write" | "publish")
            })
            .filter(|event| {
                if event.landed_late {
                    event.ended > bound
                } else {
                    event.ended + DEADLINE > bound
                }
            })
            .map(|event| {
                format!(
                    "{} {} {} ms after the first slot, the bound is {} ms",
                    if event.landed_late {
                        "the late"
                    } else {
                        "the call"
                    },
                    event.op_label,
                    event.ended.duration_since(t0).as_millis(),
                    bound.duration_since(t0).as_millis()
                )
            })
            .collect()
    }

    /// Each write of the save that landed after `read`, the first index read of the second prune, when
    /// the save answered success, or when a write of the save landed after it at all. A save that
    /// answered `Failed` lands nothing after the read.
    fn writes_after_the_second_read(
        events: &[CallEvent],
        saving: &BlobStorageNamespace,
        read: Instant,
    ) -> Vec<String> {
        events
            .iter()
            .filter(|event| {
                event.namespace == *saving && matches!(event.op_label, "write" | "publish")
            })
            .filter(|event| event.ended > read)
            .map(|event| {
                format!(
                    "the save {} {} {} ended {} ms after the first index read of the second prune",
                    if event.landed_late { "late" } else { "call" },
                    event.op_label,
                    event.path.display(),
                    event.ended.duration_since(read).as_millis()
                )
            })
            .collect()
    }

    /// Each storage call of the prune that started before `until` and ended later than one lease span
    /// after the start of the newest marker write that had succeeded when the call started.
    fn calls_after_the_lease(
        events: &[CallEvent],
        pruning: &BlobStorageNamespace,
        refreshes_fail: bool,
        until: Instant,
    ) -> Vec<String> {
        let of_prune = events
            .iter()
            .filter(|event| event.namespace == *pruning && event.started < until)
            .collect::<Vec<_>>();
        let markers = of_prune
            .iter()
            .filter(|event| {
                event.op_label == "write_claim"
                    || (event.op_label == "refresh_claim" && !refreshes_fail)
            })
            .collect::<Vec<_>>();
        let Some(claimed) = markers.iter().map(|marker| marker.started).min() else {
            return Vec::new();
        };
        let span = lease_span(GRACE, DEADLINE);
        of_prune
            .iter()
            .filter(|event| {
                event.started >= claimed
                    && (BACKEND_CALLS.contains(&event.op_label) || event.op_label == "write_ledger")
            })
            .filter_map(|event| {
                let expiry = markers
                    .iter()
                    .filter(|marker| marker.ended <= event.started)
                    .map(|marker| marker.started + span)
                    .max()?;
                (event.ended > expiry).then(|| {
                    format!(
                        "the prune call {} {} ended {} ms after the end of its lease",
                        event.op_label,
                        event.path.display(),
                        event.ended.duration_since(expiry).as_millis()
                    )
                })
            })
            .collect()
    }

    /// Runs one case and gives each broken judgement.
    async fn broken(case: Case) -> Vec<String> {
        let inner = Arc::new(InMemoryBlobStorage::new());
        let scope = new_scope();
        // The save and the marking prune race while this flag is set.
        let racing = Arc::new(AtomicBool::new(false));
        let save_storage = ScriptedBlobStorage::with_state(inner.clone(), SaveSeen::default(), {
            let racing = racing.clone();
            move |seen, op_label, path| {
                save_script(case, racing.load(Ordering::SeqCst), *seen, op_label, path)
            }
        });
        let prune_storage = ScriptedBlobStorage::new(inner, {
            let racing = racing.clone();
            move |op_label, path| prune_script(case, racing.load(Ordering::SeqCst), op_label, path)
        });
        let saving_store = store(save_storage.clone(), bound_policy());
        let pruning_store = store(prune_storage.clone(), bound_policy());
        save_each(&pruning_store, &scope, &["p-1", "p-2", "p-4"]).await;

        // The marking prune of the delete of `p-1` marks the packs of `p-1` before or during the save
        // of `p-3`, which reuses the blob of `p-1`.
        racing.store(true, Ordering::SeqCst);
        let slots = FirstGrant::default();
        let tree = one_file_tree("p-1");
        let saving = async {
            saving_store
                .save(
                    &scope,
                    &name("p-3"),
                    tree.path(),
                    None,
                    never_cancelled(),
                    &slots,
                )
                .await
        };
        let marking = async {
            let marked = pruning_store
                .delete(&scope, &[name("p-1")], &Unlimited)
                .await;
            (marked, Instant::now())
        };
        let later =
            |late: bool| tokio::time::sleep(if late { case.offset } else { Duration::ZERO });
        let (saved, (marked, marking_end)) = tokio::join!(
            async {
                later(!case.marking_after).await;
                saving.await
            },
            async {
                later(case.marking_after).await;
                marking.await
            }
        );
        racing.store(false, Ordering::SeqCst);
        // A marking prune that its lease stopped keeps its claim for the whole hold, so no second
        // prune of the agent runs in the case.
        let marking_pruned = pruning_store.take_failed_prunes().is_empty();

        // One grace period after the end of the marking prune, the second prune can delete each pack
        // that the marking prune marked.
        tokio::time::sleep_until(tokio::time::Instant::from_std(
            marking_end + GRACE + Duration::from_millis(100),
        ))
        .await;
        let second_start = Instant::now();
        let deleted = pruning_store
            .delete(&scope, &[name("p-2")], &Unlimited)
            .await;
        // Each late change lands within one deadline.
        tokio::time::sleep(DEADLINE * 2).await;
        let restored = match &saved {
            Ok(_) => Some(restored_listing(&pruning_store, &scope, &name("p-3")).await),
            Err(_) => None,
        };
        let stat = pruning_store.stat(&scope, &name("p-3")).await;
        saving_store.shut_down().await;
        pruning_store.shut_down().await;

        let (save_events, prune_events) = (save_storage.events(), prune_storage.events());
        let t0 = *slots.0.lock().unwrap();
        let second_read = prune_events
            .iter()
            .filter(|event| {
                event.started >= second_start
                    && event.path.starts_with("index")
                    && matches!(event.op_label, "list" | "read")
            })
            .map(|event| event.started)
            .min();
        let second_pruned = prune_events
            .iter()
            .any(|event| event.started >= second_start && event.op_label == "write_claim");
        let saved_well = match (&saved, &restored, &stat) {
            (Ok(_), Some(Ok(restored)), _) if *restored == listing(tree.path()) => Vec::new(),
            (Err(_), _, Ok(None)) => Vec::new(),
            _ => vec![format!(
                "the save gave {saved:?}, the restore after the second prune {restored:?} and the stat {stat:?}"
            )],
        };
        let refreshes_fail = case.refresh == Refresh::Refused;
        [
            marked
                .err()
                .map(|error| format!("the marking delete failed: {error:?}"))
                .into_iter()
                .collect(),
            deleted
                .err()
                .map(|error| format!("the second delete failed: {error:?}"))
                .into_iter()
                .collect(),
            (marking_pruned && !second_pruned)
                .then(|| "the second delete did not prune".to_string())
                .into_iter()
                .collect(),
            saved_well,
            second_read
                .map(|read| writes_after_the_second_read(&save_events, scope.namespace(), read))
                .unwrap_or_default(),
            t0.map(|t0| writes_after_the_bound(&save_events, scope.namespace(), t0))
                .unwrap_or_default(),
            calls_after_the_lease(
                &prune_events,
                scope.namespace(),
                refreshes_fail,
                marking_end,
            ),
        ]
        .concat()
    }

    /// Runs `case` on a runtime of its own.
    fn broken_in_runtime(case: Case) -> Vec<String> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(broken(case))
    }

    /// Runs 8 cases of `strategy` on a thread of the blocking pool, each on a runtime of its own, and
    /// gives the first case that breaks the bound, shrunk for at most 2 min. A case runs in real time,
    /// for about 20 s.
    async fn no_case_breaks_the_bound(strategy: impl Strategy<Value = Case> + Send + 'static) {
        let outcome = tokio::task::spawn_blocking(move || {
            proptest::test_runner::TestRunner::new(ProptestConfig {
                cases: 8,
                max_shrink_time: 120_000,
                failure_persistence: None,
                ..ProptestConfig::default()
            })
            .run(&strategy, |case| {
                prop_assert_eq!(broken_in_runtime(case), Vec::<String>::new(), "{:?}", case);
                Ok(())
            })
            .map_err(|error| error.to_string())
        })
        .await;

        assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
    }

    #[test]
    #[timeout("6m")]
    async fn every_write_of_a_save_lands_before_the_bound() {
        no_case_breaks_the_bound(case_strategy(true)).await;
    }

    #[test]
    #[timeout("6m")]
    async fn every_write_of_a_save_after_a_marking_prune_lands_before_the_bound() {
        no_case_breaks_the_bound(case_strategy(false)).await;
    }

    #[test]
    fn the_judges_of_the_bound_find_a_write_after_the_bound_and_a_call_after_the_lease() {
        // The judges themselves: a write of the save that ends after one deadline before the bound,
        // a late change after the bound, and a prune call after the lease of its only marker.
        let saving = BlobStorageNamespace::InitialAgentFiles {
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::new_v4()),
        };
        let pruning = BlobStorageNamespace::InitialAgentFiles {
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::new_v4()),
        };
        let t0 = Instant::now();
        let bound = index_read_bound(t0, GRACE, DEADLINE, refresh_period(GRACE, DEADLINE));
        let event =
            |namespace: &BlobStorageNamespace, op_label, started, ended, landed_late| CallEvent {
                namespace: namespace.clone(),
                op_label,
                path: Path::new("index/0").into(),
                started,
                ended,
                landed_late,
            };
        let claim = event(&pruning, "write_claim", t0, t0, false);
        let span = lease_span(GRACE, DEADLINE);
        let events = [
            event(&saving, "write", t0, bound - DEADLINE, false),
            event(
                &saving,
                "write",
                t0,
                bound - DEADLINE + Duration::from_millis(1),
                false,
            ),
            event(&saving, "publish", bound, bound, true),
            event(
                &saving,
                "publish",
                bound,
                bound + Duration::from_millis(1),
                true,
            ),
            claim,
            event(&pruning, "read", t0 + span, t0 + span, false),
            event(
                &pruning,
                "read",
                t0 + span,
                t0 + span + Duration::from_millis(1),
                false,
            ),
        ];

        assert_eq!(
            (
                writes_after_the_bound(&events, &saving, t0).len(),
                calls_after_the_lease(&events, &pruning, true, t0 + span + DEADLINE).len(),
                writes_after_the_second_read(&events, &saving, bound - DEADLINE).len(),
            ),
            (2, 1, 3)
        );
    }
}

#[test]
#[timeout("120s")]
async fn a_save_whose_backup_fails_with_a_pack_write_in_flight_answers_after_the_write_and_a_delete_all_leaves_nothing()
 {
    // The only pack of the backup is written in its finalize, and the write never answers: its try
    // ends at the deadline of 2 s, and the write lands 3.4 s after it was sent. A new run saves
    // the tree.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_first_pack(
        inner.clone(),
        Script::HangThenLand(Duration::from_millis(3400)),
        |_, _| Script::Pass,
    );
    let store = store(storage.clone(), runs_policy(Duration::from_secs(2), 3));
    let scope = new_scope();
    let tree = fixture_tree();

    let _ = store
        .save(
            &scope,
            &name("p-hung"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let sent_before_the_answer = wrote_a_pack(&storage);
    store.delete_all(&scope, &Unlimited).await.unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert!(sent_before_the_answer);
    assert_eq!(
        blobs(&*inner, scope.namespace(), "").await,
        Vec::<String>::new()
    );
}

#[test]
#[timeout("60s")]
async fn a_save_whose_lost_publish_try_lands_after_a_later_try_answers_only_after_it_and_a_delete_all_keeps_the_agent_empty()
 {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = scripted_publish(inner.clone(), |tried| match tried {
        1 => Script::LandAfter(Duration::from_secs(1)),
        _ => Script::Pass,
    });
    let store = store(storage.clone(), runs_policy(Duration::from_secs(2), 3));
    let scope = new_scope();
    let tree = fixture_tree();

    let saved = store
        .save(
            &scope,
            &name("p-publish"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    let sent_before_the_answer = calls_of(&storage, "publish");
    store.delete_all(&scope, &Unlimited).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(sent_before_the_answer, 2);
    assert_eq!(
        blobs(&*inner, scope.namespace(), "").await,
        Vec::<String>::new()
    );
}

#[test]
#[timeout("60s")]
async fn a_save_whose_index_file_a_prune_deleted_after_the_listing_loads_again_and_saves() {
    let shared = Arc::new(InMemoryBlobStorage::new());
    let (scope, pruning, _) = scope_with_a_prune_to_come(&shared).await;
    let (saving, storage, hold, held) = holding_index_reads(&shared);
    let tree = one_file_tree("new");
    hold.store(true, Ordering::SeqCst);
    let first = AbortOnDropHandle::new(tokio::spawn({
        let (saving, scope, path) = (saving.clone(), scope.clone(), tree.path().to_path_buf());
        async move {
            saving
                .save(
                    &scope,
                    &name("p-3"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let reached = eventually(|| held.load(Ordering::SeqCst) > 0).await;

    let deleted = pruning.delete(&scope, &[name("p-2")], &Unlimited).await;
    let pruned = ledger(&shared, &scope).await.last_prune.is_some();
    hold.store(false, Ordering::SeqCst);
    storage.open_gate();
    let first = tokio::time::timeout(LIMIT, first).await;
    let again = saving
        .save(
            &scope,
            &name("p-3"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(matches!(&first, Ok(Ok(Ok(_)))), "{first:?}");
    assert!(matches!(again, Err(SaveError::NameInUse)), "{again:?}");
    assert_eq!(
        (
            reached,
            deleted.is_ok(),
            pruned,
            restored_listing(&saving, &scope, &name("p-3")).await.ok(),
        ),
        (true, true, true, Some(listing(tree.path())))
    );
}

#[test]
fn a_save_run_can_have_a_late_write_only_before_a_finished_backup_or_after_a_recorded_try() {
    let now = std::time::Instant::now();
    let recorded = |backend: bool, publish: bool| RunLate {
        backend: backend.then_some(now),
        publish: publish.then_some(now),
    };

    assert_eq!(
        [
            settle_may_land(true, recorded(false, false)),
            settle_may_land(true, recorded(true, false)),
            settle_may_land(true, recorded(false, true)),
            settle_may_land(false, recorded(false, false)),
        ],
        [false, true, true, true]
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
fn copy_flat_tree(from: &Path, to: &Path) {
    std::fs::read_dir(from).unwrap().for_each(|entry| {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        std::fs::copy(entry.path(), &target).unwrap();
        set_modified(&target, modified(&entry.path()));
    });
}

/// Gives the change time of the file as seconds and nanoseconds.
#[cfg(unix)]
fn changed_at(path: &Path) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    (metadata.ctime(), metadata.ctime_nsec())
}

/// The longest time that [`wait_past_change_times`] waits.
#[cfg(unix)]
const CHANGE_TIME_WAIT: Duration = Duration::from_secs(10);

/// Waits until a file that changes now gets a later change time than each of the files.
///
/// The kernel takes the change time from a clock that moves in ticks of some milliseconds, so two
/// changes within one tick get the same change time. The wait changes a probe file in its own
/// directory until the change time of the probe is later than the latest change time of the
/// files. It fails the test when that does not happen within [`CHANGE_TIME_WAIT`].
#[cfg(unix)]
fn wait_past_change_times(files: &[PathBuf]) {
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
fn entries(directory: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Writes a tree of three files into a new directory, and gives the directory.
fn three_file_tree() -> Scratch {
    let tree = Scratch::new();
    ["a.txt", "b.txt", "c.txt"]
        .iter()
        .for_each(|file| std::fs::write(tree.path().join(file), file.as_bytes()).unwrap());
    tree
}

/// A storage over `inner` whose publish tries follow `script(n)` for the `n`-th try, counted from
/// 1, and whose other calls pass.
fn scripted_publish(
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

/// Whether the storage saw a write of a pack.
fn wrote_a_pack(storage: &ScriptedBlobStorage) -> bool {
    storage
        .calls()
        .iter()
        .any(|(op_label, path)| *op_label == "write" && path.starts_with("data"))
}
