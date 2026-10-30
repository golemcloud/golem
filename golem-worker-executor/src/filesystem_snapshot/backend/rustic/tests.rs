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

use super::RusticSnapshotStore;
use super::repository::open_existing;
use super::store_policy::StorePolicy;
use super::test_support::scripted::{Script, ScriptedBlobStorage};
use super::test_support::{
    ALWAYS, LIMIT, LONG_DEADLINE, NEVER, REACH_LIMIT, changed_tree, config, drop_when, eventually,
    files_of, fixture_tree, is_stopped, key, ledger, name, one_file_tree, policy, polled_until,
    prunes, restored_listing, save_each, save_numbered, store, store_with_deadline, stored_paths,
    written_times,
};
use crate::filesystem_snapshot::clock::SystemClock;
use crate::filesystem_snapshot::test_support::clock::TestClock;
use crate::filesystem_snapshot::test_support::fixture::{Scratch, Spec, listing, write_tree};
use crate::filesystem_snapshot::test_support::{OpenStore, never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    AgentSnapshots, FilesystemSnapshotStore, RestoreFailure, RunSlots, Slot, SnapshotName,
    Unlimited, Withdrawal,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use futures::{FutureExt, StreamExt, TryStreamExt};
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::core::DynamicTestRegistration;
use test_r::{test, test_gen, timeout};
use tokio_util::task::AbortOnDropHandle;

/// Measures each operation of the store on one repository that holds 1, 64, 128, 256 and 1024
/// snapshots, and prints the time, the reads of snapshot and index files, the change of resident
/// memory, and the counts of a copy. It is slow, so it runs only when asked for:
/// `cargo test -p golem-worker-executor --lib -- --ignored a_repository_with_many_snapshots_stays_usable --nocapture`.
#[test]
#[ignore = "a measurement that fills a repository with 1024 snapshots; run it on request"]
#[timeout("3600s")]
async fn a_repository_with_many_snapshots_stays_usable() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let counting = Arc::new(
        crate::filesystem_snapshot::backend::rustic::test_support::counting::CountingBlobStorage::new(
            inner.clone(),
            Duration::ZERO,
        ),
    );
    let store_with_deadline = store_with_deadline(counting.clone(), STORAGE_CALL_DEADLINE);
    let scope = new_scope();
    let sizes = [1usize, 64, 128, 256, 1024];

    futures::stream::iter(sizes.iter().enumerate())
        .fold(0usize, |filled, (round, size)| {
            let (store_with_deadline, counting, inner, scope) = (&store_with_deadline, &counting, &inner, &scope);
            async move {
                save_numbered(store_with_deadline, scope, filled..size - 1).await;
                let next = size - 1;
                let tree = changed_tree(&format!("content {next}"));
                let own = name(&format!("p-{next}"));
                let (saved, save) = measured(
                    counting,
                    "save",
                    store_with_deadline.save(
                        scope,
                        &own,
                        tree.path(),
                        None,
                        never_cancelled(), &Unlimited,
                    ),
                )
                .await;
                let into = Scratch::new();
                let (restored, restore) =
                    measured(counting, "restore", store_with_deadline.restore(scope, &own, into.path(), &Unlimited)).await;
                let (stated, stat) = measured(counting, "stat", store_with_deadline.stat(scope, &own)).await;
                let (listed, list) = measured(counting, "list", store_with_deadline.list(scope, &Unlimited)).await;
                let target = new_scope();
                let (copied, copy) =
                    measured(counting, "copy_all", store_with_deadline.copy_all(scope, &target, &Unlimited)).await;
                let copy_counts = (
                    counting.count_of("copy_read"),
                    counting.count_of("copy_write"),
                    counting.count_of("copy"),
                );
                let index_files = stored_paths(inner, scope)
                    .await
                    .into_iter()
                    .filter(|path| path.starts_with("index/"))
                    .count();
                let (deleted, delete) =
                    measured(counting, "delete", store_with_deadline.delete(scope, std::slice::from_ref(&own), &Unlimited)).await;
                let resave = changed_tree(&format!("content {next}"));
                store_with_deadline
                    .save(
                        scope,
                        &own,
                        resave.path(),
                        None,
                        never_cancelled(), &Unlimited,
                    )
                    .await
                    .unwrap();

                assert!(saved.is_ok() && restored.is_ok() && copied.is_ok() && deleted.is_ok());
                assert!(stated.is_ok_and(|info| info.is_some()));
                assert_eq!(listed.map(|listing| listing.len()).ok(), Some(*size));
                assert!(
                    save.snapshot_reads <= *size as u64 + 2,
                    "a save at {size} snapshots read {} snapshot files",
                    save.snapshot_reads
                );
                println!(
                    "MEASURED round {round} size {size} index_files {index_files} copy_reads_writes_copies {copy_counts:?}"
                );
                [save, restore, stat, list, copy, delete]
                    .iter()
                    .for_each(|measured| {
                        println!(
                            "MEASURED size {size} {} took {:?}, read {} snapshot files and {} index files, resident memory changed by {} KiB",
                            measured.operation,
                            measured.elapsed,
                            measured.snapshot_reads,
                            measured.index_reads,
                            measured.resident_kib
                        )
                    });
                *size
            }
        })
        .await;
}

#[test_gen]
fn rustic_store_keeps_the_contract(r: &mut DynamicTestRegistration) {
    crate::filesystem_snapshot::tests::register_contract(r, || {
        let storage: Arc<dyn BlobStorage> = Arc::new(InMemoryBlobStorage::new());
        let open: OpenStore = Arc::new(move || {
            Arc::new(RusticSnapshotStore::new(storage.clone(), &config()))
                as Arc<dyn FilesystemSnapshotStore>
        });
        open
    });
}

#[test]
#[timeout("120s")]
async fn a_prune_during_a_save_keeps_the_packs_of_the_save() {
    // The first index write after the arm waits at the gate. That is the index write of the
    // second save, so its packs are in no index while the delete prunes. The test stops when the
    // save does not reach that write before the delete runs. The prune of the delete writes an
    // index file too, so the gate would then hold the prune until the test times out.
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
        .save(
            &scope,
            &name("p-old"),
            old_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
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

    let saving = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        let path = new_tree.path().to_path_buf();
        async move {
            store
                .save(
                    &scope,
                    &name("p-new"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let held = polled_until(REACH_LIMIT, || index_writes() > before).await;
    assert!(held, "the save did not reach its index write");
    let deleted = store.delete(&scope, &[name("p-old")], &Unlimited).await;
    let pruned_while_held = ledger(&storage, &scope).await.last_prune.is_some();
    storage.open_gate();
    let saved = saving.await.unwrap();
    let pruned_again = store.delete(&scope, &[name("p-none")], &Unlimited).await;

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
#[timeout("60s")]
async fn a_save_and_a_prune_take_their_times_from_the_injected_clock() {
    // The clock of the store is one hour ahead of the clock of the host. The time of each save,
    // each claim marker, the final marker and the ledger entry comes from the clock of the store.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let clock = Arc::new(TestClock::default());
    clock.set_ahead(Duration::from_secs(3600));
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage.clone(),
        key(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
        clock.clone(),
    ));
    let scope = new_scope();
    let ahead = golem_common::model::Timestamp::now_utc().to_millis() + 3_600_000;
    let tree = one_file_tree("timed");

    let saved = store
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
    save_each(&store, &scope, &["p-2"]).await;
    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    let calls = storage.calls();
    let written = ["write_marker", "final_marker", "write_ledger"]
        .map(|op_label| written_times(&calls, op_label));

    assert_eq!(
        (
            saved.created_at.to_millis() >= ahead,
            written.clone().map(|times| times.len()),
            written.iter().flatten().all(|time| *time >= ahead),
        ),
        (true, [1, 1, 1], true),
        "{written:?}"
    );
}

#[test]
#[timeout("60s")]
async fn a_blob_call_of_a_cancelled_operation_does_not_start() {
    // The in-memory storage answers at the first poll, so only the check before the call keeps
    // the call from the storage.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let files = files_of(
        storage.clone(),
        new_scope().namespace().clone(),
        LONG_DEADLINE,
        cancel,
    );

    let read = files
        .get("read_ledger", Path::new("golem/prune-ledgers/1000-0-0f0f"))
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
        if op_label == "delete_namespace" {
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
    let deleting = store.delete_all(&scope, &Unlimited);
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
        matches!(&deleted, Ok(Err(error)) if is_stopped(error)),
        "{deleted:?}"
    );
    assert_eq!(
        (pending, waited, stopped, store.work_in_flight()),
        (true, true, true, 0)
    );
}

#[test]
#[timeout("60s")]
async fn shut_down_ends_running_operations_before_it_returns() {
    // The first pack write of the save waits at the gate, and it cancels `reached`, so the test
    // shuts the store down only when the save holds a running storage call. The save runs at a low
    // priority, so the test waits for that point without a bound of its own.
    let reached = tokio_util::sync::CancellationToken::new();
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let reached = reached.clone();
        move |op_label, path| {
            if op_label == "write" && path.starts_with("data") {
                reached.cancel();
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
    reached.cancelled().await;

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let saved = tokio::time::timeout(LIMIT, saving).await;
    let later = store.stat(&scope, &name("p-held")).await;
    storage.open_gate();

    assert!(
        matches!(&saved, Ok(Ok(Err(error))) if is_stopped(error)),
        "{saved:?}"
    );
    assert!(later.as_ref().is_err_and(is_stopped), "{later:?}");
    assert_eq!((stopped, store.work_in_flight()), (true, 0));
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
                    store
                        .restore(&scope, &name("p-1"), into.path(), &Unlimited)
                        .map(|_| ()),
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
            Dropped::List => {
                drop_when(
                    &storage,
                    reached,
                    store.list(&scope, &Unlimited).map(|_| ()),
                )
                .await
            }
            Dropped::Delete => {
                drop_when(
                    &storage,
                    reached,
                    store.delete(&scope, &[name("p-1")], &Unlimited).map(|_| ()),
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

#[test]
#[timeout("120s")]
async fn a_delete_dropped_in_its_prune_while_a_save_of_the_same_agent_runs_keeps_the_saved_snapshots()
 {
    // The first listing of the packs after the arm waits at the gate. That is the prune of the
    // delete. The test drops the delete there, saves a new snapshot of the same agent, and then
    // lets the prune of the dropped delete go on.
    let hold_next_listing = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let hold_next_listing = hold_next_listing.clone();
        move |op_label, path| {
            if op_label == "list"
                && path == "data"
                && hold_next_listing.swap(false, Ordering::SeqCst)
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
    let (kept_tree, new_tree) = (one_file_tree("kept"), fixture_tree());
    save_each(&store, &scope, &["p-old"]).await;
    store
        .save(
            &scope,
            &name("p-kept"),
            kept_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    hold_next_listing.store(true, Ordering::SeqCst);

    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-old")], &Unlimited).await }
    }));
    let held = eventually(|| prunes(&storage.calls()) >= 1).await;
    deleting.abort();
    let dropped = deleting.await;
    let saved = store
        .save(
            &scope,
            &name("p-new"),
            new_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    storage.open_gate();
    let ended = eventually(|| store.work_in_flight() == 0).await;
    let deleted_again = store.delete(&scope, &[name("p-none")], &Unlimited).await;

    assert!(
        dropped.as_ref().is_err_and(|error| error.is_cancelled()),
        "{dropped:?}"
    );
    assert_eq!(
        (
            held,
            saved.is_ok(),
            ended,
            deleted_again.is_ok(),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
            restored_listing(&store, &scope, &name("p-new")).await.ok(),
        ),
        (
            true,
            true,
            true,
            true,
            Some(listing(kept_tree.path())),
            Some(listing(new_tree.path())),
        )
    );
}

/// The resident memory of the process, in KiB, from `/proc/self/status`. Zero where the file is
/// not there.
fn resident_kib() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or_default()
}

/// What one operation of the measurement cost.
struct Measured {
    operation: &'static str,
    elapsed: Duration,
    snapshot_reads: u64,
    index_reads: u64,
    resident_kib: i64,
}

/// Runs `operation` and gives what it cost on `counting`.
async fn measured<T>(
    counting: &crate::filesystem_snapshot::backend::rustic::test_support::counting::CountingBlobStorage,
    label: &'static str,
    operation: impl std::future::Future<Output = T>,
) -> (T, Measured) {
    counting.reset();
    let (resident, started) = (resident_kib(), std::time::Instant::now());
    let result = operation.await;
    let measured = Measured {
        operation: label,
        elapsed: started.elapsed(),
        snapshot_reads: counting.count("read", "snapshots"),
        index_reads: counting.count("read", "index"),
        resident_kib: resident_kib() - resident,
    };
    (result, measured)
}

/// The operation of the store that a test drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dropped {
    Restore,
    Stat,
    List,
    Delete,
}

mod marked_packs {
    //! The packs that a prune marks while a later snapshot uses them, in each order of the blob calls
    //! of a save, a delete with its prune, and a restore.
    //!
    //! `p-1` and `p-2` hold the same tree, so `p-2` uses the packs of `p-1`. Two races are swept. In
    //! the first, a save of `p-2` races a delete of `p-1` whose prune marks the packs of `p-1` when it
    //! does not see `p-2`. In the second, `p-2` already uses packs that a prune marked, and a restore
    //! of `p-2` races a later prune that sees `p-2` and takes the packs back. A case holds one side at
    //! the gate before its blob call `k`, runs the other side to its end, and then opens the gate, as
    //! the race sweeps of the store do. The two sides go through two stores over one storage, as two
    //! executors do.
    //!
    //! After the race, each case checks the rule of a marked pack: it stays on the storage until a
    //! prune that sees no snapshot that uses it. A restore of `p-2` gives the whole tree in its first
    //! run, before and after a later prune. A delete of `p-2` lets its prune mark the packs, and the
    //! next prune deletes them. The later prunes run on clocks that are hours ahead, so the store counts
    //! each one past the grace period of the one before it. rustic dates a mark with the clock of the
    //! host, so the last prune has no grace period.

    use super::*;
    use crate::filesystem_snapshot::backend::rustic::test_support::*;
    use crate::filesystem_snapshot::{RunSlots, Slot, Withdrawal};
    use futures::future::BoxFuture;
    use rustic_core::repofile::{IndexFile, IndexId};
    use std::sync::Mutex;
    use test_r::{test, timeout};

    /// The most blob calls of one side that the sweep holds.
    const MOST_CALLS: usize = 96;

    /// How long a pack that a prune marks stays.
    const GRACE: Duration = Duration::from_secs(3600);

    /// What a sweep races.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Raced {
        /// A save of `p-2` and a delete of `p-1` whose prune can mark the packs of `p-2`.
        SaveAndMark,
        /// A restore of `p-2`, which uses marked packs, and a later prune that takes them back.
        RestoreAndRecover,
    }

    /// The side that a case holds at the gate.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Held {
        /// The save in [`Raced::SaveAndMark`], the restore in [`Raced::RestoreAndRecover`].
        First,
        /// The delete of the other side.
        Second,
    }

    /// A limiter without a bound that counts its takes. A call takes one slot for each run.
    #[derive(Default)]
    struct Takes(AtomicUsize);

    impl Takes {
        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl RunSlots for Takes {
        fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Unlimited.take(immediate)
        }

        fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
            Box::pin(std::future::pending())
        }
    }

    /// A store over the storage whose clock is `ahead` of the clock of the host, with a prune at each
    /// delete that frees bytes, and the grace period [`GRACE`].
    fn store_ahead(storage: Arc<ScriptedBlobStorage>, ahead: Duration) -> Arc<RusticSnapshotStore> {
        store_with_grace(storage, ahead, GRACE)
    }

    /// A store over the storage whose clock is `ahead` of the clock of the host, with a prune at each
    /// delete that frees bytes, and the grace period `grace`.
    fn store_with_grace(
        storage: Arc<ScriptedBlobStorage>,
        ahead: Duration,
        grace: Duration,
    ) -> Arc<RusticSnapshotStore> {
        let clock = Arc::new(TestClock::default());
        clock.set_ahead(ahead);
        Arc::new(RusticSnapshotStore::with_policy(
            storage,
            key(),
            policy(LONG_DEADLINE, ALWAYS, grace),
            clock,
        ))
    }

    /// Saves the tree at `path` under `name`.
    async fn save_tree(
        store: &RusticSnapshotStore,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        path: &Path,
    ) -> Result<(), Box<str>> {
        store
            .save(agent, name, path, None, never_cancelled(), &Unlimited)
            .await
            .map(|_| ())
            .map_err(|error| format!("the save of {name} gave {error:?}").into())
    }

    /// Deletes `name`.
    async fn delete_name(
        store: &RusticSnapshotStore,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<(), Box<str>> {
        store
            .delete(agent, std::slice::from_ref(name), &Unlimited)
            .await
            .map_err(|error| format!("the delete of {name} gave {error:?}").into())
    }

    /// Restores `p-2`, and checks that the restore gives `tree` in its first run.
    async fn restore_whole(
        store: &RusticSnapshotStore,
        agent: &AgentSnapshots,
        tree: &Path,
        when: &str,
    ) -> Result<(), Box<str>> {
        let takes = Takes::default();
        let into = Scratch::new();
        match store
            .restore(agent, &name("p-2"), into.path(), &takes)
            .await
        {
            Ok(_) if listing(into.path()) == listing(tree) && takes.count() == 1 => Ok(()),
            Ok(_) => Err(format!(
                "the restore {when} gave another tree or ran {} times",
                takes.count()
            )
            .into()),
            Err(error) => Err(format!("the restore {when} gave {error:?}").into()),
        }
    }

    /// Gives the packs of the scope.
    async fn packs(storage: &ScriptedBlobStorage, agent: &AgentSnapshots) -> Box<[Box<str>]> {
        blobs(storage, agent.namespace(), "data/")
            .await
            .into_iter()
            .map(String::into_boxed_str)
            .collect()
    }

    /// Gives whether an index file of the scope marks a pack for deletion. The restore sweep starts
    /// with marked packs, which the prune of its race takes back, so only the save sweep counts them.
    async fn any_marked(storage: Arc<ScriptedBlobStorage>, agent: &AgentSnapshots) -> bool {
        let backend = backend_of(storage, agent, LONG_DEADLINE);
        crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking(move || {
            let repository = open_existing(backend, &key())?.expect("the repository is there");
            repository
                .list::<IndexId>()?
                .collect::<Vec<_>>()
                .iter()
                .try_fold(false, |marked, id| {
                    Ok::<_, anyhow::Error>(
                        marked
                            || !repository
                                .get_file::<IndexFile>(id)?
                                .packs_to_delete
                                .is_empty(),
                    )
                })
        })
        .await
        .unwrap()
    }

    /// One side of a race: a call whose error is the reason that it broke the rule.
    type Side = BoxFuture<'static, Result<(), Box<str>>>;

    /// What a case observed: the held call, whether a prune marked a pack, and the breaks of the
    /// rule.
    struct Judged {
        held_call: (Box<str>, Box<str>),
        marked: bool,
        broken: Box<[Box<str>]>,
    }

    /// Runs the case that holds the side before its blob call `k`, and judges it. Gives `None` when
    /// the side ends before its call `k`.
    async fn case(raced: Raced, held: Held, k: usize) -> Option<Judged> {
        let armed = Arc::new(AtomicBool::new(false));
        let counted = Arc::new(AtomicUsize::new(0));
        let held_call = Arc::new(Mutex::new(None::<(Box<str>, Box<str>)>));
        let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
            let (armed, counted, held_call) = (armed.clone(), counted.clone(), held_call.clone());
            move |op_label, path| {
                if armed.load(Ordering::SeqCst) && counted.fetch_add(1, Ordering::SeqCst) + 1 == k {
                    *held_call.lock().unwrap() =
                        Some((op_label.into(), path.display().to_string().into()));
                    Script::WaitForGate
                } else {
                    Script::Pass
                }
            }
        });
        let scope = new_scope();
        let tree = fixture_tree();
        let filler = one_file_tree("filler");
        let first = store_ahead(storage.clone(), Duration::ZERO);
        let second = store_ahead(storage.clone(), Duration::ZERO);
        save_tree(&first, &scope, &name("p-1"), tree.path())
            .await
            .unwrap();
        let packs_of_the_tree = packs(&storage, &scope).await;

        let (first_side, second_side, later): (Side, Side, Arc<RusticSnapshotStore>) = match raced {
            Raced::SaveAndMark => {
                let (saving, marking) = (first.clone(), second.clone());
                let (saving_scope, marking_scope) = (scope.clone(), scope.clone());
                let path: Box<Path> = tree.path().into();
                (
                    Box::pin(async move {
                        save_tree(&saving, &saving_scope, &name("p-2"), &path).await
                    }),
                    Box::pin(
                        async move { delete_name(&marking, &marking_scope, &name("p-1")).await },
                    ),
                    store_ahead(storage.clone(), 2 * GRACE),
                )
            }
            Raced::RestoreAndRecover => {
                hide_while_marked(&first, &storage, &scope, tree.path()).await;
                let later = store_ahead(storage.clone(), 2 * GRACE);
                save_tree(&later, &scope, &name("p-3"), filler.path())
                    .await
                    .unwrap();
                let (restoring, recovering) = (first.clone(), later.clone());
                let (restoring_scope, recovering_scope) = (scope.clone(), scope.clone());
                let path: Box<Path> = tree.path().into();
                (
                    Box::pin(async move {
                        restore_whole(&restoring, &restoring_scope, &path, "during the race").await
                    }),
                    Box::pin(async move {
                        delete_name(&recovering, &recovering_scope, &name("p-3")).await
                    }),
                    later,
                )
            }
        };
        armed.store(true, Ordering::SeqCst);
        let is_held = || held_call.lock().unwrap().is_some();
        let (held_side, other_side) = match held {
            Held::First => (first_side, second_side),
            Held::Second => (second_side, first_side),
        };
        let held_task = tokio::spawn(held_side);
        assert!(polled_until(LIMIT, || is_held() || held_task.is_finished()).await);
        if !is_held() {
            storage.open_gate();
            let _ = held_task.await;
            return None;
        }
        let other = tokio::time::timeout(LIMIT, other_side).await.unwrap();
        storage.open_gate();
        let held_result = tokio::time::timeout(LIMIT, held_task)
            .await
            .unwrap()
            .unwrap();
        armed.store(false, Ordering::SeqCst);
        let marked = any_marked(storage.clone(), &scope).await;

        let mut broken: Vec<Box<str>> = [held_result, other]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        if broken.is_empty() {
            broken.extend(
                the_marked_packs_stay_until_no_snapshot_uses_them(
                    &first,
                    &later,
                    &storage,
                    &scope,
                    tree.path(),
                    filler.path(),
                    &packs_of_the_tree,
                )
                .await,
            );
        }
        first.shut_down().await;
        second.shut_down().await;
        later.shut_down().await;
        let held_call = held_call.lock().unwrap().clone().unwrap();
        Some(Judged {
            held_call,
            marked,
            broken: broken.into_boxed_slice(),
        })
    }

    /// Saves `p-2` with the same tree as `p-1`, and then lets a prune that does not see the snapshot
    /// file of `p-2` mark its packs after a delete of `p-1`, as a prune that read the snapshots before
    /// the publish of `p-2` does.
    async fn hide_while_marked(
        store: &RusticSnapshotStore,
        storage: &Arc<ScriptedBlobStorage>,
        agent: &AgentSnapshots,
        tree: &Path,
    ) {
        save_tree(store, agent, &name("p-2"), tree).await.unwrap();
        let (_, unused) = mark_the_packs_of_a_hidden_snapshot(storage.clone(), agent, GRACE).await;
        assert!(unused);
    }

    /// Checks the rule of a marked pack after the race: a restore of `p-2` gives the whole tree in
    /// its first run before and after a later prune, the packs of the tree stay while `p-2` is there,
    /// and the prune after the delete of `p-2`, one grace period after the prune that marked them,
    /// deletes them. Gives the first break of the rule.
    async fn the_marked_packs_stay_until_no_snapshot_uses_them(
        store: &RusticSnapshotStore,
        later: &RusticSnapshotStore,
        storage: &Arc<ScriptedBlobStorage>,
        agent: &AgentSnapshots,
        tree: &Path,
        filler: &Path,
        packs_of_the_tree: &[Box<str>],
    ) -> Option<Box<str>> {
        let kept = |when: &'static str| async move {
            let left = packs(storage, agent).await;
            if packs_of_the_tree.iter().all(|pack| left.contains(pack)) {
                Ok(())
            } else {
                Err(format!("a pack of the tree was gone {when}").into_boxed_str())
            }
        };
        let steps = async {
            restore_whole(store, agent, tree, "after the race").await?;
            // A later prune, which sees `p-2`.
            save_tree(later, agent, &name("p-4"), filler).await?;
            delete_name(later, agent, &name("p-4")).await?;
            kept("after a prune that sees p-2").await?;
            restore_whole(store, agent, tree, "after a later prune").await?;
            // The delete of `p-2`, whose prune marks the packs of the tree.
            let marking = store_ahead(storage.clone(), 4 * GRACE);
            let marked = delete_name(&marking, agent, &name("p-2")).await;
            marking.shut_down().await;
            marked?;
            kept("after the prune that marked them").await?;
            // The next prune, after the grace period, deletes them. rustic dates a mark with the clock
            // of the host, so this prune has no grace period in place of a clock that is ahead.
            let deleting = store_with_grace(storage.clone(), 6 * GRACE, Duration::ZERO);
            let deleted = delete_name(&deleting, agent, &name("p-none")).await;
            deleting.shut_down().await;
            deleted?;
            let left = packs(storage, agent).await;
            if packs_of_the_tree.iter().any(|pack| left.contains(pack)) {
                Err(
                    "a pack of the tree stayed after a prune that sees no snapshot that uses it"
                        .into(),
                )
            } else {
                Ok(())
            }
        };
        steps.await.err()
    }

    /// Runs the case at each blob call of the side. Gives the number of cases in which a prune marked
    /// a pack, and the held call with the breaks of each case that broke the rule.
    async fn swept(raced: Raced, held: Held) -> (usize, Box<[Box<str>]>) {
        // The fold ends with `Err` when the held side ends before call `k`, so that every call was
        // held once. An `Ok` after the last call means that the side made more calls than the bound.
        let swept = futures::stream::iter(1..=MOST_CALLS)
            .map(Ok::<usize, (usize, Vec<Box<str>>)>)
            .try_fold((0, Vec::new()), |(marked, mut broken), k| async move {
                let Some(judged) = case(raced, held, k).await else {
                    return Err((marked, broken));
                };
                broken.extend(
                    judged
                        .broken
                        .iter()
                        .map(|why| format!("call {k} {:?}: {why}", judged.held_call).into()),
                );
                Ok((marked + usize::from(judged.marked), broken))
            })
            .await;
        match swept {
            Err((marked, broken)) => (marked, broken.into_boxed_slice()),
            Ok(_) => {
                panic!("the {held:?} side of {raced:?} made more than {MOST_CALLS} blob calls")
            }
        }
    }

    #[test]
    #[timeout("600s")]
    async fn a_save_held_at_each_blob_call_while_a_prune_marks_its_packs_keeps_them_until_no_snapshot_uses_them()
     {
        let (marked, broken) = swept(Raced::SaveAndMark, Held::First).await;

        assert!(broken.is_empty(), "{broken:#?}");
        assert!(marked > 0, "no case marked a pack of the save");
    }

    #[test]
    #[timeout("600s")]
    async fn a_delete_held_at_each_blob_call_while_a_save_reuses_its_packs_keeps_them_until_no_snapshot_uses_them()
     {
        let (_, broken) = swept(Raced::SaveAndMark, Held::Second).await;

        assert!(broken.is_empty(), "{broken:#?}");
    }

    #[test]
    #[timeout("600s")]
    async fn a_restore_held_at_each_blob_call_while_a_prune_takes_back_its_marked_packs_gives_the_whole_tree()
     {
        let (_, broken) = swept(Raced::RestoreAndRecover, Held::First).await;

        assert!(broken.is_empty(), "{broken:#?}");
    }

    #[test]
    #[timeout("600s")]
    async fn a_prune_held_at_each_blob_call_while_a_restore_reads_its_marked_packs_gives_the_whole_tree()
     {
        let (_, broken) = swept(Raced::RestoreAndRecover, Held::Second).await;

        assert!(broken.is_empty(), "{broken:#?}");
    }
}

mod restore_prune_races {
    //! A restore of a name that races a delete of that name, as the contract case does, at each blob
    //! call of one side. A case holds one side at the gate before its blob call `k`, runs the other
    //! side to its end, and then opens the gate.
    //!
    //! In the sweeps of the own name, the store has the configured policy, as in the contract tests,
    //! so the delete of the only snapshot prunes, and the contract case judges the outcome. A variant
    //! also refuses the blob call after the held one, so a run fails at each step and the call runs
    //! again. In the sweeps of a kept name, the delete removes another snapshot whose pack also holds
    //! a file of the kept one, and its prune repacks that file into a new pack, marks the old pack,
    //! and deletes the old index files; the restore of the kept name gives the whole tree.

    use super::*;
    use crate::filesystem_snapshot::backend::rustic::test_support::*;
    use crate::filesystem_snapshot::tests::raced_restore_kept_the_contract;
    use golem_common::model::RetryConfig;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;
    use test_r::{test, timeout};

    /// The most blob calls of one side that the sweep holds.
    const MOST_CALLS: usize = 64;

    /// The side that a case holds at the gate.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Held {
        Restore,
        Delete,
    }

    /// What a sweep restores and deletes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Raced {
        /// The restore and the delete have one name, with the configured policy.
        OwnName,
        /// The same, and the blob call after the held one is refused once, with runs that come again
        /// at once.
        OwnNameWithAFailedCall,
        /// The delete removes another name, whose prune repacks a pack of the restored name.
        KeptName,
        /// The same, and the blob call after the held one is refused once.
        KeptNameWithAFailedCall,
    }

    impl Raced {
        fn kept(self) -> bool {
            matches!(self, Raced::KeptName | Raced::KeptNameWithAFailedCall)
        }

        fn with_a_failed_call(self) -> bool {
            matches!(
                self,
                Raced::OwnNameWithAFailedCall | Raced::KeptNameWithAFailedCall
            )
        }
    }

    /// The runs of a sweep with a failed call: three runs, with short waits.
    fn short_runs() -> RetryConfig {
        RetryConfig {
            max_attempts: 3,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(40),
            multiplier: 2.0,
            max_jitter_factor: None,
        }
    }

    /// The store of a sweep over `storage`.
    fn sweep_store(storage: Arc<ScriptedBlobStorage>, raced: Raced) -> Arc<RusticSnapshotStore> {
        match raced {
            Raced::OwnName => Arc::new(RusticSnapshotStore::new(storage, &config())),
            Raced::OwnNameWithAFailedCall => Arc::new(RusticSnapshotStore::with_policy(
                storage,
                key(),
                StorePolicy {
                    retry: short_runs(),
                    in_call_tries: 1,
                    ..StorePolicy::from_config(&config())
                },
                Arc::new(SystemClock),
            )),
            Raced::KeptName | Raced::KeptNameWithAFailedCall => store(
                storage,
                StorePolicy {
                    retry: short_runs(),
                    ..policy(LONG_DEADLINE, ALWAYS, Duration::ZERO)
                },
            ),
        }
    }

    /// Runs the case that holds the side before its blob call `k`. It gives the operation label and
    /// the path of the held call with the judgement, or `None` when the side ends before its call
    /// `k`.
    async fn case(
        held: Held,
        raced: Raced,
        k: usize,
    ) -> Option<((String, String), Result<(), String>)> {
        let armed = Arc::new(AtomicBool::new(false));
        let counted = Arc::new(AtomicUsize::new(0));
        let held_call = Arc::new(Mutex::new(None::<(String, String)>));
        let refused = Arc::new(AtomicBool::new(false));
        let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
            let (armed, counted, held_call, refused) = (
                armed.clone(),
                counted.clone(),
                held_call.clone(),
                refused.clone(),
            );
            move |op_label, path| {
                if !armed.load(Ordering::SeqCst) {
                    return Script::Pass;
                }
                let call = counted.fetch_add(1, Ordering::SeqCst) + 1;
                if call == k {
                    *held_call.lock().unwrap() =
                        Some((op_label.to_string(), path.display().to_string()));
                    Script::WaitForGate
                } else if call == k + 1
                    && raced.with_a_failed_call()
                    && !refused.swap(true, Ordering::SeqCst)
                {
                    Script::Refuse
                } else {
                    Script::Pass
                }
            }
        });
        let store = sweep_store(storage.clone(), raced);
        let scope = new_scope();
        let (restored_name, deleted_name) = if raced.kept() {
            (name("p-kept"), name("p-other"))
        } else {
            (name("p-raced"), name("p-raced"))
        };
        // The kept tree has the small file of the other tree. A large second file makes the prune
        // repack the pack of both: the kept file is a small share of the repository.
        let kept = new_kept_tree();
        let other = Scratch::new();
        write_tree(
            other.path(),
            &[
                ("kept.txt", file_of(b"kept file")),
                ("large.bin", file_of(&noise(256 * 1024))),
            ],
        );
        let tree = if raced.kept() { kept } else { fixture_tree() };
        if raced.kept() {
            save_tree(&store, &scope, &deleted_name, other.path()).await;
        }
        save_tree(&store, &scope, &restored_name, tree.path()).await;
        armed.store(true, Ordering::SeqCst);

        let restoring = {
            let (store, scope, restored_name) =
                (store.clone(), scope.clone(), restored_name.clone());
            async move { restored_listing(&store, &scope, &restored_name).await }
        };
        let deleting = {
            let (store, scope, deleted_name) = (store.clone(), scope.clone(), deleted_name.clone());
            async move {
                store
                    .delete(&scope, std::slice::from_ref(&deleted_name), &Unlimited)
                    .await
            }
        };
        let is_held = || held_call.lock().unwrap().is_some();
        // The side ended before call `k` gives `None`. The store shuts down on each path.
        let outcome = match held {
            Held::Restore => {
                let first = tokio::spawn(restoring);
                assert!(polled_until(LIMIT, || is_held() || first.is_finished()).await);
                if !is_held() {
                    storage.open_gate();
                    let _ = first.await;
                    None
                } else {
                    let deleted = tokio::time::timeout(LIMIT, deleting).await.unwrap();
                    storage.open_gate();
                    let restore = tokio::time::timeout(LIMIT, first).await.unwrap().unwrap();
                    Some((restore, deleted))
                }
            }
            Held::Delete => {
                let first = tokio::spawn(deleting);
                assert!(polled_until(LIMIT, || is_held() || first.is_finished()).await);
                if !is_held() {
                    storage.open_gate();
                    let _ = first.await;
                    None
                } else {
                    let restore = tokio::time::timeout(LIMIT, restoring).await.unwrap();
                    storage.open_gate();
                    let deleted = tokio::time::timeout(LIMIT, first).await.unwrap().unwrap();
                    Some((restore, deleted))
                }
            }
        };
        store.shut_down().await;
        let (restore, deleted) = outcome?;
        let held_call = held_call.lock().unwrap().clone().unwrap();
        let judged = match raced {
            Raced::OwnName | Raced::OwnNameWithAFailedCall => {
                raced_restore_kept_the_contract(&deleted, &restore, &listing(tree.path()))
            }
            Raced::KeptName | Raced::KeptNameWithAFailedCall => match (&deleted, &restore) {
                (Ok(()), Ok(restored)) if *restored == listing(tree.path()) => Ok(()),
                (deleted, restore) => Err(format!(
                    "the delete gave {deleted:?} and the restore of the kept name gave {restore:?}"
                )),
            },
        };
        Some((held_call, judged))
    }

    /// Gives `length` bytes that do not compress.
    fn noise(length: usize) -> Vec<u8> {
        std::iter::successors(Some(0x2545_f491_4f6c_dd1du64), |state| {
            Some(state ^ (state << 13) ^ (state >> 7) ^ (state << 17))
        })
        .map(|state| (state >> 24) as u8)
        .take(length)
        .collect()
    }

    /// A file with the content, at mode 0o644.
    fn file_of(content: &[u8]) -> Spec {
        Spec::File {
            content: Box::from(content),
            mode: 0o644,
        }
    }

    /// Gives a tree with the one small file that the other tree of a kept-name sweep has too.
    fn new_kept_tree() -> Scratch {
        let tree = Scratch::new();
        write_tree(tree.path(), &[("kept.txt", file_of(b"kept file"))]);
        tree
    }

    /// Saves the tree at `path` under `name`.
    async fn save_tree(
        store: &RusticSnapshotStore,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        path: &Path,
    ) {
        store
            .save(agent, name, path, None, never_cancelled(), &Unlimited)
            .await
            .unwrap();
    }

    /// Runs the case at each blob call of the side, and gives the held call and the reason of each
    /// case that broke the judgement.
    async fn broken_cases(held: Held, raced: Raced) -> Vec<String> {
        // The fold ends with `Err` when the held side ends before call `k`, so that every call was
        // held once. An `Ok` after the last call means that the side made more calls than the bound.
        let swept = futures::stream::iter(1..=MOST_CALLS)
            .map(Ok::<usize, Vec<String>>)
            .try_fold(Vec::new(), |mut broken, k| async move {
                let Some((held_call, judged)) = case(held, raced, k).await else {
                    return Err(broken);
                };
                if let Err(why) = judged {
                    broken.push(format!("call {k} {held_call:?}: {why}"));
                }
                Ok(broken)
            })
            .await;
        match swept {
            Err(broken) => broken,
            Ok(_) => panic!("the {held:?} made more than {MOST_CALLS} blob calls"),
        }
    }

    #[test]
    #[timeout("120s")]
    async fn a_restore_held_at_each_blob_call_while_a_delete_of_its_name_prunes_keeps_the_contract()
    {
        assert_eq!(
            broken_cases(Held::Restore, Raced::OwnName).await,
            Vec::<String>::new()
        );
    }

    #[test]
    #[timeout("120s")]
    async fn a_delete_held_at_each_blob_call_while_a_restore_of_its_name_runs_keeps_the_contract() {
        assert_eq!(
            broken_cases(Held::Delete, Raced::OwnName).await,
            Vec::<String>::new()
        );
    }

    #[test]
    #[timeout("180s")]
    async fn a_restore_held_at_each_blob_call_with_a_failed_call_after_it_keeps_the_contract() {
        assert_eq!(
            broken_cases(Held::Restore, Raced::OwnNameWithAFailedCall).await,
            Vec::<String>::new()
        );
    }

    #[test]
    #[timeout("180s")]
    async fn a_delete_held_at_each_blob_call_with_a_failed_call_after_it_keeps_the_contract() {
        assert_eq!(
            broken_cases(Held::Delete, Raced::OwnNameWithAFailedCall).await,
            Vec::<String>::new()
        );
    }

    #[test]
    #[timeout("180s")]
    async fn a_restore_in_each_order_of_a_prune_gives_the_answer_of_its_name() {
        let restore_held = broken_cases(Held::Restore, Raced::KeptName).await;
        let delete_held = broken_cases(Held::Delete, Raced::KeptName).await;

        assert_eq!(
            (restore_held, delete_held),
            (Vec::<String>::new(), Vec::<String>::new())
        );
    }

    #[test]
    #[timeout("180s")]
    async fn a_restore_in_each_order_of_a_prune_with_a_failed_call_gives_the_answer_of_its_name() {
        let restore_held = broken_cases(Held::Restore, Raced::KeptNameWithAFailedCall).await;
        let delete_held = broken_cases(Held::Delete, Raced::KeptNameWithAFailedCall).await;

        assert_eq!(
            (restore_held, delete_held),
            (Vec::<String>::new(), Vec::<String>::new())
        );
    }

    /// A limiter without a bound that counts its takes.
    #[derive(Default)]
    struct CountedTakes(AtomicUsize);

    impl RunSlots for CountedTakes {
        fn take(
            &self,
            immediate: bool,
        ) -> futures::future::BoxFuture<'_, Result<Slot, Withdrawal>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Unlimited.take(immediate)
        }

        fn withdrawn(&self) -> futures::future::BoxFuture<'_, Withdrawal> {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    #[timeout("60s")]
    async fn a_restore_whose_marked_pack_a_prune_deletes_for_another_copy_gives_the_whole_tree_after_one_race_run()
     {
        // The restore loads the index, which lists the pack of the kept file, and the storage holds
        // its first read of that pack. A delete of the other name prunes: it writes the kept file into
        // a new pack and marks the old one. A later prune, after the hold of the claims, deletes the
        // marked pack, because the kept file has another copy. The read of the old pack then finds no
        // blob, the index loaded again does not list the pack, and the run ends to run again; the next
        // run gives the whole tree.
        let armed = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicBool::new(false));
        let packs_of_other = Arc::new(Mutex::new(Vec::<String>::new()));
        let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
            let (armed, held, packs_of_other) =
                (armed.clone(), held.clone(), packs_of_other.clone());
            move |op_label, path| {
                let of_other = packs_of_other
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|pack| Path::new(pack) == path);
                if armed.load(Ordering::SeqCst)
                    && matches!(op_label, "read" | "read_range")
                    && of_other
                    && !held.swap(true, Ordering::SeqCst)
                {
                    Script::WaitForGate
                } else {
                    Script::Pass
                }
            }
        });
        let store = sweep_store(storage.clone(), Raced::KeptName);
        let scope = new_scope();
        let kept = new_kept_tree();
        let other = Scratch::new();
        write_tree(
            other.path(),
            &[
                ("kept.txt", file_of(b"kept file")),
                ("large.bin", file_of(&noise(256 * 1024))),
            ],
        );
        save_tree(&store, &scope, &name("p-other"), other.path()).await;
        *packs_of_other.lock().unwrap() = blobs(&*storage, scope.namespace(), "data/").await;
        save_tree(&store, &scope, &name("p-kept"), kept.path()).await;
        armed.store(true, Ordering::SeqCst);
        let slots = CountedTakes::default();
        let into = Scratch::new();

        let kept_name = name("p-kept");
        let restoring = store.restore(&scope, &kept_name, into.path(), &slots);
        tokio::pin!(restoring);
        let reached = tokio::select! {
            biased;
            reached = polled_until(LIMIT, || held.load(Ordering::SeqCst)) => reached,
            restored = &mut restoring => panic!("the restore ended before its read of the pack: {restored:?}"),
        };
        let deleted = store.delete(&scope, &[name("p-other")], &Unlimited).await;
        // The later prune runs on a clock three hours ahead, after the hold of the claims and of the
        // last prune.
        let clock = Arc::new(TestClock::default());
        clock.set_ahead(Duration::from_secs(3 * 3600));
        let later = Arc::new(RusticSnapshotStore::with_policy(
            storage.clone(),
            key(),
            StorePolicy {
                retry: short_runs(),
                ..policy(LONG_DEADLINE, ALWAYS, Duration::ZERO)
            },
            clock,
        ));
        let third = new_kept_tree();
        write_tree(third.path(), &[("third.txt", file_of(b"third file"))]);
        save_tree(&later, &scope, &name("p-third"), third.path()).await;
        let deleted_third = later.delete(&scope, &[name("p-third")], &Unlimited).await;
        let packs_after_the_prune = blobs(&*storage, scope.namespace(), "data/").await;
        storage.open_gate();
        let restored = tokio::time::timeout(LIMIT, &mut restoring).await.unwrap();
        store.shut_down().await;
        later.shut_down().await;

        assert!(reached);
        assert!(deleted.is_ok(), "{deleted:?}");
        assert!(deleted_third.is_ok(), "{deleted_third:?}");
        assert!(restored.is_ok(), "{restored:?}");
        assert!(
            packs_of_other
                .lock()
                .unwrap()
                .iter()
                .any(|pack| !packs_after_the_prune.contains(pack)),
            "the prunes deleted no pack of the other name"
        );
        assert_eq!(
            (listing(into.path()), slots.0.load(Ordering::SeqCst)),
            (listing(kept.path()), 2)
        );
    }
}

mod save_prune_ordering {
    use super::*;
    use crate::filesystem_snapshot::backend::rustic::pruning::PruneSettings;
    use crate::filesystem_snapshot::backend::rustic::test_support::*;
    use crate::filesystem_snapshot::test_support::fixture::Listed;
    use pretty_assertions::assert_eq;
    use test_r::{test, timeout};
    /// Saves `p-3` through one store while a delete of `p-1` through another store over the same
    /// storage prunes, as an upload and a clean-up of one agent do. The prune reads the index before
    /// the publish of the save when `index_read_first`, and after it otherwise. Gives the answer of
    /// the save, the restore of `p-3` after that prune, and the restore of `p-3` after the next
    /// prune.
    async fn save_and_prune(
        index_read_first: bool,
    ) -> (
        bool,
        Result<Vec<Listed>, RestoreFailure>,
        Result<Vec<Listed>, RestoreFailure>,
        Vec<Listed>,
    ) {
        let inner = Arc::new(InMemoryBlobStorage::new());
        let claimed = Arc::new(AtomicBool::new(false));
        let save_storage = ScriptedBlobStorage::new(inner.clone(), move |op_label, _| {
            if index_read_first && op_label == "publish" {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        });
        let prune_storage = ScriptedBlobStorage::new(inner.clone(), {
            let claimed = claimed.clone();
            move |op_label, path| {
                if op_label == "write_claim" {
                    claimed.store(true, Ordering::SeqCst);
                }
                if !index_read_first
                    && op_label == "list"
                    && path == Path::new("index")
                    && claimed.load(Ordering::SeqCst)
                {
                    Script::WaitForGate
                } else {
                    Script::Pass
                }
            }
        });
        let prune_policy = || policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600));
        let (saving, pruning) = (
            store(save_storage.clone(), prune_policy()),
            store(prune_storage.clone(), prune_policy()),
        );
        let scope = new_scope();
        save_each(&pruning, &scope, &["p-1", "p-2"]).await;
        let tree = one_file_tree("new tree");
        let save = || {
            let (saving, scope) = (saving.clone(), scope.clone());
            let tree = tree.path().to_path_buf();
            AbortOnDropHandle::new(tokio::spawn(async move {
                saving
                    .save(
                        &scope,
                        &name("p-3"),
                        &tree,
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await
            }))
        };
        let delete = |text: &'static str| {
            let (pruning, scope) = (pruning.clone(), scope.clone());
            AbortOnDropHandle::new(tokio::spawn(async move {
                pruning.delete(&scope, &[name(text)], &Unlimited).await
            }))
        };
        let saved = if index_read_first {
            let saving = save();
            eventually(|| calls_with(&save_storage.calls(), "publish") > 0).await;
            delete("p-1").await.unwrap().unwrap();
            save_storage.open_gate();
            saving.await.unwrap()
        } else {
            let deleting = delete("p-1");
            eventually(|| {
                claimed.load(Ordering::SeqCst)
                    && prune_storage
                        .calls()
                        .iter()
                        .any(|(op_label, path)| *op_label == "list" && path == "index")
            })
            .await;
            let saved = save().await.unwrap();
            prune_storage.open_gate();
            deleting.await.unwrap().unwrap();
            saved
        };
        let after_the_prune = restored_listing(&pruning, &scope, &name("p-3")).await;
        // The claim of the first prune holds the next prune of a delete for a whole hold, so the next
        // prune runs on the repository directly.
        let next = backend_of(inner.clone(), &scope, LONG_DEADLINE);
        crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking(move || {
            crate::filesystem_snapshot::backend::rustic::pruning::prune(
                next,
                &key(),
                &PruneSettings {
                    fast_repack: true,
                    keep_delete: Duration::from_secs(3600),
                    ..PruneSettings::default()
                },
            )
        })
        .await
        .unwrap();
        let after_the_next_prune = restored_listing(&pruning, &scope, &name("p-3")).await;
        (
            saved.is_ok(),
            after_the_prune,
            after_the_next_prune,
            listing(tree.path()),
        )
    }

    #[test]
    #[timeout("60s")]
    async fn a_save_and_a_prune_of_one_agent_in_both_orders_of_the_publish_and_the_index_read_give_whole_snapshots()
     {
        // A prune that reads the index before the publish marks the new packs of the save. A restore
        // before the next prune reads the marked packs and gives the whole tree. The next prune finds
        // the snapshot and keeps its packs.
        let outcomes = futures::stream::iter([false, true])
            .then(|index_read_first| async move {
                let (saved, after_the_prune, after_the_next_prune, tree) =
                    save_and_prune(index_read_first).await;
                let first = after_the_prune.is_ok_and(|listed| listed == tree);
                let next = match after_the_next_prune {
                    Ok(listed) => (listed == tree).to_string(),
                    Err(error) => format!("{error:?}"),
                };
                (saved, first, next)
            })
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            outcomes,
            vec![
                (true, true, "true".to_string()),
                (true, true, "true".to_string())
            ]
        );
    }
}

#[test]
#[timeout("60s")]
async fn shut_down_waits_for_the_check_of_a_local_path() {
    // The check waits on its blocking thread until the test lets it go.
    let store = store(
        Arc::new(InMemoryBlobStorage::new()),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let tree = one_file_tree("checked");
    let (reached_sender, reached) = std::sync::mpsc::channel::<()>();
    let (open, opened) = std::sync::mpsc::channel::<()>();
    let checking = AbortOnDropHandle::new(tokio::spawn({
        let (store, path) = (store.clone(), tree.path().to_path_buf());
        async move {
            super::tracked_tasks::check_local_path(&store.tracker, &path, move |_| {
                let _ = reached_sender.send(());
                let _ = opened.recv();
                Ok(())
            })
            .await
        }
    }));
    let reached = tokio::task::spawn_blocking(move || reached.recv_timeout(LIMIT).is_ok())
        .await
        .unwrap();

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    open.send(()).unwrap();
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();
    let checked = tokio::time::timeout(LIMIT, checking).await;

    assert!(matches!(&checked, Ok(Ok(Ok(())))), "{checked:?}");
    assert_eq!((reached, waited, stopped), (true, true, true));
}
