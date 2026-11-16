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
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::services::HasWorker;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::services::resource_limits::AtomicResourceEntry;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::WorkerCtx;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, SharedResourceLimits, TestContext, TestExecutorOverrides,
    TestWorkerCtx, TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
    start_with_resource_limits_and_configure,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use wasmtime::{Store, StoreMemory};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("gc_heap")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("gc_initializer")]
    PrecompiledComponent
);

const MIB: u64 = 1024 * 1024;

#[derive(Debug)]
struct MemoryObservation {
    linear: u64,
    gc: u64,
    tracked: u64,
    reserved: u64,
    billable: u64,
}

impl MemoryObservation {
    fn read(store: &Store<TestWorkerCtx>) -> Self {
        // Enumerate Wasmtime allocations independently of the executor's allocation helper,
        // memory tracker, metadata and oplog hints.
        let linear = store
            .linear_memories()
            .iter()
            .map(|memory| match memory {
                StoreMemory::Unshared(memory) => memory.data_size(store) as u64,
                StoreMemory::Shared(_) => panic!("GC fixture must not use shared linear memory"),
            })
            .sum();
        let tracker = store.data().durable_ctx().linear_memory_tracker();
        Self {
            linear,
            gc: store.gc_heap_capacity() as u64,
            tracked: tracker.current_bytes(),
            reserved: tracker.reserved_bytes(),
            billable: tracker.billable_bytes(),
        }
    }

    fn allocated(&self) -> u64 {
        self.linear + self.gc
    }

    fn assert_prepared(&self) {
        assert!(self.linear > 0 && self.gc > 0, "{self:?}");
        assert_eq!(self.tracked, self.allocated(), "{self:?}");
        assert_eq!(self.reserved, self.allocated(), "{self:?}");
        assert_eq!(self.billable, 0, "reconstruction must not bill: {self:?}");
    }
}

fn configure_gc_memory(config: &mut GolemConfig) {
    config.memory.system_memory_override = Some(1024 * MIB);
    config.memory.worker_memory_ratio = 1.0;
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
        Arc::new(SharedResourceLimits(entry)),
        Arc::new(configure_gc_memory),
    )
    .await
}

/// Starts an executor with the test worker context needed by preparation probes.
async fn start_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(configure_gc_memory)),
            ..Default::default()
        },
    )
    .await
}

fn memory_history(entries: &[PublicOplogEntryWithIndex]) -> (u64, u64) {
    let mut growth = 0;
    let mut baseline = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Create(params) => baseline = params.initial_total_linear_memory_size,
            PublicOplogEntry::GrowMemory(params) => {
                growth += params.delta;
                baseline += params.delta;
            }
            PublicOplogEntry::MemorySizeReconciled(params) => baseline = params.bytes,
            _ => {}
        }
    }
    (growth, baseline)
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

fn constructor_key(entries: &[PublicOplogEntryWithIndex]) -> InvocationId {
    let keys: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
                PublicAgentInvocation::AgentInitialization(invocation) => {
                    Some(invocation.invocation_id.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 1, "expected exactly one constructor start");
    keys.into_iter().next().unwrap()
}

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex], expected: &[InvocationId]) {
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
                        &invocation.invocation_id
                    }
                    PublicAgentInvocation::AgentMethodInvocation(invocation) => {
                        &invocation.invocation_id
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
            let (total, _) =
                memory_history(&executor.get_oplog(agent_id, OplogIndex::INITIAL).await?);
            if total >= at_least {
                return Ok(total);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

async fn settled_memory_history(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    worker: &Arc<Worker<TestWorkerCtx>>,
    expected_bytes: u64,
    expected_growth: Option<u64>,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // Lifecycle growth requests can reach the status actor after invocation success.
            // Join the real commit/fold and wait until both persisted observations catch up.
            worker
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            let errors = errors(&entries);
            assert!(errors.is_empty(), "unexpected errors: {errors:?}");
            let (growth, baseline) = memory_history(&entries);
            let metadata = executor.get_worker_metadata(worker_id).await?;
            if baseline == expected_bytes
                && metadata.total_linear_memory_size == expected_bytes
                && expected_growth.is_none_or(|expected| growth == expected)
            {
                return Ok(entries);
            }
            tokio::task::yield_now().await;
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
async fn gc_collection_reuses_released_memory_at_the_agent_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_memory_limit(deps, &context, (32 * MIB) as usize).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "released-memory");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    for _ in 0..200 {
        let retained = executor
            .invoke_and_await_agent(&component, &agent, "retain", data_value!(8u32 * 1024))
            .await?;
        assert_eq!(retained.into_typed::<u32>()?, 8 * 1024);
        executor
            .invoke_and_await_agent(&component, &agent, "release", data_value!())
            .await?;
    }
    let retained = executor
        .invoke_and_await_agent(&component, &agent, "retained_kib", data_value!())
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 0);
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    assert!(errors(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?).is_empty());
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
    let mut control_keys = vec![
        constructor_key(&control_entries),
        executor
            .external_invocation_key(&control_id, &control_read, "retained_kib")
            .await?,
    ];
    let mut grown_keys = vec![
        constructor_key(&grown_entries),
        executor
            .external_invocation_key(&grown_id, &grown_read, "retained_kib")
            .await?,
        executor
            .external_invocation_key(&grown_id, &retain_key, "retain")
            .await?,
    ];
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
        keys.push(
            executor
                .external_invocation_key(worker_id, &key, "retained_kib")
                .await?,
        );
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
    assert_eq!(memory_history(&oplog).0, grown_growth);
    let oplog = executor.get_oplog(&control_id, OplogIndex::INITIAL).await?;
    assert_eq!(memory_history(&oplog).0, control_growth);

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
    grown_keys.push(
        executor
            .external_invocation_key(&grown_id, &fresh_key, "retain")
            .await?,
    );
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
    let mut expected_keys = vec![
        constructor_key(&entries),
        executor
            .external_invocation_key(&worker_id, &retain_key, "retain")
            .await?,
    ];
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
        expected_keys.push(
            executor
                .external_invocation_key(&worker_id, &read_key, "retained_kib")
                .await?,
        );
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
    let (growth_before, _) = memory_history(&entries_before);
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
    expected_keys.push(
        executor
            .external_invocation_key(&worker_id, &fresh_key, "retain")
            .await?,
    );
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
    expected_keys.push(
        executor
            .external_invocation_key(&worker_id, &read_key, "retained_kib")
            .await?,
    );
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

#[test]
#[timeout("3m")]
async fn reconstruction_reconciles_real_gc_capacity_in_both_directions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_executor(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "real-capacity-reconstruction");
    let worker_id = AgentId::from_agent_id(component.id, &agent).map_err(anyhow::Error::msg)?;
    let (initial_tx, initial_rx) = tokio::sync::oneshot::channel();
    executor.probe_next_instance_preparation(
        &worker_id,
        |_| Box::pin(async { Ok(()) }),
        move |store| {
            Box::pin(async move {
                let worker = store.data().get_public_state().worker();
                let tracker = store.data().durable_ctx().linear_memory_tracker();
                let _ = initial_tx.send((worker, tracker));
                Ok(())
            })
        },
    );
    assert_eq!(
        executor.start_agent(&component.id, agent.clone()).await?,
        worker_id
    );
    let retained = executor
        .invoke_and_await_agent(&component, &agent, "retain", data_value!(16u32 * 1024))
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 16 * 1024);
    let (initial_worker, initial_tracker) = initial_rx.await?;
    // This stable tracker value fences the initial sample only. Reconstruction assertions
    // below use independently enumerated Store allocation, not this tracker value.
    let initial_bytes = initial_tracker.current_bytes();
    let entries =
        settled_memory_history(&executor, &worker_id, &initial_worker, initial_bytes, None).await?;
    assert_eq!(initial_tracker.current_bytes(), initial_bytes);
    let (initial_growth, _) = memory_history(&entries);
    let initial_starts = entries
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .count();
    drop(initial_worker);
    drop(initial_tracker);
    drop(executor);

    let mut allocations = Vec::new();
    for (generation, preallocation) in [MIB, 128 * MIB, MIB].into_iter().enumerate() {
        let executor = start_executor(deps, &context).await?;
        let (preallocated_tx, preallocated_rx) = tokio::sync::oneshot::channel();
        let (prepared_tx, prepared_rx) = tokio::sync::oneshot::channel();
        let observer = executor.clone();
        let observed_id = worker_id.clone();
        executor.probe_next_instance_preparation(
            &worker_id,
            move |store| {
                Box::pin(async move {
                    let before = store.gc_heap_capacity() as u64;
                    store
                        .gc_heap_grow_async(preallocation)
                        .await
                        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                    let after = store.gc_heap_capacity() as u64;
                    assert!(after >= before + preallocation, "{before} -> {after}");
                    assert_eq!(
                        store
                            .data()
                            .durable_ctx()
                            .linear_memory_tracker()
                            .billable_bytes(),
                        0
                    );
                    let _ = preallocated_tx.send((before, after));
                    Ok(())
                })
            },
            move |store| {
                Box::pin(async move {
                    let observed = MemoryObservation::read(store);
                    observed.assert_prepared();
                    let worker = store.data().get_public_state().worker();
                    let tracker = store.data().durable_ctx().linear_memory_tracker();
                    // Install after replay so a historical success cannot consume this gate.
                    let gate = observer.gate_next_agent_invocation_success(&observed_id);
                    let _ = prepared_tx.send((observed, worker, tracker, gate));
                    Ok(())
                })
            },
        );

        let invocation_executor = executor.clone();
        let invocation_component = component.clone();
        let invocation_agent = agent.clone();
        let invocation = tokio::spawn(async move {
            invocation_executor
                .invoke_and_await_agent(
                    &invocation_component,
                    &invocation_agent,
                    "retained_kib",
                    data_value!(),
                )
                .await
        });
        let (_, preallocated) = preallocated_rx.await?;
        let (observed, worker, tracker, mut gate) = prepared_rx.await?;
        assert!(observed.gc >= preallocated, "{observed:?}");
        gate.entered().await;
        // The ordinary live guest call activates usage. Nothing in the test changes meter state.
        assert_eq!(tracker.current_bytes(), observed.allocated());
        assert_eq!(tracker.reserved_bytes(), observed.allocated());
        assert_eq!(tracker.billable_bytes(), observed.allocated());

        gate.release();
        assert_eq!(invocation.await??.into_typed::<u32>()?, 16 * 1024);
        let entries = settled_memory_history(
            &executor,
            &worker_id,
            &worker,
            observed.allocated(),
            Some(initial_growth),
        )
        .await?;
        let (growth, baseline) = memory_history(&entries);
        assert_eq!(
            growth, initial_growth,
            "replay must not append additive growth"
        );
        assert_eq!(baseline, observed.allocated());
        assert_eq!(
            executor
                .get_worker_metadata(&worker_id)
                .await?
                .total_linear_memory_size,
            baseline
        );
        let starts = entries
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .count();
        assert_eq!(
            starts,
            initial_starts + generation + 1,
            "replay must not append invocation starts"
        );
        allocations.push(observed);
        drop(worker);
        drop(tracker);
        drop(executor);
    }
    assert!(allocations[1].gc > allocations[0].gc, "{allocations:?}");
    assert!(allocations[1].gc > allocations[2].gc, "{allocations:?}");
    assert_eq!(allocations[0].gc, allocations[2].gc, "{allocations:?}");
    assert_eq!(
        allocations[0].linear, allocations[1].linear,
        "{allocations:?}"
    );
    assert_eq!(
        allocations[1].linear, allocations[2].linear,
        "{allocations:?}"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn initializer_replay_cutover_counts_each_allocation_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_initializer")] gc_initializer: &PrecompiledComponent,
) -> anyhow::Result<()> {
    const READ_ONE: &str = "monotonic_clock::now";
    const READ_TWO: &str = "wall_clock::now";
    let context = TestContext::new(last_unique_id);
    let executor = start_executor(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_initializer)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "initializer-cutover");
    let worker_id = AgentId::from_agent_id(component.id, &agent).map_err(anyhow::Error::msg)?;
    let mut gate = executor.gate_next_call_end(&worker_id, true).await;
    let starter = executor.clone();
    let component_id = component.id;
    let starting_agent = agent.clone();
    let start =
        tokio::spawn(async move { starter.start_agent(&component_id, starting_agent).await });
    let (prefix, read_one_start) = loop {
        tokio::time::timeout(Duration::from_secs(15), gate.entered()).await?;
        let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let index = gate.start_index();
        if entries.iter().any(|entry| entry.oplog_index == index
            && matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name == READ_ONE))
        {
            break (entries, index);
        }
        let next = executor.gate_next_call_end(&worker_id, true).await;
        drop(gate);
        gate = next;
    };
    let prefix_tip = prefix.last().unwrap().oplog_index;
    assert!(matches!(&prefix.last().unwrap().entry,
        PublicOplogEntry::End(params) if params.start_index == read_one_start));
    assert!(!prefix.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(params) if params.function_name == READ_TWO)));
    assert!(
        !prefix.iter().any(|entry| matches!(
            entry.entry,
            PublicOplogEntry::GrowMemory(_) | PublicOplogEntry::AgentInvocationStarted(_)
        )),
        "{prefix:?}"
    );
    let (_, saved) = memory_history(&prefix);
    gate.abort();
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(gate);
    start.abort();
    let _ = start.await;
    drop(executor);

    let executor = start_executor(deps, &context).await?;
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    executor.probe_next_instance_preparation(
        &worker_id,
        |_| Box::pin(async { Ok(()) }),
        move |store| {
            Box::pin(async move {
                let observed = MemoryObservation::read(store);
                let worker = store.data().get_public_state().worker();
                let _ = observed_tx.send((observed, worker));
                Ok(())
            })
        },
    );
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
    // The fixture traverses the actual GC arrays and checks their sentinel bytes.
    assert_eq!(retained.into_typed::<u32>()?, 41 * 1024);
    let (observed, worker) = observed_rx.await?;
    assert!(observed.gc > 0, "{observed:?}");
    assert_eq!(observed.tracked, observed.allocated(), "{observed:?}");
    assert_eq!(observed.reserved, observed.allocated(), "{observed:?}");
    worker
        .commit_oplog_and_update_state(CommitLevel::Always)
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let recovery_baselines = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::MemorySizeReconciled(params) if entry.oplog_index > prefix_tip => {
                Some((entry.oplog_index, params.bytes))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recovery_baselines.len(), 1, "{entries:?}");
    let (baseline_index, baseline) = recovery_baselines[0];
    assert!(baseline > saved, "baseline={baseline}, saved={saved}");
    assert!(
        observed.allocated() > baseline,
        "{observed:?}, baseline={baseline}"
    );
    let read_two = entries
        .iter()
        .find(|entry| {
            matches!(&entry.entry,
        PublicOplogEntry::Start(params) if params.function_name == READ_TWO)
        })
        .unwrap();
    assert!(baseline_index < read_two.oplog_index);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(params) if params.function_name == READ_ONE))
            .count(),
        1
    );
    let deltas = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::GrowMemory(params) if entry.oplog_index > baseline_index => {
                Some(params.delta)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas, vec![observed.allocated() - baseline]);
    assert_eq!(memory_history(&entries).1, observed.allocated());
    assert_eq!(
        executor
            .get_worker_metadata(&worker_id)
            .await?
            .total_linear_memory_size,
        observed.allocated()
    );
    let mut expected = vec![
        constructor_key(&entries),
        executor
            .external_invocation_key(&worker_id, &read_key, "retained_kib")
            .await?,
    ];
    assert_settled_history(&entries, &expected);
    let cap = observed.allocated();
    assert!(cap < observed.allocated() + baseline - saved);
    drop(worker);
    drop(executor);

    let executor = start_with_memory_limit(deps, &context, cap as usize).await?;
    let key = IdempotencyKey::fresh();
    let retained = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "retained_kib", data_value!())
        .await?;
    assert_eq!(retained.into_typed::<u32>()?, 41 * 1024);
    expected.push(
        executor
            .external_invocation_key(&worker_id, &key, "retained_kib")
            .await?,
    );
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_settled_history(&entries, &expected);
    assert_eq!(memory_history(&entries).1, cap);
    Ok(())
}

#[test]
#[timeout("2m")]
async fn real_host_gc_growth_uses_the_durable_limiter_hint_path(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("gc_heap")] gc_heap: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_executor(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, gc_heap)
        .store()
        .await?;
    let agent = agent_id!("GcHeapAgent", "real-host-gc-hint");
    let worker_id = AgentId::from_agent_id(component.id, &agent).map_err(anyhow::Error::msg)?;
    let (initial_tx, initial_rx) = tokio::sync::oneshot::channel();
    executor.probe_next_instance_preparation(
        &worker_id,
        |_| Box::pin(async { Ok(()) }),
        move |store| {
            Box::pin(async move {
                let worker = store.data().get_public_state().worker();
                let tracker = store.data().durable_ctx().linear_memory_tracker();
                let _ = initial_tx.send((worker, tracker));
                Ok(())
            })
        },
    );
    assert_eq!(
        executor.start_agent(&component.id, agent.clone()).await?,
        worker_id
    );
    executor
        .invoke_and_await_agent(&component, &agent, "retain", data_value!(1024u32))
        .await?;
    let (initial_worker, initial_tracker) = initial_rx.await?;
    let initial_bytes = initial_tracker.current_bytes();
    let entries =
        settled_memory_history(&executor, &worker_id, &initial_worker, initial_bytes, None).await?;
    assert_eq!(initial_tracker.current_bytes(), initial_bytes);
    let (initial_growth, _) = memory_history(&entries);
    drop(initial_worker);
    drop(initial_tracker);
    drop(executor);

    let executor = start_executor(deps, &context).await?;
    let observer = executor.clone();
    let observed_id = worker_id.clone();
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    executor.probe_next_instance_preparation(
        &worker_id,
        |_| Box::pin(async { Ok(()) }),
        move |store| {
            Box::pin(async move {
                let before = MemoryObservation::read(store);
                before.assert_prepared();
                // Preparation has published live accounting. Real Wasmtime GC growth now invokes
                // TestWorkerCtx's production DurableResourceLimiter, including its growth hint.
                store
                    .gc_heap_grow_async(64 * MIB)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let after = MemoryObservation::read(store);
                after.assert_prepared();
                assert_eq!(before.linear, after.linear);
                assert!(after.gc >= before.gc + 64 * MIB);
                let worker = store.data().get_public_state().worker();
                let tracker = store.data().durable_ctx().linear_memory_tracker();
                let gate = observer.gate_next_agent_invocation_success(&observed_id);
                let _ = observed_tx.send((before, after, worker, tracker, gate));
                Ok(())
            })
        },
    );
    let invocation_executor = executor.clone();
    let invocation = tokio::spawn(async move {
        invocation_executor
            .invoke_and_await_agent(&component, &agent, "retained_kib", data_value!())
            .await
    });
    let (before, after, worker, tracker, mut gate) = observed_rx.await?;
    gate.entered().await;
    assert_eq!(tracker.current_bytes(), after.allocated());
    assert_eq!(tracker.reserved_bytes(), after.allocated());
    assert_eq!(tracker.billable_bytes(), after.allocated());
    gate.release();
    assert_eq!(invocation.await??.into_typed::<u32>()?, 1024);
    let entries = settled_memory_history(
        &executor,
        &worker_id,
        &worker,
        after.allocated(),
        Some(initial_growth + after.gc - before.gc),
    )
    .await?;
    let (growth, baseline) = memory_history(&entries);
    assert_eq!(growth - initial_growth, after.gc - before.gc);
    assert_eq!(baseline, after.allocated());
    assert_eq!(
        executor
            .get_worker_metadata(&worker_id)
            .await?
            .total_linear_memory_size,
        baseline
    );
    Ok(())
}
