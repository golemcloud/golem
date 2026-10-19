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
use crate::filesystem_snapshot::contract_tests::raced_restore_kept_the_contract;
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
        let (store, scope, restored_name) = (store.clone(), scope.clone(), restored_name.clone());
        async move { restored_listing(&store, &scope, &restored_name).await }
    };
    let deleting = {
        let (store, scope, deleted_name) = (store.clone(), scope.clone(), deleted_name.clone());
        async move {
            store
                .delete(
                    &scope,
                    std::slice::from_ref(&deleted_name),
                    &crate::filesystem_snapshot::Unlimited,
                )
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
    scope: &AgentSnapshots,
    name: &SnapshotName,
    path: &Path,
) {
    store
        .save(
            scope,
            name,
            path,
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
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
async fn a_restore_held_at_each_blob_call_while_a_delete_of_its_name_prunes_keeps_the_contract() {
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

impl crate::filesystem_snapshot::RunSlots for CountedTakes {
    fn take(
        &self,
        immediate: bool,
    ) -> futures::future::BoxFuture<
        '_,
        Result<crate::filesystem_snapshot::Slot, crate::filesystem_snapshot::Withdrawal>,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        crate::filesystem_snapshot::Unlimited.take(immediate)
    }

    fn withdrawn(&self) -> futures::future::BoxFuture<'_, crate::filesystem_snapshot::Withdrawal> {
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
        let (armed, held, packs_of_other) = (armed.clone(), held.clone(), packs_of_other.clone());
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
    *packs_of_other.lock().unwrap() = blobs(&*storage, &scope.0, "data/").await;
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
    let deleted = store
        .delete(
            &scope,
            &[name("p-other")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
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
    let deleted_third = later
        .delete(
            &scope,
            &[name("p-third")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let packs_after_the_prune = blobs(&*storage, &scope.0, "data/").await;
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
