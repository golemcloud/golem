// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

///  Like tokio Notify, but also wakes up future waiters.
#[derive(Clone, Debug)]
pub struct OneShotEvent {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Default for OneShotEvent {
    fn default() -> Self {
        Self::new()
    }
}

impl OneShotEvent {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn set(&self) {
        if !self.flag.swap(true, Ordering::Release) {
            self.notify.notify_waiters();
        }
    }

    pub async fn wait(&self) {
        if self.flag.load(Ordering::Acquire) {
            return;
        }

        let notified = self.notify.notified();
        if self.flag.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use test_r::test;
    use tokio::time::{timeout, Duration};

    #[test]
    async fn waiters_are_woken_after_set() {
        let event = Arc::new(OneShotEvent::new());
        let woken = Arc::new(AtomicUsize::new(0));

        // spawn multiple waiters
        for _ in 0..5 {
            let e = event.clone();
            let w = woken.clone();
            tokio::spawn(async move {
                e.wait().await;
                w.fetch_add(1, Ordering::SeqCst);
            });
        }

        // allow them to start waiting
        tokio::time::sleep(Duration::from_millis(50)).await;
        event.set();

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(woken.load(Ordering::SeqCst), 5, "all waiters should wake");
    }

    #[test]
    async fn future_waiters_return_immediately() {
        let event = OneShotEvent::new();
        event.set(); // signal first

        // should not block
        let start = std::time::Instant::now();
        event.wait().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(10),
            "wait after set() should return immediately"
        );
    }

    #[tokio::test]
    async fn double_set_does_not_panic_or_re_notify() {
        let event = OneShotEvent::new();
        event.set();
        event.set(); // second call is a no-op
                     // Should still behave normally
        event.wait().await;
    }

    #[test]
    async fn race_between_set_and_wait_is_handled() {
        let event = Arc::new(OneShotEvent::new());
        let e2 = event.clone();

        // Simulate race
        let waiter = tokio::spawn(async move {
            e2.wait().await;
        });

        tokio::spawn({
            let e3 = event.clone();
            async move {
                e3.set();
            }
        });

        timeout(Duration::from_millis(200), waiter)
            .await
            .expect("waiter should not deadlock")
            .expect("waiter task should complete");
    }

    #[test]
    async fn multiple_concurrent_sets_only_notify_once() {
        let event = Arc::new(OneShotEvent::new());
        let notify_count = Arc::new(AtomicUsize::new(0));

        // Waiters that count when woken
        for _ in 0..10 {
            let e = event.clone();
            let c = notify_count.clone();
            tokio::spawn(async move {
                e.wait().await;
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Race multiple set() calls
        let mut handles = vec![];
        for _ in 0..5 {
            let e = event.clone();
            handles.push(tokio::spawn(async move {
                e.set();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            notify_count.load(Ordering::SeqCst),
            10,
            "each waiter should wake exactly once"
        );
    }

    #[test]
    async fn wait_before_and_after_signal_both_work() {
        let event = Arc::new(OneShotEvent::new());
        let e2 = event.clone();

        let first = tokio::spawn(async move {
            e2.wait().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        event.set();

        // first should complete
        first.await.unwrap();

        // now future waiters should instantly complete
        let start = std::time::Instant::now();
        event.wait().await;
        assert!(
            start.elapsed() < Duration::from_millis(10),
            "future waiters must return immediately"
        );
    }
}
