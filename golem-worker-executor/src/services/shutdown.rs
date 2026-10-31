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

use golem_common::base_model::shard_lease;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, warn};

const SHUTDOWN_GRACE_MILLIS: u64 = 10_000;
const DEREGISTER_DEADLINE_MILLIS: u64 = 8_000;

/// How long a termination signal waits for the tasks spawned through [`Shutdown::spawn`] to
/// finish, including shard deregistration and executor directory cleanup after invocation and
/// snapshot work drains. This is a shared bound, not an additional grace for each task.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(SHUTDOWN_GRACE_MILLIS);

/// How long the deregister RPC itself may take. Lives here, beside the grace that contains it,
/// because the two are only meaningful against each other.
pub const DEREGISTER_DEADLINE: Duration = Duration::from_millis(DEREGISTER_DEADLINE_MILLIS);

// The grace has to outlast the RPC it exists to wait for. Were it the other way round the RPC
// would always be cut off first, and the arm that reports the lease being left to lapse could
// never run.
const _: () = assert!(DEREGISTER_DEADLINE_MILLIS < SHUTDOWN_GRACE_MILLIS);
// A deregister is one shard manager write, so a deadline under that budget would abandon every
// deregistration that had to wait for a busy manager - the shards would then sit out a whole
// lease instead of being handed back.
const _: () =
    assert!(DEREGISTER_DEADLINE_MILLIS > shard_lease::MAX_SHARD_LEASE_STATE_WRITE_TIMEOUT_MILLIS);
// Stopping never waits longer than renewing would.
const _: () =
    assert!(DEREGISTER_DEADLINE_MILLIS <= shard_lease::SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS);

/// A graph-wide shutdown signal for background tasks spawned by services.
///
/// Services that spawn background loops should use the `CancellationToken`
/// obtained via `token()` to stop promptly when the executor shuts down,
/// rather than relying solely on `Weak::upgrade()` which can race with
/// other services being torn down.
///
/// A loop that still has work to do *after* it observes the token - the shard
/// lease renewal loop deregisters on it - spawns through [`Self::spawn`] rather
/// than `tokio::spawn`, so that [`Self::wait_for_tracked`] can hold the process
/// open until that work lands. A detached task would be cut off at its next
/// await point when the runtime is dropped.
///
/// The token is cancelled explicitly via `cancel()` (from `RunDetails::drop()`,
/// or from the termination-signal path in the executor's `main`). As a safety
/// net, if all `Shutdown` handles are dropped without an explicit cancel, the
/// `Drop` impl on the inner `Arc` will cancel the token.
#[derive(Clone)]
pub struct Shutdown {
    inner: Arc<Inner>,
}

struct Inner {
    token: CancellationToken,
    tracker: TaskTracker,
}

impl Shutdown {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                token: CancellationToken::new(),
                tracker: TaskTracker::new(),
            }),
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.inner.token.clone()
    }

    pub fn cancel(&self) {
        self.inner.token.cancel();
    }

    /// Signals this executor, waits within `grace`, and warns if tracked work remains.
    /// This does not abort tasks or prove release when it returns `false`.
    pub async fn cancel_and_wait_for_tracked(&self, grace: Duration) -> bool {
        self.cancel();
        let completed = self.wait_for_tracked(grace).await;
        if !completed {
            warn!(
                ?grace,
                "Background tasks did not finish within the shutdown grace period; executor resources may still be held"
            );
        }
        completed
    }

    /// Spawns a task whose completion [`Self::wait_for_tracked`] waits for.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.inner.tracker.spawn(future)
    }

    /// Waits up to `grace` for every task spawned through [`Self::spawn`] to
    /// finish. Meant to follow [`Self::cancel`]. `true` if they all did.
    pub async fn wait_for_tracked(&self, grace: Duration) -> bool {
        tokio::time::timeout(grace, self.join_tracked())
            .await
            .is_ok()
    }

    /// Joins tracked tasks when the caller already owns the shutdown deadline.
    pub async fn join_tracked(&self) {
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// Awaits a task whose handle nothing aborts, and gives `Err` only when the task panicked.
///
/// Such a task is cancelled only by the shutdown of its runtime: the blocking pool refuses new
/// work and the scheduler drops spawned tasks, while the worker threads still poll queued tasks.
/// A cancellation is therefore not a failure of the work, and there is no result to give. The
/// caller is a task of the same runtime, which drops it at its next yield, so this parks the
/// caller instead of returning an error the caller would treat as permanent.
///
/// Only tasks of the runtime that ran `handle` may await this. A future driven outside that
/// runtime after its shutdown, for example by `block_on`, would wait forever.
pub async fn join_or_park_at_runtime_shutdown<T>(handle: JoinHandle<T>) -> Result<T, JoinError> {
    match handle.await {
        Err(error) if error.is_cancelled() => {
            debug!(
                "Task cancelled by the runtime shutdown, parking its caller until it is dropped"
            );
            std::future::pending().await
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    /// The reason the tracker exists: a task with work to do *after* the token
    /// trips has to be waited for, not cut off by the runtime being dropped.
    /// Spawning it through `tokio::spawn` instead would make `wait_for_tracked`
    /// return at once, before the work landed.
    #[test]
    async fn wait_for_tracked_holds_for_work_that_follows_the_token() {
        let shutdown = Shutdown::new();
        let landed = Arc::new(AtomicBool::new(false));
        let token = shutdown.token();
        let flag = landed.clone();
        shutdown.spawn(async move {
            token.cancelled().await;
            // stands in for the deregister RPC
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.store(true, Ordering::Release);
        });

        assert!(
            shutdown
                .cancel_and_wait_for_tracked(Duration::from_secs(2))
                .await,
            "tracked work should finish inside the grace"
        );
        assert!(
            landed.load(Ordering::Acquire),
            "the work that follows the token must have landed before the wait returned"
        );
    }

    /// The grace is a bound, not a promise: a task that never finishes must not
    /// hold the process open.
    #[test]
    async fn wait_for_tracked_gives_up_after_the_grace() {
        let shutdown = Shutdown::new();
        shutdown.spawn(std::future::pending::<()>());

        shutdown.cancel();
        assert!(!shutdown.wait_for_tracked(Duration::from_millis(50)).await);
    }

    /// Only a cancellation parks the caller: a task that panicked is still reported, so the
    /// caller keeps failing fast on it.
    #[test]
    #[test_r::timeout("30s")]
    async fn a_panicked_task_is_reported_rather_than_parked() {
        let handle = tokio::task::spawn_blocking(|| panic!("the blocking work failed"));
        let error = join_or_park_at_runtime_shutdown(handle).await.unwrap_err();
        assert!(error.is_panic());
    }

    /// With nothing tracked there is nothing to wait for.
    #[test]
    async fn wait_for_tracked_returns_at_once_with_nothing_spawned() {
        let shutdown = Shutdown::new();
        shutdown.cancel();
        assert!(shutdown.wait_for_tracked(Duration::from_millis(50)).await);
    }
}
