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

use super::{CopyError, copy_end};
use crate::filesystem_snapshot::backend::rustic::RusticSnapshotStore;
use crate::filesystem_snapshot::backend::rustic::call_execution::RunEnd;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    ALWAYS, LIMIT, LONG_DEADLINE, NEVER, REACH_LIMIT, SHORT_WRITE_DEADLINE, SharedSlots, blobs,
    calls_of, eventually, fixture_tree, is_stopped, listed_names, name, one_file_tree, one_run,
    policy, polled_until, refusing, restored_listing, runs_policy, save_each, store, store_over,
    three_runs,
};

use crate::filesystem_snapshot::test_support::fixture::listing;
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{CallError, FilesystemSnapshotStore, Unlimited};
use futures::StreamExt;
use golem_common::model::RetryConfig;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::task::AbortOnDropHandle;

#[test]
#[timeout("60s")]
async fn a_deleted_scope_holds_no_blob() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), one_file_tree("second"));
    store
        .save(
            &scope,
            &name("p-1"),
            first.path(),
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
            second.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    let before = blobs(&*storage, scope.namespace(), "").await;

    store.delete_all(&scope, &Unlimited).await.unwrap();

    assert_eq!(
        (
            before
                .iter()
                .any(|path| path.starts_with("golem/prune-ledgers/")),
            blobs(&*storage, scope.namespace(), "").await
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_whose_listed_index_file_is_gone_runs_again() {
    // The gate holds the copy of the listed index file, and the test deletes that file meanwhile.
    // The run ends with a source that is gone, and the next run lists the source again.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if op_label == "copy" && path.starts_with("index") {
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
    let copying = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let (from, to) = (from.clone(), to.clone());
        async move { store.copy_all(&from, &to, &Unlimited).await }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "copy" && path.starts_with("index"))
    })
    .await;
    let index_files = blobs(&*inner, from.namespace(), "index/").await;

    futures::stream::iter(&index_files)
        .for_each(|path| {
            let (inner, from) = (&inner, &from);
            async move {
                inner
                    .delete("test", "test", from.namespace().clone(), Path::new(path))
                    .await
                    .unwrap();
            }
        })
        .await;
    storage.open_gate();
    let copied = tokio::time::timeout(LIMIT, copying).await;

    assert!(matches!(&copied, Ok(Ok(Ok(())))), "{copied:?}");
    assert_eq!(
        (
            held,
            index_files.is_empty(),
            storage
                .calls()
                .iter()
                .filter(|(op_label, _)| *op_label == "copy_read")
                .count(),
            blobs(&*inner, to.namespace(), "index/").await
        ),
        (true, false, 2, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_leaves_out_a_snapshot_file_that_a_delete_removed_after_the_listing() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "copy" && path.starts_with("snapshots") {
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

    let copied = store.copy_all(&from, &to, &Unlimited).await;

    assert!(copied.is_ok(), "{copied:?}");
    assert_eq!(
        (
            blobs(&*storage, to.namespace(), "config").await.len(),
            blobs(&*storage, to.namespace(), "snapshots/").await
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
        .save(
            &from,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let mut copying = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let (from, to) = (from.clone(), to.clone());
        async move { store.copy_all(&from, &to, &Unlimited).await }
    }));
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
        matches!(&copied, Ok(Ok(Err(error))) if is_stopped(error)),
        "{copied:?}"
    );
    assert_eq!(
        (held, stopped, calls_after_copy),
        (true, true, calls_at_stop)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_all_and_a_copy_after_shut_down_give_stopped() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let (scope, other) = (new_scope(), new_scope());
    let tree = one_file_tree("kept");
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
    store.shut_down().await;

    let deleted = store.delete_all(&scope, &Unlimited).await;
    let copied = store.copy_all(&scope, &other, &Unlimited).await;

    assert!(deleted.as_ref().is_err_and(is_stopped), "{deleted:?}");
    assert!(copied.as_ref().is_err_and(is_stopped), "{copied:?}");
}

#[test]
#[timeout("60s")]
async fn a_lost_copy_try_followed_by_a_try_that_succeeds_holds_the_next_run() {
    // The first copy of a pack lands and loses its answer, and its next try succeeds. The run then
    // fails at its config write, whose three tries are refused. The next run starts only one
    // deadline after the lost try.
    let deadline = Duration::from_millis(500);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let (from, to) = (new_scope(), new_scope());
    save_each(&saving, &from, &["p-1"]).await;
    let lost_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let second_run_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let (lost_at, second_run_at) = (lost_at.clone(), second_run_at.clone());
        let (copies, config_reads, config_writes) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        move |op_label, path| match op_label {
            "copy" if path.starts_with("data") && copies.fetch_add(1, Ordering::SeqCst) == 0 => {
                *lost_at.lock().unwrap() = Some(std::time::Instant::now());
                Script::LoseTheAnswer
            }
            "copy_read" => {
                if config_reads.fetch_add(1, Ordering::SeqCst) == 1 {
                    *second_run_at.lock().unwrap() = Some(std::time::Instant::now());
                }
                Script::Pass
            }
            "copy_write" if config_writes.fetch_add(1, Ordering::SeqCst) < 3 => Script::Refuse,
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), runs_policy(deadline, 3));

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let gap = second_run_at
        .lock()
        .unwrap()
        .zip(*lost_at.lock().unwrap())
        .map(|(second, lost)| second.duration_since(lost));

    assert!(copied.is_ok(), "{copied:?}");
    assert!(gap.is_some_and(|gap| gap >= deadline), "{gap:?}");
    assert_eq!(
        restored_listing(&store, &to, &name("p-1")).await.ok(),
        restored_listing(&store, &from, &name("p-1")).await.ok()
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_that_fails_after_a_lost_try_waits_before_it_answers() {
    // The first copy of a pack lands and loses its answer; each later call of the copy is
    // refused. The copy answers `Failed` only one deadline after the lost try.
    let deadline = Duration::from_millis(500);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let (from, to) = (new_scope(), new_scope());
    save_each(&saving, &from, &["p-1"]).await;
    let lost_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let lost_at = lost_at.clone();
        let copies = AtomicUsize::new(0);
        move |op_label, path| match op_label {
            "copy" if path.starts_with("data") && copies.fetch_add(1, Ordering::SeqCst) == 0 => {
                *lost_at.lock().unwrap() = Some(std::time::Instant::now());
                Script::LoseTheAnswer
            }
            "copy" => Script::Refuse,
            _ => Script::Pass,
        }
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            retry: one_run(),
            ..runs_policy(deadline, 1)
        },
    );

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let answered = std::time::Instant::now();

    assert!(matches!(copied, Err(CallError::Failed(_))), "{copied:?}");
    assert!(
        lost_at
            .lock()
            .unwrap()
            .is_some_and(|lost| answered.duration_since(lost) >= deadline),
        "{:?}",
        *lost_at.lock().unwrap()
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_that_succeeds_after_a_lost_try_waits_for_it_before_it_answers() {
    // The first copy of a pack lands and loses its answer, and its next try succeeds. The run
    // answers success, and the copy answers only one deadline after the lost try.
    let deadline = Duration::from_millis(500);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let (from, to) = (new_scope(), new_scope());
    save_each(&saving, &from, &["p-1"]).await;
    let lost_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let lost_at = lost_at.clone();
        let copies = AtomicUsize::new(0);
        move |op_label, path| match op_label {
            "copy" if path.starts_with("data") && copies.fetch_add(1, Ordering::SeqCst) == 0 => {
                *lost_at.lock().unwrap() = Some(std::time::Instant::now());
                Script::LoseTheAnswer
            }
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), runs_policy(deadline, 3));

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let answered = std::time::Instant::now();

    assert!(copied.is_ok(), "{copied:?}");
    assert_eq!(calls_of(&storage, "copy_read"), 1);
    assert!(
        lost_at
            .lock()
            .unwrap()
            .is_some_and(|lost| answered.duration_since(lost) >= deadline),
        "{:?}",
        *lost_at.lock().unwrap()
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_whose_later_run_fails_removes_the_config_that_an_earlier_run_wrote() {
    // The config write of the first run lands and loses its answer. The call waits one deadline,
    // and each later run deletes the config of the target first and then fails at its read of the
    // source config. The target holds no repository, so it has no snapshot.
    let deadline = Duration::from_millis(500);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let (from, to) = (new_scope(), new_scope());
    save_each(&store_over(&inner), &from, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let (config_reads, config_writes) = (AtomicUsize::new(0), AtomicUsize::new(0));
        move |op_label, _| match op_label {
            "copy_write" if config_writes.fetch_add(1, Ordering::SeqCst) == 0 => {
                Script::LoseTheAnswer
            }
            "copy_read" if config_reads.fetch_add(1, Ordering::SeqCst) > 0 => Script::Refuse,
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), runs_policy(deadline, 1));

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let listed = store.list(&to, &Unlimited).await;

    assert!(matches!(copied, Err(CallError::Failed(_))), "{copied:?}");
    assert!(
        matches!(&listed, Ok(listed) if listed.is_empty()),
        "{listed:?}"
    );
    assert_eq!(
        (
            calls_of(&storage, "copy_remove_config"),
            blobs(&*inner, to.namespace(), "config").await
        ),
        (2, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_copy_whose_index_listing_tore_catches_up_and_every_target_snapshot_restores() {
    // The first listings of the index files and of the packs of the source leave out the files of
    // `p-2`, as listings that the writes of a save tore do. The listing of the snapshot files
    // holds `p-2`. The catch-up lists again, finds the index file of `p-2`, and copies it with its
    // packs.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let (from, to) = (new_scope(), new_scope());
    let saving = store_over(&inner);
    save_each(&saving, &from, &["p-1"]).await;
    let before = [
        blobs(&*inner, from.namespace(), "index/").await,
        blobs(&*inner, from.namespace(), "data/").await,
    ]
    .concat();
    save_each(&saving, &from, &["p-2"]).await;
    let after = [
        blobs(&*inner, from.namespace(), "index/").await,
        blobs(&*inner, from.namespace(), "data/").await,
    ]
    .concat();
    let torn = AtomicUsize::new(0);
    let storage = ScriptedBlobStorage::new(inner.clone(), move |op_label, path| {
        let tearable =
            op_label == "copy_list" && (path == Path::new("index") || path == Path::new("data"));
        if tearable && torn.fetch_add(1, Ordering::SeqCst) < 2 {
            Script::Torn
        } else {
            Script::Pass
        }
    });
    storage.hide(
        after
            .iter()
            .filter(|path| !before.contains(path))
            .map(|path| Box::<Path>::from(Path::new(path))),
    );
    storage.open_gate();
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let restored = futures::future::join_all(["p-1", "p-2"].map(|text| {
        let (store, to) = (&store, &to);
        async move { restored_listing(store, to, &name(text)).await.ok() }
    }))
    .await;
    let expected = futures::future::join_all(["p-1", "p-2"].map(|text| {
        let (store, from) = (&store, &from);
        async move { restored_listing(store, from, &name(text)).await.ok() }
    }))
    .await;

    assert!(copied.is_ok(), "{copied:?}");
    assert!(after.len() > before.len());
    assert!(restored.iter().all(Option::is_some), "{restored:?}");
    assert_eq!(restored, expected);
    assert_eq!(
        blobs(&*inner, to.namespace(), "index/").await,
        blobs(&*inner, from.namespace(), "index/").await
    );
}

#[test]
#[timeout("60s")]
async fn a_late_copy_write_of_an_earlier_run_lands_before_the_next_run() {
    // The first copy of a pack gets no answer and lands half a deadline later. The run fails at
    // once with its one try. The next run starts one deadline after the try, so after the write
    // landed, and the copy gives a target that restores as the source does.
    let deadline = Duration::from_millis(600);
    let inner = Arc::new(InMemoryBlobStorage::new());
    let (from, to) = (new_scope(), new_scope());
    save_each(&store_over(&inner), &from, &["p-1"]).await;
    let second_run_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let second_run_at = second_run_at.clone();
        let (copies, config_reads) = (AtomicUsize::new(0), AtomicUsize::new(0));
        move |op_label, path| match op_label {
            "copy" if path.starts_with("data") && copies.fetch_add(1, Ordering::SeqCst) == 0 => {
                Script::LandAfter(deadline / 2)
            }
            "copy_read" => {
                if config_reads.fetch_add(1, Ordering::SeqCst) == 1 {
                    *second_run_at.lock().unwrap() =
                        Some(crate::filesystem_snapshot::backend::rustic::call_execution::now());
                }
                Script::Pass
            }
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), runs_policy(deadline, 1));

    let copied = store.copy_all(&from, &to, &Unlimited).await;
    let landed = storage
        .events()
        .iter()
        .find(|event| event.landed_late)
        .map(|event| event.ended);
    let second_run = *second_run_at.lock().unwrap();

    assert!(copied.is_ok(), "{copied:?}");
    assert!(
        matches!((landed, second_run), (Some(landed), Some(second)) if landed < second),
        "{landed:?} {second_run:?}"
    );
    assert_eq!(
        restored_listing(&store, &to, &name("p-1")).await.ok(),
        restored_listing(&store, &from, &name("p-1")).await.ok()
    );
}

#[test]
#[timeout("120s")]
async fn a_delete_all_drains_before_it_takes_a_slot() {
    // The limiter has one slot. The first run of the save fails, and the save waits 500 ms
    // between its runs, without a slot. A delete of all snapshots that took the slot before its
    // drain would wait for the save, and the save would wait for the slot.
    let storage = refusing(1, |op_label, path| {
        op_label == "write" && path.starts_with("data")
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            retry: RetryConfig {
                min_delay: Duration::from_millis(500),
                max_delay: Duration::from_millis(500),
                multiplier: 1.0,
                ..three_runs()
            },
            ..runs_policy(SHORT_WRITE_DEADLINE, 1)
        },
    );
    let scope = new_scope();
    let tree = fixture_tree();
    let slots = SharedSlots::new(1);
    let saving = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope, slots) = (store.clone(), scope.clone(), slots.clone());
        let tree = tree.path().to_path_buf();
        async move {
            store
                .save(&scope, &name("p-1"), &tree, None, never_cancelled(), &slots)
                .await
        }
    }));
    let waiting = polled_until(REACH_LIMIT, || slots.counts().3 == 1).await;

    let deleting_all = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope, slots) = (store.clone(), scope.clone(), slots.clone());
        async move { store.delete_all(&scope, &slots).await }
    }));
    let saved = tokio::time::timeout(REACH_LIMIT, saving).await;
    let deleted_all = tokio::time::timeout(REACH_LIMIT, deleting_all).await;

    assert!(matches!(saved, Ok(Ok(Ok(_)))), "{saved:?}");
    assert!(matches!(deleted_all, Ok(Ok(Ok(())))), "{deleted_all:?}");
    assert_eq!(
        (waiting, slots.counts(), listed_names(&store, &scope).await),
        (true, (3, 0, 1, 1), Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_all_during_a_later_run_of_a_restore_starts_only_after_the_restore_returned() {
    // The first run of the restore fails at its read of the pack. The gate holds the read of the
    // pack of the second run while the delete of all snapshots begins.
    let armed = Arc::new(AtomicBool::new(false));
    let reads = AtomicUsize::new(0);
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let armed = armed.clone();
        move |op_label, path| {
            if !(armed.load(Ordering::SeqCst)
                && op_label == "read_range"
                && path.starts_with("data"))
            {
                return Script::Pass;
            }
            match reads.fetch_add(1, Ordering::SeqCst) {
                0 => Script::Refuse,
                1 => Script::WaitForGate,
                _ => Script::Pass,
            }
        }
    });
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    save_each(&store, &scope, &["p-0"]).await;
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
    armed.store(true, Ordering::SeqCst);
    let restoring = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { restored_listing(&store, &scope, &name("p-1")).await }
    }));
    let held = eventually(|| calls_of(&storage, "read_range") >= 2).await;

    let deleting_all = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete_all(&scope, &Unlimited).await }
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let waited = scope_deletes(&storage) == 0 && !deleting_all.is_finished();
    storage.open_gate();
    let restored = tokio::time::timeout(LIMIT, restoring).await;
    let deleted_all = tokio::time::timeout(LIMIT, deleting_all).await;

    assert!(matches!(deleted_all, Ok(Ok(Ok(())))), "{deleted_all:?}");
    assert_eq!(
        (
            held,
            waited,
            restored.ok().and_then(|restored| restored.ok()?.ok()),
            listed_names(&store, &scope).await,
        ),
        (true, true, Some(listing(tree.path())), Vec::<String>::new())
    );
}

/// Gives the number of calls of a delete of all snapshots.
fn scope_deletes(storage: &ScriptedBlobStorage) -> usize {
    calls_of(storage, "delete_scope")
}

mod value_rules {
    use crate::filesystem_snapshot::backend::rustic::blob_io::SnapshotFiles;
    use crate::filesystem_snapshot::backend::rustic::repository_namespace::{
        CatchUp, CopyError, MOST_CATCH_UPS, catch_up, copy_scope, delete_scope,
    };
    use crate::filesystem_snapshot::backend::rustic::test_support::files_of;
    use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
        Script, ScriptedBlobStorage,
    };
    use futures::StreamExt as _;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::{test, timeout};
    use uuid::Uuid;

    const DEADLINE: Duration = Duration::from_secs(2);

    /// The blobs of a small repository, with the ledger of the store.
    const REPOSITORY: [(&str, &str); 6] = [
        ("config", "config"),
        ("data/ab/abab", "pack"),
        ("golem/prune-ledgers/1000-0-0f0f", "ledger"),
        ("index/cdcd", "index"),
        ("keys/efef", "key"),
        ("snapshots/0101", "snapshot"),
    ];

    fn files<S: BlobStorage + 'static>(
        storage: &Arc<S>,
        namespace: &BlobStorageNamespace,
    ) -> SnapshotFiles {
        files_of(
            storage.clone(),
            namespace.clone(),
            DEADLINE,
            tokio_util::sync::CancellationToken::new(),
        )
    }

    fn new_namespace() -> BlobStorageNamespace {
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        }
    }

    async fn put_all(
        storage: &dyn BlobStorage,
        namespace: &BlobStorageNamespace,
        blobs: &[(&str, &str)],
    ) {
        futures::future::join_all(blobs.iter().map(|(path, content)| {
            storage.put_raw(
                "test",
                "test",
                namespace.clone(),
                Path::new(path),
                content.as_bytes(),
            )
        }))
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<()>>>()
        .unwrap();
    }

    /// Gives the path and the content of each blob of the namespace, in the order of the paths.
    async fn stored(
        storage: &dyn BlobStorage,
        namespace: &BlobStorageNamespace,
    ) -> Vec<(String, String)> {
        let listed = storage
            .list_blobs_below("test", "test", namespace.clone(), Path::new(""))
            .await
            .unwrap();
        let mut blobs = futures::future::join_all(listed.iter().map(|blob| async {
            let content = storage
                .get_raw("test", "test", namespace.clone(), &blob.path)
                .await
                .unwrap()
                .unwrap();
            (
                blob.path.display().to_string(),
                String::from_utf8(content).unwrap(),
            )
        }))
        .await;
        blobs.sort();
        blobs
    }

    fn owned(blobs: &[(&str, &str)]) -> Vec<(String, String)> {
        blobs
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_string()))
            .collect()
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_gives_the_target_each_blob_of_the_repository_and_not_the_ledger() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), false)
            .await
            .unwrap();

        assert_eq!(
            (stored(&*storage, &to).await, stored(&*storage, &from).await),
            (
                owned(
                    &REPOSITORY
                        .into_iter()
                        .filter(|(path, _)| !path.starts_with("golem/"))
                        .collect::<Vec<_>>()
                ),
                owned(&REPOSITORY)
            )
        );
    }

    #[test]
    #[timeout("60s")]
    async fn copy_scope_copies_server_side_in_directory_order_and_skips_a_pruned_snapshot_file() {
        // The listing of the packs waits for the gate. Before it opens, a delete removes the snapshot
        // file `snapshots/0202`, which the copy listed already. Each copy of a pack waits a little, so
        // more than one copy runs at once, and the next directory starts only after the last of them.
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                match op_label {
                    "copy_list" if path == Path::new("data") => Script::WaitForGate,
                    "copy" if path.starts_with("data") => Script::Delay(Duration::from_millis(50)),
                    _ => Script::Pass,
                }
            });
        let (from, to) = (new_namespace(), new_namespace());
        let packs = (0..12)
            .map(|index| format!("data/ab/{index:04x}"))
            .collect::<Vec<_>>();
        let repository = REPOSITORY
            .iter()
            .copied()
            .chain(packs.iter().map(|path| (path.as_str(), "pack")))
            .chain(std::iter::once(("snapshots/0202", "gone")))
            .collect::<Vec<_>>();
        put_all(&*storage, &from, &repository).await;
        let deleting = async {
            let listed = futures::stream::repeat(())
                .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                .filter(|()| {
                    std::future::ready(storage.calls().contains(&("copy_list", "data".to_string())))
                });
            std::pin::pin!(listed).next().await;
            storage
                .delete("test", "test", from.clone(), Path::new("snapshots/0202"))
                .await
                .unwrap();
            storage.open_gate();
        };

        let (from_files, to_files) = (files(&storage, &from), files(&storage, &to));
        let started = tokio::time::Instant::now();
        let (copied, ()) = tokio::join!(copy_scope(&from_files, &to_files, false), deleting);
        let copy_time = started.elapsed();

        let calls = storage.calls();
        let directory_of = |path: &str| path.split('/').next().unwrap_or("").to_string();
        let directories = calls
            .iter()
            .filter(|(op_label, _)| *op_label == "copy")
            .map(|(_, path)| directory_of(path))
            .fold(Vec::new(), |mut order, directory| {
                if order.last() != Some(&directory) {
                    order.push(directory);
                }
                order
            });
        let reads = calls
            .iter()
            .filter(|(op_label, _)| *op_label == "copy_read")
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        assert!(copied.is_ok(), "{copied:?}");
        assert_eq!(directories, vec!["data", "keys", "index", "snapshots"]);
        assert_eq!(reads, vec!["config".to_string()]);
        assert!(
            copy_time < Duration::from_millis(packs.len() as u64 * 50),
            "the packs were copied one at a time: {copy_time:?}"
        );
        assert_eq!(
            stored(&*storage, &to).await.len(),
            REPOSITORY.len() - 1 + packs.len()
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_writes_the_packs_the_keys_the_index_files_the_snapshot_files_and_then_the_config()
     {
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), false)
            .await
            .unwrap();

        assert_eq!(
            storage
                .calls()
                .into_iter()
                .filter(|(op_label, _)| *op_label == "copy" || *op_label == "copy_write")
                .map(|(_, path)| path)
                .collect::<Vec<_>>(),
            vec![
                "data/ab/abab",
                "keys/efef",
                "index/cdcd",
                "snapshots/0101",
                "config"
            ]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_lists_the_snapshot_files_before_the_index_files_the_keys_and_the_packs() {
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), false)
            .await
            .unwrap();

        assert_eq!(
            storage
                .calls()
                .into_iter()
                .filter(|(op_label, _)| *op_label == "copy_list")
                .map(|(_, path)| path)
                .collect::<Vec<_>>(),
            vec!["snapshots", "index", "keys", "data", "index", "data"]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_of_a_namespace_without_a_config_copies_nothing() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let (from, to) = (new_namespace(), new_namespace());
        put_all(
            &*storage,
            &from,
            &REPOSITORY
                .into_iter()
                .filter(|(path, _)| *path != "config")
                .collect::<Vec<_>>(),
        )
        .await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), false)
            .await
            .unwrap();

        assert_eq!(stored(&*storage, &to).await, Vec::<(String, String)>::new());
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_that_fails_gives_the_error_and_the_target_has_no_config() {
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                if op_label == "copy" && path.starts_with("index") {
                    Script::Refuse
                } else {
                    Script::Pass
                }
            });
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;

        let copied = copy_scope(&files(&storage, &from), &files(&storage, &to), false).await;

        assert_eq!(
            (
                copied.is_err(),
                stored(&*storage, &to)
                    .await
                    .into_iter()
                    .map(|(path, _)| path)
                    .collect::<Vec<_>>()
            ),
            (
                true,
                vec!["data/ab/abab".to_string(), "keys/efef".to_string()]
            )
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_deleted_scope_holds_no_blob_and_another_scope_keeps_its_blobs() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let (deleted, kept) = (new_namespace(), new_namespace());
        put_all(&*storage, &deleted, &REPOSITORY).await;
        put_all(&*storage, &kept, &REPOSITORY).await;

        delete_scope(&files(&storage, &deleted)).await.unwrap();

        assert_eq!(
            (
                stored(&*storage, &deleted).await,
                stored(&*storage, &kept).await
            ),
            (Vec::new(), owned(&REPOSITORY))
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_delete_of_a_scope_deletes_the_config_first() {
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
        let namespace = new_namespace();
        put_all(&*storage, &namespace, &REPOSITORY).await;

        delete_scope(&files(&storage, &namespace)).await.unwrap();

        assert_eq!(
            storage
                .calls()
                .into_iter()
                .filter(|(op_label, _)| *op_label == "delete_scope")
                .map(|(_, path)| path)
                .collect::<Vec<_>>(),
            vec!["config", "snapshots", "index", "keys", "data", "golem"]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_delete_of_an_unused_scope_succeeds_and_can_run_again() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let namespace = new_namespace();

        let first = delete_scope(&files(&storage, &namespace)).await;
        put_all(&*storage, &namespace, &REPOSITORY).await;
        let second = delete_scope(&files(&storage, &namespace)).await;
        let third = delete_scope(&files(&storage, &namespace)).await;

        assert_eq!(
            (
                first.is_ok(),
                second.is_ok(),
                third.is_ok(),
                stored(&*storage, &namespace).await
            ),
            (true, true, true, Vec::new())
        );
    }

    fn paths(names: &[&str]) -> Vec<Box<Path>> {
        names.iter().map(|name| Path::new(name).into()).collect()
    }

    #[test]
    fn the_catch_up_copies_the_new_blobs_while_a_listing_holds_a_new_index_file() {
        let copied = paths(&["index/a", "data/ab/1"]).into_iter().collect();

        assert_eq!(
            [
                catch_up(&copied, &paths(&["index/a", "data/ab/1"]), 1),
                catch_up(&copied, &paths(&["index/a", "data/ab/1", "data/ab/2"]), 1),
                catch_up(
                    &copied,
                    &paths(&["index/a", "index/b", "data/ab/1", "data/ab/2"]),
                    1
                ),
                catch_up(&copied, &paths(&["index/b"]), MOST_CATCH_UPS - 1),
                catch_up(&copied, &paths(&["index/b"]), MOST_CATCH_UPS),
            ],
            [
                CatchUp::Done,
                CatchUp::Done,
                CatchUp::Copy {
                    packs: paths(&["data/ab/2"]).into_boxed_slice(),
                    index: paths(&["index/b"]).into_boxed_slice(),
                },
                CatchUp::Copy {
                    packs: Box::default(),
                    index: paths(&["index/b"]).into_boxed_slice(),
                },
                CatchUp::Race,
            ]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_of_a_source_that_saves_during_it_catches_up_and_succeeds_in_one_run() {
        // A save of the source writes a new pack and a new index file after the copy listed the
        // packs, and before the copy of the snapshot files.
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                if op_label == "copy" && path.starts_with("index") {
                    Script::WaitForGate
                } else {
                    Script::Pass
                }
            });
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;
        let saving = async {
            let reached =
                futures::stream::repeat(())
                    .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                    .filter(|()| {
                        std::future::ready(storage.calls().iter().any(|(op_label, path)| {
                            *op_label == "copy" && path.starts_with("index")
                        }))
                    });
            std::pin::pin!(reached).next().await;
            put_all(
                &*storage,
                &from,
                &[("data/cd/cdcd", "new pack"), ("index/efef", "new index")],
            )
            .await;
            storage.open_gate();
        };

        let (from_files, to_files) = (files(&storage, &from), files(&storage, &to));
        let (copied, ()) = tokio::join!(copy_scope(&from_files, &to_files, false), saving);
        let target = stored(&*storage, &to)
            .await
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<_>>();

        assert!(copied.is_ok(), "{copied:?}");
        assert_eq!(
            target,
            vec![
                "config",
                "data/ab/abab",
                "data/cd/cdcd",
                "index/cdcd",
                "index/efef",
                "keys/efef",
                "snapshots/0101"
            ]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_catch_up_whose_new_pack_a_prune_removed_leaves_no_index_file_of_it_in_to() {
        // A save of the source writes a new pack and a new index file after the copy listed the
        // packs. A prune then removes the new pack before the catch-up copies it.
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                if op_label == "copy" && path.starts_with("index") {
                    Script::WaitForGate
                } else if op_label == "copy" && path == Path::new("data/cd/cdcd") {
                    Script::Vanish
                } else {
                    Script::Pass
                }
            });
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;
        let saving = async {
            let reached =
                futures::stream::repeat(())
                    .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                    .filter(|()| {
                        std::future::ready(storage.calls().iter().any(|(op_label, path)| {
                            *op_label == "copy" && path.starts_with("index")
                        }))
                    });
            std::pin::pin!(reached).next().await;
            put_all(
                &*storage,
                &from,
                &[("data/cd/cdcd", "new pack"), ("index/efef", "new index")],
            )
            .await;
            storage.open_gate();
        };

        let (from_files, to_files) = (files(&storage, &from), files(&storage, &to));
        let (copied, ()) = tokio::join!(copy_scope(&from_files, &to_files, false), saving);
        let target = stored(&*storage, &to)
            .await
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<_>>();

        assert!(
            matches!(&copied, Err(CopyError::CopySourceMissing { path }) if **path == *Path::new("data/cd/cdcd")),
            "{copied:?}"
        );
        assert!(
            !target.iter().any(|path| path == "index/efef"),
            "an index file whose pack is gone landed in the target: {target:?}"
        );
    }

    /// A source that gets a new index file before each listing of its index files never lets the
    /// catch-up end: the copy gives `Race` after exactly [`MOST_CATCH_UPS`] rounds. The source stops
    /// growing after four times that many listings, so a copy that counts its rounds wrong ends.
    #[test]
    #[timeout("60s")]
    async fn a_copy_whose_source_gets_a_new_index_file_at_each_listing_gives_race_after_the_last_round()
     {
        let inner = Arc::new(InMemoryBlobStorage::new());
        let from = new_namespace();
        let listings = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let storage = {
            let (inner, from, listings) = (inner.clone(), from.clone(), listings.clone());
            ScriptedBlobStorage::in_namespaces(inner.clone(), move |namespace, op_label, path| {
                if op_label == "copy_list" && namespace == &from && path == Path::new("index") {
                    let listing = listings.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if listing < 4 * MOST_CATCH_UPS {
                        // Each listing runs alone, so the in-memory store takes the write at once.
                        futures::executor::block_on(inner.put_raw(
                            "test",
                            "test",
                            from.clone(),
                            &Path::new("index").join(format!("{listing:08x}")),
                            b"new index",
                        ))
                        .unwrap();
                    }
                }
                Script::Pass
            })
        };
        let to = new_namespace();
        put_all(&*storage, &from, &REPOSITORY).await;

        let copied = copy_scope(&files(&storage, &from), &files(&storage, &to), false).await;

        assert!(matches!(copied, Err(CopyError::Race)), "{copied:?}");
        // The first listing of the copy, then one listing for each round.
        assert_eq!(
            listings.load(std::sync::atomic::Ordering::SeqCst),
            1 + MOST_CATCH_UPS
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_run_again_removes_the_snapshot_files_that_its_source_no_longer_has() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;
        put_all(&*storage, &to, &[("snapshots/0202", "of an earlier run")]).await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), true)
            .await
            .unwrap();

        assert_eq!(
            stored(&*storage, &to)
                .await
                .into_iter()
                .filter(|(path, _)| path.starts_with("snapshots"))
                .collect::<Vec<_>>(),
            owned(&[("snapshots/0101", "snapshot")])
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_run_again_whose_source_is_gone_leaves_to_without_snapshots() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let (from, to, first) = (new_namespace(), new_namespace(), new_namespace());
        put_all(&*storage, &to, &[("snapshots/0202", "of an earlier run")]).await;
        put_all(&*storage, &first, &[("snapshots/0303", "kept")]).await;

        copy_scope(&files(&storage, &from), &files(&storage, &to), true)
            .await
            .unwrap();
        copy_scope(&files(&storage, &from), &files(&storage, &first), false)
            .await
            .unwrap();

        assert_eq!(
            (
                stored(&*storage, &to).await,
                stored(&*storage, &first).await
            ),
            (Vec::new(), owned(&[("snapshots/0303", "kept")]))
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_copy_whose_listed_pack_a_prune_removed_ends_with_a_missing_source() {
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                if op_label == "copy" && path.starts_with("data") {
                    Script::Vanish
                } else {
                    Script::Pass
                }
            });
        let (from, to) = (new_namespace(), new_namespace());
        put_all(&*storage, &from, &REPOSITORY).await;

        let copied = copy_scope(&files(&storage, &from), &files(&storage, &to), false).await;

        assert!(
            matches!(&copied, Err(CopyError::CopySourceMissing { path }) if **path == *Path::new("data/ab/abab")),
            "{copied:?}"
        );
    }
}

mod drains {

    //! The drain of a delete of all snapshots: it waits for the store work of its incarnation that
    //! began before it, also for the work that a dropped call left.

    use super::*;
    use crate::filesystem_snapshot::CallError;
    use crate::filesystem_snapshot::backend::rustic::test_support::*;
    use pretty_assertions::assert_eq;
    use test_r::{test, timeout};
    /// The time that a test gives a delete of all snapshots that must wait, before it looks.
    const WAIT: Duration = Duration::from_millis(200);

    /// The work that a call of the store left for its incarnation, held at the gate of the storage.
    #[derive(Clone, Copy, Debug)]
    enum Left {
        /// The retry of a final marker whose first write failed, which the drop of the claim starts.
        FailedFinalMarker,
        /// The release of a claim whose claim write failed, which the drop of the claim starts.
        ReleaseAfterAFailedClaimWrite,
        /// The release of a claim of a delete that its caller dropped.
        DroppedDelete,
    }

    impl Left {
        /// The operation label of the held call of the work.
        fn label(self) -> &'static str {
            match self {
                Left::FailedFinalMarker => "final_marker",
                Left::ReleaseAfterAFailedClaimWrite => "delete_marker",
                Left::DroppedDelete => "delete_claim",
            }
        }

        /// A storage that holds the work at its gate, after the call that makes the delete leave it.
        fn storage(self) -> Arc<ScriptedBlobStorage> {
            let (claimed, final_markers) = (AtomicBool::new(false), AtomicUsize::new(0));
            ScriptedBlobStorage::new(
                Arc::new(InMemoryBlobStorage::new()),
                move |op_label, _| match (self, op_label) {
                    (Left::FailedFinalMarker, "final_marker") => {
                        if final_markers.fetch_add(1, Ordering::SeqCst) == 0 {
                            Script::Refuse
                        } else {
                            Script::WaitForGate
                        }
                    }
                    (Left::ReleaseAfterAFailedClaimWrite, "write_claim") => Script::Refuse,
                    (Left::ReleaseAfterAFailedClaimWrite, "delete_marker") => Script::WaitForGate,
                    (Left::DroppedDelete, "write_claim") => {
                        claimed.store(true, Ordering::SeqCst);
                        Script::Pass
                    }
                    (Left::DroppedDelete, "read_ledger") if claimed.load(Ordering::SeqCst) => {
                        Script::WaitForGate
                    }
                    (Left::DroppedDelete, "delete_claim") => Script::WaitForGate,
                    _ => Script::Pass,
                },
            )
        }
    }

    /// Gives the number of calls of a delete of all snapshots in the calls.
    fn scope_deletes(calls: &[(&'static str, String)]) -> usize {
        calls
            .iter()
            .filter(|(op_label, _)| *op_label == "delete_scope")
            .count()
    }

    /// Gives the number of calls with the operation label in the calls.
    /// Gives the number of incarnations whose work the store holds.
    fn live_incarnations(store: &RusticSnapshotStore) -> usize {
        store.operations.live_incarnations()
    }

    /// What a delete of all snapshots did after a delete that left the work `left`: whether the work
    /// was held, whether the delete of all made no call while it was held, its answer, whether the
    /// store work ended, and the blobs of the scope after it.
    async fn a_delete_all_after(
        left: Left,
    ) -> (bool, bool, Result<(), CallError>, bool, Vec<String>) {
        let storage = left.storage();
        let store = store(
            storage.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = new_scope();
        save_each(&store, &scope, &["p-1", "p-2"]).await;
        let deleting = AbortOnDropHandle::new(tokio::spawn({
            let (store, scope) = (store.clone(), scope.clone());
            async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
        }));
        let held = match left {
            Left::FailedFinalMarker => {
                eventually(|| calls_with(&storage.calls(), left.label()) == 2).await
            }
            Left::ReleaseAfterAFailedClaimWrite => {
                eventually(|| calls_with(&storage.calls(), left.label()) == 1).await
            }
            Left::DroppedDelete => {
                let claimed = eventually(|| calls_with(&storage.calls(), "read_ledger") >= 2).await;
                deleting.abort();
                claimed && eventually(|| calls_with(&storage.calls(), left.label()) == 1).await
            }
        };

        let deleting_all = AbortOnDropHandle::new(tokio::spawn({
            let (store, scope) = (store.clone(), scope.clone());
            async move { store.delete_all(&scope, &Unlimited).await }
        }));
        tokio::time::sleep(WAIT).await;
        let waited = scope_deletes(&storage.calls()) == 0 && !deleting_all.is_finished();
        storage.open_gate();
        let deleted_all = tokio::time::timeout(LIMIT, deleting_all)
            .await
            .unwrap()
            .unwrap();
        let ended = eventually(|| store.work_in_flight() == 0).await;

        (
            held,
            waited,
            deleted_all,
            ended,
            blobs(&*storage, scope.namespace(), "").await,
        )
    }

    #[test]
    #[timeout("60s")]
    async fn a_delete_all_waits_for_store_work_of_its_agent_that_a_dropped_call_left() {
        let outcomes = futures::stream::iter([
            Left::FailedFinalMarker,
            Left::ReleaseAfterAFailedClaimWrite,
            Left::DroppedDelete,
        ])
        .then(|left| async move {
            let (held, waited, deleted_all, ended, left_blobs) = a_delete_all_after(left).await;
            (left, held, waited, deleted_all.is_ok(), ended, left_blobs)
        })
        .map(|(left, held, waited, deleted_all, ended, left_blobs)| {
            format!("{left:?}: {held} {waited} {deleted_all} {ended} {left_blobs:?}")
        })
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            outcomes,
            vec![
                "FailedFinalMarker: true true true true []",
                "ReleaseAfterAFailedClaimWrite: true true true true []",
                "DroppedDelete: true true true true []",
            ]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_delete_leaves_no_store_work_in_flight_when_it_returns() {
        // The listing of the packs takes 300 ms, so the claim gets a refresh while the prune runs, and
        // the gate holds each refresh write. The end of the prune drops the refresh.
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
                if op_label == "list" && path == Path::new("data") {
                    Script::Delay(Duration::from_millis(300))
                } else if op_label == "refresh_claim" {
                    Script::WaitForGate
                } else {
                    Script::Pass
                }
            });
        let store = store(
            storage.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_millis(400)),
        );
        let scope = new_scope();
        save_each(&store, &scope, &["p-1", "p-2"]).await;

        let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

        assert!(deleted.is_ok(), "{deleted:?}");
        assert_eq!(
            (
                calls_with(&storage.calls(), "refresh_claim") > 0,
                prunes(&storage.calls()),
                store.work_in_flight(),
                live_incarnations(&store),
            ),
            (true, 1, 0, 0)
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_second_delete_all_during_the_drain_of_the_first_waits_for_the_save_before_both() {
        // The gate holds the pack write of the save. Both deletes of all snapshots begin while it is
        // held, the second one during the drain of the first one.
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
        let tree = one_file_tree("held");
        let saving = AbortOnDropHandle::new(tokio::spawn({
            let (store, scope) = (store.clone(), scope.clone());
            let tree = tree.path().to_path_buf();
            async move {
                store
                    .save(
                        &scope,
                        &name("p-1"),
                        &tree,
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await
            }
        }));
        let held = eventually(|| calls_with(&storage.calls(), "write") > 0).await;
        let delete_all = || {
            let (store, scope) = (store.clone(), scope.clone());
            AbortOnDropHandle::new(tokio::spawn(async move {
                store.delete_all(&scope, &Unlimited).await
            }))
        };
        let first = delete_all();
        tokio::time::sleep(WAIT).await;
        let second = delete_all();

        tokio::time::sleep(WAIT).await;
        let waited =
            scope_deletes(&storage.calls()) == 0 && !first.is_finished() && !second.is_finished();
        storage.open_gate();
        let saved = tokio::time::timeout(LIMIT, saving).await;
        let first = tokio::time::timeout(LIMIT, first).await;
        let second = tokio::time::timeout(LIMIT, second).await;

        assert!(matches!(saved, Ok(Ok(Ok(_)))), "{saved:?}");
        assert!(matches!(first, Ok(Ok(Ok(())))), "{first:?}");
        assert!(matches!(second, Ok(Ok(Ok(())))), "{second:?}");
        assert_eq!(
            (held, waited, listed_names(&store, &scope).await),
            (true, true, Vec::<String>::new())
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_delete_all_after_a_dropped_delete_all_waits_for_the_save_before_both() {
        // The gate holds the pack write of the save. The first delete of all snapshots begins its
        // drain while the write is held, and its caller drops it. The second one begins after that.
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
        let tree = one_file_tree("held");
        let saving = AbortOnDropHandle::new(tokio::spawn({
            let (store, scope) = (store.clone(), scope.clone());
            let tree = tree.path().to_path_buf();
            async move {
                store
                    .save(
                        &scope,
                        &name("p-1"),
                        &tree,
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await
            }
        }));
        let held = eventually(|| calls_with(&storage.calls(), "write") > 0).await;
        let delete_all = || {
            let (store, scope) = (store.clone(), scope.clone());
            AbortOnDropHandle::new(tokio::spawn(async move {
                store.delete_all(&scope, &Unlimited).await
            }))
        };
        let first = delete_all();
        tokio::time::sleep(WAIT).await;
        let first_waited = !first.is_finished();
        drop(first);
        let second = delete_all();

        tokio::time::sleep(WAIT).await;
        let waited = scope_deletes(&storage.calls()) == 0 && !second.is_finished();
        storage.open_gate();
        let saved = tokio::time::timeout(LIMIT, saving).await;
        let second = tokio::time::timeout(LIMIT, second).await;

        assert!(matches!(saved, Ok(Ok(Ok(_)))), "{saved:?}");
        assert!(matches!(second, Ok(Ok(Ok(())))), "{second:?}");
        assert_eq!(
            (
                held,
                first_waited,
                waited,
                listed_names(&store, &scope).await
            ),
            (true, true, true, Vec::<String>::new())
        );
    }
}
#[test]
fn copy_end_runs_a_race_again_and_maps_a_failed_call_and_a_name_error() {
    let error = || anyhow::anyhow!("error");

    assert_eq!(
        [
            copy_end(&CopyError::CopySourceMissing {
                path: Path::new("data/ab/abab").into()
            }),
            copy_end(&CopyError::Race),
            copy_end(&CopyError::CallFailed(error())),
            copy_end(&CopyError::Permanent(error())),
            copy_end(&CopyError::Cancelled(error())),
        ],
        [
            RunEnd::RaceRunAgain,
            RunEnd::RaceRunAgain,
            RunEnd::CallFailed,
            RunEnd::Permanent,
            RunEnd::Cancelled,
        ]
    );
}
