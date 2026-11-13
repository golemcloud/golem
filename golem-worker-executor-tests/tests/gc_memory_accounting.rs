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
use golem_common::model::AgentId;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::services::HasWorker;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::WorkerCtx;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerCtx,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
};
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

async fn start_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.memory.system_memory_override = Some(1024 * MIB);
                config.memory.worker_memory_ratio = 1.0;
            })),
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
            PublicOplogEntry::Error(error) => panic!("unexpected error: {error:?}"),
            _ => {}
        }
    }
    (growth, baseline)
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
