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

use super::*;
use crate::filesystem_snapshot::backend::rustic::blob_backend::{BlobBackend, KEPT_PACKS_LIMIT};
use crate::filesystem_snapshot::backend::rustic::failure_classification::is_lease_expired;
use crate::filesystem_snapshot::backend::rustic::pruning::timing::claim_hold;
use crate::filesystem_snapshot::backend::rustic::repository_layout::LEDGERS_PATH;
use crate::filesystem_snapshot::backend::rustic::test_support::files_of;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::clock::SystemClock;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use rustic_core::{FileType, ReadBackend};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use uuid::Uuid;

const GRACE: Duration = Duration::from_secs(15 * 60);

/// The hold of a claim with the grace period and the deadline, in milliseconds: the grace
/// period less one deadline, then the margin, then two deadlines.
const HOLD_MILLIS: u64 = HELD_MILLIS + 2 * 1000;

const DEADLINE: Duration = Duration::from_secs(2);

fn at(millis: u64) -> Timestamp {
    Timestamp::from(millis)
}

fn ledger(last_prune_millis: Option<u64>, awaiting_removal: bool) -> PruneLedger {
    PruneLedger {
        last_prune: last_prune_millis.map(Timestamp::from),
        awaiting_removal,
    }
}

fn new_files() -> SnapshotFiles {
    files_over(Arc::new(InMemoryBlobStorage::new()))
}

fn files_over(storage: Arc<dyn BlobStorage>) -> SnapshotFiles {
    files_with_cancel(storage, tokio_util::sync::CancellationToken::new())
}

fn files_with_cancel(
    storage: Arc<dyn BlobStorage>,
    cancel: tokio_util::sync::CancellationToken,
) -> SnapshotFiles {
    files_of(
        storage,
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        },
        DEADLINE,
        cancel,
    )
}

/// Refreshes the claim at each period until the first refresh write succeeds, and gives the
/// instant after that write. It gives `None` when the refresh ends before a write succeeded.
async fn refresh_until_written(
    files: &SnapshotFiles,
    period: Duration,
    lease: &Lease,
    span: Duration,
) -> Option<Instant> {
    let claim = claim_zero();
    std::pin::pin!(keep_claim_fresh(
        files,
        &claim,
        Instant::now(),
        period,
        lease,
        span,
        &SystemClock
    ))
    .next()
    .await
    .map(|_| Instant::now())
}

/// Gives the claim 0 of the ledger without a prune.
fn claim_zero() -> ClaimName {
    ClaimName {
        directory: claims_directory(&ledger(None, false)).into(),
        number: 0,
    }
}

/// The grace period and the margin for clock skew, in milliseconds.
const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;

#[test]
fn any_young_marker_holds_a_ledger_and_a_claim_without_a_marker_is_old() {
    let now = 10_000_000;
    let claim = ClaimEntry::Claim;
    let marker = |number, millis| ClaimEntry::Marker(number, at(millis));
    let choose = |entries: &[ClaimEntry]| next_claim(entries, at(now), claim_hold(GRACE, DEADLINE));

    assert_eq!(
        [
            choose(&[]),
            choose(&[claim(0), claim(1), marker(0, now - 1)]),
            choose(&[claim(0), claim(1), marker(1, now - HOLD_MILLIS)]),
            choose(&[claim(4)]),
            choose(&[marker(2, now - 1)]),
            choose(&[claim(0), claim(3), marker(0, now - HOLD_MILLIS)]),
        ],
        [
            ClaimChoice::Claim(0),
            ClaimChoice::Held,
            ClaimChoice::Claim(2),
            ClaimChoice::Claim(5),
            ClaimChoice::Held,
            ClaimChoice::Claim(4),
        ]
    );
}

#[test]
fn a_claim_entry_name_is_a_number_or_a_number_with_a_time_and_a_unique_part() {
    assert_eq!(
        [
            parse_claim_entry("3"),
            parse_claim_entry("3@42-a"),
            parse_claim_entry("3@42-a-b"),
            parse_claim_entry("3@42-"),
            parse_claim_entry("3@x-a"),
            parse_claim_entry("x"),
        ],
        [
            Some(ClaimEntry::Claim(3)),
            Some(ClaimEntry::Marker(3, at(42))),
            Some(ClaimEntry::Marker(3, at(42))),
            None,
            None,
            None,
        ]
    );
}

#[test]
#[timeout("60s")]
async fn the_refresh_of_a_claim_ends_when_its_operation_is_cancelled() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let files = files_with_cancel(Arc::new(InMemoryBlobStorage::new()), cancel.clone());
    cancel.cancel();

    let ended = tokio::time::timeout(
        Duration::from_secs(10),
        keep_claim_fresh(
            &files,
            &claim_zero(),
            Instant::now(),
            Duration::from_secs(3600),
            &Lease::until(Instant::now()),
            GRACE,
            &SystemClock,
        )
        .collect::<Vec<_>>(),
    )
    .await
    .is_ok();

    assert!(ended);
}

#[test]
fn the_first_refresh_starts_one_period_after_the_first_marker_and_not_after_the_call() {
    // The first marker was written 900 ms before the refresh starts, and the period is 1 s. So
    // the first refresh write starts 100 ms after the start of the refresh. The time is paused,
    // so each wait ends at its instant.
    let elapsed = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let files = files_over(Arc::new(InMemoryBlobStorage::new()));
            let started = crate::filesystem_snapshot::backend::rustic::call_execution::now();
            let marked = started.checked_sub(Duration::from_millis(900)).unwrap();
            let lease = Lease::until(started + Duration::from_secs(3600));
            let claim = claim_zero();
            let refreshes = keep_claim_fresh(
                &files,
                &claim,
                marked,
                Duration::from_secs(1),
                &lease,
                GRACE,
                &SystemClock,
            );
            std::pin::pin!(refreshes).next().await.unwrap();
            crate::filesystem_snapshot::backend::rustic::call_execution::now()
                .duration_since(started)
        });

    assert_eq!(elapsed, Duration::from_millis(100));
}

#[test]
#[timeout("60s")]
async fn a_refresh_that_starts_before_the_end_of_the_lease_and_ends_after_it_moves_the_lease_to_its_start_plus_the_span()
 {
    // The first refresh starts about 10 ms after the lease starts, before its end at 250 ms.
    // Each refresh write takes the delay, so the write ends after the end of the lease. A lease
    // from the end of the write would be later than a lease from its start by the delay.
    let delay = Duration::from_millis(500);
    let span = Duration::from_secs(1);
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
            if op_label == "refresh_claim" {
                Script::Delay(delay)
            } else {
                Script::Pass
            }
        });
    let files = files_over(storage);
    let started = Instant::now();
    let first_expiry = started + Duration::from_millis(250);
    let lease = Lease::until(first_expiry);

    let ended = refresh_until_written(&files, Duration::from_millis(10), &lease, span)
        .await
        .unwrap();

    let expiry = lease.expiry();
    assert!(
        ended > first_expiry,
        "the write ended before the end of the lease"
    );
    assert!(expiry >= started + span, "the lease did not move");
    assert!(
        expiry + delay / 2 < ended + span,
        "the lease moved to the end of the write"
    );
}

#[test]
#[timeout("60s")]
async fn a_refresh_that_starts_after_the_lease_ran_out_does_not_move_it_and_the_next_call_is_refused()
 {
    // The lease ends when the test starts. The first refresh write fails, and a later one
    // succeeds, but each refresh starts after the end of the lease. Nothing calls the backend
    // while the lease is out, and the first call after the refresh finds the lease still out.
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refused = refused.clone();
        move |op_label, _| {
            if op_label == "refresh_claim" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let files = files_over(storage.clone());
    let expiry = Instant::now();
    let lease = Arc::new(Lease::until(expiry));

    let refreshed = refresh_until_written(
        &files,
        Duration::from_millis(10),
        &lease,
        Duration::from_secs(3600),
    )
    .await
    .is_some();
    let backend = BlobBackend::new(
        files.leased(lease.clone()),
        tokio::runtime::Handle::current(),
        KEPT_PACKS_LIMIT,
    );
    let listed = tokio::task::spawn_blocking(move || backend.list(FileType::Snapshot))
        .await
        .unwrap();

    assert_eq!(
        (
            refreshed,
            refused.load(Ordering::SeqCst),
            lease.expiry() == expiry,
            listed
                .as_ref()
                .err()
                .is_some_and(|error| is_lease_expired(error)),
        ),
        (true, true, true, true),
        "{listed:?}"
    );
}

#[test]
#[timeout("60s")]
async fn a_claim_is_taken_after_its_marker_and_a_loser_deletes_its_marker() {
    let files = new_files();
    let directory = claims_directory(&ledger(Some(42), false));
    let marker = || marker_path(&directory, 0, marker_time(&SystemClock).1);
    let claim = ClaimName {
        directory: directory.clone().into(),
        number: 0,
    };
    let first_marker = marker();
    let second_marker = marker();
    let first = take_claim(&files, &claim, &first_marker).await.unwrap();
    let again = take_claim(&files, &claim, &second_marker).await.unwrap();
    let listed = list_claims(&files, &directory).await.unwrap();

    assert_eq!(
        (
            directory.display().to_string(),
            first,
            again,
            listed.len(),
            listed.contains(&ClaimEntry::Claim(0)),
            listed
                .iter()
                .any(|entry| matches!(entry, ClaimEntry::Marker(0, _))),
        ),
        (
            "golem/prune-claims/42".to_string(),
            true,
            false,
            2,
            true,
            true,
        )
    );
}

#[test]
fn a_claim_belongs_to_a_ledger_with_the_same_time_of_the_last_prune() {
    let directory = claims_directory(&ledger(Some(42), false));
    let none = claims_directory(&ledger(None, false));

    assert_eq!(
        [
            belongs_to_ledger(&directory, &ledger(Some(42), false)),
            belongs_to_ledger(&directory, &ledger(Some(42), true)),
            belongs_to_ledger(&directory, &ledger(Some(43), false)),
            belongs_to_ledger(&directory, &ledger(None, false)),
            belongs_to_ledger(&none, &ledger(Some(42), false)),
            belongs_to_ledger(&none, &ledger(None, true)),
        ],
        [true, true, false, false, false, true]
    );
}

#[test]
fn a_claim_directory_is_old_when_it_is_none_or_its_time_is_before_the_new_ledger() {
    let claim = |directory: &str, number: &str| Path::new(CLAIMS_PATH).join(directory).join(number);
    let directory = |name: &str| Path::new(CLAIMS_PATH).join(name);

    assert_eq!(
        old_claim_directories(
            [
                claim("none", "0"),
                claim("100", "0"),
                claim("100", "1@5-a"),
                claim("200", "3"),
                directory("250"),
                claim("260", "made/below"),
                claim("300", "0"),
                directory("300"),
                claim("301", "0"),
                directory("400"),
                claim("x1", "0"),
                Path::new(LEDGERS_PATH).join("5-0-a"),
                PathBuf::from(CLAIMS_PATH),
            ],
            Timestamp::from(300)
        )
        .to_vec(),
        ["100", "200", "250", "260", "none"].map(|name| directory(name).into_boxed_path())
    );
}

mod state_transitions {
    use super::super::{ClaimEvent, ClaimState, Cleanup, Markers, apply, transition};
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use test_r::test;

    fn path(name: &str) -> Box<Path> {
        Path::new(name).into()
    }

    fn markers() -> Markers {
        Markers {
            first: Arc::from(Path::new("first")),
            refreshed: Vec::new(),
        }
    }

    fn with_refresh() -> Markers {
        Markers {
            first: Arc::from(Path::new("first")),
            refreshed: vec![path("refresh")],
        }
    }

    fn release(claimed: bool) -> Option<Cleanup> {
        Some(Cleanup::Release {
            claimed,
            first: Arc::from(Path::new("first")),
            refreshed: Box::new([]),
        })
    }

    /// The events in the order of the columns of the table.
    fn events() -> [ClaimEvent; 8] {
        [
            ClaimEvent::Won,
            ClaimEvent::Lost,
            ClaimEvent::Start,
            ClaimEvent::Refreshed(path("refresh")),
            ClaimEvent::Release,
            ClaimEvent::Finish,
            ClaimEvent::FinalMarkerWritten,
            ClaimEvent::Dropped,
        ]
    }

    /// Gives the state and the cleanup after each event, from the state.
    fn row(state: ClaimState) -> Vec<(ClaimState, Option<Cleanup>)> {
        events()
            .into_iter()
            .map(|event| transition(state.clone(), event))
            .collect()
    }

    #[test]
    fn a_claim_that_writes_its_first_marker_is_taken_lost_or_released_with_that_marker() {
        let marking = || ClaimState::Marking { markers: markers() };

        assert_eq!(
            row(marking()),
            vec![
                (ClaimState::Claimed { markers: markers() }, None),
                (ClaimState::Ended, None),
                (marking(), None),
                (
                    ClaimState::Marking {
                        markers: with_refresh()
                    },
                    None
                ),
                (ClaimState::Ended, release(false)),
                (marking(), None),
                (marking(), None),
                (ClaimState::Ended, release(false)),
            ]
        );
    }

    #[test]
    fn a_claim_that_is_taken_starts_its_prune_or_is_released_with_the_claim() {
        // A finish before the start comes from a blocking task that failed before it started the
        // prune. It writes no final marker, and the drop then releases the claim.
        let claimed = || ClaimState::Claimed { markers: markers() };

        assert_eq!(
            row(claimed()),
            vec![
                (claimed(), None),
                (claimed(), None),
                (ClaimState::Started { markers: markers() }, None),
                (
                    ClaimState::Claimed {
                        markers: with_refresh()
                    },
                    None
                ),
                (ClaimState::Ended, release(true)),
                (claimed(), None),
                (claimed(), None),
                (ClaimState::Ended, release(true)),
            ]
        );
    }

    #[test]
    fn a_claim_whose_prune_started_keeps_the_claim_and_gets_its_final_marker() {
        // A finish keeps the state until the final marker is written, so a drop after a failed write
        // writes it again. A release after the start comes only when each attempt of the prune found
        // a snapshot file gone.
        let started = || ClaimState::Started { markers: markers() };

        assert_eq!(
            row(started()),
            vec![
                (started(), None),
                (started(), None),
                (started(), None),
                (
                    ClaimState::Started {
                        markers: with_refresh()
                    },
                    None
                ),
                (ClaimState::Ended, release(true)),
                (started(), Some(Cleanup::FinalMarker)),
                (ClaimState::Ended, None),
                (ClaimState::Ended, Some(Cleanup::FinalMarker)),
            ]
        );
    }

    #[test]
    fn an_ended_claim_needs_nothing_more() {
        assert_eq!(
            row(ClaimState::Ended),
            vec![(ClaimState::Ended, None); events().len()]
        );
    }

    #[test]
    fn a_released_claim_never_starts_a_prune_and_a_drop_after_the_start_does_not_release_it() {
        // The start of the prune on the blocking thread can come at the same time as a refresh or a
        // drop of the delete. The lock orders them, so one of the two comes first.
        let in_order = |first: ClaimEvent, second: ClaimEvent| {
            let state = Mutex::new(ClaimState::Claimed { markers: markers() });
            let first = apply(&state, first);
            let second = apply(&state, second);
            (first, second)
        };

        // Only the start lets the prune run, and only the first start of a claim does.
        assert_eq!(
            (
                in_order(ClaimEvent::Start, ClaimEvent::Dropped),
                in_order(ClaimEvent::Dropped, ClaimEvent::Start),
                in_order(ClaimEvent::Refreshed(path("refresh")), ClaimEvent::Start),
                in_order(ClaimEvent::Start, ClaimEvent::Start),
            ),
            (
                ((true, None), (false, Some(Cleanup::FinalMarker))),
                ((false, release(true)), (false, None)),
                ((false, None), (true, None)),
                ((true, None), (false, None)),
            )
        );
    }
}
