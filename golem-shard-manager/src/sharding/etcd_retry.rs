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

//! Retries for the etcd requests that can safely be repeated.
//!
//! Reads only: a write here is a compare-and-swap that may already have been applied when its
//! transport failed, so the write path fail-stops instead.
//!
//! Worth having because tonic keeps an unreachable endpoint in the balancer's rotation, so with
//! several endpoints a share of reads fail fast until it is back; a slow answer counts too.

use crate::sharding::error::ShardManagerError;
use golem_common::retriable_error::IsRetriableError;
use std::future::Future;
use std::time::Duration;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;
use tonic::Code;
use tracing::warn;

/// Backoff bounds for a retriable etcd failure, shared by the leadership campaign and the reads.
pub const RETRY_MIN: Duration = Duration::from_millis(100);
pub const RETRY_MAX: Duration = Duration::from_secs(5);

/// Whether a failed read is worth repeating.
pub fn is_retriable_read(err: &ShardManagerError) -> bool {
    err.is_retriable() || is_request_timeout(err)
}

fn is_request_timeout(err: &ShardManagerError) -> bool {
    matches!(
        err,
        ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(status))
            if status.code() == Code::Cancelled
    )
}

/// What ends a retry loop that has nothing else to stop it.
#[derive(Clone, Copy)]
enum GiveUp<'a> {
    /// Report [`ShardManagerError::ShutdownRequested`] once the token is cancelled.
    WhenCancelled(&'a CancellationToken),
    /// Report the last failure once the instant has passed.
    At(Instant),
}

/// Retries `op` while it fails retriably, giving up only when `shutdown` is cancelled.
pub async fn retry_retriable<T, F, Fut>(
    what: &str,
    op: F,
    shutdown: &CancellationToken,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    retry_while_retriable(what, op, GiveUp::WhenCancelled(shutdown)).await
}

/// Retries `op` while it fails retriably, until `deadline`, and then reports the last failure.
pub async fn retry_retriable_until<T, F, Fut>(
    what: &str,
    op: F,
    deadline: Instant,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    retry_while_retriable(what, op, GiveUp::At(deadline)).await
}

async fn retry_while_retriable<T, F, Fut>(
    what: &str,
    mut op: F,
    give_up: GiveUp<'_>,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    let mut backoff = RETRY_MIN;

    loop {
        let failure = match op().await {
            Ok(value) => return Ok(value),
            Err(err) if is_retriable_read(&err) => err,
            Err(err) => return Err(err),
        };

        if let GiveUp::At(deadline) = give_up
            && deadline.saturating_duration_since(Instant::now()) <= backoff
        {
            return Err(failure);
        }

        warn!(
            operation = what,
            error = %failure,
            retry_in = ?backoff,
            "An etcd request failed retriably; retrying"
        );

        match give_up {
            GiveUp::WhenCancelled(shutdown) => {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return Err(ShardManagerError::ShutdownRequested),
                    _ = sleep(backoff) => {}
                }
            }
            GiveUp::At(_) => sleep(backoff).await,
        }

        backoff = std::cmp::min(backoff * 2, RETRY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::time::timeout;

    const TEST_BUDGET: Duration = Duration::from_millis(500);

    #[test]
    async fn a_failure_that_is_not_retriable_is_reported_at_once() {
        let attempts = AtomicU32::new(0);

        let result: Result<(), ShardManagerError> = retry_retriable_until(
            "test",
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Err(ShardManagerError::ConcurrentModification) }
            },
            Instant::now() + TEST_BUDGET,
        )
        .await;

        assert!(
            matches!(result, Err(ShardManagerError::ConcurrentModification)),
            "A non-retriable failure must come back unchanged"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "A non-retriable failure was retried; every fail-stop in this crate would then be \
             delayed by the whole retry budget."
        );
    }

    #[test]
    async fn a_request_that_outran_its_timeout_is_retried() {
        let attempts = AtomicU32::new(0);

        let result: Result<(), ShardManagerError> = timeout(
            Duration::from_secs(5),
            retry_retriable_until(
                "test",
                || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    // What tonic's `GrpcTimeout` layer produces, through `etcd-client`'s wrapper.
                    async {
                        Err(ShardManagerError::EtcdError(
                            etcd_client::Error::GRpcStatus(tonic::Status::cancelled(
                                "Timeout expired",
                            )),
                        ))
                    }
                },
                Instant::now() + TEST_BUDGET,
            ),
        )
        .await
        .expect("A budgeted retry loop that never gives up would hang its caller");

        assert!(result.is_err(), "The budget is spent, so this must fail");
        assert!(
            attempts.load(Ordering::SeqCst) > 1,
            "A request that outran the channel timeout was not retried. `etcd-client` turns the \
             configured request timeout into a tonic endpoint timeout and tonic reports it as \
             `Cancelled`, so every read still fails on the first slow answer from etcd."
        );
    }

    #[test]
    // A generic "gave up" would cost the caller the only description of the fault it has.
    async fn a_spent_budget_reports_the_last_failure() {
        let attempts = AtomicU32::new(0);

        let result: Result<(), ShardManagerError> = timeout(
            Duration::from_secs(5),
            retry_retriable_until(
                "test",
                || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    async { Err(ShardManagerError::Timeout) }
                },
                Instant::now() + TEST_BUDGET,
            ),
        )
        .await
        .expect("A budgeted retry loop that never gives up would hang its caller");

        assert!(
            matches!(result, Err(ShardManagerError::Timeout)),
            "The failure the budget gave up on must be the one reported"
        );
        assert!(
            attempts.load(Ordering::SeqCst) > 1,
            "The retriable failure was never retried, so the budget bought nothing"
        );
    }

    #[test]
    // The loop a standby spends its life in, so cancellation has to land between attempts too.
    async fn a_cancelled_shutdown_ends_the_unbounded_loop() {
        let shutdown = CancellationToken::new();
        let attempts = AtomicU32::new(0);

        let result: Result<(), ShardManagerError> = timeout(
            Duration::from_secs(5),
            retry_retriable(
                "test",
                || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    // Cancel on the second attempt: the loop is proven to continue before it stops.
                    if attempt == 2 {
                        shutdown.cancel();
                    }
                    async { Err(ShardManagerError::Timeout) }
                },
                &shutdown,
            ),
        )
        .await
        .expect("A retry loop that ignores its shutdown token never returns");

        assert!(
            matches!(result, Err(ShardManagerError::ShutdownRequested)),
            "A cancelled shutdown must be reported as such, not as the failure being retried"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "The loop made another attempt after it had been cancelled"
        );
    }
}
