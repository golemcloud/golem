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

//! The generations of a never-live fork across its updates: each update starts one generation,
//! which writes one outcome and stops. An update requested while a generation stops runs in a
//! later generation, and a crash around the outcome commit leaves exactly one outcome.

use super::{live_entries, tip, update_outcomes, wait_for_update_outcomes};
use crate::Tracing;
use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentId, AgentStatus, OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    UpdateRequestPoint, WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The shard manager process that every shard push of these tests names.
const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000002";

/// The parked wait of `sleep_p3` must not suspend, so that a cut inside it folds to `Running`.
fn overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        })),
        ..Default::default()
    }
}

async fn store_component(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<ComponentDto> {
    executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await
}

/// Starts `Clock(name)`, runs one invocation, and forks it with the never-live mark at its tip.
async fn idle_fork(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    name: &str,
) -> anyhow::Result<OwnedAgentId> {
    let source = agent_key!("Clock", name.to_string());
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(component, &source, "healthcheck", data_value!())
        .await?;
    let fork = executor
        .fork_worker_never_live(&source_id, tip(executor, &source_id).await?)
        .await?;
    Ok(fork)
}

fn assert_all_successful(outcomes: &[PublicOplogEntryWithIndex]) {
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome.entry, PublicOplogEntry::SuccessfulUpdate(_))),
        "{outcomes:?}"
    );
}

async fn assert_sandboxed(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    underlying: AgentStatus,
) -> anyhow::Result<()> {
    let metadata = executor.get_worker_metadata(worker_id).await?;
    assert_eq!(metadata.status, AgentStatus::Sandboxed);
    assert_eq!(metadata.underlying_status, Some(underlying));
    Ok(())
}

async fn push_shards(
    executor: &TestWorkerExecutor,
    epoch: u64,
    revision: u64,
) -> anyhow::Result<()> {
    let assignment = executor
        .client
        .clone()
        .assign_shards(AssignShardsRequest {
            number_of_shards: 1,
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch,
            }],
            revision,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    Ok(())
}

/// E3 and E3b: never-live forks whose folded status is `Idle`, `Running` (cut inside an
/// invocation) and `Interrupted` each report `Sandboxed` over that status, and each applies a
/// first and a second automatic update to its own revision: one `SuccessfulUpdate` per update and
/// no live entry. After the shard of the forks is revoked and assigned again, each applies a third.
/// After a cold restart of the executor, no fork starts by itself, and each applies a fourth.
/// Each fork appends only the entries of its updates: no `Resumed` for the interrupted
/// invocation, and no live work.
#[test]
#[timeout("6m")]
async fn every_folded_status_of_a_never_live_fork_applies_updates_also_after_shard_reassignment(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    push_shards(&executor, 1, 1).await?;
    let component = store_component(&executor, &context, host_api_tests).await?;
    let source = agent_key!("Clock", "never-live-generations");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "healthcheck", data_value!())
        .await?;
    let idle_cut = tip(&executor, &source_id).await?;
    let sleeping = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let source = source.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &source, "sleep_p3", data_value!(60u64))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        while !executor
            .get_oplog(&source_id, idle_cut.next())
            .await?
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let running_cut = tip(&executor, &source_id).await?;
    executor.interrupt(&source_id).await?;
    assert!(sleeping.await?.is_err());
    executor
        .wait_for_status(
            &source_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let interrupted_cut = tip(&executor, &source_id).await?;

    let mut forks = Vec::new();
    for (cut, underlying) in [
        (idle_cut, AgentStatus::Idle),
        (running_cut, AgentStatus::Running),
        (interrupted_cut, AgentStatus::Interrupted),
    ] {
        let fork = executor.fork_worker_never_live(&source_id, cut).await?;
        assert_sandboxed(&executor, &fork, underlying).await?;
        let forked = tip(&executor, &fork).await?;
        for count in [1, 2] {
            executor
                .auto_update_worker(&fork, component.revision, false)
                .await?;
            let outcomes =
                wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, count, true).await?;
            assert_eq!(outcomes.len(), count, "{underlying:?}: {outcomes:?}");
            assert_all_successful(&outcomes);
        }
        let oplog = executor.get_oplog(&fork, OplogIndex::INITIAL).await?;
        assert_eq!(live_entries(&oplog, forked), Vec::<String>::new());
        assert_sandboxed(&executor, &fork, underlying).await?;
        forks.push((fork, underlying, forked));
    }

    let revoked = executor
        .client
        .clone()
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![ShardId { value: 0 }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    push_shards(&executor, 2, 2).await?;
    for (fork, underlying, _) in &forks {
        executor
            .auto_update_worker(fork, component.revision, false)
            .await?;
        let outcomes = wait_for_update_outcomes(&executor, fork, OplogIndex::NONE, 3, true).await?;
        assert_eq!(outcomes.len(), 3, "{underlying:?}: {outcomes:?}");
        assert_all_successful(&outcomes);
        assert_sandboxed(&executor, fork, *underlying).await?;
    }

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    push_shards(&executor, 3, 3).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    for (fork, underlying, forked) in &forks {
        assert!(
            !executor.worker_is_loaded(fork).await,
            "{underlying:?}: the fork started after the restart"
        );
        executor
            .auto_update_worker(fork, component.revision, false)
            .await?;
        let outcomes = wait_for_update_outcomes(&executor, fork, OplogIndex::NONE, 4, true).await?;
        assert_eq!(outcomes.len(), 4, "{underlying:?}: {outcomes:?}");
        assert_all_successful(&outcomes);
        assert_sandboxed(&executor, fork, *underlying).await?;
        let oplog = executor.get_oplog(fork, OplogIndex::INITIAL).await?;
        let appended = super::entry_kinds(&oplog, *forked);
        assert!(
            appended
                .iter()
                .all(|kind| kind == "PendingUpdate" || kind == "SuccessfulUpdate"),
            "{underlying:?}: {appended:?}"
        );
    }
    Ok(())
}

/// E3, the crash cases: the executor stops just before the outcome of a never-live fork's update
/// reaches the oplog, and, separately, just after the outcome commits. After a real restart the
/// fork holds exactly one outcome: the restarted executor applies the update that has none, and
/// applies no update a second time.
#[test]
#[timeout("6m")]
async fn a_crash_around_the_outcome_commit_of_a_never_live_fork_leaves_one_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_append in [false, true] {
        let context = TestContext::new(last_unique_id);
        let executor = start_with_overrides(deps, &context, overrides()).await?;
        let component = store_component(&executor, &context, host_api_tests).await?;
        let fork = idle_fork(&executor, &component, "never-live-crash").await?;
        let mut gate = executor
            .gate_next_update_outcome_append(&fork.agent_id, after_append)
            .await;
        executor
            .auto_update_worker(&fork, component.revision, false)
            .await?;
        tokio::time::timeout(Duration::from_secs(60), gate.entered()).await?;
        let committed = update_outcomes(&executor, &fork).await?;
        assert_eq!(committed.len(), usize::from(after_append), "{committed:?}");
        gate.abort();
        // The aborted append is the crash: the status actor that owns it fails, which the
        // shutdown reports. Any other shutdown failure fails the test.
        if let Err(error) = executor.shutdown_and_wait_for_invocation_loops().await {
            assert!(
                format!("{error:#}").contains("test checkpoint abandoned at executor shutdown"),
                "{error:#}"
            );
        }
        drop(gate);
        drop(executor);

        let executor = start_with_overrides(deps, &context, overrides()).await?;
        let outcomes =
            wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 1, true).await?;
        assert_eq!(
            outcomes.len(),
            1,
            "after append: {after_append}: {outcomes:?}"
        );
        assert_all_successful(&outcomes);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            update_outcomes(&executor, &fork).await?.len(),
            1,
            "after append: {after_append}"
        );
        assert_sandboxed(&executor, &fork, AgentStatus::Idle).await?;
    }
    Ok(())
}

/// E3c and the stop handoff: an automatic update requested while the previous update's outcome
/// commits, while the generation stops after its admission, and right after the outcome became
/// visible, while the generation unloads. The held generation is released only after the second
/// request reached its first status read, which queues behind the held commit, or, across the
/// early stop, entered its admission under the instance lock. Each later update runs in a later
/// generation: the fork ends with two outcomes and no pending update.
#[test]
#[timeout("6m")]
async fn an_update_requested_while_a_never_live_generation_stops_runs_in_a_later_generation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let component = store_component(&executor, &context, host_api_tests).await?;
    let request = |worker_id: OwnedAgentId| {
        let executor = executor.clone();
        let revision = component.revision;
        tokio::spawn(async move {
            executor
                .auto_update_worker(&worker_id, revision, false)
                .await
        })
    };

    // While the outcome commits: the outcome is committed and its append is held.
    let fork = idle_fork(&executor, &component, "handoff-commit").await?;
    let mut gate = executor
        .gate_next_update_outcome_append(&fork.agent_id, true)
        .await;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(60), gate.entered()).await?;
    let submitted = executor
        .observe_next_update_request(&fork, UpdateRequestPoint::StatusRead)
        .await?;
    let second = request(fork.clone());
    tokio::time::timeout(Duration::from_secs(60), submitted).await??;
    drop(gate);
    second.await??;
    let outcomes = wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 2, true).await?;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_all_successful(&outcomes);

    // Across the admission's early stop: the generation prepared its instance and wrote its
    // outcome, and the invocation loop has not yet taken the stop.
    let fork = idle_fork(&executor, &component, "handoff-admission").await?;
    let (entered, release) = executor.gate_next_prepared_instance(&fork.agent_id);
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(60), entered).await??;
    let admitted = executor
        .observe_next_update_request(&fork, UpdateRequestPoint::Admission)
        .await?;
    let second = request(fork.clone());
    tokio::time::timeout(Duration::from_secs(60), admitted).await??;
    let _ = release.send(());
    second.await??;
    let outcomes = wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 2, true).await?;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_all_successful(&outcomes);

    // While the generation unloads: the second request follows the first outcome at once.
    let fork = idle_fork(&executor, &component, "handoff-unload").await?;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(60), async {
        while update_outcomes(&executor, &fork).await?.is_empty() {
            tokio::task::yield_now().await;
        }
        anyhow::Ok(())
    })
    .await??;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    let outcomes = wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 2, true).await?;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_all_successful(&outcomes);
    Ok(())
}

/// An automatic update requested while a never-live generation runs is applied by that
/// generation, which unloads before the request activates the worker. The activation then finds
/// no pending update, and the request still succeeds, because the outcome of its update is
/// recorded.
#[test]
#[timeout("4m")]
async fn an_update_applied_before_its_activation_is_accepted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let component = store_component(&executor, &context, host_api_tests).await?;
    let fork = idle_fork(&executor, &component, "applied-before-activation").await?;

    let (prepared, release_generation) = executor.gate_next_prepared_instance(&fork.agent_id);
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    tokio::time::timeout(Duration::from_secs(60), prepared).await??;

    let (entered, release_activation) = executor.gate_next_update_activation(&fork).await?;
    let second = {
        let executor = executor.clone();
        let fork = fork.clone();
        let revision = component.revision;
        tokio::spawn(async move { executor.auto_update_worker(&fork, revision, false).await })
    };
    tokio::time::timeout(Duration::from_secs(60), entered).await??;

    let _ = release_generation.send(());
    let outcomes = wait_for_update_outcomes(&executor, &fork, OplogIndex::NONE, 2, true).await?;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_all_successful(&outcomes);

    let _ = release_activation.send(());
    tokio::time::timeout(Duration::from_secs(60), second).await???;
    assert_eq!(update_outcomes(&executor, &fork).await?.len(), 2);
    assert_sandboxed(&executor, &fork, AgentStatus::Idle).await?;
    Ok(())
}

/// E11: the mark is part of a fork request's identity. A never-live fork and an unmarked fork of
/// the same source, cut and target conflict in both orders; two concurrent opposite requests have
/// one winner, whose mark the target carries; and a retry of a never-live request, as after a
/// lost response, gives the same target, still not resumed.
#[test]
#[timeout("4m")]
async fn the_mark_is_part_of_the_identity_of_a_fork_request(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = store_component(&executor, &context, host_api_tests).await?;
    let source = agent_key!("Clock", "never-live-identity");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "healthcheck", data_value!())
        .await?;
    let cut = tip(&executor, &source_id).await?;
    let type_id = source_id
        .agent_id
        .type_id()
        .ok_or_else(|| anyhow::anyhow!("{source_id} has no agent type"))?;
    let target = || AgentId::random(type_id);

    let marked_first = target();
    let marked = executor
        .fork_worker_to_with_mark(&source_id, &marked_first, cut, true)
        .await?;
    let published = tip(&executor, &marked).await?;
    executor
        .fork_worker_to(&source_id, &marked_first, cut)
        .await
        .expect_err("an unmarked request conflicts with a never-live target");
    executor
        .fork_worker_to_with_mark(&source_id, &marked_first, cut, true)
        .await?;
    assert_eq!(tip(&executor, &marked).await?, published);
    assert_sandboxed(&executor, &marked, AgentStatus::Idle).await?;
    let oplog = executor.get_oplog(&marked, OplogIndex::INITIAL).await?;
    assert_eq!(live_entries(&oplog, cut), Vec::<String>::new());

    let unmarked_first = target();
    let unmarked = executor
        .fork_worker_to(&source_id, &unmarked_first, cut)
        .await?;
    executor
        .fork_worker_to_with_mark(&source_id, &unmarked_first, cut, true)
        .await
        .expect_err("a never-live request conflicts with an unmarked target");
    assert_ne!(
        executor.get_worker_metadata(&unmarked).await?.status,
        AgentStatus::Sandboxed
    );

    for _ in 0..3 {
        let raced = target();
        let (marked, unmarked) = tokio::join!(
            executor.fork_worker_to_with_mark(&source_id, &raced, cut, true),
            executor.fork_worker_to(&source_id, &raced, cut),
        );
        assert!(
            marked.is_ok() != unmarked.is_ok(),
            "exactly one of two opposite requests wins: {marked:?}, {unmarked:?}"
        );
        let raced = OwnedAgentId::new(source_id.environment_id, &raced);
        assert_eq!(
            executor.get_worker_metadata(&raced).await?.status == AgentStatus::Sandboxed,
            marked.is_ok()
        );
    }
    Ok(())
}

/// R3: a never-live fork cut inside an invocation folds to `Running`. After a real executor
/// restart, its metadata and the agent list show `Sandboxed` over `Running`, and a filter on
/// `status = sandboxed` finds it, while a filter on `status = running` does not.
#[test]
#[timeout("4m")]
async fn a_never_live_fork_is_listed_as_sandboxed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::{AgentFilter, FilterComparator, ScanCursor};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let component = store_component(&executor, &context, host_api_tests).await?;
    let source = agent_key!("Clock", "never-live-listed");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "healthcheck", data_value!())
        .await?;
    let before = tip(&executor, &source_id).await?;
    let sleeping = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let source = source.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &source, "sleep_p3", data_value!(60u64))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        while !executor
            .get_oplog(&source_id, before.next())
            .await?
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let cut = tip(&executor, &source_id).await?;
    let fork = executor.fork_worker_never_live(&source_id, cut).await?;
    sleeping.abort();
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    assert_sandboxed(&executor, &fork, AgentStatus::Running).await?;
    let listed = |status: AgentStatus| {
        let executor = executor.clone();
        let environment_id = component.environment_id;
        async move {
            let (_, agents) = executor
                .get_workers_metadata(
                    &environment_id,
                    &golem_common::model::agent::AgentTypeName("Clock".to_string()),
                    Some(AgentFilter::new_status(FilterComparator::Equal, status)),
                    ScanCursor::default(),
                    100,
                    true,
                )
                .await?;
            anyhow::Ok(agents)
        }
    };
    let sandboxed = listed(AgentStatus::Sandboxed).await?;
    let listed_fork = sandboxed
        .iter()
        .find(|agent| agent.agent_id == fork.agent_id)
        .expect("status = sandboxed finds the fork");
    assert_eq!(listed_fork.status, AgentStatus::Sandboxed);
    assert_eq!(listed_fork.underlying_status, Some(AgentStatus::Running));
    assert!(
        !listed(AgentStatus::Running)
            .await?
            .iter()
            .any(|agent| agent.agent_id == fork.agent_id)
    );
    Ok(())
}
