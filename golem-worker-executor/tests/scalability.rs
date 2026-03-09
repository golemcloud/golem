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

use crate::Tracing;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use golem_common::model::agent::AgentId;
use golem_common::model::WorkerStatus;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor_test_utils::{
    start, start_customized, start_with_oplog_config, LastUniqueId, PrecompiledComponent,
    TestContext, WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::future::Future;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tokio::task::JoinSet;
use tracing::{info, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("large_dynamic_memory")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("large_initial_memory")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let warmup_agent_id = agent_id(0);
    let _warmup_worker = executor
        .start_agent(&component.id, warmup_agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await_agent(
                &component_clone,
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
        .map(|i| (i, component.clone(), executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_clone, executor_clone)| {
            {
                spawn(async move {
                    let agent_id = agent_id(n);
                    let _worker_id = executor_clone
                        .start_agent(&component_clone.id, agent_id.clone())
                        .await?;

                    let (result, duration) = timed(async move {
                        executor_clone
                            .invoke_and_await_agent(
                                &component_clone,
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
    assert!(
        total_duration.as_secs() < 10,
        "total duration ({:?}) should be < 10s",
        total_duration
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn spawning_many_workers_that_sleep_long_enough_to_get_suspended(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let warmup_agent_id = agent_id(0);
    let _warmup_worker = executor
        .start_agent(&component.id, warmup_agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await_agent(
                &component_clone,
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
        .map(|i| (i, component.clone(), executor.clone()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, component_clone, executor_clone)| {
            spawn(
                async move {
                    let agent_id = agent_id(n);
                    let _agent = executor_clone
                        .start_agent(&component_clone.id, agent_id.clone())
                        .await?;

                    let (result, duration) = timed(async move {
                        executor_clone
                            .invoke_and_await_agent(
                                &component_clone,
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
    #[tagged_as("large_initial_memory")] large_initial_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor =
        start_customized(deps, &context, Some(768 * 1024 * 1024), None, None, None).await?;
    let component = executor
        .component_dep(&context.default_environment_id, large_initial_memory)
        .store()
        .await?;

    let mut handles = JoinSet::new();
    let mut results = Vec::new();

    const N: usize = 10;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_clone = component.clone();
        let agent_id = agent_id!("large-initial-memory-agent", format!("mem-{i}"));
        handles.spawn(
            async move {
                executor_clone
                    .start_agent(&component_clone.id, agent_id.clone())
                    .await?;

                let result = executor_clone
                    .invoke_and_await_agent(&component_clone, &agent_id, "run", data_value!())
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
#[timeout("4m")]
#[tracing::instrument]
#[allow(clippy::needless_range_loop)]
async fn dynamic_large_memory_allocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor =
        start_customized(deps, &context, Some(768 * 1024 * 1024), None, None, None).await?;
    let component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;

    let mut handles = JoinSet::new();
    let mut results = Vec::new();

    const N: usize = 3;
    for i in 0..N {
        let executor_clone = executor.clone();
        let component_clone = component.clone();
        let agent_id = agent_id!("large-dynamic-memory-agent", format!("mem-{i}"));
        handles.spawn(
            async move {
                executor_clone
                    .start_agent(&component_clone.id, agent_id.clone())
                    .await?;

                let result = executor_clone
                    .invoke_and_await_agent(&component_clone, &agent_id, "run", data_value!())
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

/// Helper that checks whether any `ArchiveOplog` action has been scheduled for the
/// given redis key prefix. It scans Redis for sorted-set keys matching
/// `{prefix}worker:schedule:*` and returns the total number of entries across all
/// matching keys. `ScheduledAction::ArchiveOplog` entries are stored in these sets.
async fn count_scheduled_actions(
    redis: &dyn golem_test_framework::components::redis::Redis,
    redis_prefix: &str,
) -> usize {
    let mut conn = redis.get_async_connection(0).await;
    let pattern = format!("{redis_prefix}worker:schedule:*");
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(&pattern)
        .query_async(&mut conn)
        .await
        .unwrap_or_default();
    let mut total = 0usize;
    for key in &keys {
        let count: usize = redis::cmd("ZCARD")
            .arg(key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);
        total += count;
    }
    total
}

/// Wait until at least one scheduled action appears in Redis, or timeout.
async fn wait_for_scheduled_archive(
    redis: &dyn golem_test_framework::components::redis::Redis,
    redis_prefix: &str,
    initial_count: usize,
    timeout: Duration,
) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let count = count_scheduled_actions(redis, redis_prefix).await;
        if count > initial_count {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[test]
#[tracing::instrument]
async fn oplog_archive_scheduled_when_worker_becomes_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let oplog_config = OplogConfig {
        archive_interval: Duration::from_secs(1),
        entry_count_limit: 5,
        ..Default::default()
    };
    let executor = start_with_oplog_config(deps, &context, Some(oplog_config)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("environment", "archive-idle-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Record the number of scheduled actions before the invocation
    let redis_prefix = context.redis_prefix();
    let before_count = count_scheduled_actions(deps.redis.as_ref(), &redis_prefix).await;

    // Invoke a simple function; after it completes the worker becomes Idle
    executor
        .invoke_and_await_agent(&component, &agent_id, "get_arguments", data_value!())
        .await?;

    // Verify the worker is idle
    let metadata = executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(5))
        .await?;
    assert_eq!(metadata.status, WorkerStatus::Idle);

    // Wait for ArchiveOplog to be scheduled
    let found = wait_for_scheduled_archive(
        deps.redis.as_ref(),
        &redis_prefix,
        before_count,
        Duration::from_secs(10),
    )
    .await;

    assert!(
        found,
        "Expected ArchiveOplog to be scheduled after worker became Idle"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn oplog_archive_scheduled_when_worker_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let oplog_config = OplogConfig {
        archive_interval: Duration::from_secs(1),
        entry_count_limit: 5,
        ..Default::default()
    };
    let executor = start_with_oplog_config(deps, &context, Some(oplog_config)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("golem-host-api", "archive-failed-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Record the number of scheduled actions before triggering failure
    let redis_prefix = context.redis_prefix();
    let before_count = count_scheduled_actions(deps.redis.as_ref(), &redis_prefix).await;

    // Invoke with 0 retries to cause immediate failure
    let _result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "fail_with_custom_max_retries",
            data_value!(0u64),
        )
        .await;

    // Verify the worker is failed
    let metadata = executor
        .wait_for_status(&worker_id, WorkerStatus::Failed, Duration::from_secs(15))
        .await?;
    assert_eq!(metadata.status, WorkerStatus::Failed);

    // Wait for ArchiveOplog to be scheduled
    let found = wait_for_scheduled_archive(
        deps.redis.as_ref(),
        &redis_prefix,
        before_count,
        Duration::from_secs(10),
    )
    .await;

    assert!(
        found,
        "Expected ArchiveOplog to be scheduled after worker Failed"
    );

    Ok(())
}
