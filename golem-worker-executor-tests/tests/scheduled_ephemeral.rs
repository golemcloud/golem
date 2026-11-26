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

use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentMode, AgentTypeName, InvocationFreshnessDisposition,
};
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentDto;
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
use golem_common::{agent_key, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand, RpcError};
use golem_worker_executor::storage::scheduler::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError,
};
use golem_worker_executor::worker::target::ResolvedTarget;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, callers, start_with_overrides,
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
    delay: jiff::SignedDuration,
    duplicates: Arc<Mutex<Vec<ScheduleId>>>,
    acknowledged: Arc<Mutex<Vec<ScheduleId>>>,
}

impl DuplicateWakes {
    fn overrides(
        delay: jiff::SignedDuration,
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
        due: jiff::Timestamp,
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
        let acknowledged = self.inner.ack(id, owner, now).await?;
        if acknowledged && self.duplicates.lock().unwrap().contains(id) {
            self.acknowledged.lock().unwrap().push(*id);
        }
        Ok(acknowledged)
    }
}

/// Holds every scheduler claim until released, and observes acknowledgement of the invocation wake.
#[derive(Debug)]
pub(crate) struct PausedScheduler {
    pub(crate) inner: Arc<dyn SchedulerStorage + Send + Sync>,
    pub(crate) release: tokio_util::sync::CancellationToken,
    pub(crate) invocation_wake: Mutex<Option<ScheduleId>>,
    pub(crate) acknowledged: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl SchedulerStorage for PausedScheduler {
    async fn insert(
        &self,
        id: ScheduleId,
        due: jiff::Timestamp,
        shard: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        self.inner.insert(id, due, shard, action).await?;
        if matches!(
            deserialize::<ScheduledAction>(action).unwrap(),
            ScheduledAction::WakeInvocation { .. }
        ) {
            *self.invocation_wake.lock().unwrap() = Some(id);
        }
        Ok(())
    }

    async fn cancel(&self, id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(id).await
    }

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.release.cancelled().await;
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
        let acknowledged = self.inner.ack(id, owner, now).await?;
        if acknowledged && self.invocation_wake.lock().unwrap().as_ref() == Some(id) {
            self.acknowledged.notify_one();
        }
        Ok(acknowledged)
    }
}

/// The target and invocation key of one schedule call.
pub(crate) type Schedule = (AgentId, Option<golem_common::model::InvocationId>);

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
        target: ResolvedTarget,
        exclusive: bool,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: IdempotencyKey,
    ) -> Result<Box<dyn RpcDemand>, golem_service_base::error::worker_executor::WorkerExecutorError>
    {
        self.inner
            .create_and_initialize(
                target,
                exclusive,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                caller,
                config,
                auth_ctx,
                idempotency_key,
            )
            .await
    }

    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: golem_common::model::InvocationId,
        receipt: ScheduleId,
        canceller: &golem_common::model::caller::Principal,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, canceller, auth)
            .await
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: golem_common::model::AgentFingerprint,
        request_key: golem_common::model::InvocationRequestKey,
        invocation: AgentInvocation,
        scheduled_at: jiff::Timestamp,
        self_agent_id: &golem_common::model::AgentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ScheduleId, RpcError> {
        let first = {
            let mut schedules = self.schedules.lock().unwrap();
            schedules.push((
                owned_agent_id.agent_id,
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
                request_key,
                invocation,
                scheduled_at,
                self_agent_id,
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
        target: ResolvedTarget,
        method: &str,
        invocation: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
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
                method,
                invocation,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
                config,
                auth,
                key,
                freshness,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        target: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<golem_common::model::card::ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        self.inner
            .invoke_and_await(
                target,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
                config,
                auth,
                card,
            )
            .await
    }

    async fn invoke(
        &self,
        target: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                target,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
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
    due: jiff::Timestamp,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(
            component,
            &agent_key!("ScheduledInvocationClient", CLIENT),
            "schedule_ephemeral_at",
            data_value!(
                target_name.to_string(),
                hold_ms,
                due.as_second() as u64,
                due.subsec_nanosecond() as u32
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
            &agent_key!("ScheduledInvocationClient", CLIENT),
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
) -> anyhow::Result<OwnedAgentId> {
    let (_, agents) = executor
        .get_workers_metadata(
            &component.environment_id,
            &AgentTypeName("ScheduledEphemeralTarget".to_string()),
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
        .map(|agent| OwnedAgentId::new(agent.environment_id, &agent.agent_id))
        .collect::<Vec<_>>();
    assert_eq!(
        targets.len(),
        1,
        "one ephemeral target must exist: {targets:?}"
    );
    Ok(targets.into_iter().next().unwrap())
}

fn scheduled_key(oplog: &[PublicOplogEntryWithIndex]) -> Option<golem_common::model::InvocationId> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::PendingAgentInvocation(pending) if pending.not_before.is_some() => {
            match &pending.invocation {
                PublicAgentInvocation::AgentMethodInvocation(method) => Some(method.invocation_id),
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
                        if Some(&method.invocation_id) == key.as_ref()))
        }),
        starts: count(&|entry| matches!(entry, PublicOplogEntry::AgentInvocationStarted(_))),
        cancellations: count(&|entry| {
            matches!(entry, PublicOplogEntry::CancelPendingInvocation(_))
        }),
    }
}

async fn sleep_until(instant: jiff::Timestamp) {
    if let Ok(remaining) = (instant - jiff::Timestamp::now()).try_into() {
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
    target: &OwnedAgentId,
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
    let overrides = DuplicateWakes::overrides(jiff::SignedDuration::ZERO, acknowledged.clone());
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "runs-once";
    let due = jiff::Timestamp::from_millisecond(jiff::Timestamp::now().as_millisecond() + 12_000)
        .unwrap();
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
    assert!(
        jiff::Timestamp::now() < due,
        "the restart must precede the deadline"
    );
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));

    wait_for_effects(&executor, &component, name, (1, 1)).await?;
    assert!(
        jiff::Timestamp::now() >= due,
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
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(20);
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
                &agent_key!("ScheduledInvocationClient", CLIENT),
                "cancel_scheduled",
                data_value!(),
            )
            .await?;
    }
    assert!(
        jiff::Timestamp::now() < due,
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

    sleep_until(due + jiff::SignedDuration::from_secs(5)).await;
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
            agent_key!("ScheduledInvocationClient", CLIENT),
        )
        .await?;
    let name = "cancellation-behind-start";
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(4);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    let key = scheduled_key(&executor.get_oplog(&target, OplogIndex::INITIAL).await?)
        .expect("the schedule must commit in the target oplog");
    let receipt = ScheduleId::for_invocation(
        executor.get_worker_metadata(&target).await?.fingerprint,
        &key,
    );
    let mut restart = executor
        .gate_after_next_append(&target.agent_id, |entry| {
            matches!(entry, OplogEntry::Restart { .. })
        })
        .await;

    tokio::time::timeout(Duration::from_secs(30), restart.entered()).await?;
    let worker = executor
        .active_agent(&OwnedAgentId::new(
            component.environment_id,
            &target.agent_id,
        ))
        .await
        .expect("the starting target must be active")
        .primary();
    // The client that scheduled the call, as the target recorded it.
    let canceller = callers::agent_in(
        owner.agent_id,
        golem_common::model::agent::AgentTypeName("ScheduledInvocationClient".to_string()),
        executor.get_worker_metadata(&owner).await?.fingerprint,
        component.environment_id,
    )
    .immediate;
    let mut cancellation = Box::pin(worker.cancel_scheduled_invocation(key, receipt, &canceller));
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
    let client = agent_key!("ScheduledInvocationClient", CLIENT);
    executor.start_agent(&component.id, client.clone()).await?;
    let name = "lost-schedule-response";
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(15);
    let input = data_value!(name.to_string(), 0u64, due.as_second() as u64, 0u32);
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
    assert_eq!(target.agent_id, schedules[0].0);
    assert!(
        jiff::Timestamp::now() < due,
        "the repair must precede the deadline"
    );
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
    let overrides =
        DuplicateWakes::overrides(jiff::SignedDuration::from_secs(10), acknowledged.clone());
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "lost-after-begin";
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(4);
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    let mut started = executor
        .gate_next_invocation_started_append(&target.agent_id)
        .await;
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

/// Deleting the target before wake delivery leaves a stale wake, which must not recreate it.
#[test]
#[timeout("3m")]
async fn deleted_scheduled_ephemeral_target_is_not_recreated_by_its_wake(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let release = tokio_util::sync::CancellationToken::new();
    let _release_on_drop = release.clone().drop_guard();
    let acknowledged = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_scheduler_storage: Some(Arc::new({
            let release = release.clone();
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
    let name = "deleted";
    let due = jiff::Timestamp::now();
    schedule(&executor, &component, name, 0, due).await?;
    let target = ephemeral_target(&executor, &component).await?;
    let oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    assert!(scheduled_key(&oplog).is_some());
    let oplog = shape(&oplog);
    assert_eq!((oplog.restarts, oplog.starts), (0, 0));
    executor.delete_worker(&target).await?;

    release.cancel();
    tokio::time::timeout(Duration::from_secs(60), acknowledged.notified())
        .await
        .map_err(|_| anyhow::anyhow!("the invocation wake was never acknowledged"))?;
    assert_eq!(effects(&executor, &component, name).await?, (0, 0));
    assert_eq!(executor.get_worker_metadata_opt(&target).await?, None);
    Ok(())
}
