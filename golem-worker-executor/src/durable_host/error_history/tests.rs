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
use test_r::test;

use crate::services::golem_config::GolemConfig;
use crate::services::oplog::retention::test_oplog::{RetainedOplog, at, idx, jump};
use crate::storage::keyvalue::KeyValueStorageError;
use crate::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_common::model::caller::Caller;
use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::payload::OplogPayload;
use golem_common::model::retry_policy::RetryPolicyState;
use golem_common::model::{InvocationRequestOperation, StoredAgentInvocationPayload, Timestamp};

/// The services of the error history walks over an oplog behind the retention layer.
struct Retained {
    oplog: RetainedOplog,
    config: Arc<GolemConfig>,
}

impl Retained {
    async fn new(entries: Vec<OplogEntry>) -> Self {
        Self::with_oplog(RetainedOplog::new().await, entries).await
    }

    /// The services over a retention record whose reads fail as `faults` decides.
    async fn with_faults(entries: Vec<OplogEntry>) -> (Self, KeyValueStorageFaults) {
        let faults = KeyValueStorageFaults::default();
        let oplog =
            RetainedOplog::with_key_value_storage(Arc::new(FaultInjectingKeyValueStorage::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                faults.clone(),
            )))
            .await;
        (Self::with_oplog(oplog, entries).await, faults)
    }

    async fn with_oplog(oplog: RetainedOplog, entries: Vec<OplogEntry>) -> Self {
        oplog.add(entries).await;
        Self {
            oplog,
            config: Arc::new(GolemConfig::default()),
        }
    }
}

fn unavailable() -> KeyValueStorageError {
    KeyValueStorageError::Other("the retention record does not read".to_string())
}

fn error(index: u64, retry_from: u64) -> OplogEntry {
    OplogEntry::Error {
        timestamp: Timestamp::from(index),
        entity_parent_start_index: None,
        kind: OplogErrorKind::Invocation,
        invocation_id: None,
        error: AgentError::Unknown("failed".to_string()),
        retry_from: idx(retry_from),
        retry_policy_state: None,
        retry_at: None,
        retrying_start_index: None,
    }
}

impl HasOplogService for Retained {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog.service.clone()
    }
}

impl HasConfig for Retained {
    fn config(&self) -> Arc<GolemConfig> {
        self.config.clone()
    }
}

fn stderr(message: &str) -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        level: LogLevel::Stderr,
        context: String::new(),
        message: message.to_string(),
        trace_context: None,
    }
}

fn started() -> OplogEntry {
    OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::now_utc(),
        invocation_id: uuid::Uuid::from_u128(1).into(),
        operation: Box::new(InvocationRequestOperation::SaveSnapshot),
        payload: OplogPayload::Inline(Box::new(StoredAgentInvocationPayload::SaveSnapshot {
            caller: Caller::anonymous(),
        })),
        trace_id: TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(InvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            pinned_card_ids: Vec::new(),
            scope_card_id: None,
        }),
    }
}

/// The last error walks back to the retained start and reads no entry below it. When a
/// failed agent has no error and no invocation boundary at or after the start, its error is
/// below the start, and the last error is the expired error with the start. When the start is
/// at index 1, the walk reads the whole oplog and finds the error.
#[test]
async fn the_last_error_walk_stops_at_the_retained_start() {
    let retained = Retained::new(vec![
        jump(1),
        OplogEntry::Error {
            timestamp: Timestamp::from(2),
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
            invocation_id: None,
            error: AgentError::Unknown("below".to_string()),
            retry_from: idx(1),
            retry_policy_state: None,
            retry_at: None,
            retrying_start_index: None,
        },
        jump(3),
        jump(4),
        jump(5),
    ])
    .await;
    let status = AgentStatusRecord {
        status: AgentStatus::Failed,
        ..Default::default()
    };
    let last = || {
        last_error(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            &status,
        )
    };
    assert_eq!(
        last().await.map(|last| last.error),
        Some(AgentError::Unknown("below".to_string()))
    );
    retained.oplog.advance(3).await;
    let expired = last().await.expect("a failed agent has a last error");
    assert!(
        matches!(
            &expired.error,
            AgentError::InternalError(text)
                if text.ends_with("expired: it starts at index 3")
        ),
        "{:?}",
        expired.error
    );
    assert_eq!(expired.retry_from, OplogIndex::NONE);
    assert_eq!(expired.stderr, "");
}

/// A walk that ends at an invocation boundary at or after the start gives no last error, also
/// when the start is above 1: the error of the agent is not in the current invocation.
#[test]
async fn a_boundary_after_the_retained_start_gives_no_last_error() {
    let retained = Retained::new(vec![jump(1), error(2, 1), jump(3), started(), jump(5)]).await;
    retained.oplog.advance(3).await;
    let failed = AgentStatusRecord {
        status: AgentStatus::Failed,
        ..Default::default()
    };
    assert_eq!(
        last_error(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            &failed,
        )
        .await
        .map(|last| last.error),
        None
    );
}

/// An agent that is not failed or retrying, but has a retry state, gives no expired error
/// when the walk reaches the start: it has no error to report.
#[test]
async fn a_running_agent_with_a_retry_state_gives_no_expired_error() {
    let retained = Retained::new(vec![jump(1), error(2, 1), jump(3), jump(4)]).await;
    retained.oplog.advance(3).await;
    let running = AgentStatusRecord {
        status: AgentStatus::Running,
        current_retry_state: [(idx(2), RetryPolicyState::Counter(1))]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    assert_eq!(
        last_error(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            &running,
        )
        .await
        .map(|last| last.error),
        None
    );
}

/// The stderr walk collects the logs of the last invocation back to the retained start. The
/// logs below the start are not read.
#[test]
async fn the_stderr_walk_stops_at_the_retained_start() {
    let retained = Retained::new(vec![
        jump(1),
        stderr("old"),
        started(),
        stderr("new"),
        started(),
        stderr("tail"),
    ])
    .await;
    let logs = || {
        recover_stderr_logs(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            idx(6),
        )
    };
    assert_eq!(logs().await, "newtail");
    retained.oplog.advance(4).await;
    assert_eq!(logs().await, "tail");
}

/// When the retention of the oplog does not read, the last error is the read failure: the walk
/// does not guess a start, because the oplog can drop index 1.
#[test]
async fn a_retention_that_does_not_read_is_the_last_error() {
    let (retained, faults) = Retained::with_faults(vec![jump(1), error(2, 1)]).await;
    let status = AgentStatusRecord {
        status: AgentStatus::Failed,
        ..Default::default()
    };
    faults.fail("read_retention", 1, unavailable());

    let last = last_error(
        &retained,
        &retained.oplog.owned_agent_id,
        AgentMode::Durable,
        &status,
    )
    .await
    .expect("a failed agent has a last error");
    assert!(
        matches!(&last.error, AgentError::InternalError(text) if text.contains("the retention record does not read")),
        "{:?}",
        last.error
    );
    assert_eq!(last.retry_from, OplogIndex::NONE);
}

/// When the retention of the oplog does not read, the stderr walk reads no entry and gives no
/// stderr.
#[test]
async fn a_retention_that_does_not_read_gives_no_stderr() {
    let (retained, faults) = Retained::with_faults(vec![jump(1), started(), stderr("tail")]).await;
    faults.fail("read_retention", 1, unavailable());

    assert_eq!(
        recover_stderr_logs(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            idx(3),
        )
        .await,
        ""
    );
}

/// The stderr walk keeps the stderr of the windows that it read when a later window does not
/// read.
#[test]
async fn a_window_that_does_not_read_keeps_the_stderr_of_the_later_windows() {
    let last = BACKWARD_OPLOG_SCAN_WINDOW + 2;
    let (retained, faults) = Retained::with_faults(
        (1..=last - 2)
            .map(jump)
            .chain([started(), stderr("tail")])
            .collect(),
    )
    .await;
    // The walk reads the record for its retention and once for each window read, so the
    // third read is the read of the second window.
    faults.fail_after("read_retention", 2, 1, unavailable());
    let reads_before = faults.calls("read_retention");

    assert_eq!(
        recover_stderr_logs(
            &retained,
            &retained.oplog.owned_agent_id,
            AgentMode::Durable,
            idx(last),
        )
        .await,
        "tail"
    );
    assert_eq!(faults.calls("read_retention") - reads_before, 3);
}

fn window(start: u64, count: u64) -> Option<OplogRange> {
    Some(OplogRange {
        start: idx(start),
        count,
    })
}

#[test]
fn a_backward_scan_window_ends_at_the_retained_start() {
    assert_eq!(
        backward_scan_window(OplogRetention::FULL, idx(10_000)),
        window(
            10_000 - BACKWARD_OPLOG_SCAN_WINDOW + 1,
            BACKWARD_OPLOG_SCAN_WINDOW
        )
    );
    assert_eq!(
        backward_scan_window(OplogRetention::FULL, idx(BACKWARD_OPLOG_SCAN_WINDOW)),
        window(1, BACKWARD_OPLOG_SCAN_WINDOW)
    );
    assert_eq!(
        backward_scan_window(OplogRetention::FULL, idx(10)),
        window(1, 10)
    );
    assert_eq!(backward_scan_window(at(4), idx(10)), window(4, 7));
    assert_eq!(backward_scan_window(at(10), idx(10)), window(10, 1));
    assert_eq!(backward_scan_window(at(11), idx(10)), None);
    assert_eq!(backward_scan_window(at(4), idx(3)), None);
    assert_eq!(
        backward_scan_window(OplogRetention::FULL, OplogIndex::NONE),
        None
    );
}
