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

use super::super::test_support::{at, ledger};
use super::super::timing::{CLOCK_SKEW_MARGIN, claim_hold};
use super::*;
use crate::filesystem_snapshot::backend::rustic::repository_layout::FREED_PATH;
use pretty_assertions::assert_eq;
use test_r::test;
const TEN_PERCENT: Percent = Percent(10);

const GRACE: Duration = Duration::from_secs(15 * 60);

/// The grace period and the margin for clock skew, in milliseconds.
const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;

/// The hold of a claim with the grace period and the deadline, in milliseconds: the grace
/// period less one deadline, then the margin, then two deadlines.
const HOLD_MILLIS: u64 = HELD_MILLIS + 2 * 1000;

const DEADLINE: Duration = Duration::from_secs(2);

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
