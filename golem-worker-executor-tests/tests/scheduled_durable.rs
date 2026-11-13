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
use crate::scheduled_ephemeral::HoldFirstSchedule;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{
    AgentId, IdempotencyKey, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::schema::{ParsedAgentKey, TypedSchemaValue};
use golem_common::serialization::deserialize;
use golem_common::{agent_id, data_value};
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
    async fn insert(
        &self,
        id: ScheduleId,
        due: DateTime<Utc>,
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

    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.inner.claim_due(now, assignment, limit, ttl).await
    }

    async fn count_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        id: &ScheduleId,
        owner: uuid::Uuid,
        until: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.extend_lease(id, owner, until).await
    }

    async fn ack(&self, id: &ScheduleId, owner: uuid::Uuid) -> Result<bool, SchedulerStorageError> {
        self.inner.ack(id, owner).await
    }
}

fn schedule_input(server_name: &str, due: DateTime<Utc>) -> TypedSchemaValue {
    data_value!(
        server_name.to_string(),
        due.timestamp() as u64,
        due.timestamp_subsec_nanos()
    )
}

async fn schedule_at(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    server_name: &str,
    due: DateTime<Utc>,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(
            component,
            &agent_id!("ScheduledInvocationClient", CLIENT),
            "schedule_at",
            schedule_input(server_name, due),
        )
        .await?;
    Ok(())
}

/// The scheduled invocations committed in the target oplog, and the starts of each of them.
struct Scheduled {
    keys: Vec<IdempotencyKey>,
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
            keys.push(method.idempotency_key.clone());
            not_before = pending.not_before;
        }
    }
    let starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if keys.contains(&method.idempotency_key)) =>
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
    target: &AgentId,
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
    agent: &ParsedAgentKey,
) -> anyhow::Result<u64> {
    executor
        .invoke_and_await_agent(component, agent, "get_global_value", data_value!())
        .await?
        .into_typed::<u64>()
}

/// Waits until the target's counter reaches `expected`, then checks that it stays there and that
/// exactly one scheduled invocation started, not before its deadline.
async fn assert_ran_once(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    server: &ParsedAgentKey,
    target: &AgentId,
    expected: u64,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        while counter(executor, component, server).await? < expected {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("the scheduled invocation never ran"))??;
    // A duplicate execution would come from a second wake or a recovery start; give it time.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(counter(executor, component, server).await?, expected);
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
) -> anyhow::Result<(ParsedAgentKey, AgentId)> {
    let server = agent_id!("ScheduledInvocationServer", server_name);
    let target = executor.start_agent(&component.id, server.clone()).await?;
    executor
        .start_agent(
            &component.id,
            agent_id!("ScheduledInvocationClient", CLIENT),
        )
        .await?;
    Ok((server, target))
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
    let (server, target) = start_agents(&executor, &component, server_name).await?;
    let client = agent_id!("ScheduledInvocationClient", CLIENT);
    let due = Utc::now() + chrono::Duration::seconds(10);
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
    assert!(Utc::now() < due, "the repair must precede the deadline");
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &server, &target, SCHEDULED_INCREMENT).await
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
    let (server, target) = start_agents(&executor, &component, server_name).await?;
    let due = Utc::now() + chrono::Duration::seconds(8);
    schedule_at(&executor, &component, server_name, due).await?;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert!(Utc::now() < due, "the restart must precede the deadline");
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &server, &target, SCHEDULED_INCREMENT).await
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
    let (server, target) = start_agents(&executor, &component, server_name).await?;
    let due = Utc::now() + chrono::Duration::seconds(3);
    schedule_at(&executor, &component, server_name, due).await?;
    let mut started = executor.gate_next_invocation_started_append(&target).await;
    tokio::time::timeout(Duration::from_secs(30), started.entered()).await?;
    started.abort();
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &server, &target, SCHEDULED_INCREMENT).await
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
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server_name = "deleted";
    let (_, target) = start_agents(&executor, &component, server_name).await?;
    let due = Utc::now() + chrono::Duration::seconds(5);
    schedule_at(&executor, &component, server_name, due).await?;
    executor.delete_worker(&target).await?;
    assert!(Utc::now() < due, "the deletion must precede the deadline");

    let wait = (due + chrono::Duration::seconds(5) - Utc::now()).to_std()?;
    tokio::time::sleep(wait).await;
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
    let (server, target) = start_agents(&executor, &component, server_name).await?;
    schedule_at(
        &executor,
        &component,
        server_name,
        Utc::now() - chrono::Duration::seconds(60),
    )
    .await?;
    assert_ran_once(&executor, &component, &server, &target, SCHEDULED_INCREMENT).await
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
        .store()
        .await?;
    let agent = agent_id!("ConstructorSelfScheduler", "constructor-self", 0u64);
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    assert_ran_once(&executor, &component, &agent, &target, 1).await
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
    let (server, target) = start_agents(&executor, &component, server_name).await?;
    let client = agent_id!("ScheduledInvocationClient", CLIENT);
    let due = Utc::now() + chrono::Duration::seconds(10);
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
    assert!(Utc::now() < due, "the repair must precede the deadline");
    wait_for_scheduled_start(&executor, &target).await?;
    assert_ran_once(&executor, &component, &server, &target, SCHEDULED_INCREMENT).await
}
