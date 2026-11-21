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
use crate::services::oplog::retention::{OplogRange, OplogRetention};

/// Number of oplog entries read per backward-scan window. Sized to match the compressed oplog
/// archive's chunk and cache sizes so that each window generally costs a single chunk decompression.
const BACKWARD_OPLOG_SCAN_WINDOW: u64 = 4096;

/// The inclusive backward-scan window that ends at `end`: at most [`BACKWARD_OPLOG_SCAN_WINDOW`]
/// retained entries. `None` when `end` is below the retained start, which ends the scan.
fn backward_scan_window(retention: OplogRetention, end: OplogIndex) -> Option<OplogRange> {
    retention.range(
        OplogIndex::from_u64(end.as_u64().saturating_sub(BACKWARD_OPLOG_SCAN_WINDOW - 1)),
        end,
    )
}

/// Finds the most recent error of the current invocation (if any) together with its retry point and
/// the surrounding stderr logs.
///
/// A possible future optimization is to maintain the relevant indices (last error, last invocation
/// start) directly in the [`AgentStatusRecord`] so that no backward oplog scan is needed at all.
pub(crate) async fn last_error<T: HasOplogService + HasConfig>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    latest_worker_status: &AgentStatusRecord,
) -> Option<LastError> {
    // Short-circuit: there is nothing to report unless the worker is currently in an error-bearing
    // state. `last_error` otherwise scans backward to the start of the current invocation, which is
    // unbounded for long-running invocations. A failed/retrying worker always has its error near the
    // tail, and a non-empty `current_retry_state` means an `Error` with a tracked retry policy was
    // recorded since the last invocation boundary; in every other case there is no error to find.
    if !matches!(
        latest_worker_status.status,
        AgentStatus::Failed | AgentStatus::Retrying
    ) && latest_worker_status.current_retry_state.is_empty()
    {
        return None;
    }

    let last_index = match this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await
    {
        Ok(last_index) => last_index,
        Err(error) => return Some(unreadable_last_error(error)),
    };
    if last_index == OplogIndex::NONE {
        return None;
    }
    let retention = match this
        .oplog_service()
        .retention(owned_agent_id, agent_mode)
        .await
    {
        Ok(retention) => retention,
        Err(error) => return Some(unreadable_last_error(error)),
    };

    let mut first_error = None;
    let mut first_retry_from = OplogIndex::NONE;
    let mut last_error_index = last_index;

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time (the latter thrashes the compressed-archive chunk cache).
    let mut window_end = last_index;
    'scan: while let Some(window) = backward_scan_window(retention, window_end) {
        let entries = match this
            .oplog_service()
            .read_exact(owned_agent_id, agent_mode, window.start, window.count)
            .await
        {
            Ok(entries) => entries,
            Err(error) => return Some(unreadable_last_error(error)),
        };

        let mut idx = window_end;
        loop {
            if latest_worker_status
                .deleted_regions
                .is_in_deleted_region(idx)
            {
                // Skip entries in deleted regions without consulting the read range.
            } else {
                match entries.get(&idx) {
                    Some(OplogEntry::RecoverySucceeded { .. }) => {
                        break 'scan;
                    }
                    Some(OplogEntry::AgentInvocationFailed { .. }) => {
                        break 'scan;
                    }
                    Some(OplogEntry::Error {
                        error, retry_from, ..
                    }) => {
                        if first_retry_from == OplogIndex::NONE || first_retry_from == *retry_from {
                            last_error_index = idx;
                            if first_error.is_none() {
                                first_error = Some(error.clone());
                                first_retry_from = *retry_from;
                            }
                        } else {
                            // Found an error entry belonging to another retry point
                            break 'scan;
                        }
                    }
                    Some(entry) if entry.is_hint() => {
                        // Skipping hint entries as they can randomly interleave the error entries (such as incoming invocation requests, etc)
                    }
                    Some(
                        OplogEntry::AgentInvocationStarted { .. }
                        | OplogEntry::AgentInvocationFinished { .. },
                    ) => {
                        // Retry counting never gets across invocation boundaries
                        break 'scan;
                    }
                    Some(_) => {
                        // Skipping non-hint entries as well, but only up to the first error entry that's different, or the beginning
                        // of the last invocation
                    }
                    None => {
                        // This is possible if the oplog has been deleted between the get_last_index and the read call
                        break 'scan;
                    }
                }
            }

            if idx == window.start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window.start.previous();
    }

    match first_error {
        Some(error) => Some(LastError {
            error,
            stderr: recover_stderr_logs(this, owned_agent_id, agent_mode, last_error_index).await,
            retry_from: first_retry_from,
        }),
        None => None,
    }
}

/// The last error of an agent whose oplog does not read back to it: the read failure itself.
fn unreadable_last_error(error: UnreadableOplog) -> LastError {
    LastError {
        error: AgentError::InternalError(WorkerExecutorError::from(error).to_string()),
        stderr: String::new(),
        retry_from: OplogIndex::NONE,
    }
}

/// Reads back oplog entries starting from `last_oplog_idx` and collects stderr logs, with a maximum
/// number of entries, and at most until the beginning of the last invocation.
pub(crate) async fn recover_stderr_logs<T: HasOplogService + HasConfig>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_oplog_idx: OplogIndex,
) -> String {
    let max_count = this.config().limits.event_history_size;

    // This might overestimate the size of stderr_entries by the size of current_stderr_entries_batch, but fine as we
    // have at most one pending batch we discard.
    let mut collected_count = 0;
    let mut stderr_entries = Vec::new();
    let mut current_stderr_entries_batch = Vec::new();
    let mut first_seen_invocation = None;

    let Ok(retention) = this
        .oplog_service()
        .retention(owned_agent_id, agent_mode)
        .await
    else {
        return String::new();
    };

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time.
    let mut window_end = last_oplog_idx;
    'scan: while let Some(window) = backward_scan_window(retention, window_end) {
        // The stderr is the context of an error, so a window that does not read ends it.
        let Ok(entries) = this
            .oplog_service()
            .read_exact(owned_agent_id, agent_mode, window.start, window.count)
            .await
        else {
            break 'scan;
        };

        let mut idx = window_end;
        loop {
            // Because of retries we might have multiple invocation start entries.
            // Read until the first invocation start entry which does not belong to the same invocation (using the trace id)
            match entries.get(&idx) {
                Some(OplogEntry::Log {
                    level,
                    message,
                    context,
                    ..
                }) if (level == &LogLevel::Warn
                    || level == &LogLevel::Error
                    || level == &LogLevel::Critical
                    || level == &LogLevel::Stderr)
                    && collected_count < max_count =>
                {
                    if level == &LogLevel::Stderr {
                        current_stderr_entries_batch.push(message.clone());
                    } else {
                        let line = format!(
                            "[{}] [{}] {}\n",
                            format!("{level:?}").to_uppercase(),
                            context,
                            message
                        );
                        current_stderr_entries_batch.push(line);
                    }
                    collected_count += 1;
                }
                Some(OplogEntry::AgentInvocationStarted {
                    invocation_id: idempotency_key,
                    ..
                }) => match &first_seen_invocation {
                    None => {
                        first_seen_invocation = Some(*idempotency_key);
                        stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                        if stderr_entries.len() >= max_count {
                            break 'scan;
                        };
                    }
                    Some(expected_idempotency_key)
                        if idempotency_key == expected_idempotency_key =>
                    {
                        stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                        if stderr_entries.len() >= max_count {
                            break 'scan;
                        };
                    }
                    Some(_) => break 'scan,
                },
                _ => {}
            }

            if idx == window.start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window.start.previous();
    }
    stderr_entries.reverse();
    stderr_entries.join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::retention::test_oplog::{RetainedOplog, at, idx, jump};
    use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_common::model::{AgentInvocationPayload, Timestamp};

    /// The services of the error history walks over an oplog behind the retention layer.
    struct Retained {
        oplog: RetainedOplog,
        config: Arc<GolemConfig>,
    }

    impl Retained {
        async fn new(entries: Vec<OplogEntry>) -> Self {
            let oplog = RetainedOplog::new().await;
            oplog.add(entries).await;
            Self {
                oplog,
                config: Arc::new(GolemConfig::default()),
            }
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
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
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

    /// The last error walks back to the retained start. An error below the start is not found,
    /// and the walk reads no entry below the start: a read below it would report the expired
    /// error as the last error.
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
        assert_eq!(last().await.map(|last| last.error), None);
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
}
