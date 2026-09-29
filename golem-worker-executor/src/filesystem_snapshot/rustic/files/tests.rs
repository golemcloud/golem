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

use super::super::fault::{LeaseExpired, OperationCancelled};
use super::super::tests::scripted::{Script, ScriptedBlobStorage};
use super::{Lease, SnapshotFiles, extended};
use futures::StreamExt;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::BlobStorageNamespace;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(10);

fn files_over(
    storage: Arc<ScriptedBlobStorage>,
    cancel: CancellationToken,
    tracker: TaskTracker,
) -> SnapshotFiles {
    SnapshotFiles::new(
        storage,
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        },
        DEADLINE,
        cancel,
        tracker,
    )
}

fn passing() -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass)
}

#[test]
fn a_write_that_starts_before_the_end_of_the_lease_moves_it_to_its_span_and_a_later_one_does_not() {
    let end = Instant::now() + Duration::from_secs(60);
    let span = Duration::from_secs(10);
    let before = end - Duration::from_secs(1);
    let long_before = end - Duration::from_secs(30);

    assert_eq!(
        [
            extended(end, before, span),
            extended(end, end - Duration::from_nanos(1), span),
            extended(end, end, span),
            extended(end, end + Duration::from_secs(1), span),
            extended(end, long_before, span),
            extended(end, before, Duration::ZERO),
        ],
        [
            before + span,
            end - Duration::from_nanos(1) + span,
            end,
            end,
            end,
            end,
        ]
    );
}

#[test]
#[timeout("60s")]
async fn a_leased_call_after_the_lease_ran_out_is_not_sent_and_a_call_without_the_lease_is() {
    let storage = passing();
    let files = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let lease = Arc::new(Lease::until(Instant::now()));

    let leased = files
        .leased(lease)
        .put("write_ledger", Path::new("golem/leased"), b"")
        .await;
    let plain = files
        .put("write_marker", Path::new("golem/plain"), b"")
        .await;

    assert!(
        leased
            .as_ref()
            .is_err_and(|error| error.is::<LeaseExpired>()),
        "{leased:?}"
    );
    assert!(plain.is_ok(), "{plain:?}");
    assert_eq!(
        storage.calls(),
        vec![("write_marker", "golem/plain".to_string())]
    );
}

#[test]
#[timeout("60s")]
async fn a_call_whose_lease_ran_out_and_whose_operation_is_cancelled_gives_lease_expired() {
    let storage = passing();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let files = files_over(storage.clone(), cancel, TaskTracker::new())
        .leased(Arc::new(Lease::until(Instant::now())));
    let unleased = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let cancelled_only = {
        let cancel = CancellationToken::new();
        cancel.cancel();
        files_over(storage.clone(), cancel, TaskTracker::new()).leased(Arc::new(Lease::until(
            Instant::now() + Duration::from_secs(60),
        )))
    };

    let both = files.get("read", Path::new("a")).await;
    let cancelled = cancelled_only.get("read", Path::new("a")).await;
    let neither = unleased.get("read", Path::new("a")).await;

    assert!(
        both.as_ref().is_err_and(|error| error.is::<LeaseExpired>()),
        "{both:?}"
    );
    assert!(
        cancelled
            .as_ref()
            .is_err_and(|error| error.is::<OperationCancelled>()),
        "{cancelled:?}"
    );
    assert!(matches!(neither, Ok(None)), "{neither:?}");
    assert_eq!(storage.calls(), vec![("read", "a".to_string())]);
}

#[test]
#[timeout("60s")]
async fn detached_files_are_not_cancelled_have_no_lease_and_their_calls_are_tracked() {
    // The held call keeps the tracker from being empty until the gate opens.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "final_marker" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let files = files_over(storage.clone(), cancel.clone(), tracker.clone())
        .leased(Arc::new(Lease::until(Instant::now())));
    cancel.cancel();
    let detached = files.detached();

    let writing = tokio::spawn(async move {
        detached
            .put_if_absent("final_marker", Path::new("golem/marker"), b"")
            .await
    });
    let held = futures::stream::repeat(())
        .then(|()| tokio::time::sleep(Duration::from_millis(5)))
        .take(2000)
        .any(|()| std::future::ready(!storage.calls().is_empty()))
        .await;
    let counted = tracker.len();
    storage.open_gate();
    let written = tokio::time::timeout(Duration::from_secs(10), writing).await;

    assert!(matches!(written, Ok(Ok(Ok(_)))), "{written:?}");
    assert_eq!((held, counted, tracker.len()), (true, 1, 0));
}

#[test]
fn a_marker_write_that_starts_at_the_end_of_the_lease_does_not_move_it_and_one_that_starts_before_does()
 {
    let end = Instant::now() + Duration::from_secs(60);
    let span = Duration::from_secs(10);
    let at_the_end = Lease::until(end);
    let just_before = Lease::until(end);

    at_the_end.extend_from(end, span);
    just_before.extend_from(end - Duration::from_nanos(1), span);

    assert_eq!(
        (at_the_end.expiry(), just_before.expiry()),
        (end, end - Duration::from_nanos(1) + span)
    );
}
