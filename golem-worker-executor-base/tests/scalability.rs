// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use test_r::{inherit_test_dep, test, timeout};

use crate::common::{start, start_limited, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use golem_common::model::ComponentId;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::future::Future;
use std::time::Duration;
use tokio::spawn;
use tokio::task::JoinSet;
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    fn worker_name(n: i32) -> String {
        format!("sleeping-worker-{}", n)
    }

    async fn timed<F>(f: F) -> (F::Output, Duration)
    where
        F: Future + Send + 'static,
    {
        let start = tokio::time::Instant::now();
        let result = f.await;
        let duration = start.elapsed();
        (result, duration)
    }

    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("clocks").await;

    let warmup_worker = executor.start_worker(&component_id, &worker_name(0)).await;

    let executor_clone = executor.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await(&warmup_worker, "run", vec![])
            .await
            .unwrap()
    })
    .await;

    info!("Warmup: {:?}", warmup_result);

    const N: i32 = 100;
    info!("{N} instances");

    let start = tokio::time::Instant::now();
    let input: Vec<(i32, ComponentId, TestWorkerExecutor)> = (1..N)
        .map(|i| (i, component_id.clone(), executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_id, executor_clone)| {
            spawn(async move {
                let worker = executor_clone
                    .start_worker(&component_id, &worker_name(n))
                    .await;
                timed(async move {
                    executor_clone
                        .invoke_and_await(&worker, "run", vec![])
                        .await
                        .unwrap()
                })
                .await
            })
        })
        .collect();

    info!("Spawned all, waiting...");
    let futures: FuturesUnordered<_> = fibers.into_iter().collect();
    let results: Vec<_> = futures.collect().await;

    let total_duration = start.elapsed();

    info!("Results: {:?}", results);
    info!("Total duration: {:?}", total_duration);

    let mut sorted = results
        .into_iter()
        .map(|r| match r {
            Ok((_, duration)) => duration.as_millis(),
            Err(err) => panic!("Error: {:?}", err),
        })
        .collect::<Vec<_>>();
    sorted.sort();
    let idx = (sorted.len() as f64 * 0.95) as usize;
    let p95 = sorted[idx];

    drop(executor);

    check!(p95 < 6000);
    check!(total_duration.as_secs() < 10);
}

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep_long_enough_to_get_suspended(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    fn worker_name(n: i32) -> String {
        format!("sleeping-suspending-worker-{}", n)
    }

    async fn timed<F>(f: F) -> (F::Output, Duration)
    where
        F: Future + Send + 'static,
    {
        let start = tokio::time::Instant::now();
        let result = f.await;
        let duration = start.elapsed();
        (result, duration)
    }

    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("clocks").await;

    let warmup_worker = executor.start_worker(&component_id, &worker_name(0)).await;

    let executor_clone = executor.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await(&warmup_worker, "sleep-for", vec![Value::F64(15.0)])
            .await
            .unwrap()
    })
    .await;

    info!("Warmup: {:?}", warmup_result);

    const N: i32 = 100;
    info!("{N} instances");

    let start = tokio::time::Instant::now();
    let input: Vec<(i32, ComponentId, TestWorkerExecutor)> = (1..N)
        .map(|i| (i, component_id.clone(), executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_id, executor_clone)| {
            spawn(async move {
                let worker = executor_clone
                    .start_worker(&component_id, &worker_name(n))
                    .await;
                timed(async move {
                    executor_clone
                        .invoke_and_await(&worker, "sleep-for", vec![Value::F64(15.0)])
                        .await
                        .unwrap()
                })
                .await
            })
        })
        .collect();

    info!("Spawned all, waiting...");
    let futures: FuturesUnordered<_> = fibers.into_iter().collect();
    let results: Vec<_> = futures.collect().await;

    let total_duration = start.elapsed();

    info!("Results: {:?}", results);
    info!("Total duration: {:?}", total_duration);

    let mut sorted1 = results
        .iter()
        .map(|r| match r {
            Ok((r, _)) => {
                let Value::F64(seconds) = r[0] else {
                    panic!("Unexpected result")
                };
                (seconds * 1000.0) as u64
            }
            Err(err) => panic!("Error: {:?}", err),
        })
        .collect::<Vec<_>>();
    sorted1.sort();
    let idx1 = (sorted1.len() as f64 * 0.95) as usize;
    let p951 = sorted1[idx1];

    let mut sorted2 = results
        .into_iter()
        .map(|r| match r {
            Ok((_, duration)) => duration.as_millis(),
            Err(err) => panic!("Error: {:?}", err),
        })
        .collect::<Vec<_>>();
    sorted2.sort();
    let idx2 = (sorted2.len() as f64 * 0.95) as usize;
    let p952 = sorted2[idx2];

    drop(executor);

    check!(p951 < 25000);
    check!(p952 < 25000);
}

#[test]
#[tracing::instrument]
#[allow(clippy::needless_range_loop)]
async fn initial_large_memory_allocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start_limited(deps, &context, Some(768 * 1024 * 1024))
        .await
        .unwrap();
    let component_id = executor.store_component("large-initial-memory").await;

    let mut handles = JoinSet::new();
    let mut results: Vec<Vec<Value>> = Vec::new();

    const N: usize = 10;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_id_clone = component_id.clone();
        handles.spawn(async move {
            let worker = executor_clone
                .start_worker(&component_id_clone, &format!("large-initial-memory-{i}"))
                .await;
            executor_clone
                .invoke_and_await(&worker, "run", vec![])
                .await
                .unwrap()
        });
    }

    while let Some(result) = handles.join_next().await {
        results.push(result.unwrap());
    }

    for i in 0..N {
        check!(results[i][0] == Value::U64(536870912));
    }
}

#[test]
#[timeout(30000)]
#[tracing::instrument]
#[allow(clippy::needless_range_loop)]
async fn dynamic_large_memory_allocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start_limited(deps, &context, Some(768 * 1024 * 1024))
        .await
        .unwrap();
    let component_id = executor.store_component("large-dynamic-memory").await;

    let mut handles = JoinSet::new();
    let mut results: Vec<Vec<Value>> = Vec::new();

    const N: usize = 3;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_id_clone = component_id.clone();
        handles.spawn(async move {
            let worker = executor_clone
                .start_worker(&component_id_clone, &format!("large-initial-memory-{i}"))
                .await;
            executor_clone
                .invoke_and_await(&worker, "run", vec![])
                .await
                .unwrap()
        });
    }

    while let Some(result) = handles.join_next().await {
        results.push(result.unwrap());
    }

    for i in 0..N {
        check!(results[i][0] == Value::U64(0));
    }
}
