// Invocation handles driven from a guest through the `HandleProbe` agent of the Rust RPC
// fixture: held and rebuilt handles after the target is deleted, re-created or lives in another
// environment; cancellation across a restart and racing a scheduled start; a schedule observed
// from `Scheduled` to `Completed`; two observers of one invocation; an ID kept in a snapshot;
// crash windows of a handle await and of a schedule; two concurrent handle awaits; a failed
// invocation; and the stream and read-only restrictions of the host functions.

use super::*;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::HostRequestGolemRpcInvocationHandle;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::oplog_shape::assert_settled_oplog;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use pretty_assertions::assert_eq;
use std::time::{SystemTime, UNIX_EPOCH};
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

const PENDING_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const HANDLE_AWAIT: &str = "golem::rpc::invocation::await";
const HANDLE_STATUS: &str = "golem::rpc::invocation::status";

#[derive(Debug, Clone, PartialEq, FromSchema)]
enum RpcCallOutcome {
    Ok,
    Denied { details: String },
    NotFound { details: String },
    ProtocolError { details: String },
    RemoteInternalError { details: String },
    Conflict { details: String },
    Cancelled { details: String },
    CannotCancelRunning { details: String },
}

#[derive(Debug, Clone, PartialEq, FromSchema)]
enum HandleAnswer {
    Status { status: String },
    Value { value: u64 },
    Unit,
    Cancel { withdrawn: bool },
    Error { error: RpcCallOutcome },
}

impl HandleAnswer {
    fn status(status: &str) -> Self {
        Self::Status {
            status: status.to_string(),
        }
    }

    fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Error {
                error: RpcCallOutcome::NotFound { .. }
            }
        )
    }

    fn is_denied(&self) -> bool {
        matches!(
            self,
            Self::Error {
                error: RpcCallOutcome::Denied { .. }
            }
        )
    }

    fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Error {
                error: RpcCallOutcome::Cancelled { .. }
            }
        )
    }

    fn is_cannot_cancel_running(&self) -> bool {
        matches!(
            self,
            Self::Error {
                error: RpcCallOutcome::CannotCancelRunning { .. }
            }
        )
    }

    fn has_started(&self) -> bool {
        matches!(self, Self::Status { status } if status == "Running" || status == "Waiting")
    }
}

/// Denies the management permissions of invocation handles (viewing and cancelling an
/// invocation), so only the recorded starter of an invocation can observe or cancel it. Other
/// verbs, such as the `Invoke` that creating an RPC client needs, stay allowed.
struct NoManagement;

#[async_trait]
impl DirectInvocationAuthService for NoManagement {
    async fn check(
        &self,
        account: AccountId,
        _: &OwnedAgentId,
        _: ComponentId,
        _: Option<AgentTypeName>,
        verb: AgentVerb,
        _: AgentResourcePattern,
        _: &AuthCtx,
    ) -> Result<EnvironmentOwnerAccountId, RpcError> {
        match verb {
            AgentVerb::View | AgentVerb::CancelInvocation => Err(RpcError::Denied {
                details: "no management permission".to_string(),
            }),
            _ => Ok(EnvironmentOwnerAccountId(account)),
        }
    }
}

fn no_management() -> TestExecutorOverrides {
    TestExecutorOverrides {
        create_direct_invocation_auth: Some(Arc::new(|| Arc::new(NoManagement))),
        ..Default::default()
    }
}

fn snapshot_every_invocation() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
        })),
        ..Default::default()
    }
}

/// The `(seconds, nanoseconds)` of the wall-clock time `after` from now.
fn due_in(after: Duration) -> (u64, u32, SystemTime) {
    let due = SystemTime::now() + after;
    let since_epoch = due.duration_since(UNIX_EPOCH).unwrap();
    (since_epoch.as_secs(), since_epoch.subsec_nanos(), due)
}

/// The ID of an invocation that the owner of `id` never had.
fn unknown_invocation_of_the_same_owner(id: &str) -> String {
    let (owner, _) = id.split_once('.').expect("an invocation ID");
    format!("{owner}.{}", uuid::Uuid::new_v4())
}

struct Fixture {
    executor: TestWorkerExecutor,
    component: ComponentDto,
}

impl Fixture {
    async fn start(
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        fixture: &PrecompiledComponent,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let executor = start_with_overrides(deps, context, overrides).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        Ok(Self {
            executor,
            component,
        })
    }

    async fn restart(
        self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let Self {
            executor,
            component,
        } = self;
        drop(executor);
        let executor = start_with_overrides(deps, context, overrides).await?;
        Ok(Self {
            executor,
            component,
        })
    }

    async fn call<T: FromSchema>(
        &self,
        agent: &ParsedAgentKey,
        method: &str,
        input: TypedSchemaValue,
    ) -> anyhow::Result<T> {
        self.executor
            .invoke_and_await_agent(&self.component, agent, method, input)
            .await?
            .into_typed::<T>()
    }

    async fn unit(
        &self,
        agent: &ParsedAgentKey,
        method: &str,
        input: TypedSchemaValue,
    ) -> anyhow::Result<()> {
        self.executor
            .invoke_and_await_agent(&self.component, agent, method, input)
            .await?;
        Ok(())
    }

    async fn promise(&self, counter: &ParsedAgentKey) -> anyhow::Result<PromiseId> {
        self.call(counter, "create_promise", data_value!()).await
    }

    async fn complete(&self, promise: &PromiseId) -> anyhow::Result<()> {
        self.executor
            .complete_promise(
                &self.executor.context.default_environment_id,
                promise,
                vec![],
            )
            .await?;
        Ok(())
    }

    async fn oplog(&self, agent: &OwnedAgentId) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        // An agent that is not loaded has nothing left to commit.
        if self.executor.active_agent(agent).await.is_some() {
            self.executor.commit_oplog(&agent.agent_id).await?;
        }
        self.executor.get_oplog(agent, OplogIndex::INITIAL).await
    }

    /// Waits until the agent's oplog holds at least `count` handle-await `Start`s.
    async fn await_starts(
        &self,
        agent: &OwnedAgentId,
        count: usize,
    ) -> anyhow::Result<Vec<OplogIndex>> {
        tokio::time::timeout(PENDING_TIMEOUT, async {
            loop {
                let starts = starts_of(&self.oplog(agent).await?, HANDLE_AWAIT);
                if starts.len() >= count {
                    return Ok::<_, anyhow::Error>(starts);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await?
    }

    /// Polls a status `method` of the probe until the invocation runs, and returns that status.
    async fn until_started(
        &self,
        probe: &ParsedAgentKey,
        method: &str,
        input: TypedSchemaValue,
    ) -> anyhow::Result<HandleAnswer> {
        tokio::time::timeout(PENDING_TIMEOUT, async {
            loop {
                let status: HandleAnswer = self.call(probe, method, input.clone()).await?;
                if status.has_started() {
                    return Ok::<_, anyhow::Error>(status);
                }
                assert_eq!(
                    status,
                    HandleAnswer::status("Pending"),
                    "an accepted invocation is pending until it runs"
                );
                tokio::time::sleep(STATUS_POLL_INTERVAL).await;
            }
        })
        .await?
    }

    /// Starts `method` in the background with `key`, so a restart can resume it.
    fn spawn(
        &self,
        agent: &ParsedAgentKey,
        key: &IdempotencyKey,
        method: &'static str,
        input: TypedSchemaValue,
    ) -> tokio::task::JoinHandle<anyhow::Result<AgentResult>> {
        let executor = self.executor.clone();
        let component = self.component.clone();
        let agent = agent.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(&component, &agent, &key, method, input)
                .await
        })
    }
}

fn starts_of(oplog: &[PublicOplogEntryWithIndex], function: &str) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == function)
                .then_some(entry.oplog_index)
        })
        .collect()
}

fn accepted(oplog: &[PublicOplogEntryWithIndex], method: &str) -> bool {
    oplog.iter().any(|entry| {
        matches!(&entry.entry, PublicOplogEntry::PendingAgentInvocation(pending)
            if matches!(&pending.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                if invocation.method_name == method))
    })
}

fn ends_of(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(
            |entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start),
        )
        .count()
}

/// The UUID of the invocation that a guest invocation ID names.
fn invocation_uuid(id: &str) -> uuid::Uuid {
    let (_, invocation) = id.split_once('.').expect("an invocation ID");
    invocation.parse().expect("an invocation UUID")
}

/// The UUID of the invocation that the handle await at `start` reaches.
fn awaited_invocation(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> uuid::Uuid {
    let entry = oplog
        .iter()
        .find(|entry| entry.oplog_index == start)
        .expect("the await Start is in the oplog");
    let PublicOplogEntry::Start(start) = &entry.entry else {
        panic!("not a Start: {entry:?}");
    };
    let request = start
        .request
        .as_ref()
        .expect("the await records its request");
    HostRequestGolemRpcInvocationHandle::from_value(request.value())
        .expect("a handle request")
        .invocation_id
        .uuid()
}

fn started_invocation_ids(oplog: &[PublicOplogEntryWithIndex], method: &str) -> Vec<uuid::Uuid> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                PublicAgentInvocation::AgentMethodInvocation(invocation)
                    if invocation.method_name == method =>
                {
                    Some(invocation.invocation_id.0)
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn started(oplog: &[PublicOplogEntryWithIndex], method: &str) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                    if invocation.method_name == method) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

/// Held and rebuilt guest handles answer the authorized caller `not-found` once their target is
/// deleted, also for an await that was waiting when the deletion happened, and they never reach
/// the re-created successor. Before the deletion, the handle is returned while the target still
/// runs, a running invocation refuses cancellation, an unknown invocation of the live owner is
/// not found, and a caller in another environment is denied and creates no owner there.
#[test]
#[timeout("3 minutes")]
async fn guest_handles_of_a_deleted_target_are_not_found(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let counter = agent_key!("RpcBlockingCounter", "deleted-handle-target");
    let target = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "deleted-handle-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let promise = f.promise(&counter).await?;

    let id: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("deleted-handle-target", promise.clone()),
        )
        .await?;

    // The trigger answered while the target still waits for its promise.
    f.until_started(&probe, "held_status", data_value!())
        .await?;
    let refused: HandleAnswer = f.call(&probe, "held_cancel", data_value!()).await?;
    assert!(refused.is_cannot_cancel_running(), "{refused:?}");
    let unknown = unknown_invocation_of_the_same_owner(&id);
    for method in ["status", "await_unit", "cancel"] {
        let answer: HandleAnswer = f.call(&probe, method, data_value!(unknown.clone())).await?;
        assert!(
            answer.is_not_found(),
            "{method} of an unknown invocation: {answer:?}"
        );
    }

    // A caller in another environment resolves the ID in its own environment only. There the
    // owner is absent and its agent type, which belongs to the other environment's component,
    // is not deployed, so the lookup cannot be authorized and is denied.
    let other_environment = golem_common::model::environment::EnvironmentId::new();
    let other_component = f
        .executor
        .component_dep(&other_environment, fixture)
        .store()
        .await?;
    let other_probe = agent_key!("HandleProbe", "deleted-handle-other-environment");
    f.executor
        .start_agent(&other_component.id, other_probe.clone())
        .await?;
    for method in ["status", "await_value", "cancel"] {
        let answer = f
            .executor
            .invoke_and_await_agent(
                &other_component,
                &other_probe,
                method,
                data_value!(id.clone()),
            )
            .await?
            .into_typed::<HandleAnswer>()?;
        assert!(
            answer.is_denied(),
            "{method} from another environment: {answer:?}"
        );
    }
    let same_address_elsewhere = OwnedAgentId::new(other_environment, &target.agent_id);
    assert!(
        f.executor
            .get_worker_metadata_opt(&same_address_elsewhere)
            .await?
            .is_none(),
        "a lookup from another environment must not create the owner there"
    );

    let earlier_awaits = starts_of(&f.oplog(&caller).await?, HANDLE_AWAIT).len();
    let key = IdempotencyKey::fresh();
    let waiting = f.spawn(&probe, &key, "held_await", data_value!());
    f.await_starts(&caller, earlier_awaits + 1).await?;
    f.executor.delete_worker(&target).await?;
    let interrupted = waiting.await??.into_typed::<HandleAnswer>()?;
    assert!(
        interrupted.is_not_found(),
        "an await waiting when its target is deleted ends as not found: {interrupted:?}"
    );

    for method in ["held_status", "held_await", "held_cancel"] {
        let answer: HandleAnswer = f.call(&probe, method, data_value!()).await?;
        assert!(
            answer.is_not_found(),
            "{method} after the deletion: {answer:?}"
        );
    }
    for method in ["status", "await_value", "cancel"] {
        let answer: HandleAnswer = f.call(&probe, method, data_value!(id.clone())).await?;
        assert!(
            answer.is_not_found(),
            "rebuilt {method} after the deletion: {answer:?}"
        );
    }

    let successor = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    for method in ["status", "await_value", "cancel"] {
        let answer: HandleAnswer = f.call(&probe, method, data_value!(id.clone())).await?;
        assert!(
            answer.is_not_found(),
            "{method} with a successor: {answer:?}"
        );
    }
    let value: u64 = f.call(&counter, "get_value", data_value!()).await?;
    assert_eq!(
        value, 0,
        "the successor runs none of its predecessor's invocations"
    );
    assert!(started(&f.oplog(&successor).await?, "inc_after_promise").is_empty());
    f.executor.check_oplog_is_queryable(&caller).await?;
    Ok(())
}

/// A withdrawn invocation stays cancelled across an executor restart: the starter's rebuilt
/// handle awaits a typed `cancelled` error with one recorded await, a repeated cancel answers
/// `false`, and the method never runs. Management permission is denied throughout, so the
/// starter is recognized from the caller principal the host derives, also after the restart,
/// while another caller is denied.
#[test]
#[timeout("3 minutes")]
async fn a_withdrawn_invocation_stays_cancelled_for_its_starter_across_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, no_management()).await?;
    let counter = agent_key!("RpcBlockingCounter", "withdrawn-target");
    let target = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "withdrawn-starter");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let other = agent_key!("HandleProbe", "withdrawn-other");
    f.executor
        .start_agent(&f.component.id, other.clone())
        .await?;
    let promise = f.promise(&counter).await?;
    let _: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("withdrawn-target", promise.clone()),
        )
        .await?;
    let id: String = f
        .call(
            &probe,
            "trigger_inc_by",
            data_value!("withdrawn-target", 1u64),
        )
        .await?;

    let pending: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(pending, HandleAnswer::status("Pending"));
    for method in ["status", "await_unit", "cancel"] {
        let answer: HandleAnswer = f.call(&other, method, data_value!(id.clone())).await?;
        assert!(answer.is_denied(), "{method} by another caller: {answer:?}");
    }
    let withdrawn: HandleAnswer = f.call(&probe, "cancel", data_value!(id.clone())).await?;
    assert_eq!(withdrawn, HandleAnswer::Cancel { withdrawn: true });

    let f = f.restart(deps, &context, no_management()).await?;
    let awaited: HandleAnswer = f
        .call(&probe, "await_unit", data_value!(id.clone()))
        .await?;
    assert!(awaited.is_cancelled(), "{awaited:?}");
    let repeated: HandleAnswer = f.call(&probe, "cancel", data_value!(id.clone())).await?;
    assert_eq!(repeated, HandleAnswer::Cancel { withdrawn: false });
    let status: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(status, HandleAnswer::status("Cancelled"));
    let denied: HandleAnswer = f.call(&other, "status", data_value!(id.clone())).await?;
    assert!(denied.is_denied(), "{denied:?}");

    f.complete(&promise).await?;
    let value: u64 = f.call(&counter, "get_value", data_value!()).await?;
    assert_eq!(value, 7, "the withdrawn increment never runs");
    assert!(started(&f.oplog(&target).await?, "inc_by").is_empty());
    let caller_oplog = f.oplog(&caller).await?;
    let awaits = starts_of(&caller_oplog, HANDLE_AWAIT);
    assert_eq!(
        awaits.len(),
        1,
        "a cancelled await is answered once, without retries"
    );
    assert_eq!(ends_of(&caller_oplog, awaits[0]), 1);
    assert_settled_oplog(&caller_oplog, &[], &[]);
    Ok(())
}

/// A scheduled invocation is `Scheduled` through its handle until it is due, an await neither
/// runs it early nor misses its result, and it is `Completed` afterwards. This holds for a
/// durable target and for an ephemeral target that has never started.
#[test]
#[timeout("3 minutes")]
async fn a_scheduled_handle_moves_from_scheduled_to_completed_at_its_due_time(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const LEAD: Duration = Duration::from_secs(3);
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let server = agent_key!("ScheduledInvocationServer", "scheduled-handle-server");
    f.executor
        .start_agent(&f.component.id, server.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "scheduled-handle-probe");
    f.executor
        .start_agent(&f.component.id, probe.clone())
        .await?;

    let (seconds, nanoseconds, due) = due_in(LEAD);
    let id: String = f
        .call(
            &probe,
            "schedule_global_inc",
            data_value!("scheduled-handle-server", 17u64, seconds, nanoseconds),
        )
        .await?;
    let scheduled: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(scheduled, HandleAnswer::status("Scheduled"));
    let early: u64 = f.call(&server, "get_global_value", data_value!()).await?;
    assert_eq!(early, 0);
    let awaited: HandleAnswer = f
        .call(&probe, "await_unit", data_value!(id.clone()))
        .await?;
    assert_eq!(awaited, HandleAnswer::Unit);
    assert!(
        SystemTime::now() >= due,
        "the await must not run the invocation early"
    );
    let completed: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(completed, HandleAnswer::status("Completed"));
    let value: u64 = f.call(&server, "get_global_value", data_value!()).await?;
    assert_eq!(value, 17);

    let (seconds, nanoseconds, due) = due_in(LEAD);
    let target = "scheduled-handle-ephemeral";
    let id: String = f
        .call(
            &probe,
            "schedule_ephemeral_inc",
            data_value!(target, 0u64, seconds, nanoseconds),
        )
        .await?;
    let scheduled: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(scheduled, HandleAnswer::status("Scheduled"));
    let effects: (u64, u64) = f
        .call(&probe, "ephemeral_effects", data_value!(target))
        .await?;
    assert_eq!(effects, (0, 0), "the ephemeral target has not started");
    let awaited: HandleAnswer = f
        .call(&probe, "await_unit", data_value!(id.clone()))
        .await?;
    assert_eq!(awaited, HandleAnswer::Unit);
    assert!(SystemTime::now() >= due);
    let effects: (u64, u64) = f
        .call(&probe, "ephemeral_effects", data_value!(target))
        .await?;
    assert_eq!(effects, (1, 1), "the ephemeral target ran once");
    let completed: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(completed, HandleAnswer::status("Completed"));
    Ok(())
}

/// A cancel near a durable schedule's due time either withdraws the invocation, which then
/// never runs and awaits as cancelled, or finds it started and is refused; it never reports a
/// withdrawal of an invocation that runs. The first and last rounds are certain to take one
/// side each.
#[test]
#[timeout("4 minutes")]
async fn a_cancel_racing_a_scheduled_start_withdraws_it_or_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const LEAD: Duration = Duration::from_secs(2);
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let counter = agent_key!("RpcBlockingCounter", "racing-schedule-target");
    let target = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "racing-schedule-probe");
    f.executor
        .start_agent(&f.component.id, probe.clone())
        .await?;

    // Offsets from the due time, in milliseconds; `None` waits until the invocation runs.
    let rounds: [Option<i64>; 7] = [
        Some(-1500),
        Some(-50),
        Some(-10),
        Some(10),
        Some(50),
        Some(400),
        None,
    ];
    let mut refused = 0;
    for offset in rounds {
        let promise = f.promise(&counter).await?;
        let (seconds, nanoseconds, due) = due_in(LEAD);
        let id: String = f
            .call(
                &probe,
                "schedule_blocked",
                data_value!(
                    "racing-schedule-target",
                    promise.clone(),
                    seconds,
                    nanoseconds
                ),
            )
            .await?;
        match offset {
            Some(offset) => {
                let at = if offset < 0 {
                    due - Duration::from_millis(offset.unsigned_abs())
                } else {
                    due + Duration::from_millis(offset as u64)
                };
                if let Ok(wait) = at.duration_since(SystemTime::now()) {
                    tokio::time::sleep(wait).await;
                }
            }
            None => {
                tokio::time::timeout(PENDING_TIMEOUT, async {
                    loop {
                        let status: HandleAnswer =
                            f.call(&probe, "status", data_value!(id.clone())).await?;
                        if status.has_started() {
                            return Ok::<_, anyhow::Error>(());
                        }
                        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                    }
                })
                .await??;
            }
        }
        let cancel: HandleAnswer = f.call(&probe, "cancel", data_value!(id.clone())).await?;
        f.complete(&promise).await?;
        let awaited: HandleAnswer = f
            .call(&probe, "await_value", data_value!(id.clone()))
            .await?;
        match &cancel {
            HandleAnswer::Cancel { withdrawn: true } => {
                assert!(awaited.is_cancelled(), "offset {offset:?}: {awaited:?}");
                let status: HandleAnswer =
                    f.call(&probe, "status", data_value!(id.clone())).await?;
                assert_eq!(status, HandleAnswer::status("Cancelled"));
            }
            answer if answer.is_cannot_cancel_running() => {
                refused += 1;
                assert_eq!(
                    awaited,
                    HandleAnswer::Value { value: 7 * refused },
                    "offset {offset:?}: a refused cancellation leaves the invocation running"
                );
            }
            other => panic!("offset {offset:?}: unexpected cancel answer {other:?}"),
        }
        if offset == Some(-1500) {
            assert_eq!(cancel, HandleAnswer::Cancel { withdrawn: true });
        }
        if offset.is_none() {
            assert!(cancel.is_cannot_cancel_running(), "{cancel:?}");
        }
    }
    assert_eq!(
        started(&f.oplog(&target).await?, "inc_after_promise").len() as u64,
        refused,
        "exactly the invocations whose cancellation was refused ran"
    );
    let value: u64 = f.call(&counter, "get_value", data_value!()).await?;
    assert_eq!(value, 7 * refused);
    Ok(())
}

/// Dropping one of two observers of a pending invocation releases only that observation: the
/// other observer receives the result and the invocation runs once.
#[test]
#[timeout("2 minutes")]
async fn dropping_one_of_two_observers_does_not_cancel_the_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let counter = agent_key!("RpcBlockingCounter", "two-observers-target");
    let target = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "two-observers-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let promise = f.promise(&counter).await?;
    let _: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("two-observers-target", promise.clone()),
        )
        .await?;
    let id: String = f
        .call(
            &probe,
            "trigger_inc_by",
            data_value!("two-observers-target", 1u64),
        )
        .await?;

    let observing = f.spawn(
        &probe,
        &IdempotencyKey::fresh(),
        "two_observers",
        data_value!(id.clone()),
    );
    f.await_starts(&caller, 2).await?;
    f.complete(&promise).await?;
    let answer = observing.await??.into_typed::<HandleAnswer>()?;
    assert_eq!(
        answer,
        HandleAnswer::Unit,
        "the remaining observer receives the result"
    );
    let value: u64 = f.call(&counter, "get_value", data_value!()).await?;
    assert_eq!(value, 8, "the pending increment ran after the blocked one");
    assert_eq!(started(&f.oplog(&target).await?, "inc_by").len(), 1);
    Ok(())
}

/// An invocation ID kept in the agent state survives a restore from a snapshot, which keeps no
/// handle: the rebuilt handle receives the typed result, and the target ran the invocation once.
#[test]
#[timeout("2 minutes")]
async fn a_kept_invocation_id_rebuilds_its_handle_after_a_snapshot_restore(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, snapshot_every_invocation()).await?;
    let counter = agent_key!("RpcBlockingCounter", "snapshot-handle-target");
    let target = f
        .executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "snapshot-handle-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let promise = f.promise(&counter).await?;
    let id: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("snapshot-handle-target", promise.clone()),
        )
        .await?;
    f.unit(&probe, "keep", data_value!(id.clone())).await?;
    assert!(
        f.oplog(&caller)
            .await?
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
        "the probe takes a snapshot after its invocations"
    );

    let f = f
        .restart(deps, &context, snapshot_every_invocation())
        .await?;
    let restored: bool = f.call(&probe, "restored", data_value!()).await?;
    assert!(restored, "the probe was restored from its snapshot");
    let kept: Option<String> = f.call(&probe, "kept", data_value!()).await?;
    assert_eq!(kept.as_deref(), Some(id.as_str()));
    f.complete(&promise).await?;
    let awaited: HandleAnswer = f.call(&probe, "await_value", data_value!(id)).await?;
    assert_eq!(awaited, HandleAnswer::Value { value: 7 });
    assert_eq!(
        started(&f.oplog(&target).await?, "inc_after_promise").len(),
        1
    );
    assert_settled_oplog(&f.oplog(&caller).await?, &[], &[]);
    Ok(())
}

fn without_snapshots() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    }
}

/// A status read through a handle is recorded: when the caller is reconstructed by replaying its
/// oplog, the read answers the status it observed then (`Pending`), not the target's current
/// one, and replay makes no new status call. A fresh read afterwards observes `Completed`.
#[test]
#[timeout("3 minutes")]
async fn a_replayed_status_read_returns_the_recorded_intermediate_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, without_snapshots()).await?;
    let counter = agent_key!("RpcBlockingCounter", "status-replay-target");
    f.executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let probe = agent_key!("HandleProbe", "status-replay-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let promise = f.promise(&counter).await?;
    let _: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("status-replay-target", promise.clone()),
        )
        .await?;
    let id: String = f
        .call(
            &probe,
            "trigger_inc_by",
            data_value!("status-replay-target", 1u64),
        )
        .await?;
    let observed: HandleAnswer = f
        .call(&probe, "status_and_keep", data_value!(id.clone()))
        .await?;
    assert_eq!(observed, HandleAnswer::status("Pending"));

    f.complete(&promise).await?;
    let completed: HandleAnswer = f
        .call(&probe, "await_unit", data_value!(id.clone()))
        .await?;
    assert_eq!(completed, HandleAnswer::Unit);
    let status_reads = starts_of(&f.oplog(&caller).await?, HANDLE_STATUS).len();

    let f = f.restart(deps, &context, without_snapshots()).await?;
    let replayed: Option<HandleAnswer> = f.call(&probe, "last", data_value!()).await?;
    assert_eq!(
        replayed,
        Some(HandleAnswer::status("Pending")),
        "replay returns the status the caller observed, not the current one"
    );
    let restored: bool = f.call(&probe, "restored", data_value!()).await?;
    assert!(
        !restored,
        "the probe was reconstructed by replay, not from a snapshot"
    );
    assert_eq!(
        starts_of(&f.oplog(&caller).await?, HANDLE_STATUS).len(),
        status_reads,
        "replay records no new status read"
    );
    let fresh: HandleAnswer = f.call(&probe, "status", data_value!(id)).await?;
    assert_eq!(fresh, HandleAnswer::status("Completed"));
    assert_settled_oplog(&f.oplog(&caller).await?, &[], &[]);
    Ok(())
}

/// A handle await lost after its `Start` (while pending) or after its `End` (before the guest
/// received the answer) is repaired with the same recorded call and attaches to the same
/// invocation: one await `Start` with one `End`, one target run. Replaying the completed await
/// later needs no target: it succeeds after the target is deleted.
#[test]
#[timeout("5 minutes")]
async fn a_handle_await_recovers_from_committed_crash_prefixes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (checkpoint, suffix) in [(RpcCheckpoint::Start, "start"), (RpcCheckpoint::End, "end")] {
        let context = TestContext::new(last_unique_id);
        let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
        let counter_name = format!("await-crash-{suffix}-target");
        let counter = agent_key!("RpcBlockingCounter", counter_name.clone());
        let target = f
            .executor
            .start_agent(&f.component.id, counter.clone())
            .await?;
        let probe = agent_key!("HandleProbe", format!("await-crash-{suffix}-probe"));
        let caller = f
            .executor
            .start_agent(&f.component.id, probe.clone())
            .await?;
        let promise = f.promise(&counter).await?;
        let id: String = f
            .call(
                &probe,
                "trigger_blocked",
                data_value!(counter_name, promise.clone()),
            )
            .await?;

        let mut gate = f
            .executor
            .gate_next_rpc_commit(&caller.agent_id, checkpoint)
            .await;
        let key = IdempotencyKey::fresh();
        let awaiting = f.spawn(&probe, &key, "await_and_keep", data_value!(id.clone()));
        if checkpoint == RpcCheckpoint::End {
            f.complete(&promise).await?;
        }
        gate.committed().await;
        let prefix = f.executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        let awaits = starts_of(&prefix, HANDLE_AWAIT);
        assert_eq!(awaits.len(), 1, "checkpoint {checkpoint:?}");
        assert_eq!(
            ends_of(&prefix, awaits[0]),
            usize::from(checkpoint == RpcCheckpoint::End),
            "checkpoint {checkpoint:?}"
        );
        gate.abort_return();
        awaiting.abort();
        drop(gate);

        let f = f
            .restart(deps, &context, TestExecutorOverrides::default())
            .await?;
        if checkpoint == RpcCheckpoint::Start {
            f.complete(&promise).await?;
        }
        let resumed = f
            .executor
            .invoke_and_await_agent_with_key(
                &f.component,
                &probe,
                &key,
                "await_and_keep",
                data_value!(id.clone()),
            )
            .await?
            .into_typed::<HandleAnswer>()?;
        assert_eq!(
            resumed,
            HandleAnswer::Value { value: 7 },
            "checkpoint {checkpoint:?}"
        );
        let recovered = f.oplog(&caller).await?;
        assert_eq!(
            starts_of(&recovered, HANDLE_AWAIT),
            awaits,
            "recovery keeps the original await at {checkpoint:?}"
        );
        assert_eq!(
            ends_of(&recovered, awaits[0]),
            1,
            "checkpoint {checkpoint:?}"
        );
        assert_settled_oplog(&recovered, &[], &[]);
        assert_eq!(
            started(&f.oplog(&target).await?, "inc_after_promise").len(),
            1,
            "the awaited invocation ran once at {checkpoint:?}"
        );

        f.executor.delete_worker(&target).await?;
        let f = f
            .restart(deps, &context, TestExecutorOverrides::default())
            .await?;
        let last: Option<HandleAnswer> = f.call(&probe, "last", data_value!()).await?;
        assert_eq!(
            last,
            Some(HandleAnswer::Value { value: 7 }),
            "the completed await replays without its deleted target at {checkpoint:?}"
        );
        assert!(
            f.executor.get_worker_metadata_opt(&target).await?.is_none(),
            "replay does not re-create the target at {checkpoint:?}"
        );
    }
    Ok(())
}

/// A schedule lost after its `Start` (before acceptance) or after its `End` (before the guest
/// received the handle) is repaired with the same recorded call: one schedule `Start` with one
/// `End`, the same invocation ID, and one run of the scheduled method.
#[test]
#[timeout("5 minutes")]
async fn a_schedule_recovers_from_committed_crash_prefixes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const SCHEDULE: &str = "golem::rpc::wasm-rpc::schedule";
    for (checkpoint, suffix) in [(RpcCheckpoint::Start, "start"), (RpcCheckpoint::End, "end")] {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let caller_component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let counter_component = executor
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let caller_id = agent_key!("GolemHostApi", format!("schedule-crash-{suffix}-caller"));
        let caller = executor
            .start_agent(&caller_component.id, caller_id.clone())
            .await?;
        let counter_name = format!("schedule-crash-{suffix}-target");
        let counter_id = agent_key!("Counter", counter_name.clone());
        let input = data_value!("Counter", counter_name.clone(), "increment");
        let key = IdempotencyKey::fresh();
        let mut gate = executor
            .gate_next_rpc_commit(&caller.agent_id, checkpoint)
            .await;
        let scheduling = {
            let executor = executor.clone();
            let caller_component = caller_component.clone();
            let caller_id = caller_id.clone();
            let key = key.clone();
            let input = input.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &caller_component,
                        &caller_id,
                        &key,
                        "outbound_agent_rpc_schedule_result",
                        input,
                    )
                    .await
            })
        };
        gate.committed().await;
        let prefix = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        let schedules = starts_of(&prefix, SCHEDULE);
        assert_eq!(schedules.len(), 1, "checkpoint {checkpoint:?}");
        assert_eq!(
            ends_of(&prefix, schedules[0]),
            usize::from(checkpoint == RpcCheckpoint::End),
            "checkpoint {checkpoint:?}"
        );
        gate.abort_return();
        scheduling.abort();
        drop(gate);
        drop(executor);

        let executor = start(deps, &context).await?;
        let id = executor
            .invoke_and_await_agent_with_key(
                &caller_component,
                &caller_id,
                &key,
                "outbound_agent_rpc_schedule_result",
                input,
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        let id = id.map_err(anyhow::Error::msg)?;
        let (_, scheduled_id) = id.split_once('.').expect("an invocation ID");
        let scheduled_id: uuid::Uuid = scheduled_id.parse()?;
        executor.commit_oplog(&caller.agent_id).await?;
        let recovered = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        assert_eq!(
            starts_of(&recovered, SCHEDULE),
            schedules,
            "recovery keeps the original schedule at {checkpoint:?}"
        );
        assert_eq!(
            ends_of(&recovered, schedules[0]),
            1,
            "checkpoint {checkpoint:?}"
        );
        assert_settled_oplog(&recovered, &[], &[]);

        let counter = executor
            .start_agent(&counter_component.id, counter_id.clone())
            .await?;
        let ran = tokio::time::timeout(PENDING_TIMEOUT, async {
            loop {
                executor.commit_oplog(&counter.agent_id).await?;
                let oplog = executor.get_oplog(&counter, OplogIndex::INITIAL).await?;
                if let Some(ran) = started_invocation_ids(&oplog, "increment").first() {
                    return Ok::<_, anyhow::Error>(*ran);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await??;
        assert_eq!(
            ran, scheduled_id,
            "the target runs the invocation whose ID the recovered schedule returned at {checkpoint:?}"
        );
        let next = executor
            .invoke_and_await_agent(&counter_component, &counter_id, "increment", data_value!())
            .await?
            .into_typed::<u32>()?;
        assert_eq!(
            next, 2,
            "the scheduled increment ran once at {checkpoint:?}"
        );
    }
    Ok(())
}

/// Two handle awaits of one caller invocation complete in the opposite order from their start,
/// across a restart between the two completions. Each answer stays attributed to its own call,
/// live and in replay, and a later invocation of the caller is admitted only after the one
/// awaiting both.
#[test]
#[timeout("3 minutes")]
async fn two_concurrent_handle_awaits_keep_their_attribution_across_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let a = agent_key!("RpcBlockingCounter", "concurrent-handles-a");
    let b = agent_key!("RpcBlockingCounter", "concurrent-handles-b");
    let target_a = f.executor.start_agent(&f.component.id, a.clone()).await?;
    let target_b = f.executor.start_agent(&f.component.id, b.clone()).await?;
    let probe = agent_key!("HandleProbe", "concurrent-handles-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    f.unit(&b, "inc_by", data_value!(10u64)).await?;
    let gate_a = f.promise(&a).await?;
    let gate_b = f.promise(&b).await?;
    let id_a: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("concurrent-handles-a", gate_a.clone()),
        )
        .await?;
    let id_b: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("concurrent-handles-b", gate_b.clone()),
        )
        .await?;

    let key = IdempotencyKey::fresh();
    let input = data_value!(id_a.clone(), id_b.clone());
    let awaiting = f.spawn(&probe, &key, "await_two", input.clone());
    let starts = f.await_starts(&caller, 2).await?;
    let oplog = f.oplog(&caller).await?;
    let start_awaiting = |id: &str| {
        let wanted = invocation_uuid(id);
        *starts
            .iter()
            .find(|start| awaited_invocation(&oplog, **start) == wanted)
            .expect("an await Start for the invocation")
    };
    let (start_a, start_b) = (start_awaiting(&id_a), start_awaiting(&id_b));
    assert_ne!(start_a, start_b);
    let queued = f.spawn(&probe, &IdempotencyKey::fresh(), "kept", data_value!());
    tokio::time::timeout(PENDING_TIMEOUT, async {
        while !accepted(&f.oplog(&caller).await?, "kept") {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    f.complete(&gate_b).await?;
    tokio::time::timeout(PENDING_TIMEOUT, async {
        loop {
            let oplog = f.oplog(&caller).await?;
            if ends_of(&oplog, start_b) == 1 {
                assert_eq!(
                    ends_of(&oplog, start_a),
                    0,
                    "the earlier await is still open when the later one completes"
                );
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await??;
    awaiting.abort();
    queued.abort();

    let f = f
        .restart(deps, &context, TestExecutorOverrides::default())
        .await?;
    f.complete(&gate_a).await?;
    let pair = f
        .executor
        .invoke_and_await_agent_with_key(&f.component, &probe, &key, "await_two", input)
        .await?
        .into_typed::<(HandleAnswer, HandleAnswer)>()?;
    let expected = (
        HandleAnswer::Value { value: 7 },
        HandleAnswer::Value { value: 17 },
    );
    assert_eq!(pair, expected, "each answer belongs to its own call");
    assert_eq!(
        started(&f.oplog(&target_a).await?, "inc_after_promise").len(),
        1
    );
    assert_eq!(
        started(&f.oplog(&target_b).await?, "inc_after_promise").len(),
        1
    );

    let f = f
        .restart(deps, &context, TestExecutorOverrides::default())
        .await?;
    let replayed: Option<(HandleAnswer, HandleAnswer)> =
        f.call(&probe, "last_pair", data_value!()).await?;
    assert_eq!(
        replayed,
        Some(expected),
        "replay keeps each answer with its call"
    );
    let oplog = f.oplog(&caller).await?;
    let mut recovered = vec![start_a, start_b];
    recovered.sort();
    assert_eq!(
        starts_of(&oplog, HANDLE_AWAIT),
        recovered,
        "recovery keeps both original awaits"
    );
    assert_settled_oplog(&oplog, &[], &[]);
    let end_a = oplog
        .iter()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start_a))
        .expect("the earlier await ended")
        .oplog_index;
    let kept_started = started(&oplog, "kept");
    assert_eq!(kept_started.len(), 1, "the queued invocation ran once");
    assert!(
        kept_started[0] > end_a,
        "the queued invocation starts only after the invocation awaiting both handles"
    );
    Ok(())
}

/// Awaiting an invocation that failed gives the guest a typed, catchable error after one
/// recorded await, and the handle reports `Failed`; cancelling it is a no-op.
#[test]
#[timeout("4 minutes")]
async fn a_failed_invocation_reaches_the_guest_as_a_typed_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let probe = agent_key!("HandleProbe", "failed-invocation-probe");
    let caller = f
        .executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let id: String = f
        .call(
            &probe,
            "trigger_failing",
            data_value!("failed-invocation-tester", "failed-invocation-counter"),
        )
        .await?;
    let awaited: HandleAnswer = f
        .call(&probe, "await_unit", data_value!(id.clone()))
        .await?;
    assert!(
        matches!(
            awaited,
            HandleAnswer::Error {
                error: RpcCallOutcome::RemoteInternalError { .. }
            }
        ),
        "{awaited:?}"
    );
    let status: HandleAnswer = f.call(&probe, "status", data_value!(id.clone())).await?;
    assert_eq!(status, HandleAnswer::status("Failed"));
    let cancel: HandleAnswer = f.call(&probe, "cancel", data_value!(id)).await?;
    assert_eq!(cancel, HandleAnswer::Cancel { withdrawn: false });
    let oplog = f.oplog(&caller).await?;
    let awaits = starts_of(&oplog, HANDLE_AWAIT);
    assert_eq!(
        awaits.len(),
        1,
        "a failed result is answered once, without retries"
    );
    assert_eq!(ends_of(&oplog, awaits[0]), 1);
    Ok(())
}

/// The host keeps live streams to awaited calls: a guest `trigger` of a stream-returning method
/// gets a typed protocol error and a guest `schedule` of it traps, and neither reaches the
/// target. A read-only method may read a handle's status but traps on `cancel`, which leaves the
/// invocation running.
#[test]
#[timeout("3 minutes")]
async fn guest_handle_calls_keep_their_stream_and_read_only_restrictions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let f = Fixture::start(deps, &context, fixture, TestExecutorOverrides::default()).await?;
    let stream_target = "restricted-stream-target";
    let probe = agent_key!("HandleProbe", "restricted-probe");
    f.executor
        .start_agent(&f.component.id, probe.clone())
        .await?;
    let triggered: HandleAnswer = f
        .call(&probe, "trigger_stream_output", data_value!(stream_target))
        .await?;
    assert!(
        matches!(
            triggered,
            HandleAnswer::Error {
                error: RpcCallOutcome::ProtocolError { .. }
            }
        ),
        "{triggered:?}"
    );
    let scheduling_probe = agent_key!("HandleProbe", "restricted-scheduling-probe");
    f.executor
        .start_agent(&f.component.id, scheduling_probe.clone())
        .await?;
    let scheduled = f
        .executor
        .invoke_and_await_agent(
            &f.component,
            &scheduling_probe,
            "schedule_stream_output",
            data_value!(stream_target),
        )
        .await;
    let error = scheduled.expect_err("a scheduled live stream traps");
    assert!(
        format!("{error:?}").contains("live streams cannot be used in scheduled invocations"),
        "{error:?}"
    );
    let stream_target_id = f
        .executor
        .start_agent(
            &f.component.id,
            agent_key!("StreamingRpcTarget", stream_target),
        )
        .await?;
    assert!(
        started(&f.oplog(&stream_target_id).await?, "produce").is_empty(),
        "neither call reaches the target"
    );

    let counter = agent_key!("RpcBlockingCounter", "restricted-counter");
    f.executor
        .start_agent(&f.component.id, counter.clone())
        .await?;
    let promise = f.promise(&counter).await?;
    let id: String = f
        .call(
            &probe,
            "trigger_blocked",
            data_value!("restricted-counter", promise.clone()),
        )
        .await?;
    f.until_started(&probe, "read_only_status", data_value!(id.clone()))
        .await?;
    let read_only_probe = agent_key!("HandleProbe", "restricted-read-only-probe");
    f.executor
        .start_agent(&f.component.id, read_only_probe.clone())
        .await?;
    let cancel = f
        .executor
        .invoke_and_await_agent(
            &f.component,
            &read_only_probe,
            "read_only_cancel",
            data_value!(id.clone()),
        )
        .await;
    let error = cancel.expect_err("a read-only method cannot cancel");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<WorkerExecutorError>(),
            Some(WorkerExecutorError::InvocationFailed {
                error: golem_common::model::oplog::AgentError::ReadOnlyViolation(violation),
                ..
            }) if violation.host_function == "golem::rpc::invocation::cancel"
        ) || matches!(
            cause.downcast_ref::<WorkerExecutorError>(),
            Some(WorkerExecutorError::ReadOnlyViolation { host_function, .. })
                if host_function == "golem::rpc::invocation::cancel"
        )),
        "a read-only cancel fails with a typed read-only violation: {error:?}"
    );
    let status: HandleAnswer = f.call(&probe, "held_status", data_value!()).await?;
    assert!(
        status.has_started(),
        "the invocation keeps running: {status:?}"
    );
    f.complete(&promise).await?;
    let awaited: HandleAnswer = f.call(&probe, "held_await", data_value!()).await?;
    assert_eq!(awaited, HandleAnswer::Value { value: 7 });
    Ok(())
}
