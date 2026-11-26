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

use super::{
    InvocationFilter, InvocationKind, InvocationListEntry, InvocationState, merge_invocations,
    page_invocations,
};
use golem_common::model::{
    AgentStatus, AgentStatusRecord, InvocationEnd, InvocationId, InvocationOperation,
    InvocationRequestRef, InvocationResultPosition, OplogIndex, PendingInvocationRef,
    RunningInvocation, Timestamp,
};
use std::collections::HashMap;
use test_r::test;
use uuid::Uuid;

fn key(value: u128) -> InvocationId {
    InvocationId(Uuid::from_u128(value))
}

fn index(value: u64) -> OplogIndex {
    OplogIndex::from_u64(value)
}

fn method(name: &str) -> Option<InvocationOperation> {
    Some(InvocationOperation::Method(name.to_string()))
}

fn result(terminal: u64, end: InvocationEnd) -> InvocationResultPosition {
    InvocationResultPosition {
        terminal: index(terminal),
        request: Some(index(1)),
        end,
        operation: method("m"),
        accepted_at: Some(Timestamp::from(1)),
        changed_at: Timestamp::from(terminal * 10),
    }
}

fn running(key: InvocationId, started: u64, last_change: u64) -> RunningInvocation {
    RunningInvocation {
        key,
        request: InvocationRequestRef {
            index: index(started),
            accepted_at: Timestamp::from(1),
            operation: method("m"),
        },
        last_change: index(last_change),
        last_change_at: Timestamp::from(last_change * 10),
    }
}

fn pending(key: InvocationId, at: u64, not_before: Option<u64>) -> PendingInvocationRef {
    PendingInvocationRef {
        timestamp: Timestamp::from(at * 10),
        not_before: not_before.map(Timestamp::from),
        oplog_index: index(at),
        idempotency_key: Some(key),
        manual_update_target_revision: None,
        operation: method("m"),
    }
}

fn states(
    status: &AgentStatusRecord,
    results: HashMap<InvocationId, InvocationResultPosition>,
    now: u64,
) -> Vec<(InvocationId, InvocationState, OplogIndex)> {
    merge_invocations(status, results, Timestamp::from(now), None)
        .into_iter()
        .map(|entry| (entry.key, entry.state, entry.last_change))
        .collect()
}

#[test]
fn a_terminal_standalone_startup_failure_ends_only_its_unanswered_invocation() {
    let invocation = key(1);
    let other = key(2);
    for not_before in [None, Some(1_000)] {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Failed,
            last_error_kind: Some(golem_common::model::oplog::OplogErrorKind::Recovery),
            terminal_failure_at: Some(Timestamp::from(70)),
            terminal_failure_index: Some(index(7)),
            oplog_idx: index(8),
            pending_invocations: vec![
                pending(invocation, 2, not_before),
                pending(other, 3, not_before),
            ],
            ..AgentStatusRecord::default()
        };
        let pending_state = if not_before.is_some() {
            InvocationState::Scheduled
        } else {
            InvocationState::Pending
        };
        for owner in [None, Some(invocation)] {
            let entries = merge_invocations(&status, HashMap::new(), Timestamp::from(80), owner);
            let entry = entries
                .iter()
                .find(|entry| entry.key == invocation)
                .unwrap();
            assert_eq!(
                entry.state,
                if owner.is_some() {
                    InvocationState::Failed
                } else {
                    pending_state
                }
            );
            assert_eq!(entry.accepted_at, Some(Timestamp::from(20)));
            if owner.is_some() {
                assert_eq!(entry.changed_at, Timestamp::from(70));
                assert_eq!(entry.last_change, index(7));
            }
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.key == other)
                    .unwrap()
                    .state,
                pending_state
            );
        }
        for (end, expected) in [
            (InvocationEnd::Completed, InvocationState::Completed),
            (InvocationEnd::Cancelled, InvocationState::Cancelled),
        ] {
            let entries = merge_invocations(
                &status,
                HashMap::from([(invocation, result(4, end))]),
                Timestamp::from(80),
                Some(invocation),
            );
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.key == invocation)
                    .unwrap()
                    .state,
                expected
            );
        }
        status.status = AgentStatus::Retrying;
        let entries = merge_invocations(
            &status,
            HashMap::new(),
            Timestamp::from(80),
            Some(invocation),
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.key == invocation)
                .unwrap()
                .state,
            pending_state
        );
        status.status = AgentStatus::Failed;
        status.running = Some(running(invocation, 4, 5));
        for (owner, expected) in [
            (None, InvocationState::Running),
            (Some(invocation), InvocationState::Failed),
        ] {
            let entries = merge_invocations(&status, HashMap::new(), Timestamp::from(80), owner);
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.key == invocation)
                    .unwrap()
                    .state,
                expected
            );
        }
    }
}

#[test]
fn the_running_invocation_after_a_retry_marker_is_retrying_and_listed_once() {
    let retried = key(1);
    let status = AgentStatusRecord {
        status: AgentStatus::Retrying,
        running: Some(running(retried, 2, 3)),
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(retried, result(3, InvocationEnd::Retrying))]),
            0
        ),
        vec![(retried, InvocationState::Retrying, index(3))]
    );
}

#[test]
fn a_running_invocation_that_recovered_from_a_retry_is_running_again() {
    let recovered = key(1);
    let status = AgentStatusRecord {
        status: AgentStatus::Running,
        running: Some(running(recovered, 2, 3)),
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(recovered, result(3, InvocationEnd::Retrying))]),
            0
        ),
        vec![(recovered, InvocationState::Running, index(3))]
    );
}

#[test]
fn a_queued_duplicate_of_a_key_that_exhausted_its_retries_is_not_listed() {
    let gave_up = key(1);
    let status = AgentStatusRecord {
        status: AgentStatus::Idle,
        pending_invocations: vec![pending(gave_up, 5, None)],
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(gave_up, result(3, InvocationEnd::Retrying))]),
            0
        ),
        vec![(gave_up, InvocationState::Failed, index(3))]
    );
}

#[test]
fn a_retry_marker_of_an_agent_that_gave_up_is_a_failure() {
    let gave_up = key(1);
    let status = AgentStatusRecord {
        status: AgentStatus::Failed,
        last_error_kind: Some(golem_common::model::oplog::OplogErrorKind::Invocation),
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(gave_up, result(3, InvocationEnd::Retrying))]),
            0
        ),
        vec![(gave_up, InvocationState::Failed, index(3))]
    );
}

#[test]
fn a_rejection_of_the_running_invocation_ends_it() {
    let rejected = key(1);
    let status = AgentStatusRecord {
        status: AgentStatus::Idle,
        running: Some(running(rejected, 2, 2)),
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(rejected, result(4, InvocationEnd::Failed))]),
            0
        ),
        vec![(rejected, InvocationState::Failed, index(4))]
    );
}

#[test]
fn a_queued_duplicate_of_a_finished_invocation_is_listed_as_its_result() {
    let finished = key(1);
    let status = AgentStatusRecord {
        pending_invocations: vec![pending(finished, 5, None)],
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(
            &status,
            HashMap::from([(finished, result(3, InvocationEnd::Completed))]),
            0
        ),
        vec![(finished, InvocationState::Completed, index(3))]
    );
}

/// A background failure lists as `background-failed` at its give-up entry, and that change
/// outranks the plain result that an older source still holds.
#[test]
fn a_background_failure_lists_at_its_give_up_and_outranks_the_older_result() {
    let failed = key(1);
    let background_failed = InvocationResultPosition {
        changed_at: Timestamp::from(70),
        ..result(3, InvocationEnd::BackgroundFailed { failure: index(7) })
    };
    assert_eq!(
        states(
            &AgentStatusRecord::default(),
            HashMap::from([(failed, background_failed.clone())]),
            0
        ),
        vec![(failed, InvocationState::BackgroundFailed, index(7))]
    );

    let mut results = HashMap::from([(failed, result(3, InvocationEnd::Completed))]);
    super::keep_latest(&mut results, failed, background_failed.clone());
    assert_eq!(results.get(&failed), Some(&background_failed));
    super::keep_latest(&mut results, failed, result(3, InvocationEnd::Completed));
    assert_eq!(results.get(&failed), Some(&background_failed));
}

#[test]
fn a_scheduled_invocation_becomes_pending_at_its_deadline_without_moving() {
    let scheduled = key(1);
    let status = AgentStatusRecord {
        pending_invocations: vec![pending(scheduled, 2, Some(1_000))],
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        states(&status, HashMap::new(), 999),
        vec![(scheduled, InvocationState::Scheduled, index(2))]
    );
    assert_eq!(
        states(&status, HashMap::new(), 1_000),
        vec![(scheduled, InvocationState::Pending, index(2))]
    );
}

#[test]
fn invocations_are_ordered_by_their_last_change_then_by_id() {
    let (low, high, waiting, cancelled) = (key(1), key(2), key(3), key(4));
    let status = AgentStatusRecord {
        status: AgentStatus::Suspended,
        // Started before `cancelled` was withdrawn, but suspended after it.
        running: Some(running(waiting, 2, 9)),
        pending_invocations: vec![pending(low, 7, None)],
        ..AgentStatusRecord::default()
    };
    let results = HashMap::from([
        (high, result(7, InvocationEnd::Completed)),
        (cancelled, result(8, InvocationEnd::Cancelled)),
    ]);

    // `low` and `high` share index 7 only in this synthetic status; the larger ID comes first.
    assert_eq!(
        states(&status, results, 0),
        vec![
            (waiting, InvocationState::Waiting, index(9)),
            (cancelled, InvocationState::Cancelled, index(8)),
            (high, InvocationState::Completed, index(7)),
            (low, InvocationState::Pending, index(7)),
        ]
    );
}

/// A cancellation is a result too, so the withdrawal of a later queued duplicate keeps it.
#[test]
fn a_withdrawn_duplicate_of_an_older_cancellation_keeps_that_cancellation() {
    let cancelled = key(1);
    let mut results = HashMap::new();
    let original = result(3, InvocationEnd::Cancelled);
    super::keep_latest(&mut results, cancelled, original.clone());
    let duplicate_withdrawal = InvocationResultPosition {
        terminal: index(9),
        request: Some(index(8)),
        end: InvocationEnd::Cancelled,
        operation: method("m"),
        accepted_at: Some(Timestamp::from(80)),
        changed_at: Timestamp::from(90),
    };
    super::keep_latest(&mut results, cancelled, duplicate_withdrawal);

    assert_eq!(results.get(&cancelled), Some(&original));
}

fn listed(
    key_value: u128,
    last_change: u64,
    state: InvocationState,
    operation: Option<InvocationOperation>,
) -> InvocationListEntry {
    InvocationListEntry {
        key: key(key_value),
        operation,
        state,
        accepted_at: Some(Timestamp::from(1)),
        changed_at: Timestamp::from(last_change * 10),
        last_change: index(last_change),
    }
}

fn keys(entries: &[InvocationListEntry]) -> Vec<u128> {
    entries.iter().map(|entry| entry.key.0.as_u128()).collect()
}

/// Pages through `entries` with `page_size`, re-reading `entries_at(page)` before each page as a
/// caller does between requests.
fn all_pages(
    entries_at: impl Fn(usize) -> Vec<InvocationListEntry>,
    filter: &InvocationFilter,
    page_size: usize,
) -> Vec<Vec<u128>> {
    let mut pages = Vec::new();
    let mut after = None;
    loop {
        let (page, next) = page_invocations(entries_at(pages.len()), filter, after, page_size);
        pages.push(keys(&page));
        match next {
            Some(next) => after = Some(next),
            None => return pages,
        }
    }
}

#[test]
fn pages_of_two_over_five_invocations_list_each_once_in_order() {
    // The list order: the latest change first, the ID breaking ties.
    let entries = vec![
        listed(5, 9, InvocationState::Completed, method("a")),
        listed(4, 7, InvocationState::Completed, method("a")),
        listed(3, 7, InvocationState::Completed, method("a")),
        listed(2, 4, InvocationState::Completed, method("a")),
        listed(1, 2, InvocationState::Completed, method("a")),
    ];

    assert_eq!(
        all_pages(|_| entries.clone(), &InvocationFilter::default(), 2),
        vec![vec![5, 4], vec![3, 2], vec![1]]
    );
}

#[test]
fn an_invocation_that_changes_between_pages_is_listed_at_most_once() {
    let before = vec![
        listed(5, 9, InvocationState::Completed, method("a")),
        listed(4, 7, InvocationState::Pending, method("a")),
        listed(3, 6, InvocationState::Pending, method("a")),
        listed(2, 4, InvocationState::Completed, method("a")),
        listed(1, 2, InvocationState::Completed, method("a")),
    ];
    // After the first page, 3 starts running and 1 completes again: both move before the cursor.
    let after = vec![
        listed(1, 12, InvocationState::Completed, method("a")),
        listed(3, 11, InvocationState::Running, method("a")),
        listed(5, 9, InvocationState::Completed, method("a")),
        listed(4, 7, InvocationState::Pending, method("a")),
        listed(2, 4, InvocationState::Completed, method("a")),
    ];

    let pages = all_pages(
        |page| {
            if page == 0 {
                before.clone()
            } else {
                after.clone()
            }
        },
        &InvocationFilter::default(),
        2,
    );
    assert_eq!(pages, vec![vec![5, 4], vec![2]]);
}

#[test]
fn a_full_last_page_still_says_whether_more_follow() {
    let entries = vec![
        listed(2, 4, InvocationState::Completed, method("a")),
        listed(1, 2, InvocationState::Completed, method("a")),
    ];

    let (page, next) = page_invocations(entries, &InvocationFilter::default(), None, 2);
    assert_eq!(keys(&page), vec![2, 1]);
    assert_eq!(next, None);
}

#[test]
fn every_filter_and_their_intersection() {
    let tool = Some(InvocationOperation::tool(&"t", &["run".to_string()]));
    let entries = vec![
        listed(6, 60, InvocationState::Failed, method("b")),
        listed(5, 50, InvocationState::Completed, tool.clone()),
        listed(4, 40, InvocationState::Cancelled, method("a")),
        listed(3, 30, InvocationState::Completed, method("a")),
        listed(
            2,
            20,
            InvocationState::Completed,
            Some(InvocationOperation::Constructor),
        ),
        listed(1, 10, InvocationState::Pending, None),
    ];
    let page =
        |filter: InvocationFilter| keys(&page_invocations(entries.clone(), &filter, None, 100).0);

    assert_eq!(
        page(InvocationFilter {
            operation: Some("a".to_string()),
            ..Default::default()
        }),
        vec![4, 3]
    );
    assert_eq!(
        page(InvocationFilter {
            operation: Some("tool:t/run".to_string()),
            ..Default::default()
        }),
        vec![5]
    );
    assert_eq!(
        page(InvocationFilter {
            states: vec![InvocationState::Completed, InvocationState::Failed],
            ..Default::default()
        }),
        vec![6, 5, 3, 2]
    );
    assert_eq!(
        page(InvocationFilter {
            kind: Some(InvocationKind::Constructor),
            ..Default::default()
        }),
        vec![2]
    );
    assert_eq!(
        page(InvocationFilter {
            kind: Some(InvocationKind::Tool),
            ..Default::default()
        }),
        vec![5]
    );
    // `since` and `until` are inclusive bounds on the change time.
    assert_eq!(
        page(InvocationFilter {
            since: Some(Timestamp::from(300)),
            until: Some(Timestamp::from(500)),
            ..Default::default()
        }),
        vec![5, 4, 3]
    );
    assert_eq!(
        page(InvocationFilter {
            operation: Some("a".to_string()),
            states: vec![InvocationState::Completed],
            kind: Some(InvocationKind::Method),
            since: Some(Timestamp::from(300)),
            until: None,
        }),
        vec![3]
    );
}

#[test]
fn a_filtered_page_continues_after_the_last_match() {
    let entries = vec![
        listed(5, 9, InvocationState::Completed, method("a")),
        listed(4, 7, InvocationState::Completed, method("b")),
        listed(3, 6, InvocationState::Completed, method("a")),
        listed(2, 4, InvocationState::Completed, method("b")),
        listed(1, 2, InvocationState::Completed, method("a")),
    ];
    let filter = InvocationFilter {
        operation: Some("a".to_string()),
        ..Default::default()
    };

    assert_eq!(
        all_pages(|_| entries.clone(), &filter, 1),
        vec![vec![5], vec![3], vec![1]]
    );
}

#[test]
fn a_page_size_of_zero_gives_the_default_page() {
    let entries: Vec<InvocationListEntry> = (1..=150)
        .rev()
        .map(|value| listed(value, value as u64, InvocationState::Completed, method("a")))
        .collect();

    let (page, next) = page_invocations(entries, &InvocationFilter::default(), None, 0);
    assert_eq!(page.len(), 100);
    assert!(next.is_some());
}
