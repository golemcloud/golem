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

//! The drain of a delete of all snapshots: it waits for the store work of its incarnation that
//! began before it, also for the work that a dropped call left.

use super::*;
use crate::filesystem_snapshot::CallError;
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
fn calls_with(calls: &[(&'static str, String)], label: &str) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == label)
        .count()
}

/// Gives the number of incarnations whose work the store holds.
fn agent_works(store: &RusticSnapshotStore) -> usize {
    store
        .works
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

/// What a delete of all snapshots did after a delete that left the work `left`: whether the work
/// was held, whether the delete of all made no call while it was held, its answer, whether the
/// store work ended, and the blobs of the scope after it.
async fn a_delete_all_after(left: Left) -> (bool, bool, Result<(), CallError>, bool, Vec<String>) {
    let storage = left.storage();
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move {
            store
                .delete(
                    &scope,
                    &[name("p-1")],
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
        }
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
        async move {
            store
                .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
                .await
        }
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
        blobs(&*storage, &scope.0, "").await,
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
            calls_with(&storage.calls(), "refresh_claim") > 0,
            prunes(&storage.calls()),
            store.work_in_flight(),
            agent_works(&store),
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
                    crate::filesystem_snapshot::never_cancelled(),
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
        }
    }));
    let held = eventually(|| calls_with(&storage.calls(), "write") > 0).await;
    let delete_all = || {
        let (store, scope) = (store.clone(), scope.clone());
        AbortOnDropHandle::new(tokio::spawn(async move {
            store
                .delete_all(&scope, &crate::filesystem_snapshot::Unlimited)
                .await
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
                    crate::filesystem_snapshot::never_cancelled(),
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
        }))
    };
    let delete = |text: &'static str| {
        let (pruning, scope) = (pruning.clone(), scope.clone());
        AbortOnDropHandle::new(tokio::spawn(async move {
            pruning
                .delete(
                    &scope,
                    &[name(text)],
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
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
    super::super::super::run_blocking(move || {
        super::super::super::prune(
            next,
            &key(),
            &PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(3600),
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
    // A prune that reads the index before the publish marks the new packs of the save, so a
    // restore before the next prune gives `Failed` with the cause of a marked pack, as the doc of
    // `restore` says. The next prune finds the snapshot and keeps its packs.
    let outcomes = futures::stream::iter([false, true])
        .then(|index_read_first| async move {
            let (saved, after_the_prune, after_the_next_prune, tree) =
                save_and_prune(index_read_first).await;
            let first = match after_the_prune {
                Ok(listed) => listed == tree,
                Err(RestoreFailure::Failed(failed)) => {
                    index_read_first && failed.cause().to_string().contains("marked for deletion")
                }
                Err(_) => false,
            };
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
