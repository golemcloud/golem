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

//! Shard revocation on a running executor. `RevokeShards` and the drain leg of
//! `SetShardAssignment` must not return before every agent of the revoked shards has actually
//! left memory (the shard manager hands the shards to another executor the moment they return),
//! and they must stop those agents concurrently rather than one by one.

use crate::Tracing;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, RevokeShardsRequest, SetShardAssignmentRequest, assign_shards_response,
    revoke_shards_response, set_shard_assignment_response,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, OwnedAgentId, ShardId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{AgentResult, TestDsl, count_agent_invocation_pair_since};
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};
use tokio::task::JoinHandle;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

/// The in-process test executor owns the single shard `0` of one, so revoking it loses every
/// agent and assigning it back restores them all.
const SHARD: i64 = 0;
const AGENTS: usize = 5;

/// `revoke_shards` must not return until every agent of the revoked shard has left memory: if
/// it returned earlier, the shard manager would hand the shard to another executor while these
/// agents can still write to their oplogs.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revoke_shards_returns_only_after_lost_agents_are_unloaded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agents = start_busy_agents(&executor, &context, &component, "revoke-drain").await?;

    let drain_elapsed = revoke_shards(&executor, &[SHARD]).await?;

    assert_all_unloaded(
        &executor,
        agents.iter().map(|agent| &agent.owned_id),
        "revoke_shards",
        drain_elapsed,
    )
    .await;

    assign_shards(&executor, &[SHARD]).await?;
    assert_invocations_completed_once(&executor, agents).await?;

    drop(executor);
    Ok(())
}

/// The drain signals every lost agent first and waits for all of them together, so its duration
/// is the slowest agent's teardown rather than the sum of all teardowns. Each agent's teardown
/// commit is delayed artificially; the recorded commit intervals overlap under a concurrent
/// drain and cannot overlap under a sequential one (agent `k+1` is only signalled after agent
/// `k` has finished its teardown).
///
/// The delayed commit happens after the interrupt acknowledgement, so the overlap assertion on
/// its own would also be satisfied by a drain that returns without waiting at all; the unload
/// assertion before it (the same one `revoke_shards_returns_only_after_lost_agents_are_unloaded`
/// makes) is what pins the waiting.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revoke_shards_drains_lost_agents_concurrently(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    const TEARDOWN_DELAY: Duration = Duration::from_millis(200);

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agents = start_busy_agents(&executor, &context, &component, "revoke-concurrent").await?;

    // The guests are parked in `poll` and commit nothing on their own, so the next commit of each
    // agent is the one its teardown performs.
    for agent in &agents {
        executor
            .arm_teardown_commit_delay(&agent.id, TEARDOWN_DELAY)
            .await;
    }

    let drain_elapsed = revoke_shards(&executor, &[SHARD]).await?;

    assert_all_unloaded(
        &executor,
        agents.iter().map(|agent| &agent.owned_id),
        "revoke_shards",
        drain_elapsed,
    )
    .await;

    let intervals = executor.teardown_commit_intervals();
    assert_eq!(
        intervals.len(),
        AGENTS,
        "every lost agent must have run its teardown commit inside the RPC: {intervals:?}"
    );
    let depth = max_overlap_depth(&intervals);
    assert!(
        depth >= 2,
        "the teardowns of the lost agents did not overlap (at most {depth} at a time), so the \
         drain stops agents one by one instead of concurrently: {intervals:?}"
    );
    // A sequential drain takes at least AGENTS x TEARDOWN_DELAY; a concurrent one takes about one
    // TEARDOWN_DELAY plus the real teardown work and the poll granularity of the unload barrier.
    let sequential_bound = TEARDOWN_DELAY * AGENTS as u32;
    assert!(
        drain_elapsed < sequential_bound * 3 / 4,
        "revoke_shards took {drain_elapsed:?} for {AGENTS} agents with a {TEARDOWN_DELAY:?} \
         teardown each; a concurrent drain finishes well under {sequential_bound:?}"
    );

    assign_shards(&executor, &[SHARD]).await?;
    assert_invocations_completed_once(&executor, agents).await?;

    drop(executor);
    Ok(())
}

/// An agent that finished its invocation stays loaded but idle. `set_interrupting` hands out no
/// acknowledgement for it, so a drain that only waited for acknowledgements would return with the
/// agent still in memory; it must be unloaded like the others before `revoke_shards` returns.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn revoke_shards_unloads_idle_agents(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let mut agents = Vec::with_capacity(AGENTS);
    for i in 0..AGENTS {
        let parsed_id = agent_id!("InstantiationGrowthCounter", format!("revoke-idle-{i}"));
        let id = executor
            .start_agent(&component.id, parsed_id.clone())
            .await?;
        let owned_id = OwnedAgentId::new(context.default_environment_id, &id);

        let count: u32 = executor
            .invoke_and_await_agent(&component, &parsed_id, "increment", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(count, 1);

        wait_for_eviction_class(&executor, &owned_id, EvictionClass::LoadedIdle).await?;
        assert!(executor.worker_is_loaded(&owned_id).await);
        agents.push((parsed_id, owned_id));
    }

    let drain_elapsed = revoke_shards(&executor, &[SHARD]).await?;

    assert_all_unloaded(
        &executor,
        agents.iter().map(|(_, owned_id)| owned_id),
        "revoke_shards",
        drain_elapsed,
    )
    .await;

    // Replay must rebuild exactly the one increment that happened before the revoke.
    assign_shards(&executor, &[SHARD]).await?;
    for (parsed_id, _) in &agents {
        let count: u32 = executor
            .invoke_and_await_agent(&component, parsed_id, "increment", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(count, 2);
    }

    drop(executor);
    Ok(())
}

/// `set_shard_assignment` can take shards away as well, and then has to drain their agents
/// before returning exactly like `revoke_shards` does.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn set_shard_assignment_drains_lost_agents_before_returning(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agents = start_busy_agents(&executor, &context, &component, "set-drain").await?;

    let drain_elapsed = set_shard_assignment(&executor, 1, &[]).await?;

    assert_all_unloaded(
        &executor,
        agents.iter().map(|agent| &agent.owned_id),
        "set_shard_assignment",
        drain_elapsed,
    )
    .await;

    set_shard_assignment(&executor, 1, &[SHARD]).await?;
    assert_invocations_completed_once(&executor, agents).await?;

    drop(executor);
    Ok(())
}

/// An agent of the `Clocks` type executing its `interruption` invocation: 100 sleeps of 100ms,
/// each far below the suspend threshold, so the agent stays loaded and busy for about ten
/// seconds and is interruptible at every sleep.
struct BusyAgent {
    id: AgentId,
    owned_id: OwnedAgentId,
    /// The `invoke_and_await` call. It stays pending while the shard is revoked and completes
    /// once the agent has been recovered and finished the invocation.
    invocation: JoinHandle<anyhow::Result<AgentResult>>,
    /// Finished invocations in the oplog before the invocation under test was enqueued.
    finished_before: usize,
}

async fn start_busy_agents(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    component: &ComponentDto,
    name_prefix: &str,
) -> anyhow::Result<Vec<BusyAgent>> {
    let mut agents = Vec::with_capacity(AGENTS);
    for i in 0..AGENTS {
        let parsed_id = agent_id!("Clocks", format!("{name_prefix}-{i}"));
        let id = executor
            .start_agent(&component.id, parsed_id.clone())
            .await?;
        let owned_id = OwnedAgentId::new(context.default_environment_id, &id);

        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let (_, finished_before) = count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL);

        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            tokio::spawn(
                async move {
                    executor
                        .invoke_and_await_agent(
                            &component,
                            &parsed_id,
                            "interruption",
                            data_value!(),
                        )
                        .await
                }
                .in_current_span(),
            )
        };

        agents.push(BusyAgent {
            id,
            owned_id,
            invocation,
            finished_before,
        });
    }

    for agent in &agents {
        wait_until_executing(executor, &agent.owned_id, Duration::from_secs(30)).await?;
    }

    Ok(agents)
}

async fn revoke_shards(
    executor: &TestWorkerExecutor,
    shard_ids: &[i64],
) -> anyhow::Result<Duration> {
    let started = Instant::now();
    let response = executor
        .client
        .clone()
        .revoke_shards(RevokeShardsRequest {
            shard_ids: proto_shard_ids(shard_ids),
        })
        .await?
        .into_inner();
    match response.result {
        Some(revoke_shards_response::Result::Success(_)) => Ok(started.elapsed()),
        other => anyhow::bail!("revoke_shards failed: {other:?}"),
    }
}

async fn assign_shards(executor: &TestWorkerExecutor, shard_ids: &[i64]) -> anyhow::Result<()> {
    let response = executor
        .client
        .clone()
        .assign_shards(AssignShardsRequest {
            shard_ids: proto_shard_ids(shard_ids),
        })
        .await?
        .into_inner();
    match response.result {
        Some(assign_shards_response::Result::Success(_)) => Ok(()),
        other => anyhow::bail!("assign_shards failed: {other:?}"),
    }
}

async fn set_shard_assignment(
    executor: &TestWorkerExecutor,
    number_of_shards: u32,
    shard_ids: &[i64],
) -> anyhow::Result<Duration> {
    let started = Instant::now();
    let response = executor
        .client
        .clone()
        .set_shard_assignment(SetShardAssignmentRequest {
            number_of_shards,
            shard_ids: proto_shard_ids(shard_ids),
        })
        .await?
        .into_inner();
    match response.result {
        Some(set_shard_assignment_response::Result::Success(_)) => Ok(started.elapsed()),
        other => anyhow::bail!("set_shard_assignment failed: {other:?}"),
    }
}

fn proto_shard_ids(shard_ids: &[i64]) -> Vec<golem_api_grpc::proto::golem::shardmanager::ShardId> {
    shard_ids
        .iter()
        .map(|shard_id| ShardId::new(*shard_id).into())
        .collect()
}

/// Waits until the agent is loaded and actively executing. Stronger than waiting for the
/// `Running` status, which is read from the deferred status blob: the eviction class is `None`
/// exactly while the worker executes.
async fn wait_until_executing(
    executor: &TestWorkerExecutor,
    owned_id: &OwnedAgentId,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if executor.worker_is_loaded(owned_id).await
            && executor.worker_eviction_class(owned_id).await.is_none()
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            anyhow::bail!(
                "agent {owned_id} did not start executing within {timeout:?} (loaded: {}, \
                 eviction class: {:?})",
                executor.worker_is_loaded(owned_id).await,
                executor.worker_eviction_class(owned_id).await
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until `worker_eviction_class(owned_id)` is `expected`, or fails after 5s.
async fn wait_for_eviction_class(
    executor: &TestWorkerExecutor,
    owned_id: &OwnedAgentId,
    expected: EvictionClass,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if executor.worker_eviction_class(owned_id).await == Some(expected) {
            return Ok(());
        }
        if Instant::now() > deadline {
            anyhow::bail!(
                "agent {owned_id} did not reach EvictionClass::{expected:?} within 5s (current \
                 class: {:?})",
                executor.worker_eviction_class(owned_id).await
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Asserts, without waiting, that none of the agents is loaded any more.
async fn assert_all_unloaded(
    executor: &TestWorkerExecutor,
    owned_ids: impl IntoIterator<Item = &OwnedAgentId>,
    call: &str,
    elapsed: Duration,
) {
    for owned_id in owned_ids {
        assert!(
            !executor.worker_is_loaded(owned_id).await,
            "agent {owned_id} was still loaded when {call} returned (after {elapsed:?}); another \
             executor could already be recovering it"
        );
    }
}

/// Joins the invocations that were interrupted by the revoke after the shard has been given back,
/// and checks that each of them ran to completion exactly once.
async fn assert_invocations_completed_once(
    executor: &TestWorkerExecutor,
    agents: Vec<BusyAgent>,
) -> anyhow::Result<()> {
    for BusyAgent {
        id,
        invocation,
        finished_before,
        ..
    } in agents
    {
        let result = tokio::time::timeout(Duration::from_secs(120), invocation)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "agent {id} did not finish its invocation after the shard was restored"
                )
            })??;
        let value: String = result?.into_typed()?;
        assert_eq!(value, "done", "agent {id}");

        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let (_, finished) = count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL);
        assert_eq!(
            finished - finished_before,
            1,
            "agent {id}: the interrupted invocation must be recorded as finished exactly once"
        );
    }
    Ok(())
}

/// The largest number of intervals that overlap at any single instant.
fn max_overlap_depth(intervals: &[(AgentId, Instant, Instant)]) -> usize {
    let mut events: Vec<(Instant, i32)> = Vec::with_capacity(intervals.len() * 2);
    for (_, started, finished) in intervals {
        events.push((*started, 1));
        events.push((*finished, -1));
    }
    // Ends sort before starts at the same instant, so touching intervals do not count as
    // overlapping.
    events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut current = 0i32;
    let mut max = 0i32;
    for (_, delta) in events {
        current += delta;
        max = max.max(current);
    }
    max as usize
}
