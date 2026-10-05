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

//! The runs of the calls of the rustic store: the slots, the runs after a failed call, the wait
//! for a write that can still land and the check of the own name, the restore that loads again,
//! and the copy that runs again.

use super::super::super::clear::ClearHook;
use super::super::super::files::LateWrites;
use super::super::super::runs::{Ran, RunEnd, Settle, Settled};
use super::*;
use crate::filesystem_snapshot::{CallError, RunSlots, Slot, SnapshotInfo, Withdrawal};
use futures::future::BoxFuture;
use golem_common::model::RetryConfig;
use pretty_assertions::assert_eq;
use std::sync::Mutex;
use test_r::{test, timeout};
use tokio::sync::Semaphore;

/// The runs of the tests of this module: three runs, with short waits.
fn three_runs() -> RetryConfig {
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
fn runs_policy(deadline: Duration, tries: u32) -> StorePolicy {
    StorePolicy {
        retry: three_runs(),
        in_call_tries: tries,
        ..policy(deadline, NEVER, Duration::ZERO)
    }
}

/// A limiter of `permits` slots that counts its takes, the slots that live and the most slots
/// that lived at once. Its clones share the slots and the counts.
#[derive(Clone)]
struct SharedSlots {
    semaphore: Arc<Semaphore>,
    takes: Arc<AtomicUsize>,
    live: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
    failed_waits: Arc<AtomicUsize>,
    late_waits: Arc<AtomicUsize>,
}

impl SharedSlots {
    fn new(permits: usize) -> Self {
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
    fn late_waits(&self) -> usize {
        self.late_waits.load(Ordering::SeqCst)
    }

    /// The takes, the slots that live now, the most slots that lived at once, and the waits after
    /// a failed run that the store reported.
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.takes.load(Ordering::SeqCst),
            self.live.load(Ordering::SeqCst),
            self.most.load(Ordering::SeqCst),
            self.failed_waits.load(Ordering::SeqCst),
        )
    }
}

/// A slot of a [`SharedSlots`]: a permit, counted as live until it drops.
struct SharedSlot {
    _permit: tokio::sync::OwnedSemaphorePermit,
    live: Arc<AtomicUsize>,
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
fn refusing(
    refused: usize,
    selected: impl Fn(&str, &Path) -> bool + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    let seen = AtomicUsize::new(0);
    ScriptedBlobStorage::new(
        Arc::new(InMemoryBlobStorage::new()),
        move |op_label, path| {
            if selected(op_label, path) && seen.fetch_add(1, Ordering::SeqCst) < refused {
                Script::Refuse
            } else {
                Script::Pass
            }
        },
    )
}

/// Gives the number of calls of the storage with the operation label.
fn calls_of(storage: &ScriptedBlobStorage, label: &str) -> usize {
    storage
        .calls()
        .iter()
        .filter(|(op_label, _)| *op_label == label)
        .count()
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
            crate::filesystem_snapshot::never_cancelled(),
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
                    crate::filesystem_snapshot::never_cancelled(),
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

/// A storage over `inner` whose publish tries follow `script(n)` for the `n`-th try, counted from
/// 1, and whose other calls pass.
fn scripted_publish(
    inner: Arc<InMemoryBlobStorage>,
    script: impl Fn(usize) -> Script + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    let tries = AtomicUsize::new(0);
    ScriptedBlobStorage::new(inner, move |op_label, _| {
        if op_label == "publish" {
            script(tries.fetch_add(1, Ordering::SeqCst) + 1)
        } else {
            Script::Pass
        }
    })
}

/// The policy of the tests of a late publish: a deadline of 300 ms, three tries, and three runs.
fn late_policy() -> StorePolicy {
    runs_policy(Duration::from_millis(300), 3)
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
            crate::filesystem_snapshot::never_cancelled(),
            &slots,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            slots.counts(),
            blobs(&*inner, &scope.0, "snapshots/").await.len(),
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
            crate::filesystem_snapshot::never_cancelled(),
            &late_slots,
        )
        .await;
    let clean_store = super::store(Arc::new(InMemoryBlobStorage::new()), late_policy());
    let clean_saved = clean_store
        .save(
            &clean_scope,
            &name("p-clean"),
            tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
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
            crate::filesystem_snapshot::never_cancelled(),
            &slots,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            slots.counts(),
            blobs(&*inner, &scope.0, "snapshots/").await.len(),
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
                    crate::filesystem_snapshot::never_cancelled(),
                    &crate::filesystem_snapshot::Unlimited,
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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

/// A store over `inner` with the policy of the tests, which passes each call.
fn store_over(inner: &Arc<InMemoryBlobStorage>) -> Arc<RusticSnapshotStore> {
    store(inner.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO))
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
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(
        (
            calls_of(&storage, "publish"),
            calls_of(&storage, "check_name"),
            blobs(&*inner, &scope.0, "snapshots/").await.len()
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
                    crate::filesystem_snapshot::never_cancelled(),
                    &crate::filesystem_snapshot::Unlimited,
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(blobs(&*inner, &scope.0, "snapshots/").await.len(), 1);
}

#[test]
#[timeout("60s")]
async fn a_restore_whose_repository_a_delete_all_removed_gives_not_found() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store_over(&inner);
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    store
        .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();

    let restored = restored_listing(&store, &scope, &name("p-1")).await;

    assert!(
        matches!(restored, Err(RestoreFailure::NotFound)),
        "{restored:?}"
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
    let index_files = blobs(&*inner, &scope.0, "index/").await;
    futures::future::join_all(
        index_files
            .iter()
            .map(|path| inner.delete("test", "test", (*scope.0).clone(), Path::new(path))),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
    let into = Scratch::new();

    let restored = store
        .restore(
            &scope,
            &name("p-1"),
            into.path(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

    assert!(
        matches!(restored, Err(RestoreFailure::Corrupt(_))),
        "{restored:?}"
    );
    assert_eq!(listing(into.path()), Vec::new());
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let before = blobs(&*inner, &scope.0, "snapshots/").await;
    saving
        .delete(
            &scope,
            &[name("p-1")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let hidden = blobs(&*inner, &scope.0, "snapshots/").await;
    let hidden_path = hidden.first().cloned().unwrap();
    let hidden_bytes = inner
        .get_raw("test", "test", (*scope.0).clone(), Path::new(&hidden_path))
        .await
        .unwrap()
        .unwrap();
    inner
        .delete("test", "test", (*scope.0).clone(), Path::new(&hidden_path))
        .await
        .unwrap();
    let marking = backend_of(inner.clone(), &scope, LONG_DEADLINE);
    let report = super::super::super::run_blocking(move || {
        super::super::super::prune(
            marking,
            &key(),
            &PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(3600),
            },
        )
    })
    .await
    .unwrap();
    inner
        .put_raw(
            "test",
            "test",
            (*scope.0).clone(),
            Path::new(&hidden_path),
            &hidden_bytes,
        )
        .await
        .unwrap();

    (
        inner,
        scope,
        tree,
        before,
        report.is_some_and(|report| report.packs_unused > 0),
    )
}

#[test]
#[timeout("60s")]
async fn a_restore_of_a_name_whose_deduped_pack_a_prune_marked_gives_failed_and_after_the_next_prune_the_whole_tree()
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
        .delete(
            &scope,
            &[name("p-3")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let after_the_next_prune = restored_listing(&saving, &scope, &name("p-2")).await;

    assert_eq!((before.len(), unused), (2, true));
    assert!(
        matches!(
            &first,
            Err(RestoreFailure::Failed(failed))
                if failed.cause().to_string().contains("marked for deletion")
        ),
        "{first:?}"
    );
    assert_eq!(after_the_next_prune.ok(), Some(listing(tree.path())));
    // The first restore answered in its first run, with no race run, and wrote nothing.
    assert_eq!(
        (first_slots.counts().0, listing(first_into.path())),
        (1, Vec::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_marked_pack_after_a_load_that_missed_an_index_file_gives_failed_in_the_first_run() {
    // The first listing of the restore holds an index file that is gone at its read, and the
    // later listings do not hold it. The load after it finds the name, and the check misses a
    // pack that a prune marked. The read of the marked packs reads only the index files that the
    // run did not find gone. It reads in the order of the paths, and the gone file comes first.
    let (inner, scope, _tree, _, unused) = marked_dedup().await;
    let gone = Path::new("index").join("00".repeat(32));
    inner
        .put_raw("test", "test", (*scope.0).clone(), &gone, b"gone")
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

    assert!(
        matches!(
            &restored,
            Err(RestoreFailure::Failed(failed))
                if failed.cause().to_string().contains("marked for deletion")
        ),
        "{restored:?}"
    );
    assert_eq!(
        (
            unused,
            slots.counts().0,
            storage
                .calls()
                .iter()
                .filter(|(op_label, path)| *op_label == "read" && Path::new(path) == gone)
                .count()
        ),
        (true, 1, 1)
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
        .restore(
            &scope,
            &name("p-1"),
            into.path(),
            &crate::filesystem_snapshot::Unlimited,
        )
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let expected = listing(tree.path());
    // The first restore writes the whole tree with its modes; a refused read of the second makes
    // its first run fail after it wrote into the same directory.
    let first = Scratch::new();
    saving
        .restore(
            &scope,
            &name("p-1"),
            first.path(),
            &crate::filesystem_snapshot::Unlimited,
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
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let into = Scratch::new();

    let restored = store
        .restore(
            &scope,
            &name("p-1"),
            into.path(),
            &crate::filesystem_snapshot::Unlimited,
        )
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

#[test]
#[timeout("60s")]
async fn a_shutdown_during_a_restore_gives_stopped_and_clears_nothing() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let saving = store_over(&inner);
    let scope = new_scope();
    save_each(&saving, &scope, &["p-1"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if op_label == "read_range" && path.starts_with("data") {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let store = store(storage.clone(), runs_policy(LONG_DEADLINE, 1));
    let into = Scratch::new();
    let restoring = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope, path) = (store.clone(), scope.clone(), into.path().to_path_buf());
        async move {
            store
                .restore(
                    &scope,
                    &name("p-1"),
                    &path,
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
        }
    }));
    let held = eventually(|| calls_of(&storage, "read_range") > 0).await;

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let restored = tokio::time::timeout(LIMIT, restoring).await;
    storage.open_gate();

    assert!(
        matches!(
            restored,
            Ok(Ok(Err(RestoreFailure::Stopped(Withdrawal::Stopped))))
        ),
        "{restored:?}"
    );
    assert_eq!((held, stopped), (true, true));
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

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
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

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
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

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
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
async fn the_claim_protocol_makes_one_store_try_for_each_call() {
    // The claim write is refused once. A blob call of the claim protocol has one try, so the
    // prune does not run, and the delete answers success.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1", "p-2"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let refused = AtomicBool::new(false);
        move |op_label, _| {
            if op_label == "write_claim" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            in_call_tries: 3,
            ..policy(LONG_DEADLINE, ALWAYS, Duration::ZERO)
        },
    );

    let deleted = store
        .delete(
            &scope,
            &[name("p-1")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            calls_of(&storage, "write_claim"),
            listed_names(&store, &scope).await
        ),
        (1, vec!["p-2".to_string()])
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_freed_record_is_written_again_and_the_delete_forgets() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1", "p-2"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let refused = AtomicBool::new(false);
        move |op_label, _| {
            if op_label == "write_freed" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            in_call_tries: 3,
            ..policy(LONG_DEADLINE, NEVER, Duration::ZERO)
        },
    );

    let deleted = store
        .delete(
            &scope,
            &[name("p-1")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            calls_of(&storage, "write_freed"),
            listed_names(&store, &scope).await
        ),
        (2, vec!["p-2".to_string()])
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
            blobs(&*storage, &scope.0, "snapshots/").await,
            started_after_the_answer
        ),
        (0, Vec::<String>::new(), 0)
    );
}

/// Deletes each blob of the scope, as a delete of all snapshots does.
async fn delete_everything(inner: &InMemoryBlobStorage, scope: &AgentSnapshots) {
    let paths = blobs(inner, &scope.0, "").await;
    futures::future::join_all(
        paths
            .iter()
            .map(|path| inner.delete("test", "test", (*scope.0).clone(), Path::new(path))),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();
}

/// The call of the third open of the repository that a test holds: its first look at the config,
/// which then finds no config, or its read of the config, which then finds the config gone.
#[derive(Clone, Copy)]
enum Held {
    Stat,
    Read,
}

impl Held {
    fn label(self) -> &'static str {
        match self {
            Held::Stat => "stat",
            Held::Read => "read",
        }
    }

    /// The number of the held call: an open looks at the config twice and reads it once.
    fn number(self) -> usize {
        match self {
            Held::Stat => 5,
            Held::Read => 3,
        }
    }
}

/// A storage over `inner` that holds the call `held` of the third open of the repository at its
/// gate, and that gives no blob to the reads that `vanishing` picks.
fn holding_the_third_open(
    inner: Arc<InMemoryBlobStorage>,
    held: Held,
    vanishing: impl Fn(&str, &Path) -> bool + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    let config_calls = AtomicUsize::new(0);
    ScriptedBlobStorage::new(inner, move |op_label, path| {
        if op_label == held.label() && path == Path::new("config") {
            if config_calls.fetch_add(1, Ordering::SeqCst) + 1 == held.number() {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        } else if vanishing(op_label, path) {
            Script::Vanish
        } else {
            Script::Pass
        }
    })
}

/// Restores `p-1` while the storage holds the call `held` of the third open, deletes every blob
/// of the scope while it is held, and then opens the gate.
async fn restore_through_a_delete_of_all(
    storage: &Arc<ScriptedBlobStorage>,
    inner: &InMemoryBlobStorage,
    scope: &AgentSnapshots,
    held: Held,
) -> Result<SnapshotInfo, RestoreFailure> {
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let into = Scratch::new();
    let restored_name = name("p-1");
    let restoring = store.restore(
        scope,
        &restored_name,
        into.path(),
        &crate::filesystem_snapshot::Unlimited,
    );
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| {
            storage
                .calls()
                .iter()
                .filter(|(op_label, path)| *op_label == held.label() && path == "config")
                .count()
                == held.number()
        }) => reached,
        restored = &mut restoring => panic!("the restore ended before its third open: {restored:?}"),
    };
    assert!(reached);
    delete_everything(inner, scope).await;
    storage.open_gate();
    tokio::time::timeout(LIMIT, restoring).await.unwrap()
}

/// Restores a name whose index files are gone, so the check fails, the second listing of the
/// index files gives no new name, and the snapshots read again hold the name. A delete of all
/// snapshots removes the repository at the call `held` of the open of the read of the marked packs.
async fn a_marked_read_through_a_delete_of_all(held: Held) -> Result<SnapshotInfo, RestoreFailure> {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let index_files = blobs(&*inner, &scope.0, "index/").await;
    futures::future::join_all(
        index_files
            .iter()
            .map(|path| inner.delete("test", "test", (*scope.0).clone(), Path::new(path))),
    )
    .await;
    let storage = holding_the_third_open(inner.clone(), held, |_, _| false);
    restore_through_a_delete_of_all(&storage, &inner, &scope, held).await
}

#[test]
#[timeout("60s")]
async fn a_delete_of_all_snapshots_before_the_read_of_the_marked_packs_gives_not_found() {
    // The open finds no config, or finds the config and then its read finds it gone.
    let no_config = a_marked_read_through_a_delete_of_all(Held::Stat).await;
    let config_gone = a_marked_read_through_a_delete_of_all(Held::Read).await;

    assert!(
        matches!(
            (&no_config, &config_gone),
            (Err(RestoreFailure::NotFound), Err(RestoreFailure::NotFound))
        ),
        "{no_config:?} {config_gone:?}"
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
    let storage = holding_the_third_open(inner.clone(), Held::Stat, move |op_label, path| {
        op_label == "read" && path.starts_with("data") && !vanished.swap(true, Ordering::SeqCst)
    });

    let restored = restore_through_a_delete_of_all(&storage, &inner, &scope, Held::Stat).await;

    assert!(
        matches!(restored, Err(RestoreFailure::NotFound)),
        "{restored:?}"
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

    let restored = store
        .restore(
            &scope,
            &name("p-1"),
            &into,
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;

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

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
    let listed = store
        .list(&to, &crate::filesystem_snapshot::Unlimited)
        .await;

    assert!(matches!(copied, Err(CallError::Failed(_))), "{copied:?}");
    assert!(
        matches!(&listed, Ok(listed) if listed.is_empty()),
        "{listed:?}"
    );
    assert_eq!(
        (
            calls_of(&storage, "copy_remove_config"),
            blobs(&*inner, &to.0, "config").await
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
        blobs(&*inner, &from.0, "index/").await,
        blobs(&*inner, &from.0, "data/").await,
    ]
    .concat();
    save_each(&saving, &from, &["p-2"]).await;
    let after = [
        blobs(&*inner, &from.0, "index/").await,
        blobs(&*inner, &from.0, "data/").await,
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

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
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
        blobs(&*inner, &to.0, "index/").await,
        blobs(&*inner, &from.0, "index/").await
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
                    *second_run_at.lock().unwrap() = Some(super::super::super::runs::now());
                }
                Script::Pass
            }
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), runs_policy(deadline, 1));

    let copied = store
        .copy_all(&from, &to, &crate::filesystem_snapshot::Unlimited)
        .await;
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let tried = storage
        .events()
        .iter()
        .find(|event| event.op_label == "publish" && !event.landed_late)
        .map(|event| event.ended)
        .unwrap();
    let answered = super::super::super::runs::now();
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

/// A limiter without a bound whose withdrawal comes when `withdraw` is cancelled, and that counts
/// the waits after a failed run that the store reports.
struct WithdrawnSlots {
    withdraw: CancellationToken,
    failed_waits: AtomicUsize,
}

impl RunSlots for WithdrawnSlots {
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        crate::filesystem_snapshot::Unlimited.take(immediate)
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
    let restoring = store.restore(
        &scope,
        &restored_name,
        &into,
        &crate::filesystem_snapshot::Unlimited,
    );
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

#[test]
#[timeout("60s")]
async fn a_prune_that_rewrites_the_index_before_the_read_of_the_marked_packs_gives_the_whole_tree()
{
    // As in the test of the deduped pack, `p-2` uses packs that a prune marked, so the check of
    // its restore fails, the second listing of the index files gives no new name, and the
    // snapshots read again hold the name. The storage holds the first read of an index file after
    // that listing, the read of the marked packs. Meanwhile the next prune takes the packs back
    // into its new index file and deletes the old ones. The read then finds its index file gone,
    // and the run loads again and gives the whole tree.
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    saving
        .delete(
            &scope,
            &[name("p-1")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let hidden = blobs(&*inner, &scope.0, "snapshots/").await;
    let hidden_path = hidden.first().cloned().unwrap();
    let hidden_bytes = inner
        .get_raw("test", "test", (*scope.0).clone(), Path::new(&hidden_path))
        .await
        .unwrap()
        .unwrap();
    inner
        .delete("test", "test", (*scope.0).clone(), Path::new(&hidden_path))
        .await
        .unwrap();
    let marking = backend_of(inner.clone(), &scope, LONG_DEADLINE);
    super::super::super::run_blocking(move || {
        super::super::super::prune(
            marking,
            &key(),
            &PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(3600),
            },
        )
    })
    .await
    .unwrap();
    inner
        .put_raw(
            "test",
            "test",
            (*scope.0).clone(),
            Path::new(&hidden_path),
            &hidden_bytes,
        )
        .await
        .unwrap();
    let index_listings = Arc::new(AtomicUsize::new(0));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let (index_listings, held) = (index_listings.clone(), held.clone());
        move |op_label, path| {
            if op_label == "list" && path == Path::new("index") {
                index_listings.fetch_add(1, Ordering::SeqCst);
                Script::Pass
            } else if op_label == "read"
                && path.starts_with("index")
                && index_listings.load(Ordering::SeqCst) == 2
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let restoring_store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::from_secs(3600)),
    );
    let into = Scratch::new();
    let restored_name = name("p-2");
    let restoring = restoring_store.restore(
        &scope,
        &restored_name,
        into.path(),
        &crate::filesystem_snapshot::Unlimited,
    );
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| held.load(Ordering::SeqCst)) => reached,
        restored = &mut restoring => panic!("the restore ended before the read of the marked packs: {restored:?}"),
    };
    let pruning = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    save_each(&pruning, &scope, &["p-3"]).await;
    pruning
        .delete(
            &scope,
            &[name("p-3")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    storage.open_gate();
    let restored = tokio::time::timeout(LIMIT, restoring).await.unwrap();

    assert!(reached);
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(listing(into.path()), listing(tree.path()));
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
    let backup_end = super::super::super::publish::backup_end(
        super::super::super::publish::index_read_bound(
            started,
            grace,
            deadline,
            super::super::super::prune::refresh_period(grace, deadline),
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
            blobs(&*storage, &scope.0, "snapshots/").await
        ),
        (0, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn the_text_of_a_local_io_error_of_the_fork_keeps_the_form_that_the_store_reads() {
    // The store reads the kind of a local I/O error of the fork from its text: an error of the
    // kind `InputOutput` whose cause shows `Os { code: N`. A directory appears at the path of the
    // only file of the tree while the run loads the repository, so the write cannot open the file,
    // for each user. A fork whose text changes fails this test.
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
    let into = Scratch::new();
    let restored_name = name("p-1");
    let restoring = store.restore(
        &scope,
        &restored_name,
        into.path(),
        &crate::filesystem_snapshot::Unlimited,
    );
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| first_read.load(Ordering::SeqCst)) => reached,
        restored = &mut restoring => panic!("the restore ended before its load: {restored:?}"),
    };
    std::fs::create_dir(into.path().join("file.txt")).unwrap();
    storage.open_gate();
    let restored = tokio::time::timeout(LIMIT, restoring).await.unwrap();

    assert!(reached);
    match &restored {
        Err(RestoreFailure::Destination(error)) => {
            let text = error.to_string();
            assert_eq!(
                (
                    error.kind(),
                    text.starts_with(
                        "`rustic_core` experienced an error related to `input/output operations`."
                    ),
                    text.contains("Os { code: 21"),
                ),
                (std::io::ErrorKind::IsADirectory, true, true),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// Gives the number of calls of a delete of all snapshots.
fn scope_deletes(storage: &ScriptedBlobStorage) -> usize {
    calls_of(storage, "delete_scope")
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
                .save(
                    &scope,
                    &name("p-1"),
                    &tree,
                    None,
                    crate::filesystem_snapshot::never_cancelled(),
                    &slots,
                )
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
        async move {
            store
                .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
                .await
        }
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

/// A storage over `inner` whose first write of a pack follows `first`, and whose other calls
/// follow `other`.
fn scripted_first_pack(
    inner: Arc<InMemoryBlobStorage>,
    first: Script,
    other: impl Fn(&str, &Path) -> Script + Send + Sync + 'static,
) -> Arc<ScriptedBlobStorage> {
    let packs = AtomicUsize::new(0);
    ScriptedBlobStorage::new(inner, move |op_label, path| {
        if op_label == "write"
            && path.starts_with("data")
            && packs.fetch_add(1, Ordering::SeqCst) == 0
        {
            first
        } else {
            other(op_label, path)
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let sent_before_the_answer = wrote_a_pack(&storage);
    store
        .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert!(sent_before_the_answer);
    assert_eq!(blobs(&*inner, &scope.0, "").await, Vec::<String>::new());
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    if !wrote_a_pack(&storage) {
        return None;
    }
    store
        .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();
    tokio::time::sleep(
        (started + Duration::from_secs(17)).saturating_duration_since(std::time::Instant::now())
            + Duration::from_secs(1),
    )
    .await;
    Some((saved, blobs(&*inner, &scope.0, "").await))
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let took = started.elapsed();

    assert!(matches!(saved, Err(SaveError::Source(_))), "{saved:?}");
    assert!(took < deadline, "the save took {took:?}");
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let sent_before_the_answer = calls_of(&storage, "publish");
    store
        .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(sent_before_the_answer, 2);
    assert_eq!(blobs(&*inner, &scope.0, "").await, Vec::<String>::new());
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let took = started.elapsed();

    assert!(saved.is_ok(), "{saved:?}");
    assert!(took >= deadline, "the second run started after {took:?}");
    assert_eq!(calls_of(&storage, "check_name"), 0);
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
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
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
                crate::filesystem_snapshot::never_cancelled(),
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
                crate::filesystem_snapshot::never_cancelled(),
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
            let ended = super::super::super::runs::now();
            late.record(ended);
            drop(token);
            ended
        })
    };
    let Settle { wait, finish, .. } = super::super::save_settle(
        new_scope(),
        name("p-00000000-0000-4000-8000-0000000000ab"),
        super::super::SaveExit::Ended {
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

#[test]
#[timeout("60s")]
async fn the_settle_of_a_save_waits_for_a_holder_of_its_blobs_past_one_deadline_and_warns_with_its_name()
 {
    use tracing::instrument::WithSubscriber;

    let run_calls = tokio_util::task::TaskTracker::new();
    let _held = run_calls.token();
    let saved = name("p-00000000-0000-4000-8000-0000000000aa");
    let Settle { wait, .. } = super::super::save_settle(
        new_scope(),
        saved.clone(),
        super::super::SaveExit::Answered(Err(SaveError::NameInUse)),
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
