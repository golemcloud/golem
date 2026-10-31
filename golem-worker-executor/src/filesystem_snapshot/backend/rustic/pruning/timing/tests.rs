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

use super::super::test_support::{DEADLINE, GRACE};
use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

const MILLI: Duration = Duration::from_millis(1);

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
