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

use super::{InvocationState, merge_invocations};
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
    merge_invocations(status, results, Timestamp::from(now))
        .into_iter()
        .map(|(entry, _)| (entry.key, entry.state, entry.last_change))
        .collect()
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
