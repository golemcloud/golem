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

use crate::Tracing;
use pretty_assertions::assert_eq;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use golem_common::model::agent::AgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    start, start_customized, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use std::future::Future;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tokio::task::JoinSet;
use tracing::{info, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    fn agent_id(n: i32) -> AgentId {
        agent_id!("clocks", format!("sleeping-agent-{n}"))
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

    let executor = start(deps, &context).await?;
    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let warmup_agent_id = agent_id(0);
    let _warmup_worker = executor
        .start_agent(&component.id, warmup_agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await_agent(
                &component.id,
                &warmup_agent_id,
                "use_std_time_apis",
                data_value!(),
            )
            .await
            .unwrap()
    })
    .await;

    info!("Warmup: {:?}", warmup_result);

    const N: i32 = 100;
    info!("{N} instances");

    let start = tokio::time::Instant::now();
    let input: Vec<(i32, _, _)> = (1..N)
        .map(|i| (i, component.id, executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_id, executor_clone)| {
            {
                spawn(async move {
                    let agent_id = agent_id(n);
                    let worker_id = executor_clone
                        .start_agent(&component_id, agent_id.clone())
                        .await?;

                    let (result, duration) = timed(async move {
                        executor_clone
                            .invoke_and_await_agent(
                                &component.id,
                                &agent_id,
                                "use_std_time_apis",
                                data_value!(),
                            )
                            .await
                    })
                    .await;

                    Ok::<_, anyhow::Error>((result?, duration))
                })
            }
            .in_current_span()
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
            Ok(Ok((_, duration))) => duration.as_millis(),
            other => panic!("Error: {other:?}"),
        })
        .collect::<Vec<_>>();
    sorted.sort();
    let idx = (sorted.len() as f64 * 0.95) as usize;
    let p95 = sorted[idx];

    assert!(p95 < 6000, "p95 ({p95}) should be < 6000");
    assert!(total_duration.as_secs() < 10, "total duration ({:?}) should be < 10s", total_duration);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep_long_enough_to_get_suspended(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    fn agent_id(n: i32) -> AgentId {
        agent_id!("clocks", format!("sleeping-suspending-agent-{n}"))
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

    let executor = start(deps, &context).await?;
    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let warmup_agent_id = agent_id(0);
    let _warmup_worker = executor
        .start_agent(&component.id, warmup_agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await_agent(
                &component.id,
                &warmup_agent_id,
                "sleep_for",
                data_value!(15.0),
            )
            .await
    })
    .await;

    info!("Warmup: {:?}", warmup_result);

    const N: i32 = 100;
    info!("{N} instances");

    let start = tokio::time::Instant::now();
    let input: Vec<(i32, _, _)> = (1..N)
        .map(|i| (i, component.id, executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_id, executor_clone)| {
            spawn(
                async move {
                    let agent_id = agent_id(n);
                    let _agent = executor_clone
                        .start_agent(&component_id, agent_id.clone())
                        .await?;

                    let (result, duration) = timed(async move {
                        executor_clone
                            .invoke_and_await_agent(
                                &component_id,
                                &agent_id,
                                "sleep_for",
                                data_value!(15.0),
                            )
                            .await
                    })
                    .await;
                    Ok::<_, anyhow::Error>((result?, duration))
                }
                .in_current_span(),
            )
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
            Ok(Ok((r, _))) => {
                let Value::F64(seconds) = r
                    .clone()
                    .into_return_value()
                    .expect("Expected single return value")
                else {
                    panic!("Unexpected result")
                };
                (seconds * 1000.0) as u64
            }
            other => panic!("Error: {other:?}"),
        })
        .collect::<Vec<_>>();
    sorted1.sort();
    let idx1 = (sorted1.len() as f64 * 0.95) as usize;
    let p951 = sorted1[idx1];

    let mut sorted2 = results
        .into_iter()
        .map(|r| match r {
            Ok(Ok((_, duration))) => duration.as_millis(),
            other => panic!("Error: {other:?}"),
        })
        .collect::<Vec<_>>();
    sorted2.sort();
    let idx2 = (sorted2.len() as f64 * 0.95) as usize;
    let p952 = sorted2[idx2];

    drop(executor);

    assert!(p951 < 25000, "p951 ({p951}) should be < 25000");
    assert!(p952 < 25000, "p952 ({p952}) should be < 25000");

    Ok(())
}

#[test]
#[tracing::instrument]
#[allow(clippy::needless_range_loop)]
async fn initial_large_memory_allocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_customized(deps, &context, Some(768 * 1024 * 1024), None).await?;
    let component = executor
        .component(
            &context.default_environment_id,
            "scalability_large_initial_memory_release",
        )
        .name("scalability:large-initial-memory")
        .store()
        .await?;

    let mut handles = JoinSet::new();
    let mut results = Vec::new();

    const N: usize = 10;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_id = component.id;
        let agent_id = agent_id!("large-initial-memory-agent", format!("mem-{i}"));
        handles.spawn(
            async move {
                executor_clone
                    .start_agent(&component_id, agent_id.clone())
                    .await?;

                let result = executor_clone
                    .invoke_and_await_agent(&component_id, &agent_id, "run", data_value!())
                    .await?;

                Ok::<_, anyhow::Error>(result)
            }
            .in_current_span(),
        );
    }

    while let Some(result) = handles.join_next().await {
        results.push(result??);
    }

    for i in 0..N {
        assert_eq!(results[i], data_value!(536870912u64));
    }

    Ok(())
}

#[test]
#[timeout(60000)]
#[tracing::instrument]
#[allow(clippy::needless_range_loop)]
async fn dynamic_large_memory_allocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_customized(deps, &context, Some(768 * 1024 * 1024), None).await?;
    let component = executor
        .component(
            &context.default_environment_id,
            "scalability_large_dynamic_memory_release",
        )
        .name("scalability:large-dynamic-memory")
        .store()
        .await?;

    let mut handles = JoinSet::new();
    let mut results = Vec::new();

    const N: usize = 3;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_id = component.id;
        let agent_id = agent_id!("large-dynamic-memory-agent", format!("mem-{i}"));
        handles.spawn(
            async move {
                executor_clone
                    .start_agent(&component_id, agent_id.clone())
                    .await?;

                let result = executor_clone
                    .invoke_and_await_agent(&component_id, &agent_id, "run", data_value!())
                    .await?;

                Ok::<_, anyhow::Error>(result)
            }
            .in_current_span(),
        );
    }

    while let Some(result) = handles.join_next().await {
        results.push(result??);
    }

    for i in 0..N {
        assert_eq!(results[i], data_value!(0u64));
    }

    Ok(())
}
