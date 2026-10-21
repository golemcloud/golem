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

use super::super::backend::{BlobBackend, KEPT_PACKS_LIMIT};
use super::super::fault::is_lease_expired;
use super::super::files::SnapshotFiles;
use super::super::tests::files_of;
use super::super::tests::scripted::{Script, ScriptedBlobStorage};
use super::{
    CLAIMS_PATH, CLOCK_SKEW_MARGIN, ClaimChoice, ClaimEntry, ClaimName, FREED_PATH, FreedRecord,
    FreedRecords, LEDGERS_PATH, Lease, ListedFreed, Next, Observed, Percent, PruneLedger,
    PrunePolicy, belongs_to_ledger, claim_hold, claims_directory, keep_claim_fresh, lease_bound,
    lease_span, list_claims, list_freed_names, marker_path, marker_time, may_be_due, named_bytes,
    newest_ledger, next, next_claim, old_claim_directories, older_entries, parse_claim_entry,
    parse_freed, parse_ledger_entry, parse_record, read_ledger, record_content, record_freed,
    refresh_period, settle, settle_freed, take_claim, write_ledger,
};
use crate::filesystem_snapshot::clock::SystemClock;
use futures::StreamExt;
use golem_common::model::Timestamp;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::BlobStorageNamespace;
use golem_service_base::storage::blob::ListedBlob;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use rustic_core::{FileType, ReadBackend};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;
use test_r::{test, timeout};
use uuid::Uuid;

const TEN_PERCENT: Percent = Percent(10);
const GRACE: Duration = Duration::from_secs(15 * 60);
/// The grace period and the margin for clock skew, in milliseconds.
const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;
/// The hold of a claim with the grace period and the deadline, in milliseconds: the grace
/// period less one deadline, then the margin, then two deadlines.
const HOLD_MILLIS: u64 = HELD_MILLIS + 2 * 1000;
const DEADLINE: Duration = Duration::from_secs(2);
const MILLI: Duration = Duration::from_millis(1);

fn at(millis: u64) -> Timestamp {
    Timestamp::from(millis)
}

fn ledger(last_prune_millis: Option<u64>, awaiting_removal: bool) -> PruneLedger {
    PruneLedger {
        last_prune: last_prune_millis.map(Timestamp::from),
        awaiting_removal,
    }
}

/// Gives one listed record of freed bytes whose name holds the bytes.
fn named(bytes: u64) -> Box<[ListedFreed]> {
    Box::new([ListedFreed {
        path: Path::new(FREED_PATH).join(format!("{bytes}-a")).into(),
        bytes,
    }])
}

/// Tells whether a prune is due, through [`next`]: the delete found the ledger, a record whose
/// name holds the freed bytes, the size of the repository, and settled records with the freed
/// bytes. The prune is due when [`next`] then asks for the claims.
fn prune_due(
    ledger: &PruneLedger,
    freed_bytes: u64,
    now: Timestamp,
    repository_bytes: u64,
    threshold: Percent,
    grace: Duration,
    deadline: Duration,
) -> bool {
    let observed = Observed {
        ledger: Some(*ledger),
        freed: Some(named(freed_bytes)),
        size: Some(repository_bytes),
        settled: Some(FreedRecords {
            bytes: freed_bytes,
            counted: Box::default(),
        }),
        claims: None,
    };
    let policy = PrunePolicy {
        grace,
        deadline,
        threshold,
    };
    matches!(next(&observed, now, &policy), Next::ListClaims(_))
}

/// Tells whether the due check needs the size of the repository, through [`next`]: the delete
/// found the ledger and a record whose name holds the freed bytes, and [`next`] then asks for
/// the packs.
fn needs_repository_size(
    ledger: &PruneLedger,
    freed_bytes: u64,
    now: Timestamp,
    grace: Duration,
    deadline: Duration,
) -> bool {
    let observed = Observed {
        ledger: Some(*ledger),
        freed: Some(named(freed_bytes)),
        ..Observed::default()
    };
    let policy = PrunePolicy {
        grace,
        deadline,
        threshold: TEN_PERCENT,
    };
    next(&observed, now, &policy) == Next::ListPacks
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

/// The policy of the value tests of the due check: the grace period, the deadline and 10%.
const POLICY: PrunePolicy = PrunePolicy {
    grace: GRACE,
    deadline: DEADLINE,
    threshold: TEN_PERCENT,
};

fn settled(bytes: u64) -> FreedRecords {
    FreedRecords {
        bytes,
        counted: Box::default(),
    }
}

#[test]
fn a_due_check_lists_each_kind_once_in_order_and_ends_with_the_claim_after_the_largest() {
    let now = at(10_000_000);
    let last = ledger(Some(10_000_000 - HOLD_MILLIS), false);
    let directory = claims_directory(&last);
    let ledger_found = Observed {
        ledger: Some(last),
        ..Observed::default()
    };
    let freed_found = Observed {
        freed: Some(named(200)),
        ..ledger_found.clone()
    };
    let size_found = Observed {
        size: Some(1000),
        ..freed_found.clone()
    };
    let settled_found = Observed {
        settled: Some(settled(150)),
        ..size_found.clone()
    };
    let claims_found = Observed {
        claims: Some(Box::new([ClaimEntry::Claim(3)])),
        ..settled_found.clone()
    };

    assert_eq!(
        [
            &Observed::default(),
            &ledger_found,
            &freed_found,
            &size_found,
            &settled_found,
            &claims_found,
        ]
        .map(|observed| next(observed, now, &POLICY)),
        [
            Next::ListLedgers,
            Next::ListFreed,
            Next::ListPacks,
            Next::SettleFreed,
            Next::ListClaims(directory.clone()),
            Next::Claim(ClaimName {
                directory: directory.into(),
                number: 4,
            }),
        ]
    );
}

#[test]
fn a_due_check_stops_as_soon_as_no_prune_can_be_due_and_reads_no_more() {
    let now = at(10_000_000);
    let passed = Some(10_000_000 - HOLD_MILLIS);
    let observed = |ledger, freed, size, settled, claims| Observed {
        ledger: Some(ledger),
        freed: Some(named(freed)),
        size,
        settled,
        claims,
    };
    let decide = |observed: Observed| next(&observed, now, &POLICY);

    assert_eq!(
        [
            // Within the hold, nothing more is read.
            decide(Observed {
                ledger: Some(ledger(Some(10_000_000 - HOLD_MILLIS + 1), true)),
                ..Observed::default()
            }),
            // Nothing freed and nothing marked.
            decide(observed(ledger(passed, false), 0, None, None, None)),
            // Marked packs need no size, so the records come next.
            decide(observed(ledger(passed, true), 0, None, None, None)),
            // The named bytes are below the threshold, so no record is read.
            decide(observed(ledger(passed, false), 99, Some(1000), None, None)),
            // The settled bytes are below the threshold.
            decide(observed(
                ledger(passed, false),
                200,
                Some(1000),
                Some(settled(99)),
                None
            )),
            // A young marker of another delete holds the claims.
            decide(observed(
                ledger(passed, false),
                200,
                Some(1000),
                Some(settled(100)),
                Some(Box::new([ClaimEntry::Marker(0, at(10_000_000 - 1))])),
            )),
        ],
        [
            Next::Stop,
            Next::Stop,
            Next::SettleFreed,
            Next::Stop,
            Next::Stop,
            Next::Stop,
        ]
    );
}

#[test]
fn a_due_check_that_observed_everything_gives_a_claim_or_stops() {
    // The loop of the due check makes at most one pass for each kind of listing, so `next`
    // must not ask for a listing again once each one ran.
    let now = at(10_000_000);
    let ledgers = [
        ledger(None, false),
        ledger(None, true),
        ledger(Some(10_000_000), false),
        ledger(Some(10_000_000 - HOLD_MILLIS), true),
    ];
    let claims: [Box<[ClaimEntry]>; 2] = [
        Box::new([]),
        Box::new([ClaimEntry::Marker(0, at(10_000_000))]),
    ];
    let terminal = ledgers.iter().all(|ledger| {
        [0, 99, 100, 5000].iter().all(|bytes| {
            claims.iter().all(|claims| {
                let observed = Observed {
                    ledger: Some(*ledger),
                    freed: Some(named(*bytes)),
                    size: Some(1000),
                    settled: Some(settled(*bytes)),
                    claims: Some(claims.clone()),
                };
                matches!(next(&observed, now, &POLICY), Next::Stop | Next::Claim(_))
            })
        })
    });

    assert!(terminal);
}

#[test]
fn the_bytes_in_the_names_of_the_records_can_make_a_prune_due_only_when_they_reach_the_threshold() {
    let may = |awaiting_removal, named, repository_bytes| {
        may_be_due(
            &ledger(None, awaiting_removal),
            named,
            repository_bytes,
            TEN_PERCENT,
        )
    };

    assert_eq!(
        [
            may(false, 99, 1000),
            may(false, 100, 1000),
            may(false, 0, 0),
            may(false, 1, 0),
            may(true, 0, 1000),
        ],
        [false, true, false, true, true]
    );
}

#[test]
fn a_prune_is_due_when_the_freed_bytes_reach_ten_percent_of_the_repository_rounded_down() {
    let now = at(10_000_000);
    let due = |freed, repository_bytes| {
        prune_due(
            &ledger(None, false),
            freed,
            now,
            repository_bytes,
            TEN_PERCENT,
            GRACE,
            DEADLINE,
        )
    };

    assert_eq!(
        [
            due(99, 1000),
            due(100, 1000),
            due(101, 1000),
            due(0, 0),
            due(1, 0),
        ],
        [false, true, true, false, true]
    );
}

#[test]
fn no_second_prune_runs_within_the_hold_of_a_claim_after_the_last_prune() {
    // The grace period and the margin passed at `HELD_MILLIS`, and the full hold at
    // `HOLD_MILLIS`.
    let last = 1_000_000;
    let full = |now| {
        prune_due(
            &ledger(Some(last), true),
            1000,
            at(now),
            1000,
            TEN_PERCENT,
            GRACE,
            DEADLINE,
        )
    };

    assert_eq!(
        [
            full(last),
            full(last + HELD_MILLIS),
            full(last + HOLD_MILLIS - 1),
            full(last + HOLD_MILLIS),
            full(last + HOLD_MILLIS + 1),
        ],
        [false, false, false, true, true]
    );
}

#[test]
fn marked_packs_make_a_prune_due_after_the_hold_without_freed_bytes() {
    let last = 1_000_000;
    let after_hold = at(last + HOLD_MILLIS);
    let due = |awaiting_removal| {
        prune_due(
            &ledger(Some(last), awaiting_removal),
            0,
            after_hold,
            1000,
            TEN_PERCENT,
            GRACE,
            DEADLINE,
        )
    };

    assert_eq!([due(true), due(false)], [true, false]);
}

#[test]
fn a_zero_threshold_prunes_after_each_delete_that_freed_bytes() {
    let now = at(10_000_000);
    let due = |ledger, freed| {
        prune_due(
            &ledger,
            freed,
            now,
            1000,
            Percent(0),
            Duration::ZERO,
            DEADLINE,
        )
    };
    let hold = u64::try_from(claim_hold(Duration::ZERO, DEADLINE).as_millis()).unwrap();

    assert_eq!(
        [
            due(ledger(None, false), 0),
            due(ledger(None, false), 1),
            due(ledger(Some(10_000_000 - hold), false), 1),
        ],
        [false, true, true]
    );
}

#[test]
fn only_freed_bytes_after_the_hold_without_marked_packs_need_the_repository_size() {
    let last = 1_000_000;
    let needs = |freed, awaiting_removal, now| {
        needs_repository_size(
            &ledger(Some(last), awaiting_removal),
            freed,
            at(now),
            GRACE,
            DEADLINE,
        )
    };

    assert_eq!(
        [
            needs(1, false, last + HOLD_MILLIS),
            needs(1, false, last + HOLD_MILLIS - 1),
            needs(0, false, last + HOLD_MILLIS),
            needs(1, true, last + HOLD_MILLIS),
        ],
        [true, false, false, false]
    );
}

#[test]
fn the_hold_holds_the_ledger_and_the_claims_and_a_time_more_than_the_margin_ahead_counts_as_missing()
 {
    let now = 10_000_000;
    let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap();
    let due = |last| {
        prune_due(
            &ledger(Some(last), false),
            1,
            at(now),
            0,
            Percent(0),
            GRACE,
            DEADLINE,
        )
    };
    let claim = |claimed_at| {
        next_claim(
            &[ClaimEntry::Claim(0), ClaimEntry::Marker(0, at(claimed_at))],
            at(now),
            claim_hold(GRACE, DEADLINE),
        )
    };

    assert_eq!(
        (
            [
                due(now - HOLD_MILLIS + 1),
                due(now - HOLD_MILLIS),
                due(now + margin),
                due(now + margin + 1)
            ],
            [
                claim(now - HOLD_MILLIS + 1),
                claim(now - HOLD_MILLIS),
                claim(now + margin),
                claim(now + margin + 1)
            ]
        ),
        (
            [false, true, false, true],
            [
                ClaimChoice::Held,
                ClaimChoice::Claim(1),
                ClaimChoice::Held,
                ClaimChoice::Claim(1)
            ]
        )
    );
}

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
fn the_lease_is_one_period_and_two_deadlines_and_at_most_the_lease_bound() {
    let second = Duration::from_secs(1);
    let minute = Duration::from_secs(60);
    let cases = [
        (GRACE, minute),
        (2 * minute, minute),
        (2 * minute - second, minute),
        (Duration::ZERO, minute),
        (minute, Duration::ZERO),
        (Duration::ZERO, second),
        (Duration::from_millis(16), minute),
    ];
    let period = |grace: Duration, deadline: Duration| refresh_period(grace, deadline);

    assert_eq!(
        cases.map(|(grace, deadline)| lease_span(grace, deadline)),
        [
            period(GRACE, minute) + 2 * minute,
            minute - MILLI,
            minute - MILLI,
            minute - MILLI,
            period(minute, Duration::ZERO),
            second - MILLI,
            minute - MILLI,
        ]
    );
    // The invariant of the claim protocol: a call of a prune whose lease ran out lands before
    // another delete can take the claim over.
    assert!(cases.iter().all(|(grace, deadline)| {
        lease_span(*grace, *deadline) + *deadline <= lease_bound(*grace, *deadline) + 2 * *deadline
    }));
    assert_eq!(
        lease_span(GRACE, minute),
        Duration::from_secs(330) - Duration::from_micros(250)
    );
}

#[test]
fn the_hold_is_the_lease_then_one_margin_then_two_deadlines() {
    let margin = CLOCK_SKEW_MARGIN;
    let minute = Duration::from_secs(60);

    assert_eq!(
        [
            claim_hold(GRACE, minute),
            claim_hold(GRACE, DEADLINE),
            claim_hold(Duration::ZERO, minute),
            claim_hold(Duration::ZERO, Duration::from_millis(1)),
        ],
        [
            GRACE + margin + minute,
            GRACE + margin + DEADLINE,
            minute + margin + 2 * minute,
            MILLI + margin + 2 * MILLI,
        ]
    );
}

#[test]
fn the_refresh_period_is_a_fourth_of_the_grace_or_of_the_margin_and_at_most_a_fourth_of_the_lease_bound()
 {
    let minute = Duration::from_secs(60);
    let cases = [
        (GRACE, minute),
        (GRACE, DEADLINE),
        (Duration::ZERO, minute),
        (Duration::ZERO, 2 * MILLI),
        (Duration::from_millis(16), minute),
        (Duration::from_secs(2), Duration::from_secs(1)),
    ];

    assert_eq!(
        cases.map(|(grace, deadline)| refresh_period(grace, deadline)),
        [
            (GRACE - minute - MILLI) / 4,
            (GRACE - DEADLINE - MILLI) / 4,
            (minute - MILLI) / 4,
            Duration::from_micros(250),
            Duration::from_millis(4),
            Duration::from_micros(249_750),
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
            let started = super::super::runs::now();
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
            super::super::runs::now().duration_since(started)
        });

    assert_eq!(elapsed, Duration::from_millis(100));
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

#[test]
fn a_ledger_entry_name_gives_the_end_time_and_the_marked_packs() {
    assert_eq!(
        [
            parse_ledger_entry("42-1-a"),
            parse_ledger_entry("43-0-a-b"),
            parse_ledger_entry("42-2-a"),
            parse_ledger_entry("42-1-"),
            parse_ledger_entry("42-1"),
            parse_ledger_entry("x-1-a"),
        ],
        [
            Some(ledger(Some(42), true)),
            Some(ledger(Some(43), false)),
            None,
            None,
            None,
            None
        ]
    );
}

#[test]
fn the_newest_entry_within_the_margin_is_the_ledger() {
    let now = 10_000_000;
    let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap();
    let entry = |name: &str| ListedBlob {
        path: Path::new(LEDGERS_PATH).join(name).into(),
        size: 0,
    };
    let newest = |names: &[&str]| {
        newest_ledger(
            &names.iter().map(|name| entry(name)).collect::<Vec<_>>(),
            at(now),
        )
    };

    assert_eq!(
        [
            newest(&[]),
            newest(&["5-0-a", "9-1-b", "7-0-c"]),
            newest(&["5-0-a", "not-an-entry"]),
            newest(&["5-0-a", &format!("{}-1-b", now + margin)]),
            newest(&["5-0-a", &format!("{}-1-b", now + margin + 1)]),
        ],
        [
            PruneLedger::default(),
            ledger(Some(9), true),
            ledger(Some(5), false),
            ledger(Some(now + margin), true),
            ledger(Some(5), false),
        ]
    );
}

#[test]
fn only_entries_before_the_end_of_a_prune_are_older() {
    let entry = |name: &str| ListedBlob {
        path: Path::new(LEDGERS_PATH).join(name).into(),
        size: 0,
    };

    assert_eq!(
        older_entries(
            &[
                entry("5-0-a"),
                entry("9-1-own"),
                entry("9-0-same-time"),
                entry("12-0-newer"),
                entry("bad"),
            ],
            at(9)
        )
        .to_vec(),
        vec![Path::new(LEDGERS_PATH).join("5-0-a").into_boxed_path()]
    );
}

#[test]
fn the_freed_bytes_of_a_record_are_the_number_before_the_first_dash() {
    assert_eq!(
        [
            parse_freed("123-0f4e"),
            parse_freed("0-a-b"),
            parse_freed("123"),
            parse_freed("123-"),
            parse_freed("x-0f4e"),
            parse_freed("-0f4e"),
        ],
        [Some(123), Some(0), None, None, None, None]
    );
}

#[test]
fn a_record_counts_only_when_each_of_its_snapshot_files_is_gone() {
    let id = |digit: char| {
        std::iter::repeat_n(digit, 64)
            .collect::<String>()
            .into_boxed_str()
    };
    let record = |name: &str, bytes: u64, snapshots: Option<Vec<Box<str>>>| FreedRecord {
        path: Path::new(FREED_PATH).join(name).into(),
        bytes,
        snapshots: snapshots.map(Vec::into_boxed_slice),
    };
    let existing = [id('b')].into_iter().collect();

    let settled = settle(
        &[
            record("5-gone", 5, Some(vec![id('a')])),
            record("7-kept", 7, Some(vec![id('a'), id('b')])),
            record("11-empty", 11, Some(Vec::new())),
            record("13-bad", 13, None),
            record(&format!("{}-max", u64::MAX), u64::MAX, Some(vec![id('c')])),
        ],
        &existing,
    );

    assert_eq!(
        settled,
        FreedRecords {
            bytes: u64::MAX,
            counted: Box::new([
                Path::new(FREED_PATH).join("5-gone").into(),
                Path::new(FREED_PATH)
                    .join(format!("{}-max", u64::MAX))
                    .into(),
            ]),
        }
    );
}

#[test]
fn the_content_of_a_record_holds_one_snapshot_id_on_each_line() {
    let id = |digit: char| {
        std::iter::repeat_n(digit, 64)
            .collect::<String>()
            .into_boxed_str()
    };
    let ids = [id('a'), id('b')];

    assert_eq!(
        [
            parse_record(record_content(&ids).as_bytes()),
            parse_record(b""),
            parse_record(b"not an id"),
            parse_record(&[0xff, 0xfe]),
        ],
        [
            Some(Box::new(ids.clone()) as Box<[Box<str>]>),
            None,
            None,
            None
        ]
    );
}

#[test]
fn a_record_line_parses_only_when_it_has_64_characters_that_are_each_hex() {
    // A record whose content does not parse names no snapshot file, so it counts as zero
    // bytes and stays.
    let hex = "0123456789abcdef".repeat(4);
    let parsed = [
        parse_record("g".repeat(64).as_bytes()),
        parse_record(b"abcdef0123"),
        parse_record(hex.as_bytes()),
    ];
    let records = parsed
        .iter()
        .enumerate()
        .map(|(index, snapshots)| FreedRecord {
            path: Path::new(FREED_PATH)
                .join(format!("10-{index}"))
                .into_boxed_path(),
            bytes: 10,
            snapshots: snapshots.clone(),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        (
            parsed.clone(),
            settle(&records, &std::collections::HashSet::new()),
        ),
        (
            [
                None,
                None,
                Some(Box::new([hex.clone().into_boxed_str()]) as Box<[Box<str>]>)
            ],
            FreedRecords {
                bytes: 10,
                counted: Box::new([Path::new(FREED_PATH).join("10-2").into_boxed_path()]),
            },
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_record_of_freed_bytes_is_written_and_listed() {
    let files = new_files();

    let gone = ["0".repeat(64).into_boxed_str()];
    record_freed(&files, 40, &gone).await.unwrap();
    record_freed(&files, 2, &gone).await.unwrap();
    let listed = list_freed_names(&files).await.unwrap();
    let settled = settle_freed(&files, &listed).await.unwrap();

    assert_eq!(
        (named_bytes(&listed), settled.bytes, settled.counted.len()),
        (42, 42, 2)
    );
}

#[test]
#[timeout("60s")]
async fn a_written_entry_is_the_ledger_that_a_read_gives() {
    let files = new_files();
    let ended = Timestamp::from(Timestamp::now_utc().to_millis());

    let before = read_ledger(&files, &SystemClock).await.unwrap();
    write_ledger(&files, ended, true).await.unwrap();
    let after = read_ledger(&files, &SystemClock).await.unwrap();

    assert_eq!(
        (before, after),
        (
            PruneLedger::default(),
            PruneLedger {
                last_prune: Some(ended),
                awaiting_removal: true
            }
        )
    );
}
