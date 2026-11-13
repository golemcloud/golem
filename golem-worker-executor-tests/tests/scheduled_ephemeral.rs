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

//! A guest schedule on an ephemeral target creates the target at schedule time and commits the
//! scheduled invocation in its oplog. The target runs nothing before its deadline; its first
//! execution commits `Restart` before any guest code, and after that it is fail-stop.

use crate::Tracing;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentMode, AgentTarget, InvocationFreshnessDisposition,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{
    AgentFilter, AgentId, AgentInvocation, FilterComparator, IdempotencyKey, OwnedAgentId,
    ScanCursor, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::schema::SchemaValue;
use golem_common::serialization::deserialize;
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand, RpcError};
use golem_worker_executor::storage::scheduler::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
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

const CLIENT: &str = "scheduled-ephemeral-owner";

/// Inserts every invocation wake a second time, under another schedule id, `delay` after the
/// original, and records which of these duplicates the scheduler acknowledged.
#[derive(Debug)]
struct DuplicateWakes {
    inner: Arc<dyn SchedulerStorage + Send + Sync>,
    delay: chrono::Duration,
    duplicates: Arc<Mutex<Vec<ScheduleId>>>,
    acknowledged: Arc<Mutex<Vec<ScheduleId>>>,
}

impl DuplicateWakes {
    fn overrides(
        delay: chrono::Duration,
        acknowledged: Arc<Mutex<Vec<ScheduleId>>>,
    ) -> TestExecutorOverrides {
        let duplicates = Arc::new(Mutex::new(Vec::new()));
        TestExecutorOverrides {
            wrap_scheduler_storage: Some(Arc::new(move |inner| {
                Arc::new(DuplicateWakes {
                    inner,
                    delay,
                    duplicates: duplicates.clone(),
                    acknowledged: acknowledged.clone(),
                })
            })),
            ..Default::default()
        }
    }
}

#[async_trait]
impl SchedulerStorage for DuplicateWakes {
    async fn insert(
        &self,
        id: ScheduleId,
        due: DateTime<Utc>,
        shard: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        self.inner.insert(id, due, shard, action).await?;
        if matches!(
            deserialize::<ScheduledAction>(action).unwrap(),
            ScheduledAction::WakeInvocation { .. }
        ) {
            let duplicate = ScheduleId::fresh();
            self.inner
                .insert(duplicate, due + self.delay, shard, action)
                .await?;
            self.duplicates.lock().unwrap().push(duplicate);
        }
        Ok(())
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
        let acknowledged = self.inner.ack(id, owner).await?;
        if acknowledged && self.duplicates.lock().unwrap().contains(id) {
            self.acknowledged.lock().unwrap().push(*id);
        }
        Ok(acknowledged)
    }
}

/// The target and invocation key of one schedule call.
pub(crate) type Schedule = (AgentId, Option<IdempotencyKey>);

/// Records the freshness of every `inc` target creation and the target and key of every schedule.
/// The first schedule never returns: the call stays pending until the executor is dropped. With
/// `commit`, it first reaches the target, which commits it; without, the target exists but holds
/// no scheduled invocation yet.
pub(crate) struct HoldFirstSchedule {
    pub(crate) inner: Arc<dyn Rpc>,
    pub(crate) commit: bool,
    pub(crate) creations: Arc<Mutex<Vec<InvocationFreshnessDisposition>>>,
    pub(crate) schedules: Arc<Mutex<Vec<Schedule>>>,
    pub(crate) held: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Rpc for HoldFirstSchedule {
    async fn create_and_initialize(
        &self,
        target: AgentTarget,
        environment_id: EnvironmentId,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: IdempotencyKey,
    ) -> Result<Box<dyn RpcDemand>, golem_service_base::error::worker_executor::WorkerExecutorError>
    {
        self.inner
            .create_and_initialize(
                target,
                environment_id,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                config,
                auth_ctx,
                idempotency_key,
            )
            .await
    }

    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: IdempotencyKey,
        receipt: ScheduleId,
        owner: &AgentId,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, owner, auth)
            .await
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: golem_common::model::AgentFingerprint,
        invocation: AgentInvocation,
        scheduled_at: DateTime<Utc>,
        auth_ctx: &AuthCtx,
    ) -> Result<ScheduleId, RpcError> {
        let first = {
            let mut schedules = self.schedules.lock().unwrap();
            schedules.push((
                owned_agent_id.agent_id.clone(),
                invocation.idempotency_key().cloned(),
            ));
            schedules.len() == 1
        };
        if first && !self.commit {
            self.held.notify_one();
            std::future::pending::<()>().await;
        }
        let result = self
            .inner
            .schedule_invocation(
                owned_agent_id,
                expected_fingerprint,
                invocation,
                scheduled_at,
                auth_ctx,
            )
            .await;
        if first {
            self.held.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

    async fn create_demand(
        &self,
        target: AgentTarget,
        environment_id: EnvironmentId,
        method: &str,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        if method == "inc" {
            self.creations.lock().unwrap().push(freshness);
        }
        self.inner
            .create_demand(
                target,
                environment_id,
                method,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
                key,
                freshness,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        target: AgentTarget,
        environment_id: EnvironmentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<golem_common::model::card::ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        self.inner
            .invoke_and_await(
                target,
                environment_id,
                key,
                freshness,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
                card,
            )
            .await
    }

    async fn invoke(
        &self,
        target: AgentTarget,
        environment_id: EnvironmentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                target,
                environment_id,
                key,
                freshness,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
            )
            .await
    }
}

async fn schedule(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target_name: &str,
    hold_ms: u64,
    due: DateTime<Utc>,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(
            component,
            &agent_id!("ScheduledInvocationClient", CLIENT),
            "schedule_ephemeral_at",
            data_value!(
                target_name.to_string(),
                hold_ms,
                due.timestamp() as u64,
                due.timestamp_subsec_nanos()
            ),
        )
        .await?;
    Ok(())
}

/// Constructor and `inc` runs of the targets named `target_name`.
async fn effects(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target_name: &str,
) -> anyhow::Result<(u64, u64)> {
    executor
        .invoke_and_await_agent(
            component,
            &agent_id!("ScheduledInvocationClient", CLIENT),
            "read_effects",
            data_value!(target_name.to_string()),
        )
        .await?
        .into_typed::<(u64, u64)>()
}

async fn wait_for_effects(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target_name: &str,
    expected: (u64, u64),
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if effects(executor, component, target_name).await? == expected {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?
}

/// The ephemeral agents of the component; every test schedules exactly one.
async fn ephemeral_target(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
) -> anyhow::Result<AgentId> {
    let (_, agents) = executor
        .get_workers_metadata(
            &component.id,
            Some(AgentFilter::new_mode(
                FilterComparator::Equal,
                AgentMode::Ephemeral,
            )),
            ScanCursor::default(),
            100,
            true,
        )
        .await?;
    let targets = agents
        .into_iter()
        .map(|agent| agent.agent_id)
        .collect::<Vec<_>>();
    assert_eq!(
        targets.len(),
        1,
        "one ephemeral target must exist: {targets:?}"
    );
    Ok(targets.into_iter().next().unwrap())
}

fn scheduled_key(oplog: &[PublicOplogEntryWithIndex]) -> Option<IdempotencyKey> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::PendingAgentInvocation(pending) if pending.not_before.is_some() => {
            match &pending.invocation {
                PublicAgentInvocation::AgentMethodInvocation(method) => {
                    Some(method.idempotency_key.clone())
                }
                _ => None,
            }
        }
        _ => None,
    })
}

struct Shape {
    restarts: usize,
    scheduled_starts: usize,
    starts: usize,
    cancellations: usize,
}

fn shape(oplog: &[PublicOplogEntryWithIndex]) -> Shape {
    let key = scheduled_key(oplog);
    let count = |predicate: &dyn Fn(&PublicOplogEntry) -> bool| {
        oplog.iter().filter(|entry| predicate(&entry.entry)).count()
    };
    Shape {
        restarts: count(&|entry| matches!(entry, PublicOplogEntry::Restart(_))),
        scheduled_starts: count(&|entry| {
            matches!(entry, PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if Some(&method.idempotency_key) == key.as_ref()))
        }),
        starts: count(&|entry| matches!(entry, PublicOplogEntry::AgentInvocationStarted(_))),
        cancellations: count(&|entry| {
            matches!(entry, PublicOplogEntry::CancelPendingInvocation(_))
        }),
    }
}

async fn sleep_until(instant: DateTime<Utc>) {
    if let Ok(remaining) = (instant - Utc::now()).to_std() {
        tokio::time::sleep(remaining).await;
    }
}

async fn wait_for_acknowledged_duplicate(
    acknowledged: &Arc<Mutex<Vec<ScheduleId>>>,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), async {
        while acknowledged.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the duplicate wake was never acknowledged"))
}

/// A file read on a target that cannot start now must be answered, not parked until a start.
async fn assert_inspection_is_answered(
    executor: &TestWorkerExecutor,
    target: &AgentId,
) -> anyhow::Result<()> {
    let read = tokio::time::timeout(
        Duration::from_secs(3),
        executor.get_file_contents(target, "/missing"),
    )
    .await;
    assert!(
        read.is_ok(),
        "inspecting a target that cannot start must not wait for a start"
    );
    Ok(())
}

/// Before its deadline the target exists with its scheduled invocation committed and has run
/// nothing, not even its constructor. A real executor restart does not run it early, and an
/// inspection is answered without waiting for the deadline. When it is due, two wakes arrive;
/// the target runs its constructor and the method exactly once, after one committed `Restart`.
#[test]
#[timeout("3m")]
async fn scheduled_ephemeral_target_runs_once_at_its_deadline_across_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    let overrides = DuplicateWakes::overrides(chrono::Duration::zero(), acknowledged.clone());
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "runs-once";
    let due = DateTime::from_timestamp_millis(Utc::now().timestamp_millis() + 12_000).unwrap();
    schedule(&executor, &component, name, 0, due).await?;

    let target = ephemeral_target(&executor, &component).await?;
    let before = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    assert!(
        scheduled_key(&before).is_some(),
        "the schedule must commit in the target oplog"
    );
    let before = shape(&before);
    assert_eq!((before.restarts, before.starts), (0, 0));
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert_inspection_is_answered(&executor, &target).await?;
    assert!(Utc::now() < due, "the restart must precede the deadline");
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));

    wait_for_effects(&executor, &component, name, (1, 1)).await?;
    assert!(
        Utc::now() >= due,
        "the target must not run before its deadline"
    );
    wait_for_acknowledged_duplicate(&acknowledged).await?;
    assert_eq!(effects(&executor, &component, name).await?, (1, 1));

    let after = shape(&executor.get_oplog(&target, OplogIndex::INITIAL).await?);
    assert_eq!((after.restarts, after.scheduled_starts), (1, 1));
    Ok(())
}

/// Token cancellation across an executor restart withdraws the scheduled work and the
/// constructor reservation of the never-started target. An inspection of the withdrawn target is
/// answered, and the due wake runs nothing.
#[test]
#[timeout("3m")]
async fn cancelled_scheduled_ephemeral_target_never_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    cancelled_scheduled_ephemeral_target_case(last_unique_id, deps, agent_rpc_rust, false).await
}

#[test]
#[timeout("3m")]
async fn publicly_cancelled_scheduled_ephemeral_target_never_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    cancelled_scheduled_ephemeral_target_case(last_unique_id, deps, agent_rpc_rust, true).await
}

async fn cancelled_scheduled_ephemeral_target_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    public_cancel: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "cancelled";
    let due = Utc::now() + chrono::Duration::seconds(20);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    drop(executor);

    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    if public_cancel {
        let key = scheduled_key(&executor.get_oplog(&target, OplogIndex::INITIAL).await?)
            .expect("the schedule must commit in the target oplog");
        assert!(executor.cancel_invocation(&target, &key).await?);
    } else {
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id!("ScheduledInvocationClient", CLIENT),
                "cancel_scheduled",
                data_value!(),
            )
            .await?;
    }
    assert!(
        Utc::now() < due,
        "the cancellation must precede the deadline"
    );
    executor.resume(&target, false).await?;
    assert_inspection_is_answered(&executor, &target).await?;
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    drop(executor);

    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let error = executor.resume(&target, false).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("An ephemeral agent cannot accept another invocation or be resumed"),
        "{error}"
    );
    assert_inspection_is_answered(&executor, &target).await?;

    sleep_until(due + chrono::Duration::seconds(5)).await;
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    let oplog = shape(&executor.get_oplog(&target, OplogIndex::INITIAL).await?);
    assert_eq!(
        (oplog.restarts, oplog.starts, oplog.cancellations),
        (0, 0, 2),
        "the cancellation must withdraw both the method and the constructor reservation"
    );
    Ok(())
}

/// The due start holds the instance mutex while it records `Restart`. A cancellation arriving
/// then waits for it, finds the target begun, and reports nothing to cancel; the target runs
/// everything once.
#[test]
#[timeout("3m")]
async fn scheduled_ephemeral_cancellation_behind_its_start_cancels_nothing(
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
    let owner = executor
        .start_agent(
            &component.id,
            agent_id!("ScheduledInvocationClient", CLIENT),
        )
        .await?;
    let name = "cancellation-behind-start";
    let due = Utc::now() + chrono::Duration::seconds(4);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    let key = scheduled_key(&executor.get_oplog(&target, OplogIndex::INITIAL).await?)
        .expect("the schedule must commit in the target oplog");
    let receipt = ScheduleId::for_invocation(
        executor.get_worker_metadata(&target).await?.fingerprint,
        &key,
    );
    let mut restart = executor
        .gate_after_next_append(&target, |entry| matches!(entry, OplogEntry::Restart { .. }))
        .await;

    tokio::time::timeout(Duration::from_secs(30), restart.entered()).await?;
    let worker = executor
        .active_agent(&OwnedAgentId::new(component.environment_id, &target))
        .await
        .expect("the starting target must be active")
        .primary();
    let mut cancellation = Box::pin(worker.cancel_scheduled_invocation(key, receipt, &owner));
    assert!(futures::poll!(&mut cancellation).is_pending());
    drop(restart);
    assert!(
        !tokio::time::timeout(Duration::from_secs(30), cancellation).await??,
        "a cancellation behind the begun start must cancel nothing"
    );

    wait_for_effects(&executor, &component, name, (1, 1)).await?;
    let oplog = shape(&executor.get_oplog(&target, OplogIndex::INITIAL).await?);
    assert_eq!(
        (oplog.restarts, oplog.scheduled_starts, oplog.cancellations),
        (1, 1, 0)
    );
    Ok(())
}

/// The scheduling call commits on the target, but its response is lost and the caller restarts.
/// The repair finds the target the first attempt created, schedules the same key again and gets
/// the original receipt: one target, one scheduled invocation, one execution.
#[test]
#[timeout("3m")]
async fn ambiguous_ephemeral_schedule_retry_reuses_the_created_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    lost_schedule_response_case(last_unique_id, deps, agent_rpc_rust, true).await
}

/// The caller is lost after it created the target and before the target committed the
/// scheduled invocation. A file read in that window is answered without starting the target. The
/// never-started target, holding only its constructor reservation, admits the repair, which
/// schedules the invocation once.
#[test]
#[timeout("3m")]
async fn ephemeral_schedule_lost_before_its_commit_is_repaired_on_the_created_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    lost_schedule_response_case(last_unique_id, deps, agent_rpc_rust, false).await
}

async fn lost_schedule_response_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    commit: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let creations = Arc::new(Mutex::new(Vec::new()));
    let schedules = Arc::new(Mutex::new(Vec::new()));
    let held = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let creations = creations.clone();
            let schedules = schedules.clone();
            let held = held.clone();
            move |rpc| {
                Arc::new(HoldFirstSchedule {
                    inner: rpc,
                    commit,
                    creations: creations.clone(),
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
    let client = agent_id!("ScheduledInvocationClient", CLIENT);
    executor.start_agent(&component.id, client.clone()).await?;
    let name = "lost-schedule-response";
    let due = Utc::now() + chrono::Duration::seconds(15);
    let input = data_value!(name.to_string(), 0u64, due.timestamp() as u64, 0u32);
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let client = client.clone();
        let key = key.clone();
        let input = input.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &client,
                    &key,
                    "schedule_ephemeral_at",
                    input,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), held.notified()).await?;
    let created = ephemeral_target(&executor, &component).await?;
    if !commit {
        // Starting the target now would run its constructor and use it up before the schedule.
        assert_inspection_is_answered(&executor, &created).await?;
    }
    let pending_before_loss = shape(&executor.get_oplog(&created, OplogIndex::INITIAL).await?);
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
            "schedule_ephemeral_at",
            input,
        ),
    )
    .await??;
    assert_eq!(
        (pending_before_loss.restarts, pending_before_loss.starts),
        (0, 0)
    );

    let schedules = schedules.lock().unwrap().clone();
    assert_eq!(schedules.len(), 2, "{schedules:?}");
    assert!(schedules[0].1.is_some(), "{schedules:?}");
    assert_eq!(
        schedules[1], schedules[0],
        "the repair must schedule the same key on the same target"
    );
    assert_eq!(
        *creations.lock().unwrap(),
        vec![
            InvocationFreshnessDisposition::KnownFresh,
            InvocationFreshnessDisposition::MayExist
        ],
        "only the first live dispatch may create a fresh target"
    );
    let target = ephemeral_target(&executor, &component).await?;
    assert_eq!(target, schedules[0].0);
    assert!(Utc::now() < due, "the repair must precede the deadline");
    assert_eq!(
        effects(&executor, &component, name).await?,
        (0, 0),
        "the repair must not run the target before its deadline"
    );

    wait_for_effects(&executor, &component, name, (1, 1)).await?;
    let oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    let pending = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::PendingAgentInvocation(pending)
                if pending.not_before.is_some())
        })
        .count();
    assert_eq!(pending, 1, "the retry must not append another invocation");
    let oplog = shape(&oplog);
    assert_eq!((oplog.restarts, oplog.scheduled_starts), (1, 1));
    Ok(())
}

/// The target is lost after it committed `Restart` and before any guest code ran. Its scheduled
/// invocation is still pending and due, so a late wake reaches a start. The begun target is
/// fail-stop: the start is refused, the scheduler acknowledges the wake, and nothing runs.
#[test]
#[timeout("3m")]
async fn scheduled_ephemeral_target_lost_after_it_began_never_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    let overrides = DuplicateWakes::overrides(chrono::Duration::seconds(10), acknowledged.clone());
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "lost-after-begin";
    let due = Utc::now() + chrono::Duration::seconds(4);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    let mut started = executor.gate_next_invocation_started_append(&target).await;
    tokio::time::timeout(Duration::from_secs(30), started.entered()).await?;
    started.abort();
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    wait_for_acknowledged_duplicate(&acknowledged).await?;
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    let oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    assert!(
        scheduled_key(&oplog).is_some(),
        "the scheduled invocation must still be pending"
    );
    let oplog = shape(&oplog);
    assert_eq!(
        (oplog.restarts, oplog.starts),
        (1, 0),
        "the begun marker must be committed before guest code runs"
    );
    Ok(())
}

/// Deleting the target before its deadline leaves a stale wake, which must not recreate it.
#[test]
#[timeout("3m")]
async fn deleted_scheduled_ephemeral_target_is_not_recreated_by_its_wake(
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
    let name = "deleted";
    let due = Utc::now() + chrono::Duration::seconds(5);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    executor.delete_worker(&target).await?;
    assert!(Utc::now() < due, "the deletion must precede the deadline");

    sleep_until(due + chrono::Duration::seconds(5)).await;
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    assert_eq!(executor.get_worker_metadata_opt(&target).await?, None);
    Ok(())
}
