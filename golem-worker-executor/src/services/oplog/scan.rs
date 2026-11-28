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

pub(super) const SCAN_CURSOR_PREFIX: &str = "gsc1_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OplogScanState {
    pub(crate) layer: usize,
    pub(crate) mode: AgentMode,
    pub(crate) resume: Option<ScanResume>,
}

pub(crate) fn decode_scan_cursor(
    cursor: &ScanCursor,
    modes: Option<AgentMode>,
) -> Result<OplogScanState, WorkerExecutorError> {
    if cursor.is_finished() {
        return Ok(OplogScanState {
            layer: 0,
            mode: modes.unwrap_or(AgentMode::Durable),
            resume: None,
        });
    }

    let encoded = cursor
        .as_str()
        .strip_prefix(SCAN_CURSOR_PREFIX)
        .ok_or_else(|| WorkerExecutorError::invalid_request("Invalid agent scan cursor version"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| WorkerExecutorError::invalid_request("Invalid agent scan cursor encoding"))?;
    let state: OplogScanState = serde_json::from_slice(&bytes)
        .map_err(|_| WorkerExecutorError::invalid_request("Invalid agent scan cursor payload"))?;

    if let Some(mode) = modes
        && state.mode != mode
    {
        return Err(WorkerExecutorError::invalid_request(
            "Agent scan cursor does not match the requested agent mode",
        ));
    }

    Ok(state)
}

pub(super) fn encode_scan_cursor(state: OplogScanState) -> Result<ScanCursor, WorkerExecutorError> {
    let bytes = serde_json::to_vec(&state).map_err(|error| {
        WorkerExecutorError::unknown(format!("Failed to encode agent scan cursor: {error}"))
    })?;
    Ok(ScanCursor::new(format!(
        "{SCAN_CURSOR_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )))
}

pub(crate) fn first_scan_cursor(
    layer: usize,
    modes: Option<AgentMode>,
) -> Result<ScanCursor, WorkerExecutorError> {
    encode_scan_cursor(OplogScanState {
        layer,
        mode: modes.unwrap_or(AgentMode::Durable),
        resume: None,
    })
}

pub(crate) fn next_scan_cursor(
    state: OplogScanState,
    modes: Option<AgentMode>,
    resume: Option<ScanResume>,
) -> Result<ScanCursor, WorkerExecutorError> {
    match resume {
        Some(resume) => encode_scan_cursor(OplogScanState {
            resume: Some(resume),
            ..state
        }),
        None if modes.is_none() && state.mode == AgentMode::Durable => {
            encode_scan_cursor(OplogScanState {
                mode: AgentMode::Ephemeral,
                resume: None,
                ..state
            })
        }
        None => Ok(ScanCursor::default()),
    }
}

/// Runs a scan of the stored oplog keys under the retry policy. A refused resume point is an
/// invalid request. A storage that does not answer after the retries gives an unavailable oplog
/// read, which a later scan can pass. Any other storage failure gives a runtime failure. The scan
/// does not panic.
pub(crate) async fn retry_scan_storage_op<T, F, Fut>(
    retry_config: &golem_common::model::RetryConfig,
    op_name: &str,
    target: &str,
    mut op: F,
) -> Result<T, WorkerExecutorError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(value) => return Ok(value),
            Err(IndexedStorageError::InvalidResume(message)) => {
                return Err(WorkerExecutorError::invalid_request(message));
            }
            Err(IndexedStorageError::Transient(message)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    crate::metrics::oplog::record_oplog_storage_retry(op_name);
                    tracing::warn!(
                        op = op_name,
                        key = target,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {message}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    return Err(WorkerExecutorError::oplog_read_unavailable(format!(
                        "Indexed storage operation '{op_name}' failed for key '{target}' after {attempts} attempts: Transient storage error: {message}"
                    )));
                }
            }
            Err(error) => {
                return Err(WorkerExecutorError::runtime(format!(
                    "Indexed storage operation '{op_name}' failed for key '{target}': {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::RetryConfig;
    use golem_service_base::error::worker_executor::RecoveryCause;
    use std::sync::atomic::{AtomicU32, Ordering};
    use test_r::test;

    fn retry_twice() -> RetryConfig {
        RetryConfig {
            max_attempts: 2,
            min_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            max_jitter_factor: None,
        }
    }

    /// A scan whose storage does not answer after the retry budget returns an unavailable read,
    /// which a later scan can pass. The budget stays the same.
    #[test]
    async fn an_exhausted_transient_scan_returns_an_unavailable_read() {
        let attempts = AtomicU32::new(0);

        let result: Result<(), _> =
            retry_scan_storage_op(&retry_twice(), "scan", "prefix-7", || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(IndexedStorageError::Transient(
                        "injected outage".to_string(),
                    ))
                }
            })
            .await;

        let error = result.expect_err("an exhausted scan is not a page");
        let WorkerExecutorError::RecoveryRequired { cause, details, .. } = &error else {
            panic!("expected an unavailable read, got {error:?}");
        };
        assert!(
            matches!(cause, RecoveryCause::OplogReadUnavailable { .. }),
            "{cause:?}"
        );
        assert!(details.contains("prefix-7"), "{details}");
        assert!(details.contains("injected outage"), "{details}");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// A scan that fails for a cause the retries cannot remove returns the failure at once.
    #[test]
    async fn a_failed_scan_returns_a_runtime_failure() {
        let attempts = AtomicU32::new(0);

        let result: Result<(), _> =
            retry_scan_storage_op(&retry_twice(), "scan", "prefix-9", || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Err(IndexedStorageError::Other("injected failure".to_string())) }
            })
            .await;

        let error = result.expect_err("a failed scan is not a page");
        let WorkerExecutorError::Runtime { details } = &error else {
            panic!("expected a runtime failure, got {error:?}");
        };
        assert!(details.contains("prefix-9"), "{details}");
        assert!(details.contains("injected failure"), "{details}");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
