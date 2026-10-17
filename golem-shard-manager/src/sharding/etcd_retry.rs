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
//! Reads, which change nothing, and writes that never reached etcd. A write is a compare-and-swap
//! that may already have been applied when its transport failed, so a write that may have been
//! received is never repeated and the write path fail-stops on it instead.
//!
//! Worth having because tonic keeps an unreachable endpoint in the balancer's rotation, so with
//! several endpoints a share of reads fail fast until it is back; a slow answer counts too.

use crate::config::EtcdConfig;
use crate::sharding::error::ShardManagerError;
use golem_common::retriable_error::IsRetriableError;
use std::error::Error as _;
use std::future::Future;
use std::time::Duration;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;
use tonic::Code;
use tracing::warn;

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

/// Whether a failed request was never sent, because no connection to the chosen endpoint could be
/// made.
///
/// A request that was never sent cannot have landed, so a compare-and-swap that failed this way
/// still has the right expected revision and sending it again is safe. That is the only failure
/// for which it is: an answer from etcd is final, and a timeout or a reset connection may come
/// after etcd received the request, so those stay false.
///
/// `etcd-client` hands each request to a random endpoint of its balancer, and an endpoint that
/// refuses connections stays in the rotation, so with one member down a share of the requests
/// fail this way at once. The failure arrives as `Unavailable` whose source is the transport error
/// of the connection attempt, ending in the refusal of the operating system. The same code with a
/// reset connection ends in a different error, and a timeout or a cancelled request has other
/// codes altogether.
pub fn never_reached_etcd(err: &etcd_client::Error) -> bool {
    let etcd_client::Error::GRpcStatus(status) = err else {
        return false;
    };
    if status.code() != Code::Unavailable {
        return false;
    }

    let mut source = status.source();
    if !source.is_some_and(|first| first.is::<tonic::transport::Error>()) {
        return false;
    }
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return true;
        }
        source = error.source();
    }
    false
}

/// Sends a request again while it fails without having reached etcd, until `deadline`, and then
/// reports that failure. Any other failure is returned at once.
///
/// For writes, whose outcome is unknown unless the request was never sent; see
/// [`never_reached_etcd`].
pub async fn send_again_while_unsent<T, F, Fut>(
    what: &str,
    mut op: F,
    deadline: Instant,
    delay: Duration,
) -> Result<T, etcd_client::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, etcd_client::Error>>,
{
    loop {
        let failure = match op().await {
            Ok(value) => return Ok(value),
            Err(err) if never_reached_etcd(&err) => err,
            Err(err) => return Err(err),
        };

        if deadline.saturating_duration_since(Instant::now()) <= delay {
            return Err(failure);
        }

        warn!(
            operation = what,
            error = %failure,
            retry_in = ?delay,
            "An etcd write failed before reaching etcd; sending it again"
        );
        sleep(delay).await;
    }
}

/// How long a read may keep retrying, and how it backs off between attempts, as configured.
#[derive(Clone, Copy, Debug)]
pub struct ReadRetry {
    pub timeout: Duration,
    pub min_delay: Duration,
    pub max_delay: Duration,
}

impl ReadRetry {
    pub fn from_config(config: &EtcdConfig) -> Self {
        Self {
            timeout: config.read_retry_timeout,
            min_delay: config.retry_min_delay,
            max_delay: config.retry_max_delay,
        }
    }

    /// The instant a read starting now gives up.
    pub fn deadline(&self) -> Result<Instant, ShardManagerError> {
        Instant::now().checked_add(self.timeout).ok_or_else(|| {
            ShardManagerError::Internal(
                "Configured etcd read retry timeout exceeds the clock range".to_string(),
            )
        })
    }
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
    min_delay: Duration,
    max_delay: Duration,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    retry_while_retriable(
        what,
        op,
        GiveUp::WhenCancelled(shutdown),
        min_delay,
        max_delay,
    )
    .await
}

/// Retries `op` while it fails retriably, until `deadline`, and then reports the last failure.
pub async fn retry_retriable_until<T, F, Fut>(
    what: &str,
    op: F,
    deadline: Instant,
    min_delay: Duration,
    max_delay: Duration,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    retry_while_retriable(what, op, GiveUp::At(deadline), min_delay, max_delay).await
}

async fn retry_while_retriable<T, F, Fut>(
    what: &str,
    mut op: F,
    give_up: GiveUp<'_>,
    min_delay: Duration,
    max_delay: Duration,
) -> Result<T, ShardManagerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ShardManagerError>>,
{
    let mut backoff = min_delay;

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

        backoff = std::cmp::min(backoff.saturating_mul(2), max_delay);
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::time::timeout;

    const TEST_BUDGET: Duration = Duration::from_millis(500);
    const TEST_RETRY_MIN: Duration = Duration::from_millis(10);
    const TEST_RETRY_MAX: Duration = Duration::from_millis(20);

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
            TEST_RETRY_MIN,
            TEST_RETRY_MAX,
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
                TEST_RETRY_MIN,
                TEST_RETRY_MAX,
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
                TEST_RETRY_MIN,
                TEST_RETRY_MAX,
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
                TEST_RETRY_MIN,
                TEST_RETRY_MAX,
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
