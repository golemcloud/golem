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

//! Read-only method tests — issue #3393.
//!
//! Exercises the `ReadonlyAgent` / `ReadonlyCaller` agents in the shared
//! `agent_sdk_rust` test component. Covers caching, side-effect traps and
//! oplog effects of `#[read_only]` methods, both over the executor API and
//! over RPC.

use crate::Tracing;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{TestDsl, count_agent_invocation_pair_since};
use golem_wasm::Value;
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const AGENT_TYPE: &str = "ReadonlyAgent";
const CALLER_TYPE: &str = "ReadonlyCaller";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn as_u64(value: &Value) -> u64 {
    match value {
        Value::U64(v) => *v,
        other => panic!("expected u64, got {other:?}"),
    }
}

fn as_u32(value: &Value) -> u32 {
    match value {
        Value::U32(v) => *v,
        other => panic!("expected u32, got {other:?}"),
    }
}

/// True if the error chain mentions a `ReadOnlyViolation` for the given
/// agent-method name. We match on the rendered string because the executor
/// API surfaces the typed error through `Debug` on `WorkerExecutorError`.
fn is_read_only_violation(err: &anyhow::Error, method: &str) -> bool {
    let s = format!("{err:?}");
    s.contains("ReadOnlyViolation") && s.contains(method)
}

/// Wait until the worker's oplog has a matching number of
/// `AgentInvocationStarted` and `AgentInvocationFinished` entries.
///
/// `start_agent` returns once the agent constructor has been enqueued, but the
/// `AgentInvocationStarted/Finished` pair for the initialization invocation
/// may still be in the process of being persisted to the oplog. Subsequent
/// `oplog_max_index()` snapshots can therefore land *between* the Started and
/// Finished entries of initialization, which leads to spurious off-by-one
/// errors in tests that count Started/Finished pairs after a baseline. Call
/// this helper before snapshotting to make those tests deterministic.
async fn wait_oplog_settled<E: TestDsl + Sync>(
    executor: &E,
    worker_id: &AgentId,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let (started, finished) = count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL);
        if started >= 1 && started == finished {
            return Ok(());
        }
        if Instant::now() > deadline {
            panic!(
                "agent did not settle within 10s: started={started} finished={finished} \
                 (expected at least one matched Started/Finished pair from initialization)"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// T1 — read-only on a fresh agent returns the correct value
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t1_read_only_returns_value_on_fresh_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t1-{unique_id}"));
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");

    assert_eq!(as_u64(&result), 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// T2 — back-to-back read-only calls: second one is a cache hit
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t2_read_only_caches_until_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t2-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First read: cache miss, fills cache via the detached observer.
    let first = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&first), 0);

    // Give the observer task a moment to populate the cache; the observer
    // runs in a detached `tokio::spawn` after invocation completion.
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    Ok(())
}

/// Polls `get_count` until two back-to-back reads cause the oplog to grow by
/// zero `AgentInvocationStarted` entries — i.e. the second read was served
/// from the read-only cache.
async fn wait_for_cache_hit<E: TestDsl + Sync>(
    executor: &E,
    component: &golem_common::model::component::ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
    worker_id: &AgentId,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let before = executor.oplog_max_index(worker_id).await?;
        let _ = executor
            .invoke_and_await_agent(component, agent_id, "get_count", data_value!())
            .await?
            .into_return_value()
            .expect("expected return value");
        let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let (started, _) = count_agent_invocation_pair_since(&entries, before);
        if started == 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            panic!(
                "expected a read-only cache hit within 5s; last `get_count` still recorded a new \
                 AgentInvocationStarted entry"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// T3 — write between reads invalidates the cache
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t3_read_only_invalidates_after_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t3-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Warm the read-only cache.
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?;
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    // Mutating call — bumps the read-only cache epoch.
    let after_increment = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&after_increment), 1);

    // Snapshot the oplog and read again; the post-write read must miss the
    // cache and produce exactly one Started/Finished pair on the oplog.
    let before = executor.oplog_max_index(&worker_id).await?;
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&result), 1);

    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(started, 1, "expected exactly one new Started entry");
    assert_eq!(finished, 1, "expected exactly one new Finished entry");

    Ok(())
}

// ---------------------------------------------------------------------------
// T4 — read-only bypasses queue during slow_increment (HEADLINE)
// ---------------------------------------------------------------------------

/// Headline test from issue #3393: a slow non-readonly call holds the queue
/// while a read-only call returns immediately from cache.
///
/// The read-only cache epoch is bumped on *successful completion* of a
/// mutating invocation rather than on enqueue (see
/// [`DurableWorkerCtx::on_agent_invocation_success`] in
/// `golem-worker-executor/src/durable_host/mod.rs`), so a previously-warmed
/// cache entry stays serviceable while a slow `slow_increment(2000)` is
/// queued/running. Foreground `get_count` is then served from the cache and
/// returns within the deadline.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t4_read_only_bypasses_queue_during_slow_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t4-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Warm the read-only cache so a later cache lookup would hit if the
    // mutating enqueue did not invalidate it.
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?;
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    // Background slow mutation.
    let mutator = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "slow_increment",
                    data_value!(2000u64),
                )
                .await
        })
    };

    // Give the mutating enqueue a moment to land.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Foreground read-only call must come back from the cache within 50ms.
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(50),
        executor.invoke_and_await_agent(&component, &agent_id, "get_count", data_value!()),
    )
    .await
    .expect("read-only call must return within 50ms even while slow_increment holds the queue")?
    .into_return_value()
    .expect("expected return value");

    assert!(
        started.elapsed() < Duration::from_millis(50),
        "read-only call took {:?}",
        started.elapsed()
    );
    assert_eq!(as_u64(&result), 0, "must return pre-write value");

    let _ = mutator.await;

    Ok(())
}

// ---------------------------------------------------------------------------
// T5 — read-only cache survives wasmtime instance eviction
// ---------------------------------------------------------------------------

/// Issue #3393 §10 "Bypassing agent loading on cache hit":
///
/// > The cache lives on `Worker`, which exists independently of whether the
/// > wasmtime instance is loaded. If the agent is currently evicted but a
/// > `Worker` shell remains with a populated cache, a read-only call
/// > returning a cache hit must **not** trigger instance loading.
///
/// Instead of using a test-only forced-eviction hook, we drive the
/// **production memory-pressure eviction path**: the executor is started with
/// a tight worker-memory budget so that loading a second worker forces the
/// first one to be unloaded by `ActiveWorkers::try_free_up_memory`. The
/// `Worker` shell (and its read-only cache) stays alive in `ActiveWorkers`.
/// Then we issue another `get_count` on the evicted worker and assert:
///   1. it returns the cached value without recording a new
///      `AgentInvocationStarted/Finished` pair on the oplog (cache hit), and
///   2. the wasmtime instance is **not** reloaded — `worker_is_loaded(R)`
///      remains `false`.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t5_read_only_bypasses_agent_loading(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    // Tight memory budget: enough for one agent_sdk_rust worker, not two.
    // - `worker_memory_ratio = 1.0` makes the worker pool == system_memory_override
    //   so the math is easy to reason about.
    // - 6 MiB has been chosen experimentally so that one Rust agent_sdk
    //   worker (whose `Worker::memory_requirement()` is ~4 MiB) fits while
    //   two do not. The assertions below verify the calibration is still
    //   valid — if the component's linear memory grows or shrinks materially,
    //   the test will fail with a clear diagnostic instead of silently
    //   passing or hanging.
    const SYSTEM_MEMORY: u64 = 6 * 1024 * 1024;
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.memory.system_memory_override = Some(SYSTEM_MEMORY);
            config.memory.worker_memory_ratio = 1.0;
            // Tight `acquire_retry_delay` so Q's permit-wait doesn't hang the
            // test if memory pressure eviction takes a moment to settle.
            config.memory.acquire_retry_delay = Duration::from_millis(25);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    // R: the worker whose read-only cache we want to keep alive across eviction.
    let unique_id = context.redis_prefix();
    let r_agent_id = agent_id!(AGENT_TYPE, format!("t5-r-{unique_id}"));
    let r_worker_id = executor
        .start_agent(&component.id, r_agent_id.clone())
        .await?;
    let r_owned = OwnedAgentId::new(context.default_environment_id, &r_worker_id);

    // Warm R's read-only cache: cache miss + populate via the detached observer.
    let _ = executor
        .invoke_and_await_agent(&component, &r_agent_id, "get_count", data_value!())
        .await?;
    wait_for_cache_hit(&executor, &component, &r_agent_id, &r_worker_id).await?;

    // Sanity-check the memory budget. The test only makes sense when
    // `req <= budget < 2 * req`, which is the regime that forces Q's load to
    // evict R but lets R load standalone.
    let r_req = executor.worker_memory_requirement(&r_owned).await?;
    assert!(
        r_req <= SYSTEM_MEMORY,
        "memory budget too tight: budget={SYSTEM_MEMORY} but R needs {r_req}. Increase SYSTEM_MEMORY."
    );
    assert!(
        2 * r_req > SYSTEM_MEMORY,
        "memory budget too generous: budget={SYSTEM_MEMORY}, R req={r_req}, 2*req={} fits inside budget so loading Q would not evict R. \
         Decrease SYSTEM_MEMORY (or the component grew significantly).",
        2 * r_req
    );

    // Wait until R is `LoadedIdle` — only then will `try_free_up_memory`
    // pick it as an eviction candidate.
    wait_for_eviction_class(&executor, &r_owned, EvictionClass::LoadedIdle).await?;
    assert!(executor.worker_is_loaded(&r_owned).await);

    // Q: a second worker of the same agent type. Loading Q reserves another
    // `r_req` from the worker memory semaphore, which forces eviction of R.
    let q_agent_id = agent_id!(AGENT_TYPE, format!("t5-q-{unique_id}"));
    let q_worker_id = executor
        .start_agent(&component.id, q_agent_id.clone())
        .await?;
    let q_owned = OwnedAgentId::new(context.default_environment_id, &q_worker_id);

    // Trigger Q's instance load. The invocation forces the worker out of
    // `Unloaded`, which acquires memory from `ActiveWorkers` and runs
    // `try_free_up_memory` if necessary.
    let q_result = tokio::time::timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(&component, &q_agent_id, "get_count", data_value!()),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "Q startup did not complete within 10s. Likely cause: memory budget miscalibrated \
             (R never evicted) — check that `R req = {r_req}` plus the second copy actually \
             exceeds budget {SYSTEM_MEMORY}, and that R reached `LoadedIdle` before Q started."
        )
    })??
    .into_return_value()
    .expect("expected return value");
    assert_eq!(as_u64(&q_result), 0);

    // The production eviction path should have unloaded R while keeping its
    // `Worker` shell (and read-only cache) resident. Poll briefly because
    // unloading is best-effort with respect to scheduling.
    wait_until(Duration::from_secs(5), || async {
        !executor.worker_is_loaded(&r_owned).await
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "R was not unloaded by the production memory-pressure path after Q started. \
             Eviction candidate selection may have changed, or another loaded worker \
             absorbed the eviction."
        )
    })?;
    assert!(!executor.worker_is_loaded(&r_owned).await);
    assert!(executor.worker_is_loaded(&q_owned).await);

    // Cache-hit invariant: a `get_count` on R after eviction must not produce
    // a new Started/Finished pair on the oplog ...
    let before = executor.oplog_max_index(&r_worker_id).await?;
    let result = executor
        .invoke_and_await_agent(&component, &r_agent_id, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&result), 0);

    let entries = executor
        .get_oplog(&r_worker_id, OplogIndex::INITIAL)
        .await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 0,
        "post-eviction read-only call must be served from cache (no new Started entry, \
         got {started} started / {finished} finished)"
    );

    // ... and the wasmtime instance must not have been reloaded.
    assert!(
        !executor.worker_is_loaded(&r_owned).await,
        "post-eviction read-only cache hit must not trigger a new wasmtime instance load"
    );

    Ok(())
}

/// Poll until `worker_eviction_class(owned)` matches `expected`, or fail
/// after 5s. The window between "start_agent returned" and the worker
/// becoming `LoadedIdle` covers instance startup and the initial invocation
/// settling.
async fn wait_for_eviction_class(
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    owned: &OwnedAgentId,
    expected: EvictionClass,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if executor.worker_eviction_class(owned).await == Some(expected) {
            return Ok(());
        }
        if Instant::now() > deadline {
            anyhow::bail!(
                "worker {owned} did not reach EvictionClass::{expected:?} within 5s \
                 (current class: {:?})",
                executor.worker_eviction_class(owned).await
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll `predicate` every 25ms until it returns `true`, or fail after `timeout`.
async fn wait_until<F, Fut>(timeout: Duration, mut predicate: F) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate().await {
            return Ok(());
        }
        if Instant::now() > deadline {
            anyhow::bail!("predicate did not become true within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// T6 — per-principal cache partitioning
// ---------------------------------------------------------------------------

/// Issue #3393 T6: `uses_principal = true` methods (the SDK auto-derives this
/// from the presence of a `Principal` parameter) must cache independently per
/// principal, while `uses_principal = false` methods share a single cache
/// entry regardless of which principal invokes them.
///
/// The agent exposes:
///   - `get_count_for(_principal: Principal) -> u64` — `#[read_only]` with
///     `uses_principal = true` (derived from the `Principal` parameter).
///   - `get_count() -> u64` — `#[read_only]` with `uses_principal = false`.
///
/// Both return the same counter, so the test asserts on oplog deltas: a cache
/// miss writes one `AgentInvocationStarted/Finished` pair; a cache hit writes
/// nothing.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn t6_principal_aware_caches_per_principal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{GolemUserPrincipal, Principal};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t6-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    // Two distinct principals — same shape, different account IDs.
    // `Principal` is auto-injected from `InvokeAgentRequest.principal` (the
    // SDK macro derives `uses_principal = true` from the `Principal`
    // parameter), so `method_parameters` stays `data_value!()` below.
    let principal_a = Principal::GolemUser(GolemUserPrincipal {
        account_id: AccountId::new(),
    });
    let principal_b = Principal::GolemUser(GolemUserPrincipal {
        account_id: AccountId::new(),
    });
    assert_ne!(
        principal_a, principal_b,
        "test bug: principals must differ to exercise per-principal partitioning"
    );

    // -- get_count_for: uses_principal = true ------------------------------

    // First read as A: cache miss → one Started/Finished pair.
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal_a.clone(),
            "get_count_for",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("expected return value");
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started_a, finished_a) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started_a, 1,
        "first read as principal A must miss the cache (got {started_a} started)"
    );
    assert_eq!(finished_a, 1);

    // Wait for the detached observer to populate the cache for principal A.
    wait_for_principal_cache_hit(&executor, &component, &agent_id, &worker_id, &principal_a)
        .await?;

    // First read as B: must also miss because the cache key is partitioned
    // by principal_digest (see `build_read_only_cache_key`).
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal_b.clone(),
            "get_count_for",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("expected return value");
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started_b, finished_b) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started_b, 1,
        "first read as principal B must miss the cache (got {started_b} started)"
    );
    assert_eq!(finished_b, 1);

    // Wait for the cache to populate for principal B.
    wait_for_principal_cache_hit(&executor, &component, &agent_id, &worker_id, &principal_b)
        .await?;

    // Second read as A must hit the cache (still partitioned, not evicted).
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal_a.clone(),
            "get_count_for",
            data_value!(),
        )
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started_a2, _) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started_a2, 0,
        "second read as principal A must hit the cache (got {started_a2} started)"
    );

    // -- get_count: uses_principal = false ---------------------------------

    // First `get_count` as A: cache miss → one Started/Finished pair.
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal_a.clone(),
            "get_count",
            data_value!(),
        )
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started_gc_a, _) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started_gc_a, 1,
        "first `get_count` (any principal) must miss the cache (got {started_gc_a} started)"
    );

    // Wait for the cache to populate (using anonymous because get_count is
    // principal-unaware — any principal works).
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    // `get_count` as B must hit even though the cache was warmed as A,
    // because `uses_principal = false` means the principal is not part of
    // the cache key.
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal_b.clone(),
            "get_count",
            data_value!(),
        )
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started_gc_b, _) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started_gc_b, 0,
        "second `get_count` (different principal, same cache key) must hit the cache \
         (got {started_gc_b} started)"
    );

    Ok(())
}

/// Polls until `get_count_for` as `principal` is served from the per-principal
/// read-only cache (no new `AgentInvocationStarted` entries on the oplog).
async fn wait_for_principal_cache_hit<E: TestDsl + Sync>(
    executor: &E,
    component: &golem_common::model::component::ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
    worker_id: &AgentId,
    principal: &golem_common::model::agent::Principal,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let before = executor.oplog_max_index(worker_id).await?;
        let _ = executor
            .invoke_and_await_agent_as_principal(
                component,
                agent_id,
                principal.clone(),
                "get_count_for",
                data_value!(),
            )
            .await?;
        let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let (started, _) = count_agent_invocation_pair_since(&entries, before);
        if started == 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            panic!(
                "expected a per-principal read-only cache hit within 5s for principal {principal:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// T7 — TTL cache entries expire
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t7_ttl_expires_cache_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t7-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First read: miss + populate cache (TTL = 2s).
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "read_only_with_ttl", data_value!())
        .await?;

    // Wait until the cache has been populated by polling for a cache hit
    // (a no-Started-entry second call).
    wait_for_ttl_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    // Wait beyond the TTL.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // After TTL expiry, the next call must miss again — verified via the
    // oplog growing by exactly one Started/Finished pair.
    let before = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "read_only_with_ttl", data_value!())
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started, _) = count_agent_invocation_pair_since(&entries, before);
    assert!(
        started >= 1,
        "TTL-expired entry must re-run: expected >= 1 Started entry, got {started}"
    );

    Ok(())
}

async fn wait_for_ttl_cache_hit<E: TestDsl + Sync>(
    executor: &E,
    component: &golem_common::model::component::ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
    worker_id: &AgentId,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let before = executor.oplog_max_index(worker_id).await?;
        let _ = executor
            .invoke_and_await_agent(component, agent_id, "read_only_with_ttl", data_value!())
            .await?;
        let entries = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let (started, _) = count_agent_invocation_pair_since(&entries, before);
        if started == 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            panic!("TTL cache did not populate within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// T8 — no_cache runs every time
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t8_no_cache_runs_every_time(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t8-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    let before = executor.oplog_max_index(&worker_id).await?;

    for _ in 0..2 {
        let r = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "pure_compute",
                data_value!(2u32, 3u32),
            )
            .await?
            .into_return_value()
            .expect("expected return value");
        assert_eq!(as_u32(&r), (2u32.wrapping_add(3)).wrapping_mul(3));
    }

    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 2,
        "no_cache: expected 2 Started entries, got {started}"
    );
    assert_eq!(
        finished, 2,
        "no_cache: expected 2 Finished entries, got {finished}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// T9 — read-only violations trap with `ReadOnlyViolation`
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t9_bad_write_traps(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t9w-{unique_id}"));
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "bad_write", data_value!())
        .await;

    let err = result.expect_err("bad_write must trap");
    assert!(
        is_read_only_violation(&err, "bad_write"),
        "expected ReadOnlyViolation for `bad_write`, got: {err:?}"
    );

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t9_bad_remote_read_traps(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t9r-{unique_id}"));
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "bad_remote_read", data_value!())
        .await;

    let err = result.expect_err("bad_remote_read must trap");
    assert!(
        is_read_only_violation(&err, "bad_remote_read"),
        "expected ReadOnlyViolation for `bad_remote_read`, got: {err:?}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// T10 — cache miss writes one Started/Finished pair; cache hit writes none
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t10_read_only_oplog_markers_present_when_executed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t10-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    // Miss: must produce exactly one Started/Finished pair on the oplog.
    let before_miss = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (miss_started, miss_finished) = count_agent_invocation_pair_since(&entries, before_miss);
    assert_eq!(miss_started, 1, "miss must produce one Started entry");
    assert_eq!(miss_finished, 1, "miss must produce one Finished entry");

    // Wait for the cache observer to populate, then verify a subsequent
    // cache-hit invocation writes nothing to the oplog.
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    let before_hit = executor.oplog_max_index(&worker_id).await?;
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (hit_started, hit_finished) = count_agent_invocation_pair_since(&entries, before_hit);
    assert_eq!(hit_started, 0, "hit must NOT produce a Started entry");
    assert_eq!(hit_finished, 0, "hit must NOT produce a Finished entry");

    Ok(())
}

// ---------------------------------------------------------------------------
// T11 — concurrent first-time Await misses coalesce
// ---------------------------------------------------------------------------
//
// Issue #3393 T11 (and §14.7 of `read-only-agent-methods.md`): N concurrent
// identical first-time read-only Await calls share a single pending entry
// inside `Worker::invoke_and_await` and therefore produce exactly one
// `AgentInvocationStarted/Finished` pair on the oplog.
//
// Coalescing is implemented via
// `golem_common::cache::Cache::get_or_insert_simple` on the per-worker
// `read_only_cache`: the first caller runs the underlying invocation and
// every concurrent caller awaiting the same `ReadOnlyCacheKey` receives the
// same result.

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn t11_concurrent_misses_coalesce(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t11-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    const N: usize = 8;

    let before = executor.oplog_max_index(&worker_id).await?;

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        handles.push(tokio::spawn(async move {
            executor
                .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
                .await
        }));
    }
    let mut results = Vec::with_capacity(N);
    for h in handles {
        let dv = h
            .await??
            .into_return_value()
            .expect("expected return value");
        results.push(as_u64(&dv));
    }

    for r in &results {
        assert_eq!(*r, 0, "all concurrent reads must return the same value");
    }

    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 1,
        "expected exactly 1 Started entry (concurrent Await misses coalesce); got {started}"
    );
    assert_eq!(
        finished, 1,
        "expected exactly 1 Finished entry (concurrent Await misses coalesce); got {finished}"
    );

    // A subsequent read must also be served from the cache.
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// T11b — coalesced owner survives caller cancellation (regression)
// ---------------------------------------------------------------------------
//
// Issue #3393 T11/R3 coalescing uses
// `Cache::get_or_insert_simple_spawned`, which spawns the owner future via
// `tokio::task::spawn`. If the original Await caller drops mid-flight
// (e.g. its gRPC request is cancelled), the spawned owner must continue and
// resolve the pending cache entry — otherwise the key would be stuck
// Pending forever, blocking all subsequent Await callers.
//
// To make the test exercise the spawned cancellation path deterministically
// instead of relying on lucky timing, we use the read-only `slow_read(ms)`
// method, which sleeps for the requested number of milliseconds inside the
// agent before returning. That way the coalesced owner is guaranteed to
// still be pending when the original caller is aborted, and we can prove
// that a second Await — joining as a waiter on the same pending entry —
// still receives the value after the abort.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t11b_coalesced_owner_survives_caller_cancellation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t11b-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    // Read-only call that takes ~2 s on the guest. The coalesced owner
    // future is spawned by `Cache::get_or_insert_simple_spawned`, so when
    // we abort the caller below the owner future is still pending.
    let slow_ms: u64 = 2_000;
    let before = executor.oplog_max_index(&worker_id).await?;
    let cancelled = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(&component, &agent_id, "slow_read", data_value!(slow_ms))
                .await
        })
    };

    // Wait deterministically until the first request became the
    // coalescing owner: the oplog must show exactly one new
    // `AgentInvocationStarted` since the baseline and zero `Finished`.
    // Without this poll, a slow test machine could abort before the
    // request reached the coalescer and the second call would become the
    // actual owner — the test would still pass, but it would not be
    // exercising the cancellation path.
    let poll_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let (started, finished) = count_agent_invocation_pair_since(&entries, before);
        if started == 1 && finished == 0 {
            break;
        }
        if Instant::now() > poll_deadline {
            panic!(
                "first slow_read did not become coalescing owner within 5s: \
                 started={started}, finished={finished}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancelled.abort();
    let _ = cancelled.await;

    // A second Await with the same (method, args, principal, epoch) must
    // join the pending cache entry as a waiter and successfully receive
    // the value from the spawned owner — proving the spawned owner kept
    // running past caller abort. The timeout must comfortably exceed the
    // remaining `slow_ms` sleep, but a hung pending entry would block
    // forever without the cancellation-safe spawned owner.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(&component, &agent_id, "slow_read", data_value!(slow_ms)),
    )
    .await?
    .expect("second Await must not hang on an abandoned pending cache entry")
    .into_return_value()
    .expect("expected return value");
    // The agent's counter hasn't been touched, so `slow_read` returns 0.
    assert_eq!(as_u64(&result), 0);

    // After both calls settle, the oplog must show exactly one Started /
    // Finished pair — the second call must have joined the pending cache
    // entry as a waiter instead of enqueueing a second invocation. This
    // is the coalescing invariant exercised under cancellation.
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 1,
        "exactly one slow_read invocation must reach the queue; the second \
         caller must coalesce as a waiter (got {started} Started)"
    );
    assert_eq!(
        finished, 1,
        "the spawned owner must complete the queued invocation even after \
         the original caller is aborted (got {finished} Finished)"
    );

    // And the cache must work normally afterwards.
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// T11c — coalescing must NOT cache an idempotency replay (regression)
// ---------------------------------------------------------------------------
//
// `Worker::invoke_and_await` coalesces Await misses by spawning a single
// owner future per `ReadOnlyCacheKey`. The owner stores the resulting
// `AgentInvocationOutput` in the read-only cache under the current epoch.
// If the underlying `invoke_and_await_uncoalesced` returns an idempotency
// replay (a result recorded under a previous epoch), caching that under the
// current epoch key would poison future reads — fresh callers would receive
// the stale replayed value forever, even though subsequent mutations have
// changed the state.
//
// This regression test:
//   1. Runs `get_count` with idempotency key `K1` → 0; cache populated.
//   2. Triggers `increment` — bumps the per-worker read-only cache epoch.
//   3. Re-runs `get_count` with the SAME key `K1` — this is an idempotency
//      replay; it must return the originally recorded 0 *without* poisoning
//      the new epoch's read-only cache.
//   4. Runs `get_count` with a fresh key — must return the current value 1.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn t11c_coalescing_does_not_cache_idempotency_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::IdempotencyKey;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!(AGENT_TYPE, format!("t11c-{unique_id}"));
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &worker_id).await?;

    // 1. Initial read with a stable idempotency key — count is 0.
    let key = IdempotencyKey::fresh();
    let r1 = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&r1), 0);

    // 2. Mutation bumps the read-only cache epoch.
    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    // 3. Replay the original read with the SAME idempotency key.
    //    Idempotency replay must return the originally recorded 0 — and
    //    must NOT poison the post-mutation read-only cache.
    let r2 = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(
        as_u64(&r2),
        0,
        "replay of the original idempotency key must return the originally \
         recorded read-only value"
    );

    // 4. A fresh `get_count` call must observe the post-mutation value (1).
    //    If the coalescing path had inserted the replayed 0 under the new
    //    epoch's key, this read would hit that poisoned entry and return 0.
    let r3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(
        as_u64(&r3),
        1,
        "post-mutation read must reflect the new state (1); a poisoned \
         cache entry would have returned the stale replayed value (0)"
    );

    // 5. Warm the current-epoch read-only cache with the new value.
    wait_for_cache_hit(&executor, &component, &agent_id, &worker_id).await?;

    // 6. Replay the original idempotency key again, this time with a warm
    //    current-epoch cache. The replay must still return the originally
    //    recorded 0, not the cached current value 1 — i.e. the read-only
    //    cache HIT fast path must not shadow the recorded idempotency
    //    result.
    let r4 = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "get_count", data_value!())
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(
        as_u64(&r4),
        0,
        "idempotency replay must bypass the read-only cache fast path even \
         when the current epoch cache is warm"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// R1 — RPC read-only fast path
// ---------------------------------------------------------------------------

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn r1_rpc_read_only_uses_cache_fast_path(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let target_str = format!("r1-target-{unique_id}");
    let target_agent_id = agent_id!(AGENT_TYPE, target_str.clone());
    let target_worker_id = executor
        .start_agent(&component.id, target_agent_id.clone())
        .await?;

    let caller_agent_id = agent_id!(CALLER_TYPE, format!("r1-caller-{unique_id}"));
    let _caller_worker_id = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    // First RPC read — populates target cache.
    let r1 = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "read_via_rpc",
            data_value!(target_str.clone()),
        )
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&r1), 0);

    // Wait for cache to populate on the target.
    wait_for_cache_hit(&executor, &component, &target_agent_id, &target_worker_id).await?;

    // Second RPC read must hit the target's cache: no new Started entry.
    let before = executor.oplog_max_index(&target_worker_id).await?;
    let r2 = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "read_via_rpc",
            data_value!(target_str.clone()),
        )
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&r2), 0);
    let entries = executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;
    let (started, _) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 0,
        "second RPC read must be served from target cache (no new Started entry)"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// R2 — RPC read-only during slow target invocation (HEADLINE)
// ---------------------------------------------------------------------------

/// With the issue #3393 fix in place (epoch bumped on *successful completion*
/// of a mutating invocation, not on enqueue), the target's cached `get_count`
/// stays serviceable during the queued/running `slow_increment`, so the
/// embedded RPC read returns immediately from cache via the caller's
/// `slow_then_read` flow.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn r2_rpc_read_only_during_slow_target_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let target_str = format!("r2-target-{unique_id}");
    let target_agent_id = agent_id!(AGENT_TYPE, target_str.clone());
    let target_worker_id = executor
        .start_agent(&component.id, target_agent_id.clone())
        .await?;

    let caller_agent_id = agent_id!(CALLER_TYPE, format!("r2-caller-{unique_id}"));
    let _caller_worker_id = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    // Warm target cache so an embedded get_count would hit without the
    // slow_increment enqueue invalidating it.
    let _ = executor
        .invoke_and_await_agent(&component, &target_agent_id, "get_count", data_value!())
        .await?;
    wait_for_cache_hit(&executor, &component, &target_agent_id, &target_worker_id).await?;

    let started_at = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "slow_then_read",
            data_value!(target_str.clone(), 2000u64),
        ),
    )
    .await??
    .into_return_value()
    .expect("expected return value");

    assert!(
        started_at.elapsed() < Duration::from_millis(500),
        "slow_then_read took {:?} (cache bypass expected)",
        started_at.elapsed()
    );
    assert_eq!(as_u64(&result), 0, "must return pre-write value");

    Ok(())
}

// ---------------------------------------------------------------------------
// R3 — RPC parallel reads coalesce on the target's read-only cache
// ---------------------------------------------------------------------------
//
// Issue #3393 R3 (and §14.7 of `read-only-agent-methods.md`): N concurrent
// identical RPC read-only Await calls to the same target agent share a single
// pending entry inside the target worker's `invoke_and_await` and therefore
// produce exactly one `AgentInvocationStarted/Finished` pair on the target
// oplog. Each RPC invocation funnels through the target worker's
// `Worker::invoke_and_await`, which coalesces via
// `golem_common::cache::Cache::get_or_insert_simple`.

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn r3_rpc_parallel_reads_coalesce(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let target_str = format!("r3-target-{unique_id}");
    let target_agent_id = agent_id!(AGENT_TYPE, target_str.clone());
    let target_worker_id = executor
        .start_agent(&component.id, target_agent_id.clone())
        .await?;
    wait_oplog_settled(&executor, &target_worker_id).await?;

    let caller_agent_id = agent_id!(CALLER_TYPE, format!("r3-caller-{unique_id}"));
    let _caller_worker_id = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    // Snapshot the target's oplog before triggering the parallel batch so we
    // can count exactly how many Started/Finished pairs landed on it.
    let before_target = executor.oplog_max_index(&target_worker_id).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "parallel_reads",
            data_value!(target_str.clone(), 8u32),
        )
        .await?
        .into_return_value()
        .expect("expected return value");

    let values = match &result {
        Value::List(items) => items.iter().map(as_u64).collect::<Vec<_>>(),
        other => panic!("expected a list, got {other:?}"),
    };
    assert!(!values.is_empty());
    for v in &values {
        assert_eq!(*v, 0);
    }

    let entries = executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;
    let (started, finished) = count_agent_invocation_pair_since(&entries, before_target);
    assert_eq!(
        started, 1,
        "expected exactly 1 Started entry on the target (concurrent RPC Await misses coalesce); \
         got {started}"
    );
    assert_eq!(
        finished, 1,
        "expected exactly 1 Finished entry on the target; got {finished}"
    );

    // A subsequent RPC read must also be served from the target's cache.
    let before = executor.oplog_max_index(&target_worker_id).await?;
    let r = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "read_via_rpc",
            data_value!(target_str.clone()),
        )
        .await?
        .into_return_value()
        .expect("expected return value");
    assert_eq!(as_u64(&r), 0);
    let entries = executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;
    let (started, _) = count_agent_invocation_pair_since(&entries, before);
    assert_eq!(
        started, 0,
        "follow-up RPC read must hit the target's read-only cache (got {started} new Started)"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// R4 — read-only method cannot make RPC
// ---------------------------------------------------------------------------

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn r4_rpc_read_only_method_cannot_make_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let target_str = format!("r4-target-{unique_id}");
    let _target_agent_id = agent_id!(AGENT_TYPE, target_str.clone());
    let caller_agent_id = agent_id!(CALLER_TYPE, format!("r4-caller-{unique_id}"));
    let _caller_worker_id = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "bad_rpc",
            data_value!(target_str.clone()),
        )
        .await;

    let err = result.expect_err("bad_rpc must trap");
    assert!(
        is_read_only_violation(&err, "bad_rpc"),
        "expected ReadOnlyViolation for `bad_rpc`, got: {err:?}"
    );

    Ok(())
}
