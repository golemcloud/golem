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
mod tests;
