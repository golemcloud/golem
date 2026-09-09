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
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

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
        self.inner.tracker.close();
        tokio::time::timeout(grace, self.inner.tracker.wait())
            .await
            .is_ok()
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

        shutdown.cancel();
        assert!(
            shutdown.wait_for_tracked(Duration::from_secs(2)).await,
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

    /// With nothing tracked there is nothing to wait for.
    #[test]
    async fn wait_for_tracked_returns_at_once_with_nothing_spawned() {
        let shutdown = Shutdown::new();
        shutdown.cancel();
        assert!(shutdown.wait_for_tracked(Duration::from_millis(50)).await);
    }
}
