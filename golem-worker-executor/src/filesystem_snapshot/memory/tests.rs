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

use super::InMemorySnapshotStore;
use crate::filesystem_snapshot::contract_tests::clock::TestClock;
use crate::filesystem_snapshot::contract_tests::{self, OpenStore, new_scope};
use crate::filesystem_snapshot::{FilesystemSnapshotStore, SaveError, SnapshotInfo, SnapshotName};
use golem_common::model::Timestamp;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_r::core::DynamicTestRegistration;
use test_r::{test, test_gen};

#[test_gen]
fn in_memory_store_keeps_the_contract(r: &mut DynamicTestRegistration) {
    contract_tests::register(r, || {
        let store = InMemorySnapshotStore::new();
        let open: OpenStore =
            Arc::new(move || Arc::new(store.clone()) as Arc<dyn FilesystemSnapshotStore>);
        open
    });
}

/// A snapshot that a store saved with a clock that is ahead of the clock now has a time later than
/// the time now. The next save still gets a later time, so the listing stays newest first, also
/// when the clocks of two executors differ.
#[test]
async fn a_save_after_a_snapshot_from_a_clock_that_is_ahead_gets_a_later_time() {
    let clock = Arc::new(TestClock::default());
    let store = InMemorySnapshotStore::with_clock(clock.clone());
    let scope = new_scope();
    let tree = tempfile::tempdir().unwrap();
    clock.set_ahead(Duration::from_secs(3600));
    let ahead = store
        .save(
            &scope,
            &SnapshotName::new("p-ahead").unwrap(),
            tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    clock.set_ahead(Duration::ZERO);
    let later_than_now =
        ahead.created_at.to_millis() >= Timestamp::now_utc().to_millis() + 3_600_000 - 60_000;

    let saved = store
        .save(
            &scope,
            &SnapshotName::new("p-next").unwrap(),
            tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let listed = store
        .list(&scope, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            later_than_now,
            saved.created_at > ahead.created_at,
            listed
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        ),
        (true, true, vec!["p-next", "p-ahead"])
    );
}

/// Gives a tree that holds `file.txt` with the text `marker`, and a larger file that makes the
/// read of the tree take some time.
fn tree_with(marker: &str) -> TempDir {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("file.txt"), marker).unwrap();
    std::fs::write(tree.path().join("large.bin"), vec![7u8; 4 * 1024 * 1024]).unwrap();
    tree
}

/// Two stores over one storage save one name at the same time. Each save finds the name free
/// when it starts. The store checks the name again when it publishes a snapshot, so only one
/// save wins. The contract does not give a winner for two saves of one name, so this is a test of
/// this store only.
#[test]
async fn of_two_saves_of_one_name_at_the_same_time_one_wins() {
    let first = InMemorySnapshotStore::new();
    let second = first.clone();
    let scope = new_scope();
    let name = SnapshotName::new("p-same").unwrap();
    let first_tree = tree_with("first tree");
    let second_tree = tree_with("second tree");

    let (first_saved, second_saved) = futures::join!(
        first.save(
            &scope,
            &name,
            first_tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited
        ),
        second.save(
            &scope,
            &name,
            second_tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited
        )
    );
    let into = tempfile::tempdir().unwrap();
    first
        .restore(
            &scope,
            &name,
            into.path(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let restored = std::fs::read_to_string(into.path().join("file.txt")).unwrap();

    let outcome = |saved: &Result<SnapshotInfo, SaveError>| match saved {
        Ok(_) => "saved",
        Err(SaveError::NameInUse) => "already exists",
        Err(_) => "another error",
    };
    let winner = if first_saved.is_ok() {
        "first tree"
    } else {
        "second tree"
    };
    let mut outcomes = [outcome(&first_saved), outcome(&second_saved)];
    outcomes.sort();
    assert_eq!(
        (outcomes, restored.as_str()),
        (["already exists", "saved"], winner)
    );
}

#[cfg(unix)]
mod unix {
    use crate::filesystem_snapshot::contract_tests::new_scope;
    use crate::filesystem_snapshot::{
        FilesystemSnapshotStore, InMemorySnapshotStore, SaveError, SnapshotName,
    };
    use std::io::ErrorKind;
    use std::os::unix::net::UnixListener;
    use test_r::test;

    /// A socket is not a regular file, a directory or a symlink, so it is outside the contract.
    /// The store must not open such an entry, because the read of a FIFO waits for a writer that
    /// never comes. The store gives `Source` with the `InvalidInput` kind of its own check, and
    /// not the error of an open.
    #[test]
    async fn a_tree_with_a_socket_gives_source_without_an_open_and_publishes_nothing() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("file"), b"file").unwrap();
        let _listener = UnixListener::bind(tree.path().join("s")).unwrap();
        let store = InMemorySnapshotStore::new();
        let scope = new_scope();
        let name = SnapshotName::new("p-socket").unwrap();

        let saved = store
            .save(
                &scope,
                &name,
                tree.path(),
                None,
                crate::filesystem_snapshot::never_cancelled(),
                &crate::filesystem_snapshot::Unlimited,
            )
            .await;
        let listed = store
            .list(&scope, &crate::filesystem_snapshot::Unlimited)
            .await
            .unwrap();

        assert!(
            matches!(
                &saved,
                Err(SaveError::Source(error)) if error.kind() == ErrorKind::InvalidInput
            ),
            "{saved:?}"
        );
        assert!(listed.is_empty(), "{listed:?}");
    }
}

/// A caller never writes into an agent after a delete of all its snapshots. The test store refuses
/// such a save or copy, and records it for the finish step of the tests that use it.
#[test]
async fn a_save_or_a_copy_into_an_agent_after_a_delete_of_all_its_snapshots_is_refused_and_recorded()
 {
    let store = InMemorySnapshotStore::new();
    let (deleted, other) = (new_scope(), new_scope());
    let tree = tree_with("tree");
    store
        .delete_all(&deleted, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap();

    let saved = store
        .save(
            &deleted,
            &SnapshotName::new("p-after").unwrap(),
            tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    let copied = store
        .copy_all(&other, &deleted, &crate::filesystem_snapshot::Unlimited)
        .await;
    let read = store
        .list(&deleted, &crate::filesystem_snapshot::Unlimited)
        .await;

    assert!(matches!(saved, Err(SaveError::Failed(_))), "{saved:?}");
    assert!(copied.is_err(), "{copied:?}");
    assert!(read.is_ok_and(|listed| listed.is_empty()));
    assert_eq!(
        store
            .writes_after_delete_all()
            .iter()
            .map(|write| write.split(" into ").next().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        ["a save", "a copy"]
    );
}
