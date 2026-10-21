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

/// Number of oplog entries read per backward-scan window. Sized to match the compressed oplog
/// archive's chunk and cache sizes so that each window generally costs a single chunk decompression.
const BACKWARD_OPLOG_SCAN_WINDOW: u64 = 4096;

/// Returns the start index of the inclusive backward-scan window ending at `end`, clamped to
/// [`OplogIndex::INITIAL`].
fn backward_scan_window_start(end: OplogIndex) -> OplogIndex {
    let initial = OplogIndex::INITIAL.as_u64();
    let start = end
        .as_u64()
        .saturating_sub(BACKWARD_OPLOG_SCAN_WINDOW - 1)
        .max(initial);
    OplogIndex::from_u64(start)
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

    let mut first_error = None;
    let mut first_retry_from = OplogIndex::NONE;
    let mut last_error_index = last_index;

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time (the latter thrashes the compressed-archive chunk cache).
    let mut window_end = last_index;
    'scan: loop {
        let window_start = backward_scan_window_start(window_end);
        let entries = match this
            .oplog_service()
            .read_exact(
                owned_agent_id,
                agent_mode,
                window_start,
                window_end.as_u64() - window_start.as_u64() + 1,
            )
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

            if idx == OplogIndex::INITIAL {
                break 'scan;
            }
            if idx == window_start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window_start.previous();
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

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time.
    let mut window_end = last_oplog_idx;
    'scan: loop {
        let window_start = backward_scan_window_start(window_end);
        // The stderr is the context of an error, so a window that does not read ends it.
        let Ok(entries) = this
            .oplog_service()
            .read_exact(
                owned_agent_id,
                agent_mode,
                window_start,
                window_end.as_u64() - window_start.as_u64() + 1,
            )
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
                    idempotency_key, ..
                }) => match &first_seen_invocation {
                    None => {
                        first_seen_invocation = Some(idempotency_key.clone());
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

            if idx == OplogIndex::INITIAL {
                break 'scan;
            }
            if idx == window_start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window_start.previous();
    }
    stderr_entries.reverse();
    stderr_entries.join("")
}
