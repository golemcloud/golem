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

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, EphemeralSleepTooLongError};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};

use crate::metrics::ephemeral::{dec_promise_waiting, inc_promise_waiting};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkOutcome {
    Ready,
    Interrupted(InterruptKind),
    EphemeralTooLong {
        requested_nanos: u64,
        max_nanos: u64,
    },
}

/// Unclassified waits only observe readiness and interruption. Automatic suspension is
/// elected by the owner from runtime observations, never by a borrowed host wait.
pub(crate) async fn wait_for_ready<R: Future<Output = ()>>(
    mode: AgentMode,
    max_sleep: Duration,
    remaining: Option<Duration>,
    mut interrupt: Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
    ready: R,
) -> ParkOutcome {
    if mode == AgentMode::Durable {
        return tokio::select! {
            _ = ready => ParkOutcome::Ready,
            kind = &mut interrupt => ParkOutcome::Interrupted(kind),
        };
    }
    let max_nanos = std_duration_to_nanos(max_sleep);
    if let Some(remaining) = remaining {
        let requested_nanos = std_duration_to_nanos(remaining);
        if remaining >= max_sleep {
            return ParkOutcome::EphemeralTooLong {
                requested_nanos,
                max_nanos,
            };
        }
        return tokio::select! {
            _ = ready => ParkOutcome::Ready,
            kind = &mut interrupt => ParkOutcome::Interrupted(kind),
        };
    }
    let _promise_waiting = PromiseWaiting::new();
    tokio::select! {
        _ = ready => ParkOutcome::Ready,
        kind = &mut interrupt => ParkOutcome::Interrupted(kind),
        _ = tokio::time::sleep(max_sleep) => ParkOutcome::EphemeralTooLong {
            requested_nanos: max_nanos,
            max_nanos,
        },
    }
}

struct PromiseWaiting;

impl PromiseWaiting {
    fn new() -> Self {
        inc_promise_waiting();
        Self
    }
}

impl Drop for PromiseWaiting {
    fn drop(&mut self) {
        dec_promise_waiting();
    }
}

pub(crate) fn ephemeral_sleep_too_long_error(
    requested_nanos: u64,
    max_nanos: u64,
) -> wasmtime::Error {
    wasmtime::Error::from_anyhow(anyhow::anyhow!(WorkerExecutorError::InvocationFailed {
        error: AgentError::EphemeralSleepTooLong(EphemeralSleepTooLongError {
            requested_nanos,
            max_nanos,
        }),
        stderr: String::new(),
    }))
}

pub(crate) fn std_duration_to_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

pub(crate) fn chrono_duration_to_nanos(duration: chrono::Duration) -> u64 {
    duration
        .to_std()
        .map(std_duration_to_nanos)
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};
    use test_r::test;

    #[test]
    async fn suppressed_durable_wait_has_no_ephemeral_timeout_or_local_election() {
        let result = wait_for_ready(
            AgentMode::Durable,
            Duration::ZERO,
            None,
            Box::pin(pending()),
            async { tokio::time::sleep(Duration::from_millis(2)).await },
        )
        .await;
        assert_eq!(result, ParkOutcome::Ready);
        let result = wait_for_ready(
            AgentMode::Durable,
            Duration::ZERO,
            Some(Duration::MAX),
            Box::pin(ready(InterruptKind::Restart)),
            pending(),
        )
        .await;
        assert_eq!(result, ParkOutcome::Interrupted(InterruptKind::Restart));
    }

    #[test]
    async fn ephemeral_clocks_enforce_exact_boundary() {
        let max = Duration::from_millis(10);
        for remaining in [max, max + Duration::from_nanos(1)] {
            assert_eq!(
                wait_for_ready(
                    AgentMode::Ephemeral,
                    max,
                    Some(remaining),
                    Box::pin(pending()),
                    ready(()),
                )
                .await,
                ParkOutcome::EphemeralTooLong {
                    requested_nanos: std_duration_to_nanos(remaining),
                    max_nanos: std_duration_to_nanos(max),
                }
            );
        }
        assert_eq!(
            wait_for_ready(
                AgentMode::Ephemeral,
                max,
                Some(max / 2),
                Box::pin(pending()),
                tokio::time::sleep(max / 2),
            )
            .await,
            ParkOutcome::Ready
        );
    }

    #[test]
    async fn ephemeral_promises_complete_timeout_and_interrupt() {
        fn gauge() -> f64 {
            prometheus::gather()
                .into_iter()
                .find(|metric| metric.name() == "ephemeral_promise_waiting")
                .unwrap()
                .get_metric()[0]
                .get_gauge()
                .value()
        }
        drop(PromiseWaiting::new());
        let baseline = gauge();
        let max = Duration::from_millis(2);
        assert_eq!(
            wait_for_ready(
                AgentMode::Ephemeral,
                max,
                None,
                Box::pin(pending()),
                ready(()),
            )
            .await,
            ParkOutcome::Ready
        );
        assert_eq!(gauge(), baseline);
        assert_eq!(
            wait_for_ready(
                AgentMode::Ephemeral,
                max,
                None,
                Box::pin(ready(InterruptKind::Restart)),
                pending(),
            )
            .await,
            ParkOutcome::Interrupted(InterruptKind::Restart)
        );
        assert_eq!(gauge(), baseline);
        assert_eq!(
            wait_for_ready(
                AgentMode::Ephemeral,
                max,
                None,
                Box::pin(pending()),
                pending(),
            )
            .await,
            ParkOutcome::EphemeralTooLong {
                requested_nanos: std_duration_to_nanos(max),
                max_nanos: std_duration_to_nanos(max),
            }
        );
        assert_eq!(gauge(), baseline);
        let mut wait = Box::pin(wait_for_ready(
            AgentMode::Ephemeral,
            Duration::from_secs(60),
            None,
            Box::pin(pending()),
            pending(),
        ));
        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        assert_eq!(gauge(), baseline + 1.0);
        drop(wait);
        assert_eq!(gauge(), baseline);
    }
}
