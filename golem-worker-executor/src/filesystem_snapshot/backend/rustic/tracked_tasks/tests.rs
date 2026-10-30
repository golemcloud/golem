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

use crate::filesystem_snapshot::backend::rustic::test_support::{
    LIMIT, LONG_DEADLINE, NEVER, one_file_tree, policy, store,
};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::task::AbortOnDropHandle;

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
            store
                .check_path(&path, move |_| {
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
