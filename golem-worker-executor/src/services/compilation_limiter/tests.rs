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
use futures::future::join_all;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::test;

#[test]
async fn limits_concurrent_compilations_to_the_configured_maximum() {
    let limit = 4usize;
    let bursts = 200usize;
    let limiter = CompilationLimiter::new(limit);

    let current = Arc::new(AtomicUsize::new(0));
    let observed_max = Arc::new(AtomicUsize::new(0));

    let futs: Vec<_> = (0..bursts)
        .map(|_| {
            let limiter = limiter.clone();
            let current = current.clone();
            let observed_max = observed_max.clone();
            async move {
                limiter
                    .run(async {
                        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                        observed_max.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        current.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
            }
        })
        .collect();

    tokio::time::timeout(Duration::from_secs(30), join_all(futs))
        .await
        .expect("gated compilations timed out");

    let peak = observed_max.load(Ordering::SeqCst);
    assert!(
        peak <= limit,
        "compilation limiter with limit {limit} allowed {peak} concurrent compilations"
    );
}

#[test]
async fn unlimited_limiter_does_not_gate() {
    let limiter = CompilationLimiter::new(0);
    let current = Arc::new(AtomicUsize::new(0));
    let observed_max = Arc::new(AtomicUsize::new(0));
    let bursts = 50usize;

    let futs: Vec<_> = (0..bursts)
        .map(|_| {
            let limiter = limiter.clone();
            let current = current.clone();
            let observed_max = observed_max.clone();
            async move {
                limiter
                    .run(async {
                        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                        observed_max.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        current.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
            }
        })
        .collect();

    tokio::time::timeout(Duration::from_secs(30), join_all(futs))
        .await
        .expect("ungated compilations timed out");

    let peak = observed_max.load(Ordering::SeqCst);
    assert!(
        peak > 1,
        "unlimited limiter should allow concurrent execution, observed max {peak}"
    );
}
