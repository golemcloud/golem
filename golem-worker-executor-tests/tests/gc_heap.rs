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
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_resource_limits_and_configure,
};
use std::collections::{HashMap, HashSet};
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
    start_with_resource_limits_and_configure(
        deps,
        context,
        Arc::new(FixedLimits(entry)),
        Arc::new(|config| {
            config.memory.system_memory_override = Some(1024 * MIB);
            config.memory.worker_memory_ratio = 1.0;
        }),
    )
    .await
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

fn constructor_key(entries: &[PublicOplogEntryWithIndex]) -> IdempotencyKey {
    let keys: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
                PublicAgentInvocation::AgentInitialization(invocation) => {
                    Some(invocation.idempotency_key.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 1, "expected exactly one constructor start");
    keys.into_iter().next().unwrap()
}

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex], expected: &[IdempotencyKey]) {
    let expected_keys: HashSet<_> = expected.iter().cloned().collect();
    assert_eq!(
        expected_keys.len(),
        expected.len(),
        "duplicate expected key"
    );
    let mut starts = HashMap::new();
    let mut finishes = HashMap::new();
    let mut active = None;
    let mut pending_calls = HashSet::new();
    let last_finished = entries
        .iter()
        .rposition(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("settled history must contain a finished invocation");
    for (position, entry) in entries.iter().enumerate() {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => {
                let key = match &params.invocation {
                    PublicAgentInvocation::AgentInitialization(invocation) => {
                        &invocation.idempotency_key
                    }
                    PublicAgentInvocation::AgentMethodInvocation(invocation) => {
                        &invocation.idempotency_key
                    }
                    other => panic!("unexpected invocation: {other:?}"),
                };
                assert!(
                    active.replace(key.clone()).is_none(),
                    "overlapping invocations"
                );
                *starts.entry(key.clone()).or_insert(0) += 1;
            }
            PublicOplogEntry::AgentInvocationFinished(_) => {
                // Finished has no key; it terminates the preceding active invocation.
                let key = active.take().expect("finished invocation has no start");
                *finishes.entry(key).or_insert(0) += 1;
                assert!(
                    pending_calls.is_empty(),
                    "unfinished calls at invocation finish"
                );
            }
            PublicOplogEntry::Start(_) => {
                // Core initialization may call the host before the constructor starts.
                assert!(
                    active.is_some() || starts.is_empty(),
                    "Start between invocations"
                );
                assert!(position < last_finished, "Start after final Finished");
                assert!(
                    pending_calls.insert(entry.oplog_index),
                    "duplicate call start"
                );
            }
            PublicOplogEntry::End(params) => {
                assert!(
                    active.is_some() || starts.is_empty(),
                    "End between invocations"
                );
                assert!(position < last_finished, "End after final Finished");
                assert!(
                    pending_calls.remove(&params.start_index),
                    "End without unique Start"
                );
            }
            PublicOplogEntry::Cancelled(params) => {
                assert!(
                    active.is_some() || starts.is_empty(),
                    "Cancelled between invocations"
                );
                assert!(
                    pending_calls.remove(&params.start_index),
                    "Cancelled without unique Start"
                );
            }
            _ => {}
        }
    }
    assert!(active.is_none(), "unfinished invocation");
    assert!(pending_calls.is_empty(), "unfinished durable calls");
    let expected: HashMap<_, _> = expected_keys.into_iter().map(|key| (key, 1)).collect();
    assert_eq!(
        starts, expected,
        "invocation starts must match expected identities"
    );
    assert_eq!(
        finishes, expected,
        "invocation finishes must match expected identities"
    );
    assert!(errors(entries).is_empty(), "{:?}", errors(entries));
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

    let control_read = IdempotencyKey::fresh();
    let grown_read = IdempotencyKey::fresh();
    let retain_key = IdempotencyKey::fresh();
    for (agent, key) in [(&control, &control_read), (&grown, &grown_read)] {
        executor
            .invoke_and_await_agent_with_key(&component, agent, key, "retained_kib", data_value!())
            .await?;
    }
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &grown,
            &retain_key,
            "retain",
            data_value!(32u32 * 1024),
        )
        .await?;
    let control_growth = wait_for_memory_growth(&executor, &control_id, 0).await?;
    let grown_growth =
        wait_for_memory_growth(&executor, &grown_id, control_growth + 32 * MIB).await?;
    let control_entries = executor.get_oplog(&control_id, OplogIndex::INITIAL).await?;
    let grown_entries = executor.get_oplog(&grown_id, OplogIndex::INITIAL).await?;
    let mut control_keys = vec![constructor_key(&control_entries), control_read];
    let mut grown_keys = vec![constructor_key(&grown_entries), grown_read, retain_key];
    assert_settled_history(&control_entries, &control_keys);
    assert_settled_history(&grown_entries, &grown_keys);

    drop(executor);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;

    // Replay rebuilds the GC heap from the recorded invocations: the retained arrays are back.
    for (agent, worker_id, keys, expected) in [
        (&control, &control_id, &mut control_keys, 0u32),
        (&grown, &grown_id, &mut grown_keys, 32 * 1024),
    ] {
        let key = IdempotencyKey::fresh();
        let retained = executor
            .invoke_and_await_agent_with_key(&component, agent, &key, "retained_kib", data_value!())
            .await?;
        assert_eq!(retained.into_typed::<u32>()?, expected);
        keys.push(key);
        let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        assert_settled_history(&entries, keys);
    }

    // Metadata folds the measured allocation, including GC capacity, after reconstruction.
    let control_memory = executor
        .get_worker_metadata(&control_id)
        .await?
        .total_linear_memory_size;
    let grown_memory = executor
        .get_worker_metadata(&grown_id)
        .await?
        .total_linear_memory_size;
    assert!(
        grown_memory >= control_memory + 32 * MIB,
        "the reconstructed allocation must include GC capacity: control={control_memory}, grown={grown_memory}"
    );

    // Replay may reconcile capacity, but it must not append additive growth for old work.
    let oplog = executor.get_oplog(&grown_id, OplogIndex::INITIAL).await?;
    assert_eq!(total_memory_growth(&oplog), grown_growth);
    let oplog = executor.get_oplog(&control_id, OplogIndex::INITIAL).await?;
    assert_eq!(total_memory_growth(&oplog), control_growth);

    let fresh_key = IdempotencyKey::fresh();
    let retained = executor
        .invoke_and_await_agent_with_key(
            &component,
            &grown,
            &fresh_key,
            "retain",
            data_value!(4u32 * 1024),
        )
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 36 * 1024);
    grown_keys.push(fresh_key);
    let entries = executor.get_oplog(&grown_id, OplogIndex::INITIAL).await?;
    assert_settled_history(&entries, &grown_keys);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn restart_reconciles_both_sides_of_a_saved_memory_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "reconciled");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let retain_key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &retain_key,
            "retain",
            data_value!(32u32 * 1024),
        )
        .await?;
    wait_for_memory_growth(&executor, &worker_id, 32 * MIB).await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let mut expected_keys = vec![constructor_key(&entries), retain_key];
    assert_settled_history(&entries, &expected_keys);

    // Commit explicit saved requirements through the production status owner. This varies
    // only the prepaid baseline, not the actual collector allocation.
    for saved in [8 * MIB, 256 * MIB] {
        let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
        executor
            .production_active_agent(&owner_id)
            .await
            .unwrap()
            .primary()
            .add_and_commit_oplog(OplogEntry::memory_size_reconciled(saved))
            .await?;
        drop(executor);
        executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
        let read_key = IdempotencyKey::fresh();
        let retained = executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &read_key,
                "retained_kib",
                data_value!(),
            )
            .await?;
        assert_eq!(retained.into_typed::<u32>()?, 32 * 1024);
        expected_keys.push(read_key);
        let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_settled_history(&entries, &expected_keys);
        let reconciled = executor
            .get_worker_metadata(&worker_id)
            .await?
            .total_linear_memory_size;
        assert!(
            reconciled > 8 * MIB && reconciled < 256 * MIB,
            "reconciled={reconciled}, saved={saved}"
        );
    }

    let entries_before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let growth_before = total_memory_growth(&entries_before);
    let baseline = executor
        .get_worker_metadata(&worker_id)
        .await?
        .total_linear_memory_size;
    let fresh_key = IdempotencyKey::fresh();
    let retained = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &fresh_key,
            "retain",
            data_value!(64u32 * 1024),
        )
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 96 * 1024);
    expected_keys.push(fresh_key);
    let growth_after =
        wait_for_memory_growth(&executor, &worker_id, growth_before + 64 * MIB).await?;
    assert_eq!(
        executor
            .get_worker_metadata(&worker_id)
            .await?
            .total_linear_memory_size,
        baseline + growth_after - growth_before
    );
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_settled_history(&entries, &expected_keys);
    drop(executor);
    let executor = start_with_memory_limit(deps, &context, usize::MAX).await?;
    let read_key = IdempotencyKey::fresh();
    let retained = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &read_key,
            "retained_kib",
            data_value!(),
        )
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 96 * 1024);
    expected_keys.push(read_key);
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_settled_history(&entries, &expected_keys);
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
