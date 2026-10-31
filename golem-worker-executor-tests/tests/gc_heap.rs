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

//! End-to-end tests for the wasm GC and exception-handling proposals. The `gc_heap` fixture is a
//! Rust agent composed with a WAT component that keeps GC arrays reachable and throws wasm
//! exceptions.

use crate::Tracing;
use golem_common::model::account::AccountId;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_resource_limits,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("gc_heap")]
    PrecompiledComponent
);

const MIB: u64 = 1024 * 1024;

struct FixedLimits(Arc<AtomicResourceEntry>);

#[async_trait::async_trait]
impl ResourceLimits for FixedLimits {
    async fn initialize_account(
        &self,
        _account_id: AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(self.0.clone())
    }
}

/// Starts an executor with the production worker context and its resource limiter, with an agent
/// memory limit of `max_memory` bytes.
async fn start_with_memory_limit(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_memory: usize,
) -> anyhow::Result<TestWorkerExecutor> {
    let entry = Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        max_memory,
        usize::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
    ));
    start_with_resource_limits(deps, context, Arc::new(FixedLimits(entry))).await
}

fn total_memory_growth(entries: &[PublicOplogEntryWithIndex]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::GrowMemory(params) => Some(params.delta),
            _ => None,
        })
        .sum()
}

fn errors(entries: &[PublicOplogEntryWithIndex]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Error(error) => Some(error.error.clone()),
            _ => None,
        })
        .collect()
}

/// Waits until the asynchronously persisted `GrowMemory` hints of the agent reach `at_least`
/// bytes, and returns their total.
async fn wait_for_memory_growth(
    executor: &TestWorkerExecutor,
    agent_id: &AgentId,
    at_least: u64,
) -> anyhow::Result<u64> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let total =
                total_memory_growth(&executor.get_oplog(agent_id, OplogIndex::INITIAL).await?);
            if total >= at_least {
                return Ok(total);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn gc_heap_growth_is_counted_and_hinted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "counted");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let retained = executor
        .invoke_and_await_agent(&component, &agent, "retained_kib", data_value!())
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 0);
    let before = wait_for_memory_growth(&executor, &worker_id, 0).await?;

    let retained = executor
        .invoke_and_await_agent(&component, &agent, "retain", data_value!(16u32 * 1024))
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 16 * 1024);

    // The Rust agent's linear memory does not grow for this call, so only the GC heap growth
    // can raise the hinted total by the retained 16 MiB.
    let after = wait_for_memory_growth(&executor, &worker_id, before + 16 * MIB).await?;
    assert!(after - before >= 16 * MIB, "before={before}, after={after}");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn gc_allocation_past_the_memory_limit_fails_like_linear_memory(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_memory_limit(deps, &context, (64 * MIB) as usize).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "over-limit");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let retained = executor
        .invoke_and_await_agent(&component, &agent, "retain", data_value!(4u32 * 1024))
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 4 * 1024);

    let result = executor
        .invoke_and_await_agent(&component, &agent, "retain", data_value!(128u32 * 1024))
        .await;
    assert!(result.is_err(), "{result:?}");

    executor
        .wait_for_status(&worker_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        errors(&oplog),
        vec!["Exceeded plan memory limit".to_string()]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn reload_after_restart_reserves_gc_heap_growth_without_double_counting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let control = agent_id!("GcHeapAgent", "reload-control");
    let grown = agent_id!("GcHeapAgent", "reload-grown");
    let control_id = executor.start_agent(&component.id, control.clone()).await?;
    let grown_id = executor.start_agent(&component.id, grown.clone()).await?;

    for agent in [&control, &grown] {
        executor
            .invoke_and_await_agent(&component, agent, "retained_kib", data_value!())
            .await?;
    }
    executor
        .invoke_and_await_agent(&component, &grown, "retain", data_value!(32u32 * 1024))
        .await?;
    let control_growth = wait_for_memory_growth(&executor, &control_id, 0).await?;
    let grown_growth =
        wait_for_memory_growth(&executor, &grown_id, control_growth + 32 * MIB).await?;

    drop(executor);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;

    // Replay rebuilds the GC heap from the recorded invocations: the retained arrays are back.
    for (agent, expected) in [(&control, 0u32), (&grown, 32 * 1024)] {
        let retained = executor
            .invoke_and_await_agent(&component, agent, "retained_kib", data_value!())
            .await?;
        assert_eq!(retained.into_typed::<u32>()?, expected);
    }

    // The reload reserves the folded `total_linear_memory_size` of the agent status.
    let control_reserved = executor
        .get_worker_metadata(&control_id)
        .await?
        .total_linear_memory_size;
    let grown_reserved = executor
        .get_worker_metadata(&grown_id)
        .await?
        .total_linear_memory_size;
    assert!(
        grown_reserved >= control_reserved + 32 * MIB,
        "the reload must reserve the GC heap growth: control={control_reserved}, grown={grown_reserved}"
    );

    // Replaying the growth writes no further hints, so the hinted totals are unchanged.
    let oplog = executor.get_oplog(&grown_id, OplogIndex::INITIAL).await?;
    assert_eq!(total_memory_growth(&oplog), grown_growth);
    let oplog = executor.get_oplog(&control_id, OplogIndex::INITIAL).await?;
    assert_eq!(total_memory_growth(&oplog), control_growth);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn exceptions_are_caught_in_the_guest_and_escaping_exceptions_fail_the_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "exceptions");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let caught = executor
        .invoke_and_await_agent(&component, &agent, "catch_thrown", data_value!(41u32))
        .await?;
    assert_eq!(caught.into_typed::<u32>()?, 42);

    let result = executor
        .invoke_and_await_agent(&component, &agent, "throw_uncaught", data_value!())
        .await;
    assert!(result.is_err(), "{result:?}");

    // An escaping exception is a deterministic trap: the agent fails without retrying.
    executor
        .wait_for_status(&worker_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(errors(&oplog).len(), 1, "{:?}", errors(&oplog));

    Ok(())
}
