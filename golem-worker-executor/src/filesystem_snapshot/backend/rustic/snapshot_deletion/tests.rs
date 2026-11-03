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

use crate::filesystem_snapshot::backend::rustic::pruning::test_support::freed;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    LONG_DEADLINE, NEVER, blobs, calls_of, fixture_tree, is_failed, listed_names, name,
    one_file_tree, policy, save_each, snapshot_files, store, store_over,
};

use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{FilesystemSnapshotStore, Unlimited};
use futures::StreamExt;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{test, timeout};

#[test]
#[timeout("60s")]
async fn a_batch_delete_lists_the_snapshots_once_and_deletes_each_name_of_the_batch() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(storage.clone(), policy(LONG_DEADLINE, NEVER, LONG_DEADLINE));
    let scope = new_scope();
    let trees = ["one", "two", "three", "four"].map(one_file_tree);
    futures::stream::iter(["p-1", "p-2", "p-3", "p-kept"].into_iter().zip(&trees))
        .for_each(|(snapshot, tree)| {
            let store = &store;
            let scope = &scope;
            async move {
                store
                    .save(
                        scope,
                        &name(snapshot),
                        tree.path(),
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await
                    .unwrap();
            }
        })
        .await;
    let before = storage.calls().len();

    store
        .delete(
            &scope,
            &[name("p-1"), name("p-2"), name("p-unknown"), name("p-3")],
            &Unlimited,
        )
        .await
        .unwrap();
    let calls = storage.calls();
    let remaining = store
        .list(&scope, &Unlimited)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        (snapshot_listings(&calls[before..]), remaining),
        (1, vec!["p-kept".to_string()])
    );
}

#[test]
#[timeout("60s")]
async fn a_forget_that_fails_after_the_record_write_leaves_the_record() {
    // The forget deletes the snapshot file, and the storage refuses that call.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "delete" && path.starts_with("snapshots") {
                Script::Refuse
            } else {
                Script::Pass
            }
        });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.as_ref().is_err_and(is_failed), "{deleted:?}");
    assert_eq!(
        (
            freed(&storage, &scope).await > 0,
            listed_names(&store, &scope).await
        ),
        (true, vec!["p-1".to_string()])
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_that_frees_nothing_writes_no_ledger() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(
            &scope,
            &name("p-kept"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();

    store
        .delete(&scope, &[name("p-unknown")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        blobs(&*storage, scope.namespace(), "golem/").await,
        Vec::<String>::new()
    );
}

#[test]
#[timeout("60s")]
async fn the_ledger_counts_the_packed_bytes_that_the_deleted_snapshot_added() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(
            &scope,
            &name("p-deleted"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let added = snapshot_files(storage.clone(), &scope)
        .await
        .iter()
        .filter_map(|snapshot| snapshot.summary.as_ref())
        .map(|summary| summary.data_added_packed)
        .sum::<u64>();

    store
        .delete(&scope, &[name("p-deleted")], &Unlimited)
        .await
        .unwrap();

    assert_eq!((added > 1, freed(&storage, &scope).await), (true, added));
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

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            calls_of(&storage, "write_freed"),
            listed_names(&store, &scope).await
        ),
        (2, vec!["p-2".to_string()])
    );
}

/// Counts the listings of the snapshot files of a repository in `calls`.
fn snapshot_listings(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, path)| *op_label == "list" && path.starts_with("snapshots"))
        .count()
}
