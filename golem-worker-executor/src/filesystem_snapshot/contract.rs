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

//! The contract of [`FilesystemSnapshotStore`]: the behaviour that each store must have.
//!
//! Each case sees a store only through the trait. A store registers the suite with
//! [`register`] from a `#[test_gen]` function, and gives the suite a function that opens
//! stores over one new storage for each case. The suite writes its own trees and reads them back
//! with its own code, so it does not use the code of a store to check that store.

mod tree;

use super::{
    FilesystemSnapshotStore, SnapshotInfo, SnapshotName, SnapshotScope, SnapshotStoreError,
};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId, Timestamp};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use test_r::core::{DynamicTestRegistration, TestProperties};
use tree::{
    Listed, Scratch, Spec, files_and_bytes, fixture, listing, one_file, pattern, write_tree,
};
use uuid::Uuid;

/// Opens a store over the storage of one case. Each call opens another store over the same
/// storage, as another executor has.
pub(crate) type OpenStore = Arc<dyn Fn() -> Arc<dyn FilesystemSnapshotStore> + Send + Sync>;

type Case = fn(OpenStore) -> BoxFuture<'static, ()>;

/// Each case of the contract, with its name.
const CASES: &[(&str, Case)] = &[
    ("a_saved_tree_comes_back_the_same", |open| {
        a_saved_tree_comes_back_the_same(open).boxed()
    }),
    (
        "each_name_of_a_hard_linked_file_comes_back_as_its_own_file",
        |open| each_name_of_a_hard_linked_file_comes_back_as_its_own_file(open).boxed(),
    ),
    ("save_stat_list_and_restore_give_the_same_info", |open| {
        save_stat_list_and_restore_give_the_same_info(open).boxed()
    }),
    ("a_save_leaves_the_tree_as_it_was", |open| {
        a_save_leaves_the_tree_as_it_was(open).boxed()
    }),
    (
        "a_name_in_use_gives_already_exists_and_changes_nothing",
        |open| a_name_in_use_gives_already_exists_and_changes_nothing(open).boxed(),
    ),
    (
        "a_tree_that_cannot_be_read_gives_source_and_publishes_nothing",
        |open| a_tree_that_cannot_be_read_gives_source_and_publishes_nothing(open).boxed(),
    ),
    (
        "an_entry_that_cannot_be_read_gives_source_and_publishes_nothing",
        |open| an_entry_that_cannot_be_read_gives_source_and_publishes_nothing(open).boxed(),
    ),
    (
        "sequential_saves_get_later_times_and_list_newest_first",
        |open| sequential_saves_get_later_times_and_list_newest_first(open).boxed(),
    ),
    ("an_unknown_name_gives_not_found_every_time", |open| {
        an_unknown_name_gives_not_found_every_time(open).boxed()
    }),
    (
        "a_restore_into_a_directory_that_is_not_empty_writes_nothing",
        |open| a_restore_into_a_directory_that_is_not_empty_writes_nothing(open).boxed(),
    ),
    (
        "a_restore_into_a_path_that_is_not_a_directory_writes_nothing",
        |open| a_restore_into_a_path_that_is_not_a_directory_writes_nothing(open).boxed(),
    ),
    ("a_deleted_name_stops_resolving_at_once", |open| {
        a_deleted_name_stops_resolving_at_once(open).boxed()
    }),
    ("delete_is_idempotent", |open| {
        delete_is_idempotent(open).boxed()
    }),
    ("a_delete_keeps_every_other_snapshot", |open| {
        a_delete_keeps_every_other_snapshot(open).boxed()
    }),
    (
        "a_restore_that_races_a_delete_of_its_name_gives_a_whole_tree_or_nothing",
        |open| {
            a_restore_that_races_a_delete_of_its_name_gives_a_whole_tree_or_nothing(open).boxed()
        },
    ),
    (
        "a_restore_during_a_delete_of_another_name_gives_the_whole_tree",
        |open| a_restore_during_a_delete_of_another_name_gives_the_whole_tree(open).boxed(),
    ),
    (
        "a_deleted_scope_is_as_unused_as_before_its_first_save",
        |open| a_deleted_scope_is_as_unused_as_before_its_first_save(open).boxed(),
    ),
    (
        "delete_scope_is_idempotent_and_keeps_other_scopes",
        |open| delete_scope_is_idempotent_and_keeps_other_scopes(open).boxed(),
    ),
    (
        "a_copied_scope_has_the_same_names_infos_and_trees",
        |open| a_copied_scope_has_the_same_names_infos_and_trees(open).boxed(),
    ),
    ("copied_scopes_are_independent", |open| {
        copied_scopes_are_independent(open).boxed()
    }),
    (
        "a_copy_of_an_unused_scope_leaves_the_target_unused",
        |open| a_copy_of_an_unused_scope_leaves_the_target_unused(open).boxed(),
    ),
    ("one_name_in_two_scopes_gives_two_snapshots", |open| {
        one_name_in_two_scopes_gives_two_snapshots(open).boxed()
    }),
    (
        "a_save_through_one_store_resolves_through_another",
        |open| a_save_through_one_store_resolves_through_another(open).boxed(),
    ),
    (
        "two_stores_save_into_a_new_scope_at_the_same_time",
        |open| two_stores_save_into_a_new_scope_at_the_same_time(open).boxed(),
    ),
    (
        "a_dropped_save_publishes_the_whole_tree_or_nothing",
        |open| a_dropped_save_publishes_the_whole_tree_or_nothing(open).boxed(),
    ),
    ("no_method_blocks_the_runtime", |open| {
        no_method_blocks_the_runtime(open).boxed()
    }),
];

/// Registers each case of the contract as a test of the calling `#[test_gen]` function.
///
/// `new_storage` gives the function that opens stores over one new, empty storage. The suite
/// calls it one time for each case.
pub(crate) fn register(
    r: &mut DynamicTestRegistration,
    new_storage: impl Fn() -> OpenStore + Send + Sync + Clone + 'static,
) {
    CASES.iter().for_each(|&(name, case)| {
        let new_storage = new_storage.clone();
        r.add_async_test(name, TestProperties::unit_test(), None, move |_| {
            case(new_storage())
        });
    });
}

/// Gives a scope that no other case uses.
fn new_scope() -> SnapshotScope {
    SnapshotScope::agent(&OwnedAgentId::new(
        EnvironmentId(Uuid::new_v4()),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "counter(\"contract\")".to_string(),
        },
    ))
}

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

/// Writes the entries into a new directory, and gives the directory.
fn new_tree(entries: &[(&str, Spec)]) -> Scratch {
    let tree = Scratch::new();
    write_tree(tree.path(), entries);
    tree
}

/// Restores the snapshot into a new directory, and gives the listing of the directory with the
/// info that the restore gave.
async fn restored(
    store: &dyn FilesystemSnapshotStore,
    scope: &SnapshotScope,
    name: &SnapshotName,
) -> Result<(Vec<Listed>, SnapshotInfo), SnapshotStoreError> {
    let into = Scratch::new();
    let info = store.restore(scope, name, into.path()).await?;
    Ok((listing(into.path()), info))
}

/// Gives the listing of the restored snapshot, or panics with the error of the restore.
async fn restored_listing(
    store: &dyn FilesystemSnapshotStore,
    scope: &SnapshotScope,
    name: &SnapshotName,
) -> Vec<Listed> {
    restored(store, scope, name).await.unwrap().0
}

/// Gives the names of the listing of a scope, in the order of the listing.
async fn listed_names(store: &dyn FilesystemSnapshotStore, scope: &SnapshotScope) -> Vec<String> {
    store
        .list(scope)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect()
}

fn is_not_found<T>(result: &Result<T, SnapshotStoreError>) -> bool {
    matches!(result, Err(SnapshotStoreError::NotFound))
}

async fn a_saved_tree_comes_back_the_same(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());

    let saved = store
        .save(&scope, &name("p-fixture"), tree.path())
        .await
        .unwrap();
    let (restored, info) = restored(&*store, &scope, &name("p-fixture")).await.unwrap();

    assert_eq!((restored, info), (listing(tree.path()), saved));
}

async fn each_name_of_a_hard_linked_file_comes_back_as_its_own_file(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&one_file("linked"));
    std::fs::hard_link(
        tree.path().join("file.txt"),
        tree.path().join("second-name.txt"),
    )
    .unwrap();
    let into = Scratch::new();

    store
        .save(&scope, &name("p-linked"), tree.path())
        .await
        .unwrap();
    store
        .restore(&scope, &name("p-linked"), into.path())
        .await
        .unwrap();

    assert_eq!(listing(into.path()), listing(tree.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let first = std::fs::metadata(into.path().join("file.txt")).unwrap();
        let second = std::fs::metadata(into.path().join("second-name.txt")).unwrap();
        assert_eq!(
            (first.nlink(), second.nlink(), first.ino() == second.ino()),
            (1, 1, false)
        );
    }
}

async fn save_stat_list_and_restore_give_the_same_info(open: OpenStore) {
    // The scope is new, so the time of the snapshot is the time of the save, between the times
    // before and after the call.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());
    let (files, bytes) = files_and_bytes(&listing(tree.path()));

    let before = Timestamp::now_utc();
    let saved = store
        .save(&scope, &name("p-info"), tree.path())
        .await
        .unwrap();
    let after = Timestamp::now_utc();
    let stat = store.stat(&scope, &name("p-info")).await.unwrap();
    let list = store.list(&scope).await.unwrap();
    let (_, restored) = restored(&*store, &scope, &name("p-info")).await.unwrap();

    assert_eq!(
        (
            (saved.files, saved.bytes),
            before <= saved.created_at && saved.created_at <= after,
            stat,
            list.into_vec(),
            restored
        ),
        (
            (files, bytes),
            true,
            Some(saved),
            vec![(name("p-info"), saved)],
            saved
        )
    );
}

async fn a_save_leaves_the_tree_as_it_was(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());
    let before = listing(tree.path());

    store
        .save(&scope, &name("p-source"), tree.path())
        .await
        .unwrap();

    assert_eq!(listing(tree.path()), before);
}

async fn a_name_in_use_gives_already_exists_and_changes_nothing(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let first = new_tree(&one_file("first"));
    let second = new_tree(&one_file("second tree"));
    let saved = store
        .save(&scope, &name("p-taken"), first.path())
        .await
        .unwrap();

    let again = store.save(&scope, &name("p-taken"), second.path()).await;

    assert!(
        matches!(again, Err(SnapshotStoreError::AlreadyExists)),
        "{again:?}"
    );
    assert_eq!(
        (
            store.stat(&scope, &name("p-taken")).await.unwrap(),
            store.list(&scope).await.unwrap().into_vec(),
            restored_listing(&*store, &scope, &name("p-taken")).await
        ),
        (
            Some(saved),
            vec![(name("p-taken"), saved)],
            listing(first.path())
        )
    );
}

async fn a_tree_that_cannot_be_read_gives_source_and_publishes_nothing(open: OpenStore) {
    // A path that has nothing, and a path of a file, are trees that the store cannot read. After
    // each failure the name is free, so a later save of the name succeeds.
    let store = open();
    let scope = new_scope();
    let parent = Scratch::new();
    let missing = parent.path().join("missing");
    let file = parent.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    let tree = new_tree(&one_file("real"));

    let from_missing = store.save(&scope, &name("p-unread"), &missing).await;
    let from_file = store.save(&scope, &name("p-unread"), &file).await;
    let stat = store.stat(&scope, &name("p-unread")).await.unwrap();
    let names = listed_names(&*store, &scope).await;
    let restore = restored(&*store, &scope, &name("p-unread")).await;
    let later = store.save(&scope, &name("p-unread"), tree.path()).await;

    assert!(
        matches!(from_missing, Err(SnapshotStoreError::Source(_))),
        "{from_missing:?}"
    );
    assert!(
        matches!(from_file, Err(SnapshotStoreError::Source(_))),
        "{from_file:?}"
    );
    assert!(is_not_found(&restore), "{restore:?}");
    assert_eq!(
        (stat, names, later.is_ok()),
        (None, Vec::<String>::new(), true)
    );
}

async fn an_entry_that_cannot_be_read_gives_source_and_publishes_nothing(open: OpenStore) {
    // A file without read permission is an entry that the store cannot read. Permissions do not
    // bind a process that runs as root, so the case checks nothing there.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: `geteuid` has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let store = open();
        let scope = new_scope();
        let tree = new_tree(&one_file("readable"));
        let locked = tree.path().join("locked.txt");
        std::fs::write(&locked, b"locked").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let saved = store.save(&scope, &name("p-locked"), tree.path()).await;
        let stat = store.stat(&scope, &name("p-locked")).await.unwrap();
        let names = listed_names(&*store, &scope).await;

        assert!(
            matches!(saved, Err(SnapshotStoreError::Source(_))),
            "{saved:?}"
        );
        assert_eq!((stat, names), (None, Vec::<String>::new()));
    }
    #[cfg(not(unix))]
    drop(open);
}

async fn sequential_saves_get_later_times_and_list_newest_first(open: OpenStore) {
    // The saves follow each other at once, so the clock can give the same time to two of them.
    // Each save still gets a time that is later than the time of each snapshot in the scope.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&one_file("repeated"));
    let names = ["p-1", "p-2", "p-3", "p-4"];

    let saved = futures::stream::iter(names)
        .then(|text| {
            let store = store.clone();
            let scope = scope.clone();
            let tree = tree.path().to_path_buf();
            async move { store.save(&scope, &name(text), &tree).await.unwrap() }
        })
        .collect::<Vec<_>>()
        .await;
    let listed = store.list(&scope).await.unwrap();

    assert!(
        saved
            .windows(2)
            .all(|pair| pair[0].created_at < pair[1].created_at),
        "{saved:?}"
    );
    assert_eq!(
        listed.into_vec(),
        names
            .iter()
            .zip(&saved)
            .rev()
            .map(|(text, info)| (name(text), *info))
            .collect::<Vec<_>>()
    );
}

async fn an_unknown_name_gives_not_found_every_time(open: OpenStore) {
    // The first scope has never held a snapshot. The second holds another name.
    let store = open();
    let unused = new_scope();
    let used = new_scope();
    let tree = new_tree(&one_file("other"));
    store
        .save(&used, &name("p-other"), tree.path())
        .await
        .unwrap();

    let results = futures::stream::iter([&unused, &unused, &used, &used])
        .then(|scope| {
            let store = store.clone();
            async move {
                (
                    is_not_found(&restored(&*store, scope, &name("p-unknown")).await),
                    store.stat(scope, &name("p-unknown")).await.unwrap(),
                )
            }
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(results, vec![(true, None); 4]);
}

async fn a_restore_into_a_directory_that_is_not_empty_writes_nothing(open: OpenStore) {
    // The entry that is already in the directory has a name that the tree does not have, so
    // only the check of an empty directory can refuse the restore.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&one_file("content"));
    store
        .save(&scope, &name("p-busy"), tree.path())
        .await
        .unwrap();
    let into = new_tree(&[(
        "already-there.txt",
        Spec::File {
            content: b"already there".to_vec(),
            mode: 0o644,
        },
    )]);
    let before = listing(into.path());

    let result = store.restore(&scope, &name("p-busy"), into.path()).await;

    assert!(
        matches!(result, Err(SnapshotStoreError::Destination(_))),
        "{result:?}"
    );
    assert_eq!(listing(into.path()), before);
}

async fn a_restore_into_a_path_that_is_not_a_directory_writes_nothing(open: OpenStore) {
    // A path that has nothing, and a path of a file, cannot take the tree.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&one_file("content"));
    store
        .save(&scope, &name("p-nowhere"), tree.path())
        .await
        .unwrap();
    let parent = Scratch::new();
    let missing = parent.path().join("missing");
    let file = parent.path().join("file");
    std::fs::write(&file, b"a file").unwrap();

    let into_missing = store.restore(&scope, &name("p-nowhere"), &missing).await;
    let into_file = store.restore(&scope, &name("p-nowhere"), &file).await;

    assert!(
        matches!(into_missing, Err(SnapshotStoreError::Destination(_))),
        "{into_missing:?}"
    );
    assert!(
        matches!(into_file, Err(SnapshotStoreError::Destination(_))),
        "{into_file:?}"
    );
    assert_eq!(
        (missing.exists(), std::fs::read(&file).unwrap()),
        (false, b"a file".to_vec())
    );
}

async fn a_deleted_name_stops_resolving_at_once(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&one_file("deleted"));
    store
        .save(&scope, &name("p-deleted"), tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-kept"), tree.path())
        .await
        .unwrap();

    store.delete(&scope, &name("p-deleted")).await.unwrap();
    let stat = store.stat(&scope, &name("p-deleted")).await.unwrap();
    let restore = restored(&*store, &scope, &name("p-deleted")).await;
    let names = listed_names(&*store, &scope).await;

    assert!(is_not_found(&restore), "{restore:?}");
    assert_eq!((stat, names), (None, vec!["p-kept".to_string()]));
}

async fn delete_is_idempotent(open: OpenStore) {
    // A name of a scope that was never used, a name that was never saved, and a name that is
    // already deleted.
    let store = open();
    let unused = new_scope();
    let scope = new_scope();
    let tree = new_tree(&one_file("twice"));
    store
        .save(&scope, &name("p-twice"), tree.path())
        .await
        .unwrap();

    let results = [
        store.delete(&unused, &name("p-twice")).await.is_ok(),
        store.delete(&scope, &name("p-never")).await.is_ok(),
        store.delete(&scope, &name("p-twice")).await.is_ok(),
        store.delete(&scope, &name("p-twice")).await.is_ok(),
    ];

    assert_eq!(
        (results, listed_names(&*store, &scope).await),
        ([true; 4], Vec::<String>::new())
    );
}

async fn a_delete_keeps_every_other_snapshot(open: OpenStore) {
    // Two snapshots of one tree share all their data, and a third has a tree of its own.
    let store = open();
    let scope = new_scope();
    let shared = new_tree(&fixture());
    let other = new_tree(&one_file("other"));
    store
        .save(&scope, &name("p-twin-1"), shared.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-twin-2"), shared.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-other"), other.path())
        .await
        .unwrap();

    store.delete(&scope, &name("p-twin-1")).await.unwrap();

    assert_eq!(
        (
            restored_listing(&*store, &scope, &name("p-twin-2")).await,
            restored_listing(&*store, &scope, &name("p-other")).await,
            listed_names(&*store, &scope).await
        ),
        (
            listing(shared.path()),
            listing(other.path()),
            vec!["p-other".to_string(), "p-twin-2".to_string()]
        )
    );
}

async fn a_restore_that_races_a_delete_of_its_name_gives_a_whole_tree_or_nothing(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());
    store
        .save(&scope, &name("p-raced"), tree.path())
        .await
        .unwrap();

    let raced = name("p-raced");
    let (restore, deleted) = futures::join!(
        restored(&*store, &scope, &raced),
        store.delete(&scope, &raced)
    );

    deleted.unwrap();
    match restore {
        Ok((restored, _)) => assert_eq!(restored, listing(tree.path())),
        Err(SnapshotStoreError::NotFound | SnapshotStoreError::Corrupt(_)) => {}
        Err(error) => panic!("the restore gave {error:?}"),
    }
}

async fn a_restore_during_a_delete_of_another_name_gives_the_whole_tree(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());
    let other = new_tree(&fixture());
    store
        .save(&scope, &name("p-restored"), tree.path())
        .await
        .unwrap();
    store
        .save(&scope, &name("p-deleted"), other.path())
        .await
        .unwrap();

    let (restored_name, deleted_name) = (name("p-restored"), name("p-deleted"));
    let (restore, deleted) = futures::join!(
        restored(&*store, &scope, &restored_name),
        store.delete(&scope, &deleted_name)
    );

    deleted.unwrap();
    assert_eq!(restore.unwrap().0, listing(tree.path()));
}

async fn a_deleted_scope_is_as_unused_as_before_its_first_save(open: OpenStore) {
    let store = open();
    let scope = new_scope();
    let old = new_tree(&one_file("old"));
    let new = new_tree(&one_file("new tree"));
    store.save(&scope, &name("p-1"), old.path()).await.unwrap();
    store.save(&scope, &name("p-2"), old.path()).await.unwrap();

    store.delete_scope(&scope).await.unwrap();
    let names = listed_names(&*store, &scope).await;
    let stat = store.stat(&scope, &name("p-1")).await.unwrap();
    let restore = restored(&*store, &scope, &name("p-2")).await;
    store.save(&scope, &name("p-1"), new.path()).await.unwrap();

    assert!(is_not_found(&restore), "{restore:?}");
    assert_eq!(
        (
            names,
            stat,
            restored_listing(&*store, &scope, &name("p-1")).await
        ),
        (Vec::<String>::new(), None, listing(new.path()))
    );
}

async fn delete_scope_is_idempotent_and_keeps_other_scopes(open: OpenStore) {
    let store = open();
    let deleted = new_scope();
    let kept = new_scope();
    let tree = new_tree(&one_file("kept"));
    store
        .save(&deleted, &name("p-1"), tree.path())
        .await
        .unwrap();
    store.save(&kept, &name("p-1"), tree.path()).await.unwrap();

    let results = [
        store.delete_scope(&new_scope()).await.is_ok(),
        store.delete_scope(&deleted).await.is_ok(),
        store.delete_scope(&deleted).await.is_ok(),
    ];

    assert_eq!(
        (
            results,
            listed_names(&*store, &deleted).await,
            listed_names(&*store, &kept).await,
            restored_listing(&*store, &kept, &name("p-1")).await
        ),
        (
            [true; 3],
            Vec::<String>::new(),
            vec!["p-1".to_string()],
            listing(tree.path())
        )
    );
}

async fn a_copied_scope_has_the_same_names_infos_and_trees(open: OpenStore) {
    let store = open();
    let from = new_scope();
    let to = new_scope();
    let first = new_tree(&fixture());
    let second = new_tree(&one_file("second"));
    store.save(&from, &name("p-1"), first.path()).await.unwrap();
    store
        .save(&from, &name("u-2"), second.path())
        .await
        .unwrap();
    let source = store.list(&from).await.unwrap();

    store.copy_scope(&from, &to).await.unwrap();

    assert_eq!(
        (
            store.list(&to).await.unwrap(),
            store.list(&from).await.unwrap(),
            restored(&*store, &to, &name("p-1")).await.unwrap(),
            restored(&*store, &to, &name("u-2")).await.unwrap(),
            restored_listing(&*store, &from, &name("p-1")).await
        ),
        (
            source.clone(),
            source.clone(),
            (listing(first.path()), source[1].1),
            (listing(second.path()), source[0].1),
            listing(first.path())
        )
    );
}

async fn copied_scopes_are_independent(open: OpenStore) {
    // A delete in the source, a save in the target, and a delete of the target scope each leave
    // the other scope as it was.
    let store = open();
    let from = new_scope();
    let to = new_scope();
    let tree = new_tree(&one_file("copied"));
    let later = new_tree(&one_file("later"));
    store.save(&from, &name("p-1"), tree.path()).await.unwrap();
    store.save(&from, &name("p-2"), tree.path()).await.unwrap();
    store.copy_scope(&from, &to).await.unwrap();

    store.delete(&from, &name("p-1")).await.unwrap();
    store.save(&to, &name("p-3"), later.path()).await.unwrap();
    let target_after_source_delete = restored_listing(&*store, &to, &name("p-1")).await;
    let source_names = listed_names(&*store, &from).await;
    store.delete_scope(&to).await.unwrap();

    assert_eq!(
        (
            target_after_source_delete,
            source_names,
            listed_names(&*store, &from).await,
            restored_listing(&*store, &from, &name("p-2")).await
        ),
        (
            listing(tree.path()),
            vec!["p-2".to_string()],
            vec!["p-2".to_string()],
            listing(tree.path())
        )
    );
}

async fn a_copy_of_an_unused_scope_leaves_the_target_unused(open: OpenStore) {
    // Another scope of the storage holds a snapshot, so the storage itself is not empty.
    let store = open();
    let other = new_scope();
    let to = new_scope();
    let tree = new_tree(&one_file("other scope"));
    store
        .save(&other, &name("p-other"), tree.path())
        .await
        .unwrap();

    store.copy_scope(&new_scope(), &to).await.unwrap();

    assert_eq!(listed_names(&*store, &to).await, Vec::<String>::new());
}

async fn one_name_in_two_scopes_gives_two_snapshots(open: OpenStore) {
    let store = open();
    let first_scope = new_scope();
    let second_scope = new_scope();
    let first = new_tree(&one_file("first scope"));
    let second = new_tree(&one_file("second scope"));
    store
        .save(&first_scope, &name("p-same"), first.path())
        .await
        .unwrap();
    store
        .save(&second_scope, &name("p-same"), second.path())
        .await
        .unwrap();
    let first_restored = restored_listing(&*store, &first_scope, &name("p-same")).await;
    let second_restored = restored_listing(&*store, &second_scope, &name("p-same")).await;

    store.delete(&first_scope, &name("p-same")).await.unwrap();

    assert_eq!(
        (
            first_restored,
            second_restored,
            restored_listing(&*store, &second_scope, &name("p-same")).await
        ),
        (
            listing(first.path()),
            listing(second.path()),
            listing(second.path())
        )
    );
}

async fn a_save_through_one_store_resolves_through_another(open: OpenStore) {
    let writer = open();
    let reader = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());

    let saved = writer
        .save(&scope, &name("p-shared"), tree.path())
        .await
        .unwrap();

    assert_eq!(
        (
            reader.stat(&scope, &name("p-shared")).await.unwrap(),
            reader.list(&scope).await.unwrap().into_vec(),
            restored(&*reader, &scope, &name("p-shared")).await.unwrap()
        ),
        (
            Some(saved),
            vec![(name("p-shared"), saved)],
            (listing(tree.path()), saved)
        )
    );
}

async fn two_stores_save_into_a_new_scope_at_the_same_time(open: OpenStore) {
    // Each store makes the first save of the scope, as two executors can during a short overlap
    // of ownership.
    let first = open();
    let second = open();
    let scope = new_scope();
    let first_tree = new_tree(&one_file("first store"));
    let second_tree = new_tree(&one_file("second store"));

    let (first_name, second_name) = (name("p-first"), name("p-second"));
    let (first_saved, second_saved) = futures::join!(
        first.save(&scope, &first_name, first_tree.path()),
        second.save(&scope, &second_name, second_tree.path())
    );
    let mut names = listed_names(&*first, &scope).await;
    names.sort();

    assert_eq!(
        (
            first_saved.is_ok(),
            second_saved.is_ok(),
            names,
            restored_listing(&*second, &scope, &name("p-first")).await,
            restored_listing(&*first, &scope, &name("p-second")).await
        ),
        (
            true,
            true,
            vec!["p-first".to_string(), "p-second".to_string()],
            listing(first_tree.path()),
            listing(second_tree.path())
        )
    );
}

async fn a_dropped_save_publishes_the_whole_tree_or_nothing(open: OpenStore) {
    // The save is polled one time and then dropped. A store can finish the save after the drop,
    // so the name can resolve, but only to the whole tree.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&fixture());

    let dropped = store
        .save(&scope, &name("p-dropped"), tree.path())
        .now_or_never();
    let restore = restored(&*store, &scope, &name("p-dropped")).await;

    match (dropped, restore) {
        (Some(Ok(_)), Ok((restored, _))) | (None, Ok((restored, _))) => {
            assert_eq!(restored, listing(tree.path()))
        }
        (None, Err(SnapshotStoreError::NotFound)) => {}
        (dropped, restore) => panic!("the save gave {dropped:?} and the restore {restore:?}"),
    }
}

async fn no_method_blocks_the_runtime(open: OpenStore) {
    // The calls run in a `LocalSet` on one thread, next to a local task that counts. The task
    // runs only while a call waits, so a call that does its work without giving the thread back
    // leaves the count as it was. test-r runs a test on a runtime with more than one thread, so
    // a task of that runtime could count on another thread; the `LocalSet` keeps the count on
    // the thread of the calls.
    let store = open();
    let scope = new_scope();
    let tree = new_tree(&[(
        "large.bin",
        Spec::File {
            content: pattern(32 * 1024 * 1024),
            mode: 0o644,
        },
    )]);
    let into = Scratch::new();
    let tree_path = tree.path().to_path_buf();
    let into_path = into.path().to_path_buf();
    let handle = tokio::runtime::Handle::current();

    let (during_save, during_restore) = tokio::task::spawn_blocking(move || {
        handle.block_on(tokio::task::LocalSet::new().run_until(async move {
            let ticks = Arc::new(AtomicU64::new(0));
            let ticker = tokio::task::spawn_local({
                let ticks = ticks.clone();
                futures::stream::repeat(()).for_each(move |()| {
                    let ticks = ticks.clone();
                    async move {
                        ticks.fetch_add(1, Ordering::SeqCst);
                        tokio::task::yield_now().await;
                    }
                })
            });
            let counted = |from: u64| ticks.load(Ordering::SeqCst) - from;

            let before_save = ticks.load(Ordering::SeqCst);
            store
                .save(&scope, &name("p-large"), &tree_path)
                .await
                .unwrap();
            let during_save = counted(before_save);
            let before_restore = ticks.load(Ordering::SeqCst);
            store
                .restore(&scope, &name("p-large"), &into_path)
                .await
                .unwrap();
            let during_restore = counted(before_restore);
            ticker.abort();
            (during_save, during_restore)
        }))
    })
    .await
    .unwrap();

    assert!(
        during_save > 0 && during_restore > 0,
        "the local task counted {during_save} times during the save and {during_restore} times during the restore"
    );
    assert_eq!(listing(into.path()), listing(tree.path()));
}
