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

//! A guest schedule on a durable target commits the scheduled invocation in the target's oplog
//! and stores a payload-free wake. Each test loses the executor at one point of that lifecycle and
//! counts the target's effects to show that the invocation runs exactly once, at its deadline.

use crate::Tracing;
use crate::scheduled_ephemeral::{HoldFirstSchedule, PausedScheduler};
use async_trait::async_trait;

use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{
    IdempotencyKey, OwnedAgentId, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::schema::TypedSchemaValue;
use golem_common::serialization::deserialize;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::storage::scheduler::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

const CLIENT: &str = "scheduled-durable-owner";

/// The amount `ScheduledInvocationClient::schedule_at` adds to the server's counter.
const SCHEDULED_INCREMENT: u64 = 17;

/// The first wake insert never returns: the schedule call stays pending after the target
/// committed its scheduled invocation, until the executor is dropped.
#[derive(Debug)]
struct HoldFirstWake {
    inner: Arc<dyn SchedulerStorage + Send + Sync>,
    first: Arc<AtomicBool>,
    held: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl SchedulerStorage for HoldFirstWake {
    async fn reschedule(
        &self,
        id: &ScheduleId,
        owner: uuid::Uuid,
        due: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.reschedule(id, owner, due, now).await
    }

    async fn insert(
        &self,
        id: ScheduleId,
        due: jiff::Timestamp,
        shard: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        if matches!(
            deserialize::<ScheduledAction>(action).unwrap(),
            ScheduledAction::WakeInvocation { .. }
        ) && !self.first.swap(true, Ordering::AcqRel)
        {
            self.held.notify_one();
            std::future::pending::<()>().await;
        }
        self.inner.insert(id, due, shard, action).await
    }

    async fn cancel(&self, id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(id).await
    }

    async fn get_action(&self, id: &ScheduleId) -> Result<Option<Vec<u8>>, SchedulerStorageError> {
        self.inner.get_action(id).await
    }

    async fn replace_action(
        &self,
        id: &ScheduleId,
        expected: &[u8],
        action: &[u8],
        due: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.replace_action(id, expected, action, due).await
    }

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.inner.claim_due(now, assignment, limit, ttl).await
    }

    async fn count_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        id: &ScheduleId,
        owner: uuid::Uuid,
        until: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.extend_lease(id, owner, until, now).await
    }

    async fn ack(
        &self,
        id: &ScheduleId,
        owner: uuid::Uuid,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.ack(id, owner, now).await
    }
}

fn schedule_input(server_name: &str, due: jiff::Timestamp) -> TypedSchemaValue {
    data_value!(
        server_name.to_string(),
        due.as_second() as u64,
        due.subsec_nanosecond() as u32
    )
}

async fn schedule_at(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    server_name: &str,
    due: jiff::Timestamp,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(
            component,
            &agent_key!("ScheduledInvocationClient", CLIENT),
            "schedule_at",
            schedule_input(server_name, due),
        )
        .await?;
    Ok(())
}

/// The scheduled invocations committed in the target oplog, and the starts of each of them.
struct Scheduled {
    keys: Vec<golem_common::model::InvocationId>,
    not_before: Option<golem_common::model::Timestamp>,
    starts: Vec<golem_common::model::Timestamp>,
}

fn scheduled(oplog: &[PublicOplogEntryWithIndex]) -> Scheduled {
    let mut keys = Vec::new();
    let mut not_before = None;
    for entry in oplog {
        if let PublicOplogEntry::PendingAgentInvocation(pending) = &entry.entry
            && pending.not_before.is_some()
            && let PublicAgentInvocation::AgentMethodInvocation(method) = &pending.invocation
        {
            keys.push(method.invocation_id);
            not_before = pending.not_before;
        }
    }
    let starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if keys.contains(&method.invocation_id)) =>
            {
                Some(started.timestamp)
            }
            _ => None,
        })
        .collect();
    Scheduled {
        keys,
        not_before,
        starts,
    }
}

/// Waits for the scheduled invocation to start by reading the target oplog only: loading the
/// target would register its wakes again and start its due work, hiding a lost wake.
async fn wait_for_scheduled_start(
    executor: &TestWorkerExecutor,
    target: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        while scheduled(&executor.get_oplog(target, OplogIndex::INITIAL).await?)
            .starts
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("nothing started the scheduled invocation"))?
}

async fn counter(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &OwnedAgentId,
) -> anyhow::Result<u64> {
    crate::agent::invoke_and_await_agent_by_id(
        executor,
        component,
        agent,
        None,
        "get_global_value",
        data_value!(),
    )
    .await?
    .into_typed::<u64>()
}

/// Waits until the target's counter reaches `expected`, then checks that it stays there and that
/// exactly one scheduled invocation started, not before its deadline.
async fn assert_ran_once(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target: &OwnedAgentId,
    expected: u64,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        while counter(executor, component, target).await? < expected {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("the scheduled invocation never ran"))??;
    // A duplicate execution would come from a second wake or a recovery start; give it time.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(counter(executor, component, target).await?, expected);
    let scheduled = scheduled(&executor.get_oplog(target, OplogIndex::INITIAL).await?);
    assert_eq!(
        scheduled.keys.len(),
        1,
        "one scheduled invocation is committed"
    );
    assert_eq!(scheduled.starts.len(), 1, "it starts exactly once");
    assert!(
        scheduled.starts[0].to_millis() >= scheduled.not_before.unwrap().to_millis(),
        "the scheduled invocation must not start before its deadline"
    );
    Ok(())
}

async fn start_agents(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    server_name: &str,
) -> anyhow::Result<OwnedAgentId> {
    let server = agent_key!("ScheduledInvocationServer", server_name);
    let target = executor.start_agent(&component.id, server).await?;
    executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", CLIENT),
        )
        .await?;
    Ok(target)
}

/// The executor is lost after the target committed the scheduled invocation and before the wake
/// was stored. The caller's same-key repair adds no second entry, the wake is restored, and the
/// invocation runs once at its deadline.
#[test]
#[timeout("4m")]
async fn scheduled_invocation_lost_before_its_wake_is_stored_runs_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let held = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_scheduler_storage: Some(Arc::new({
            let held = held.clone();
            let first = Arc::new(AtomicBool::new(false));
            move |inner| {
                Arc::new(HoldFirstWake {
                    inner,
                    first: first.clone(),
                    held: held.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "lost-before-wake";
    let target = start_agents(&executor, &component, server_name).await?;
    let client = agent_key!("ScheduledInvocationClient", CLIENT);
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(10);
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let client = client.clone();
        let key = key.clone();
        let input = schedule_input(server_name, due);
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(&component, &client, &key, "schedule_at", input)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), held.notified()).await?;
    let committed = scheduled(&executor.get_oplog(&target, OplogIndex::INITIAL).await?);
    assert_eq!(
        committed.keys.len(),
        1,
        "the target committed before the wake"
    );
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &client,
            &key,
            "schedule_at",
            schedule_input(server_name, due),
        ),
    )
    .await??;
    assert!(
        jiff::Timestamp::now() < due,
        "the repair must precede the deadline"
    );
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await
}

/// The executor is lost after the wake was stored and before it fired. After the restart the
/// stored wake runs the invocation once, at its deadline.
#[test]
#[timeout("4m")]
async fn scheduled_invocation_survives_a_restart_before_its_wake_fires(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides::default();
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "restart-before-fire";
    let target = start_agents(&executor, &component, server_name).await?;
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(8);
    schedule_at(&executor, &component, server_name, due).await?;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert!(
        jiff::Timestamp::now() < due,
        "the restart must precede the deadline"
    );
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await
}

/// The receipt the guest gets names the invocation the target commits, which is the invocation
/// the wake starts. After the target is deleted, the caller still replays the completed schedule
/// from its own oplog: the replayed receipt is the same and the target is not created again.
#[test]
#[timeout("4m")]
async fn schedule_receipt_names_the_scheduled_invocation_and_replays_without_the_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides::default();
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "receipt";
    let target = start_agents(&executor, &component, server_name).await?;
    let client = agent_key!("ScheduledInvocationClient", CLIENT);
    let receipt = |executor: &TestWorkerExecutor| {
        let executor = executor.clone();
        let component = component.clone();
        let client = client.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &client,
                    "scheduled_invocation_id",
                    data_value!(),
                )
                .await?
                .into_typed::<Option<String>>()
        }
    };
    schedule_at(
        &executor,
        &component,
        server_name,
        jiff::Timestamp::now() + jiff::SignedDuration::from_secs(2),
    )
    .await?;
    let receipt_before_restart = receipt(&executor).await?;

    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await?;
    let committed = scheduled(&executor.get_oplog(&target, OplogIndex::INITIAL).await?);
    assert_eq!(
        receipt_before_restart,
        Some(format!("{}.{}", target.agent_id, committed.keys[0])),
        "the receipt must name the committed invocation, which is the one that started"
    );

    executor.delete_worker(&target).await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert_eq!(receipt(&executor).await?, receipt_before_restart);
    assert_eq!(executor.get_worker_metadata_opt(&target).await?, None);
    Ok(())
}

/// The executor is lost after the wake fired and before the target recorded the invocation's
/// start. The invocation is still pending and due, and runs once after the restart.
#[test]
#[timeout("4m")]
async fn scheduled_invocation_lost_at_its_start_runs_once_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides::default();
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "lost-at-start";
    let target = start_agents(&executor, &component, server_name).await?;
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(3);
    schedule_at(&executor, &component, server_name, due).await?;
    let mut started = executor
        .gate_next_invocation_started_append(&target.agent_id)
        .await;
    tokio::time::timeout(Duration::from_secs(30), started.entered()).await?;
    started.abort();
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await
}

/// Deleting the target before its deadline leaves a stale wake, which neither recreates the
/// target nor runs anything.
#[test]
#[timeout("3m")]
async fn deleted_scheduled_target_is_not_recreated_by_its_wake(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let release = tokio_util::sync::CancellationToken::new();
    release.cancel();
    let acknowledged = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_scheduler_storage: Some(Arc::new({
            let acknowledged = acknowledged.clone();
            move |inner| {
                Arc::new(PausedScheduler {
                    inner,
                    release: release.clone(),
                    invocation_wake: Mutex::new(None),
                    acknowledged: acknowledged.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "deleted";
    let target = start_agents(&executor, &component, server_name).await?;
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(5);
    schedule_at(&executor, &component, server_name, due).await?;
    executor.delete_worker(&target).await?;
    assert!(
        jiff::Timestamp::now() < due,
        "the deletion must precede the deadline"
    );

    tokio::time::timeout(Duration::from_secs(60), acknowledged.notified())
        .await
        .map_err(|_| anyhow::anyhow!("the invocation wake was never acknowledged"))?;
    assert_eq!(executor.get_worker_metadata_opt(&target).await?, None);
    Ok(())
}

/// A schedule whose time has already passed is accepted and runs on the next scheduler tick.
#[test]
#[timeout("3m")]
async fn schedule_in_the_past_runs_on_the_next_tick(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "past";
    let target = start_agents(&executor, &component, server_name).await?;
    schedule_at(
        &executor,
        &component,
        server_name,
        jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60),
    )
    .await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await
}

/// An agent that schedules an invocation on itself from its constructor is not blocked by its
/// own construction, and a past time runs on the next tick.
#[test]
#[timeout("3m")]
async fn constructor_schedule_on_itself_is_accepted_and_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .without_default_host_permissions("ConstructorSelfScheduler")
        .update_agent_provision_config("ConstructorSelfScheduler", |config| {
            config.initial_permissions.lower_bound.positive.push(
                golem_common::model::card::parse_polymorphic_permission(
                    "agent(?agent) @ * : invoke : inc_global_by",
                )
                .expect("valid self-method permission"),
            );
        })
        .store()
        .await?;
    let mut targets = Vec::new();
    let agent = agent_key!("ConstructorSelfScheduler", "constructor-self", 0u64);
    for keyed in [true, false] {
        let target = if keyed {
            executor.start_agent(&component.id, agent.clone()).await?
        } else {
            crate::agent::create_unkeyed_agent(&executor, &component, &agent).await?
        };
        assert_ran_once(&executor, &component, &target, 1).await?;
        targets.push(target);
    }
    drop(executor);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    for target in targets {
        assert_ran_once(&executor, &component, &target, 1).await?;
        let entries = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
        // The client of another agent type rejects the binding in the guest, from
        // `GOLEM_AGENT_TYPE`, so only the constructor's own binding reaches the host.
        assert_eq!(entries.iter().filter(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::Start(call) if call.function_name == "golem::rpc::wasm-rpc::bind-self"
        )).count(), 1, "reconstruction must reuse the constructor's binding");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(
                    &entry.entry,
                    PublicOplogEntry::AgentInvocationStarted(start)
                        if matches!(start.invocation, PublicAgentInvocation::AgentInitialization(_))
                ))
                .count(),
            1,
            "self binding must not run another constructor"
        );
    }
    Ok(())
}

/// A fork replays a self binding as the recorded source incarnation, including when its End
/// is absent. Continuing after that binding schedules new work on the source, not on the fork.
#[test]
#[timeout("4m")]
async fn fork_preserves_completed_and_incomplete_self_bindings(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_end in [false, true] {
        let context = TestContext::new(last_unique_id);
        let executor = crate::fork::start_with_local_resume_and_snapshot_policy(
            deps,
            &context,
            golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled,
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let source = agent_key!("ConstructorSelfScheduler", "fork-self-binding", 0u64);
        let source_id = executor.start_agent(&component.id, source.clone()).await?;
        assert_ran_once(&executor, &component, &source_id, 1).await?;
        let key = IdempotencyKey::fresh();
        let mut end = executor
            .gate_next_call_end(&source_id.agent_id, after_end)
            .await;
        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            let source = source.clone();
            let key = key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &source,
                        &key,
                        "bind_and_schedule",
                        data_value!(),
                    )
                    .await
            })
        };
        // The client reads `GOLEM_AGENT_TYPE` through a durable environment read before it
        // binds, so the gate moves on to the next End until it holds the binding's End.
        let prefix = loop {
            tokio::time::timeout(Duration::from_secs(30), end.entered()).await?;
            let prefix = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
            if prefix.iter().any(|entry| {
                entry.oplog_index == end.start_index()
                    && matches!(&entry.entry, PublicOplogEntry::Start(call)
                        if call.function_name == "golem::rpc::wasm-rpc::bind-self")
            }) {
                break prefix;
            }
            let next = executor
                .gate_next_call_end(&source_id.agent_id, after_end)
                .await;
            drop(std::mem::replace(&mut end, next));
        };
        assert_eq!(
            prefix.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::End(call) if call.start_index == end.start_index())),
            after_end,
            "fork prefix must contain exactly the selected binding boundary"
        );
        let cut = prefix.last().unwrap().oplog_index;
        drop(end);
        assert_eq!(
            invocation.await??.into_typed::<String>()?,
            source_id.agent_id.to_string()
        );
        let target = executor.fork_worker(&source_id, cut).await?;
        assert_eq!(
            crate::agent::invoke_and_await_agent_by_id(
                &executor,
                &component,
                &target,
                None,
                "last_binding_id",
                data_value!()
            )
            .await?
            .into_typed::<Option<String>>()?,
            Some(source_id.agent_id.to_string()),
            "fork replay must preserve the recorded binding ID"
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            while counter(&executor, &component, &source_id).await? < 15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::Ok(())
        })
        .await??;
        assert_eq!(counter(&executor, &component, &source_id).await?, 15);
        let source_schedules =
            scheduled(&executor.get_oplog(&source_id, OplogIndex::INITIAL).await?);
        assert_eq!(
            source_schedules.keys.len(),
            3,
            "constructor, source continuation and fork continuation each schedule once"
        );
        assert_eq!(
            source_schedules.starts.len(),
            3,
            "each source schedule executes once"
        );
        assert_eq!(counter(&executor, &component, &target).await?, 1);
    }
    Ok(())
}

/// The caller is lost after the target committed its schedule and before the caller recorded the
/// result. The repair reuses the key and finds the committed invocation: the target holds one
/// scheduled invocation, which runs once.
#[test]
#[timeout("4m")]
async fn durable_schedule_retry_with_the_same_key_commits_one_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let schedules = Arc::new(Mutex::new(Vec::new()));
    let held = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let schedules = schedules.clone();
            let held = held.clone();
            move |rpc| {
                Arc::new(HoldFirstSchedule {
                    inner: rpc,
                    commit: true,
                    creations: Arc::new(Mutex::new(Vec::new())),
                    schedules: schedules.clone(),
                    held: held.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "same-key-retry";
    let target = start_agents(&executor, &component, server_name).await?;
    let client = agent_key!("ScheduledInvocationClient", CLIENT);
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(10);
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let client = client.clone();
        let key = key.clone();
        let input = schedule_input(server_name, due);
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(&component, &client, &key, "schedule_at", input)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), held.notified()).await?;
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &client,
            &key,
            "schedule_at",
            schedule_input(server_name, due),
        ),
    )
    .await??;
    let schedules = schedules.lock().unwrap().clone();
    assert_eq!(schedules.len(), 2, "{schedules:?}");
    assert!(schedules[0].1.is_some(), "{schedules:?}");
    assert_eq!(
        schedules[1], schedules[0],
        "the repair must schedule the same key on the same target"
    );
    assert!(
        jiff::Timestamp::now() < due,
        "the repair must precede the deadline"
    );
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &target, SCHEDULED_INCREMENT).await
}
