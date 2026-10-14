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
use crate::filesystem_snapshot::{RunSlots, Slot, Withdrawal};
use futures::future::BoxFuture;
use pretty_assertions::assert_eq;
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
        crate::filesystem_snapshot::Unlimited.take(immediate)
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
    scope: &AgentSnapshots,
    name: &SnapshotName,
    path: &Path,
) -> Result<(), String> {
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
        .map(|_| ())
        .map_err(|error| format!("the save of {name} gave {error:?}"))
}

/// Deletes `name`.
async fn delete_name(
    store: &RusticSnapshotStore,
    scope: &AgentSnapshots,
    name: &SnapshotName,
) -> Result<(), String> {
    store
        .delete(
            scope,
            std::slice::from_ref(name),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .map_err(|error| format!("the delete of {name} gave {error:?}"))
}

/// Restores `p-2`, and checks that the restore gives `tree` in its first run.
async fn restore_whole(
    store: &RusticSnapshotStore,
    scope: &AgentSnapshots,
    tree: &Path,
    when: &str,
) -> Result<(), String> {
    let takes = Takes::default();
    let into = Scratch::new();
    match store
        .restore(scope, &name("p-2"), into.path(), &takes)
        .await
    {
        Ok(_) if listing(into.path()) == listing(tree) && takes.count() == 1 => Ok(()),
        Ok(_) => Err(format!(
            "the restore {when} gave another tree or ran {} times",
            takes.count()
        )),
        Err(error) => Err(format!("the restore {when} gave {error:?}")),
    }
}

/// Gives the packs of the scope.
async fn packs(storage: &ScriptedBlobStorage, scope: &AgentSnapshots) -> Vec<String> {
    blobs(storage, &scope.0, "data/").await
}

/// Gives whether an index file of the scope marks a pack for deletion. The restore sweep starts
/// with marked packs, which the prune of its race takes back, so only the save sweep counts them.
async fn any_marked(storage: Arc<ScriptedBlobStorage>, scope: &AgentSnapshots) -> bool {
    let backend = backend_of(storage, scope, LONG_DEADLINE);
    super::super::super::run_blocking(move || {
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
type Side = BoxFuture<'static, Result<(), String>>;

/// What a case observed: the held call, whether a prune marked a pack, and the breaks of the
/// rule.
struct Judged {
    held_call: (String, String),
    marked: bool,
    broken: Vec<String>,
}

/// Runs the case that holds the side before its blob call `k`, and judges it. Gives `None` when
/// the side ends before its call `k`.
async fn case(raced: Raced, held: Held, k: usize) -> Option<Judged> {
    let armed = Arc::new(AtomicBool::new(false));
    let counted = Arc::new(AtomicUsize::new(0));
    let held_call = Arc::new(Mutex::new(None::<(String, String)>));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (armed, counted, held_call) = (armed.clone(), counted.clone(), held_call.clone());
        move |op_label, path| {
            if armed.load(Ordering::SeqCst) && counted.fetch_add(1, Ordering::SeqCst) + 1 == k {
                *held_call.lock().unwrap() =
                    Some((op_label.to_string(), path.display().to_string()));
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
            let path = tree.path().to_path_buf();
            (
                Box::pin(
                    async move { save_tree(&saving, &saving_scope, &name("p-2"), &path).await },
                ),
                Box::pin(async move { delete_name(&marking, &marking_scope, &name("p-1")).await }),
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
            let path = tree.path().to_path_buf();
            (
                Box::pin(async move {
                    restore_whole(&restoring, &restoring_scope, &path, "during the race").await
                }),
                Box::pin(
                    async move { delete_name(&recovering, &recovering_scope, &name("p-3")).await },
                ),
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

    let mut broken: Vec<String> = [held_result, other]
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
        broken,
    })
}

/// Saves `p-2` with the same tree as `p-1`, and then lets a prune that does not see the snapshot
/// file of `p-2` mark its packs after a delete of `p-1`, as a prune that read the snapshots before
/// the publish of `p-2` does.
async fn hide_while_marked(
    store: &RusticSnapshotStore,
    storage: &Arc<ScriptedBlobStorage>,
    scope: &AgentSnapshots,
    tree: &Path,
) {
    save_tree(store, scope, &name("p-2"), tree).await.unwrap();
    let (_, unused) = mark_the_packs_of_a_hidden_snapshot(storage.clone(), scope, GRACE).await;
    assert!(unused);
}

/// Checks the rule of a marked pack after the race: a restore of `p-2` gives the whole tree in
/// its first run before and after a later prune, the packs of the tree stay while `p-2` is there,
/// and the prune after the delete of `p-2`, one grace period after the prune that marked them,
/// deletes them. Gives each break of the rule.
async fn the_marked_packs_stay_until_no_snapshot_uses_them(
    store: &RusticSnapshotStore,
    later: &RusticSnapshotStore,
    storage: &Arc<ScriptedBlobStorage>,
    scope: &AgentSnapshots,
    tree: &Path,
    filler: &Path,
    packs_of_the_tree: &[String],
) -> Vec<String> {
    let kept = |when: &'static str| async move {
        let left = packs(storage, scope).await;
        if packs_of_the_tree.iter().all(|pack| left.contains(pack)) {
            Ok(())
        } else {
            Err(format!("a pack of the tree was gone {when}"))
        }
    };
    let steps = async {
        restore_whole(store, scope, tree, "after the race").await?;
        // A later prune, which sees `p-2`.
        save_tree(later, scope, &name("p-4"), filler).await?;
        delete_name(later, scope, &name("p-4")).await?;
        kept("after a prune that sees p-2").await?;
        restore_whole(store, scope, tree, "after a later prune").await?;
        // The delete of `p-2`, whose prune marks the packs of the tree.
        let marking = store_ahead(storage.clone(), 4 * GRACE);
        let marked = delete_name(&marking, scope, &name("p-2")).await;
        marking.shut_down().await;
        marked?;
        kept("after the prune that marked them").await?;
        // The next prune, after the grace period, deletes them. rustic dates a mark with the clock
        // of the host, so this prune has no grace period in place of a clock that is ahead.
        let deleting = store_with_grace(storage.clone(), 6 * GRACE, Duration::ZERO);
        let deleted = delete_name(&deleting, scope, &name("p-none")).await;
        deleting.shut_down().await;
        deleted?;
        let left = packs(storage, scope).await;
        if packs_of_the_tree.iter().any(|pack| left.contains(pack)) {
            Err("a pack of the tree stayed after a prune that sees no snapshot that uses it".into())
        } else {
            Ok(())
        }
    };
    steps.await.err().into_iter().collect()
}

/// Runs the case at each blob call of the side. Gives the number of cases in which a prune marked
/// a pack, and the held call with the breaks of each case that broke the rule.
async fn swept(raced: Raced, held: Held) -> (usize, Vec<String>) {
    // The fold ends with `Err` when the held side ends before call `k`, so that every call was
    // held once. An `Ok` after the last call means that the side made more calls than the bound.
    let swept = futures::stream::iter(1..=MOST_CALLS)
        .map(Ok::<usize, (usize, Vec<String>)>)
        .try_fold((0, Vec::new()), |(marked, mut broken), k| async move {
            let Some(judged) = case(raced, held, k).await else {
                return Err((marked, broken));
            };
            broken.extend(
                judged
                    .broken
                    .iter()
                    .map(|why| format!("call {k} {:?}: {why}", judged.held_call)),
            );
            Ok((marked + usize::from(judged.marked), broken))
        })
        .await;
    match swept {
        Err(result) => result,
        Ok(_) => panic!("the {held:?} side of {raced:?} made more than {MOST_CALLS} blob calls"),
    }
}

#[test]
#[timeout("600s")]
async fn a_save_held_at_each_blob_call_while_a_prune_marks_its_packs_keeps_them_until_no_snapshot_uses_them()
 {
    let (marked, broken) = swept(Raced::SaveAndMark, Held::First).await;

    assert_eq!(broken, Vec::<String>::new());
    assert!(marked > 0, "no case marked a pack of the save");
}

#[test]
#[timeout("600s")]
async fn a_delete_held_at_each_blob_call_while_a_save_reuses_its_packs_keeps_them_until_no_snapshot_uses_them()
 {
    let (_, broken) = swept(Raced::SaveAndMark, Held::Second).await;

    assert_eq!(broken, Vec::<String>::new());
}

#[test]
#[timeout("600s")]
async fn a_restore_held_at_each_blob_call_while_a_prune_takes_back_its_marked_packs_gives_the_whole_tree()
 {
    let (_, broken) = swept(Raced::RestoreAndRecover, Held::First).await;

    assert_eq!(broken, Vec::<String>::new());
}

#[test]
#[timeout("600s")]
async fn a_prune_held_at_each_blob_call_while_a_restore_reads_its_marked_packs_gives_the_whole_tree()
 {
    let (_, broken) = swept(Raced::RestoreAndRecover, Held::Second).await;

    assert_eq!(broken, Vec::<String>::new());
}
