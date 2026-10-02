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
//! call of one side. The store has the configured policy, as in the contract tests, so the delete
//! of the only snapshot prunes. A case holds one side at the gate before its blob call `k`, runs
//! the other side to its end, and then opens the gate. The contract case judges the outcome.

use super::*;
use crate::filesystem_snapshot::contract_tests::raced_restore_kept_the_contract;
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

/// Runs the case that holds the side before its blob call `k`. It gives the operation label and
/// the path of the held call with the judgement of the contract case, or `None` when the side ends
/// before its call `k`.
async fn case(held: Held, k: usize) -> Option<((String, String), Result<(), String>)> {
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
    let store = Arc::new(RusticSnapshotStore::new(storage.clone(), &config()));
    let scope = new_scope();
    let tree = fixture_tree();
    let raced = name("p-raced");
    store.save(&scope, &raced, tree.path(), None).await.unwrap();
    armed.store(true, Ordering::SeqCst);

    let restoring = {
        let (store, scope, raced) = (store.clone(), scope.clone(), raced.clone());
        async move { restored_listing(&store, &scope, &raced).await }
    };
    let deleting = {
        let (store, scope, raced) = (store.clone(), scope.clone(), raced.clone());
        async move { store.delete(&scope, std::slice::from_ref(&raced)).await }
    };
    let is_held = || held_call.lock().unwrap().is_some();
    let (restore, deleted) = match held {
        Held::Restore => {
            let first = tokio::spawn(restoring);
            assert!(polled_until(LIMIT, || is_held() || first.is_finished()).await);
            if !is_held() {
                storage.open_gate();
                let _ = first.await;
                return None;
            }
            let deleted = tokio::time::timeout(LIMIT, deleting).await.unwrap();
            storage.open_gate();
            let restore = tokio::time::timeout(LIMIT, first).await.unwrap().unwrap();
            (restore, deleted)
        }
        Held::Delete => {
            let first = tokio::spawn(deleting);
            assert!(polled_until(LIMIT, || is_held() || first.is_finished()).await);
            if !is_held() {
                storage.open_gate();
                let _ = first.await;
                return None;
            }
            let restore = tokio::time::timeout(LIMIT, restoring).await.unwrap();
            storage.open_gate();
            let deleted = tokio::time::timeout(LIMIT, first).await.unwrap().unwrap();
            (restore, deleted)
        }
    };
    store.shut_down().await;
    let held_call = held_call.lock().unwrap().clone().unwrap();
    Some((
        held_call,
        raced_restore_kept_the_contract(&deleted, &restore, &listing(tree.path())),
    ))
}

/// Runs the case at each blob call of the side, and gives the held call and the reason of each
/// case that broke the contract.
async fn broken_cases(held: Held) -> Vec<String> {
    // The fold ends with `Err` when the held side ends before call `k`, so that every call was
    // held once. An `Ok` after the last call means that the side made more calls than the bound.
    let swept = futures::stream::iter(1..=MOST_CALLS)
        .map(Ok::<usize, Vec<String>>)
        .try_fold(Vec::new(), |mut broken, k| async move {
            let Some((held_call, judged)) = case(held, k).await else {
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
    assert_eq!(broken_cases(Held::Restore).await, Vec::<String>::new());
}

#[test]
#[timeout("120s")]
async fn a_delete_held_at_each_blob_call_while_a_restore_of_its_name_runs_keeps_the_contract() {
    assert_eq!(broken_cases(Held::Delete).await, Vec::<String>::new());
}
