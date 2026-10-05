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

use super::super::fault::{CallFailure, LeaseExpired, OperationCancelled};
use super::super::tests::polled_until;
use super::super::tests::scripted::{Script, ScriptedBlobStorage};
use super::{
    BeforeTry, CallAgain, IN_CALL_TRIES, IN_CALL_WAITS, LateWrites, Lease, SnapshotFiles,
    Unanswered, before_try, call_again, extended, lands_by, later,
};
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
    let held = polled_until(Duration::from_secs(10), || !storage.calls().is_empty()).await;
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

#[test]
fn the_location_of_the_files_names_their_namespace() {
    let namespace = BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::new_v4()),
    };
    let files = SnapshotFiles::new(
        passing(),
        namespace.clone(),
        DEADLINE,
        CancellationToken::new(),
        TaskTracker::new(),
    );

    assert_eq!(
        files.location(),
        format!("golem-blob-storage:{namespace:?}")
    );
}

#[test]
#[timeout("60s")]
async fn a_leased_call_that_gets_no_answer_ends_at_the_expiry_of_the_lease() {
    // The deadline of the files is 10 s, so only the lease can end the call near 500 ms.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| {
        Script::NeverAnswer
    });
    let files = files_over(storage, CancellationToken::new(), TaskTracker::new());
    let started = Instant::now();
    let expiry = started + Duration::from_millis(500);

    let read = files
        .leased(Arc::new(Lease::until(expiry)))
        .get("read", Path::new("a"))
        .await;
    let ended = Instant::now();

    assert!(
        read.as_ref().is_err_and(|error| error.is::<LeaseExpired>()),
        "{read:?}"
    );
    assert!(
        ended >= expiry && ended < expiry + Duration::from_secs(1),
        "the call ended {:?} after its start",
        ended - started
    );
}

#[test]
#[timeout("60s")]
async fn a_refresh_of_the_lease_during_a_call_does_not_move_the_end_of_that_call() {
    // A task waits until the call reached the storage, and then moves the end of the lease 10 s
    // later. The call still ends at the expiry that the lease had when the call started.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| {
        Script::NeverAnswer
    });
    let files = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let started = Instant::now();
    let expiry = started + Duration::from_millis(500);
    let lease = Arc::new(Lease::until(expiry));
    let refreshing = tokio::spawn({
        let (storage, lease) = (storage.clone(), lease.clone());
        async move {
            let reached =
                polled_until(Duration::from_millis(400), || !storage.calls().is_empty()).await;
            let refreshed_at = Instant::now();
            lease.extend_from(refreshed_at, Duration::from_secs(10));
            (reached, refreshed_at)
        }
    });

    let read = files
        .leased(lease.clone())
        .get("read", Path::new("a"))
        .await;
    let ended = Instant::now();
    let (reached, refreshed_at) = refreshing.await.unwrap();

    assert!(
        read.as_ref().is_err_and(|error| error.is::<LeaseExpired>()),
        "{read:?}"
    );
    assert_eq!(
        (
            reached,
            refreshed_at < expiry,
            lease.expiry() > expiry + Duration::from_secs(5),
            ended >= expiry && ended < expiry + Duration::from_secs(1),
        ),
        (true, true, true, true),
        "the call ended {:?} after its start",
        ended - started
    );
}

#[test]
fn a_call_tries_again_only_after_a_failure_with_tries_and_time_left_and_cuts_the_try_at_the_time_left()
 {
    let deadline = Duration::from_secs(60);
    let spent = Duration::from_secs(20);

    assert_eq!(
        [
            call_again(1, IN_CALL_TRIES, CallFailure::Failed, spent, deadline),
            call_again(2, IN_CALL_TRIES, CallFailure::Failed, spent, deadline),
            call_again(3, IN_CALL_TRIES, CallFailure::Failed, spent, deadline),
            call_again(1, 1, CallFailure::Failed, spent, deadline),
            call_again(1, IN_CALL_TRIES, CallFailure::Failed, deadline, deadline),
            call_again(
                1,
                IN_CALL_TRIES,
                CallFailure::Failed,
                deadline - Duration::from_millis(1),
                deadline
            ),
            call_again(1, IN_CALL_TRIES, CallFailure::TimedOut, spent, deadline),
            call_again(1, IN_CALL_TRIES, CallFailure::Cancelled, spent, deadline),
            call_again(1, IN_CALL_TRIES, CallFailure::LeaseExpired, spent, deadline),
            call_again(1, IN_CALL_TRIES, CallFailure::Permanent, spent, deadline),
        ],
        [
            CallAgain::After {
                wait: IN_CALL_WAITS[0],
                cut: Duration::from_secs(40)
            },
            CallAgain::After {
                wait: IN_CALL_WAITS[1],
                cut: Duration::from_secs(40)
            },
            CallAgain::End,
            CallAgain::End,
            CallAgain::End,
            CallAgain::After {
                wait: IN_CALL_WAITS[0],
                cut: Duration::from_millis(1)
            },
            CallAgain::End,
            CallAgain::End,
            CallAgain::End,
            CallAgain::End,
        ]
    );
}

/// A storage whose first `refused` calls give an error after `delay`, and whose later calls pass.
fn refusing_first(refused: usize, delay: Duration) -> Arc<ScriptedBlobStorage> {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |_, _| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < refused {
            Script::RefuseAfter(delay)
        } else {
            Script::Pass
        }
    })
}

#[test]
#[timeout("60s")]
async fn a_blob_call_that_fails_fast_fewer_times_than_the_in_call_tries_does_not_end_the_run() {
    let storage = refusing_first(2, Duration::ZERO);
    let files = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let once = files.once();

    let read = files.get("read", Path::new("a")).await;
    let storage_once = refusing_first(1, Duration::ZERO);
    let read_once = files_over(
        storage_once.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    )
    .once()
    .get("read", Path::new("a"))
    .await;
    drop(once);

    assert!(matches!(read, Ok(None)), "{read:?}");
    assert!(read_once.is_err(), "{read_once:?}");
    assert_eq!((storage.calls().len(), storage_once.calls().len()), (3, 1));
}

#[test]
fn a_blob_call_never_holds_its_run_longer_than_one_deadline_and_the_waits() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(the_tries_of_a_blob_call_that_never_answers());
}

/// The tries of a call whose storage answers each try with an error after 4 s, with a deadline
/// of 10 s.
async fn the_tries_of_a_blob_call_that_never_answers() {
    let deadline = Duration::from_secs(10);
    let storage = refusing_first(usize::MAX, Duration::from_secs(4));
    let files = SnapshotFiles::new(
        storage.clone(),
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        },
        deadline,
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let started = tokio::time::Instant::now();

    let read = files.get("read", Path::new("a")).await;

    assert!(read.is_err(), "{read:?}");
    assert_eq!(
        (storage.calls().len(), started.elapsed()),
        (3, deadline + IN_CALL_WAITS[0] + IN_CALL_WAITS[1])
    );
}

#[test]
#[timeout("60s")]
async fn a_write_whose_try_ended_without_an_answer_is_late_also_when_a_later_try_succeeds() {
    let late = Arc::new(LateWrites::default());
    let lost = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        move |_, _| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Script::LoseTheAnswer
            } else {
                Script::Pass
            }
        }
    });
    let files =
        files_over(lost, CancellationToken::new(), TaskTracker::new()).recording(late.clone());
    let read_only = Arc::new(LateWrites::default());
    let refused_reads = files_over(
        refusing_first(usize::MAX, Duration::ZERO),
        CancellationToken::new(),
        TaskTracker::new(),
    )
    .recording(read_only.clone());

    let written = files.put("write", Path::new("a"), b"a").await;
    let read = refused_reads.get("read", Path::new("a")).await;

    assert!(written.is_ok(), "{written:?}");
    assert!(read.is_err(), "{read:?}");
    assert_eq!(
        (late.latest().is_some(), read_only.latest().is_some()),
        (true, false)
    );
}

#[test]
fn a_refusal_before_a_try_comes_in_a_fixed_order_and_only_a_try_has_a_cut() {
    let cut = Some(Duration::from_secs(1));

    assert_eq!(
        [
            before_try(true, true, true, None),
            before_try(false, true, true, None),
            before_try(false, false, true, None),
            before_try(false, false, true, cut),
            before_try(false, false, false, None),
            before_try(false, false, false, cut),
            before_try(true, false, false, cut),
            before_try(false, true, false, cut),
        ],
        [
            BeforeTry::LeaseOut,
            BeforeTry::Cancelled,
            BeforeTry::Stop,
            BeforeTry::Stop,
            BeforeTry::NoTimeLeft,
            BeforeTry::Try(Duration::from_secs(1)),
            BeforeTry::LeaseOut,
            BeforeTry::Cancelled,
        ]
    );
}

#[test]
fn the_latest_late_try_is_kept_and_lands_one_deadline_after_its_end() {
    let t = Instant::now();
    let second = Duration::from_secs(1);

    assert_eq!(
        (
            later(None, t),
            later(Some(t + second), t),
            later(Some(t), t + second),
            lands_by(None, DEADLINE),
            lands_by(Some(t), DEADLINE),
        ),
        (t, t + second, t + second, None, Some(t + DEADLINE))
    );
}

#[test]
#[timeout("60s")]
async fn a_write_that_the_cancel_refused_before_its_try_records_nothing() {
    let storage = passing();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let late = Arc::new(LateWrites::default());
    let files = files_over(storage.clone(), cancel, TaskTracker::new()).recording(late.clone());

    let written = files.put("test", Path::new("a"), b"a").await;

    assert_eq!(
        (
            written.map_err(|error| error.is::<OperationCancelled>()),
            late.latest(),
            storage.calls().len()
        ),
        (Err(true), None, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_bounded_write_whose_stop_fires_between_its_tries_stops_after_one_try() {
    // The first try is refused with an answer. The stop fires during the wait before the second
    // try, so the second try does not start.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Refuse);
    let files = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    )
    .with_tries(3);
    let stop = CancellationToken::new();
    let stopping = {
        let stop = stop.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stop.cancel();
        }
    };

    let (written, ()) = tokio::join!(
        files.put_if_absent_bounded(
            "test",
            Path::new("a"),
            b"a",
            |start| Some(start.deadline.saturating_sub(start.spent)),
            &stop,
        ),
        stopping
    );

    assert_eq!(
        (
            written.map(|_| ()).map_err(|not_answered| not_answered.why),
            storage.calls().len()
        ),
        (Err(Unanswered::Stopped), 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_bounded_write_whose_stop_and_whose_lack_of_time_hold_at_once_stops() {
    let storage = passing();
    let files = files_over(
        storage.clone(),
        CancellationToken::new(),
        TaskTracker::new(),
    );
    let stop = CancellationToken::new();
    stop.cancel();

    let written = files
        .put_if_absent_bounded("test", Path::new("a"), b"a", |_| None, &stop)
        .await;

    assert_eq!(
        (
            written.map(|_| ()).map_err(|not_answered| not_answered.why),
            storage.calls().len()
        ),
        (Err(Unanswered::Stopped), 0)
    );
}
