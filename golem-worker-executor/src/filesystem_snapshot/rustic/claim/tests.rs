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

use super::{ClaimEvent, ClaimState, Cleanup, Markers, apply, transition};
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
