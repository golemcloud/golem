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

use super::{InMemorySnapshotStore, Stored};
use crate::filesystem_snapshot::contract::{self, OpenStore, new_scope};
use crate::filesystem_snapshot::{FilesystemSnapshotStore, SnapshotInfo, SnapshotName};
use golem_common::model::Timestamp;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_r::core::DynamicTestRegistration;
use test_r::{test, test_gen};

#[test_gen]
fn in_memory_store_keeps_the_contract(r: &mut DynamicTestRegistration) {
    contract::register(r, || {
        let store = InMemorySnapshotStore::new();
        let open: OpenStore =
            Arc::new(move || Arc::new(store.clone()) as Arc<dyn FilesystemSnapshotStore>);
        open
    });
}

/// A snapshot that another store saved with a clock that is ahead of this clock has a time
/// later than the time now. The next save still gets a later time, so the listing stays newest
/// first, also when the clocks of two executors differ.
#[test]
async fn a_save_after_a_snapshot_from_a_clock_that_is_ahead_gets_a_later_time() {
    let store = InMemorySnapshotStore::new();
    let scope = new_scope();
    let ahead = Timestamp::from(Timestamp::now_utc().to_millis() + 3_600_000);
    store.scopes().insert(
        scope.clone(),
        Arc::from(vec![Stored {
            name: SnapshotName::new("p-ahead").unwrap(),
            info: SnapshotInfo {
                created_at: ahead,
                files: 0,
                bytes: 0,
            },
            tree: Arc::from(Vec::new()),
        }]),
    );
    let tree = tempfile::tempdir().unwrap();

    let saved = store
        .save(&scope, &SnapshotName::new("p-next").unwrap(), tree.path())
        .await
        .unwrap();
    let listed = store.list(&scope).await.unwrap();

    assert_eq!(
        (
            saved.created_at > ahead,
            listed
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        ),
        (true, vec!["p-next", "p-ahead"])
    );
}

#[cfg(unix)]
mod unix {
    use crate::filesystem_snapshot::contract::new_scope;
    use crate::filesystem_snapshot::{
        FilesystemSnapshotStore, InMemorySnapshotStore, SnapshotName, SnapshotStoreError,
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

        let saved = store.save(&scope, &name, tree.path()).await;
        let listed = store.list(&scope).await.unwrap();

        assert!(
            matches!(
                &saved,
                Err(SnapshotStoreError::Source(error)) if error.kind() == ErrorKind::InvalidInput
            ),
            "{saved:?}"
        );
        assert!(listed.is_empty(), "{listed:?}");
    }
}
