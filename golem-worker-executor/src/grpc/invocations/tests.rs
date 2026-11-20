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

use super::{bound_from_proto, decode_cursor, encode_cursor};
use crate::worker::invocations::InvocationPagePosition;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId, InvocationId, OplogIndex, OwnedAgentId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use test_r::test;
use uuid::Uuid;

fn agent(h: u128) -> OwnedAgentId {
    OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, h))
}

fn position() -> InvocationPagePosition {
    InvocationPagePosition {
        last_change: OplogIndex::from_u64(42),
        key: InvocationId(Uuid::new_v4()),
    }
}

fn is_invalid_request(result: Result<InvocationPagePosition, WorkerExecutorError>) -> bool {
    matches!(result, Err(WorkerExecutorError::InvalidRequest { .. }))
}

#[test]
fn a_cursor_round_trips_for_its_own_agent() {
    let owner = agent(1);
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    let position = position();

    let cursor = encode_cursor(&owner, fingerprint, position).unwrap();
    assert_eq!(
        decode_cursor(&cursor, &owner, fingerprint).unwrap(),
        position
    );
}

#[test]
fn a_cursor_of_another_agent_or_incarnation_is_refused() {
    let owner = agent(1);
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    let cursor = encode_cursor(&owner, fingerprint, position()).unwrap();

    assert!(is_invalid_request(decode_cursor(
        &cursor,
        &agent(2),
        fingerprint
    )));
    // A deleted and re-created agent has a new fingerprint.
    assert!(is_invalid_request(decode_cursor(
        &cursor,
        &owner,
        AgentFingerprint(Uuid::new_v4())
    )));
}

#[test]
fn a_malformed_cursor_is_an_invalid_request_and_never_panics() {
    let owner = agent(1);
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    // Not base64, empty, an unknown serialization version, and truncated data.
    for value in ["%%%", "", "_w", "Aw"] {
        assert!(
            is_invalid_request(decode_cursor(value, &owner, fingerprint)),
            "{value:?}"
        );
    }
}

#[test]
fn a_bound_outside_the_representable_range_is_an_invalid_request() {
    let bound = |seconds, nanos| {
        bound_from_proto("until", prost_types::Timestamp { seconds, nanos }, false)
    };

    assert_eq!(
        bound(1_700_000_000, 123_456_789).unwrap(),
        golem_common::model::Timestamp::from(1_700_000_000_123)
    );
    for (seconds, nanos) in [
        (-1, 0),
        (0, -1),
        (0, 1_000_000_000),
        (i64::MAX, 0),
        (253_402_300_800, 0),
    ] {
        assert!(
            matches!(
                bound(seconds, nanos),
                Err(WorkerExecutorError::InvalidRequest { .. })
            ),
            "{seconds}s {nanos}ns"
        );
    }
}

#[test]
fn a_sub_millisecond_bound_never_widens_the_inclusive_range() {
    let bound = |nanos, lower| {
        bound_from_proto(
            "bound",
            prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos,
            },
            lower,
        )
        .unwrap()
    };
    let millis = golem_common::model::Timestamp::from;

    assert_eq!(bound(500_000, true), millis(1_700_000_000_001));
    assert_eq!(bound(500_000, false), millis(1_700_000_000_000));
    assert_eq!(bound(2_000_000, true), millis(1_700_000_000_002));
    assert_eq!(bound(2_000_000, false), millis(1_700_000_000_002));
}
