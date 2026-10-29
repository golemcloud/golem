//! Replays of agents that release RPC handles and futures at points that differ between the
//! original run and a run restored from an application snapshot (see `gc_timing.rs` in the
//! `agent-rpc-rust` test component). Releasing a client records no durable call, so a replay
//! that releases at other points still claims the same history.

use super::*;
use golem_common::schema::IntoSchema;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);

const WASM_RPC_DROP: &str = "golem::rpc::wasm-rpc::drop";
const WASM_RPC_NEW: &str = "golem::rpc::wasm-rpc::new";
const REPLAY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, IntoSchema)]
struct ReleaseSchedule {
    every: u32,
    reverse: bool,
    release_in_save: bool,
}

const RETAIN: ReleaseSchedule = ReleaseSchedule {
    every: 0,
    reverse: false,
    release_in_save: false,
};
const IMMEDIATE: ReleaseSchedule = ReleaseSchedule {
    every: 1,
    reverse: false,
    release_in_save: false,
};

const fn every(n: u32) -> ReleaseSchedule {
    ReleaseSchedule {
        every: n,
        reverse: false,
        release_in_save: false,
    }
}

#[derive(Debug, Clone, Copy, IntoSchema)]
struct AbandonSchedule {
    drop_at: u32,
}

const BEFORE_NEXT_CALL: AbandonSchedule = AbandonSchedule { drop_at: 0 };
const AFTER_NEXT_CALL: AbandonSchedule = AbandonSchedule { drop_at: 1 };
const NEVER: AbandonSchedule = AbandonSchedule { drop_at: 2 };

#[derive(Debug, Clone, PartialEq, FromSchema)]
struct GcTimingReport {
    completed: u64,
    checksum: u64,
    restored: bool,
    retained: u32,
    retained_futures: u32,
}

/// Overrides that take an automatic snapshot after every invocation, so the snapshot taken
/// after the last finished invocation is the cut a restart restores from.
fn snapshot_every_invocation() -> TestExecutorOverrides {
    snapshot_every_nth_invocation(1)
}

fn snapshot_every_nth_invocation(count: u16) -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count };
            config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
        })),
        ..Default::default()
    }
}

fn triangular(n: u64) -> u64 {
    n * (n + 1) / 2
}

struct Caller {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    id: ParsedAgentId,
    agent: AgentId,
    target: ParsedAgentId,
}

impl Caller {
    async fn start(
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        fixture: &PrecompiledComponent,
        overrides: TestExecutorOverrides,
        name: &str,
    ) -> anyhow::Result<Self> {
        let executor = start_with_overrides(deps, context, overrides).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let id = agent_id!("GcTimingCaller", name);
        let agent = executor.start_agent(&component.id, id.clone()).await?;
        Ok(Self {
            executor,
            component,
            id,
            agent,
            target: agent_id!("GcTimingTarget", name),
        })
    }

    /// Drops the executor and starts a new one over the same storage.
    async fn restart(
        self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let Self {
            executor,
            component,
            id,
            agent,
            target,
        } = self;
        drop(executor);
        let executor = start_with_overrides(deps, context, overrides).await?;
        Ok(Self {
            executor,
            component,
            id,
            agent,
            target,
        })
    }

    async fn invoke(&self, method: &str, params: TypedSchemaValue) -> anyhow::Result<AgentResult> {
        tokio::time::timeout(
            REPLAY_TIMEOUT,
            self.executor
                .invoke_and_await_agent(&self.component, &self.id, method, params),
        )
        .await
        .map_err(|_| anyhow::anyhow!("{method} did not complete within {REPLAY_TIMEOUT:?}"))?
    }

    async fn configure(
        &self,
        live: ReleaseSchedule,
        after_restore: ReleaseSchedule,
    ) -> anyhow::Result<()> {
        self.invoke("configure", data_value!(live, after_restore))
            .await?;
        Ok(())
    }

    async fn create_promise(&self) -> anyhow::Result<PromiseId> {
        self.invoke("create_promise", data_value!())
            .await?
            .into_typed()
    }

    /// Starts `method` without awaiting it, and returns once the caller is suspended on the
    /// promise the method awaits.
    async fn start_and_suspend(
        &self,
        key: &IdempotencyKey,
        method: &str,
        params: TypedSchemaValue,
    ) -> anyhow::Result<()> {
        self.executor
            .invoke_agent_with_key(&self.component, &self.id, key, method, params)
            .await?;
        self.executor
            .wait_for_status(&self.agent, AgentStatus::Suspended, REPLAY_TIMEOUT)
            .await?;
        Ok(())
    }

    async fn run(&self, count: u32) -> anyhow::Result<u64> {
        self.invoke("run", data_value!(count)).await?.into_typed()
    }

    async fn inspect(&self) -> anyhow::Result<GcTimingReport> {
        self.invoke("inspect", data_value!()).await?.into_typed()
    }

    async fn target_count(&self, target: &ParsedAgentId) -> anyhow::Result<u64> {
        self.executor
            .invoke_and_await_agent(&self.component, target, "get", data_value!())
            .await?
            .into_typed()
    }

    async fn released_spans(&self) -> anyhow::Result<usize> {
        Ok(self
            .oplog()
            .await?
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::SpanClosed(_)))
            .count())
    }

    async fn oplog(&self) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        // An unloaded (suspended) agent has committed its whole oplog.
        if self
            .executor
            .worker_is_loaded(&OwnedAgentId::new(
                self.component.environment_id,
                &self.agent,
            ))
            .await
        {
            self.executor.commit_oplog(&self.agent).await?;
        }
        self.executor
            .get_oplog(&self.agent, OplogIndex::INITIAL)
            .await
    }
}

fn is_snapshot(entry: &PublicOplogEntryWithIndex) -> bool {
    matches!(&entry.entry, PublicOplogEntry::Snapshot(_))
}

fn starts_named<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    name: &'a str,
) -> impl Iterator<Item = OplogIndex> + 'a {
    oplog.iter().filter_map(move |entry| match &entry.entry {
        PublicOplogEntry::Start(start) if start.function_name == name => Some(entry.oplog_index),
        _ => None,
    })
}

/// The settled-history rules every test in this file checks: no `wasm-rpc::drop` call, every
/// `Start` has a terminal, every `SpanClosed` refers to a `wasm-rpc::new` span, and nothing
/// positional follows the last `AgentInvocationFinished`.
fn assert_settled_history(oplog: &[PublicOplogEntryWithIndex]) {
    assert_eq!(
        starts_named(oplog, WASM_RPC_DROP).count(),
        0,
        "releasing a client must not record a durable call"
    );
    let clients = starts_named(oplog, WASM_RPC_NEW).collect::<HashSet<_>>();
    for entry in oplog {
        if let PublicOplogEntry::SpanClosed(params) = &entry.entry {
            let start = params
                .resource_start_index
                .expect("a recorded SpanClosed names the Start that opened the span");
            assert!(
                clients.contains(&start),
                "the SpanClosed at {} names {start}, which is not a wasm-rpc::new Start",
                entry.oplog_index
            );
        }
    }
    for entry in oplog {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            let index = entry.oplog_index;
            assert!(
                oplog.iter().any(|terminal| match &terminal.entry {
                    PublicOplogEntry::End(end) => end.start_index == index,
                    PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == index,
                    _ => false,
                }),
                "the Start of {} at {index} has no terminal",
                start.function_name
            );
        }
    }
    let last_finished = oplog
        .iter()
        .rev()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|entry| entry.oplog_index)
        .expect("the history has a finished invocation");
    assert!(
        !oplog.iter().any(|entry| entry.oplog_index > last_finished
            && matches!(
                &entry.entry,
                PublicOplogEntry::Start(_)
                    | PublicOplogEntry::End(_)
                    | PublicOplogEntry::Cancelled(_)
            )),
        "no positional entry may follow the last AgentInvocationFinished"
    );
}

/// A suspended invocation that a restart replays with the heap restored from the last snapshot.
struct Scenario {
    name: &'static str,
    live: ReleaseSchedule,
    after_restore: ReleaseSchedule,
    /// Iterations of `run` before the suspended invocation.
    setup_runs: u32,
    release_first: bool,
    count: u32,
    two_targets: bool,
    /// Clients the live run releases inside the suspended invocation before the restart.
    releases_before_crash: usize,
    /// Clients the resumed invocation releases live after the promise completes.
    releases_after_resume: usize,
    /// Increments of the first target when the invocation finishes.
    completed: u64,
}

/// Runs `scenario` up to a suspended `run_then_await`, restarts the executor, completes the
/// promise and checks that the restored agent replays the suspended invocation and continues.
async fn crash_while_awaiting(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    fixture: &PrecompiledComponent,
    scenario: Scenario,
) -> anyhow::Result<Caller> {
    let caller = Caller::start(
        deps,
        context,
        fixture,
        snapshot_every_invocation(),
        scenario.name,
    )
    .await?;
    caller
        .configure(scenario.live, scenario.after_restore)
        .await?;
    if scenario.setup_runs > 0 {
        caller.run(scenario.setup_runs).await?;
    }
    let promise = caller.create_promise().await?;
    let released_before = caller.released_spans().await?;
    caller
        .start_and_suspend(
            &IdempotencyKey::fresh(),
            "run_then_await",
            data_value!(
                scenario.release_first,
                scenario.count,
                scenario.two_targets,
                promise.clone()
            ),
        )
        .await?;
    let oplog = caller.oplog().await?;
    let mut closed = HashSet::new();
    for entry in &oplog {
        if let PublicOplogEntry::SpanClosed(params) = &entry.entry {
            assert!(
                closed.insert(params.resource_start_index),
                "before reconstruction each resource is released at most once"
            );
        }
    }
    let snapshots = oplog.iter().filter(|entry| is_snapshot(entry)).count();
    assert!(
        snapshots >= 2,
        "a snapshot follows every finished invocation"
    );
    let released_after_restart = caller.released_spans().await?;
    assert_eq!(
        released_after_restart - released_before,
        scenario.releases_before_crash,
        "the live run records one SpanClosed per released client"
    );

    let caller = caller
        .restart(deps, context, snapshot_every_invocation())
        .await?;
    caller.executor.complete_promise(&promise, vec![]).await?;
    let report = caller.inspect().await?;
    assert_eq!(report.completed, scenario.completed);
    assert_eq!(
        report.checksum,
        expected_checksum(scenario.completed, scenario.two_targets, scenario.count)
    );
    assert!(report.restored, "the agent was restored from a snapshot");
    assert_eq!(
        caller.target_count(&caller.target).await?,
        scenario.completed
    );
    let oplog = caller.oplog().await?;
    assert_settled_history(&oplog);
    assert_eq!(
        caller.released_spans().await?,
        released_after_restart + scenario.releases_after_resume,
        "replay appends no SpanClosed; only the releases of the resumed live run are recorded"
    );
    Ok(caller)
}

/// The checksum of `completed` increments of the first target, plus the increments of the
/// second target in a two-target run of `count` iterations.
fn expected_checksum(completed: u64, two_targets: bool, count: u32) -> u64 {
    triangular(completed)
        + if two_targets {
            triangular(count as u64)
        } else {
            0
        }
}

/// Handles created before the snapshot cut and released after it: the restored heap does not
/// hold them, so the restored run never releases them. This is the failure reported in
/// GOL-1040.
#[test]
#[timeout("3 minutes")]
async fn snapshot_restore_with_pre_cut_handles_released_after_cut(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = crash_while_awaiting(
        deps,
        &context,
        fixture,
        Scenario {
            name: "pre-cut",
            live: RETAIN,
            after_restore: RETAIN,
            setup_runs: 6,
            release_first: true,
            count: 2,
            two_targets: false,
            releases_before_crash: 6,
            releases_after_resume: 0,
            completed: 9,
        },
    )
    .await?;
    assert_eq!(caller.run(1).await?, 10);
    assert_eq!(caller.target_count(&caller.target).await?, 10);
    Ok(())
}

/// The restored heap releases each client as soon as its call returns; the live run retained
/// them all.
#[test]
#[timeout("3 minutes")]
async fn restored_heap_releases_earlier_than_original(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = crash_while_awaiting(
        deps,
        &context,
        fixture,
        Scenario {
            name: "earlier",
            live: RETAIN,
            after_restore: IMMEDIATE,
            setup_runs: 0,
            release_first: false,
            count: 4,
            two_targets: false,
            releases_before_crash: 0,
            releases_after_resume: 1,
            completed: 5,
        },
    )
    .await?;
    assert_eq!(caller.inspect().await?.retained, 0);
    Ok(())
}

/// The live run released every second client; the restored heap retains all of them.
#[test]
#[timeout("3 minutes")]
async fn restored_heap_releases_later_than_original(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = crash_while_awaiting(
        deps,
        &context,
        fixture,
        Scenario {
            name: "later",
            live: every(2),
            after_restore: RETAIN,
            setup_runs: 0,
            release_first: false,
            count: 4,
            two_targets: false,
            releases_before_crash: 2,
            releases_after_resume: 0,
            completed: 5,
        },
    )
    .await?;
    let report = caller.inspect().await?;
    assert_eq!(
        report.retained, 5,
        "the restored heap retained every client"
    );
    Ok(())
}

/// The live run released every client as soon as its call returned; the restored heap never
/// releases any.
#[test]
#[timeout("3 minutes")]
async fn restored_heap_never_releases(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = crash_while_awaiting(
        deps,
        &context,
        fixture,
        Scenario {
            name: "never",
            live: IMMEDIATE,
            after_restore: RETAIN,
            setup_runs: 0,
            release_first: false,
            count: 4,
            two_targets: false,
            releases_before_crash: 4,
            releases_after_resume: 0,
            completed: 5,
        },
    )
    .await?;
    let report = caller.inspect().await?;
    assert_eq!(report.retained, 5, "the restored heap released nothing");
    Ok(())
}

/// Two targets, two clients per iteration. The live run retains every client and releases them
/// newest first; the restored heap releases each one as soon as its call returns, in creation
/// order.
#[test]
#[timeout("3 minutes")]
async fn multiple_owners_release_in_different_order(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let reverse = ReleaseSchedule {
        every: 0,
        reverse: true,
        release_in_save: false,
    };
    let caller = crash_while_awaiting(
        deps,
        &context,
        fixture,
        Scenario {
            name: "owners",
            live: reverse,
            after_restore: IMMEDIATE,
            setup_runs: 2,
            release_first: true,
            count: 3,
            two_targets: true,
            releases_before_crash: 2,
            releases_after_resume: 1,
            completed: 6,
        },
    )
    .await?;
    let second = agent_id!("GcTimingTarget", "owners-b");
    assert_eq!(caller.target_count(&second).await?, 3);
    Ok(())
}

/// A crash right after the invocation that released the clients: recovery restores the agent
/// and every released client has exactly one `SpanClosed`.
#[test]
#[timeout("3 minutes")]
async fn crash_after_release_keeps_one_span_closed_per_client(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        snapshot_every_invocation(),
        "crash",
    )
    .await?;
    caller.configure(RETAIN, RETAIN).await?;
    assert_eq!(caller.run(3).await?, 3);
    let released: u32 = caller
        .invoke("release_retained", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(released, 3);
    caller.executor.simulated_crash(&caller.agent).await?;

    let report = caller.inspect().await?;
    assert_eq!(report.completed, 3);
    assert_eq!(report.retained, 0);
    assert_eq!(caller.target_count(&caller.target).await?, 3);
    assert_eq!(caller.run(1).await?, 4);
    assert_eq!(caller.target_count(&caller.target).await?, 4);
    let oplog = caller.oplog().await?;
    assert_settled_history(&oplog);
    assert_eq!(caller.released_spans().await?, 3);
    Ok(())
}

/// Releasing clients is permitted in a read-only invocation and records only hints.
#[test]
#[timeout("3 minutes")]
async fn release_in_read_only_invocation_records_only_hints(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        snapshot_every_invocation(),
        "read-only",
    )
    .await?;
    caller.configure(RETAIN, RETAIN).await?;
    assert_eq!(caller.run(3).await?, 3);
    let starts_before = caller
        .oplog()
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
        .count();
    let released: u32 = caller
        .invoke("release_retained_read_only", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(released, 3);
    let oplog = caller.oplog().await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
            .count(),
        starts_before,
        "releasing a client records no durable call"
    );
    assert_eq!(caller.released_spans().await?, 3);
    assert_settled_history(&oplog);
    Ok(())
}

/// `save_snapshot` releases the retained clients before it serializes the state. The releases
/// record nothing inside the snapshot, and a restart restores from that snapshot.
#[test]
#[timeout("3 minutes")]
async fn release_during_snapshot_save_records_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        snapshot_every_invocation(),
        "in-save",
    )
    .await?;
    let release_in_save = ReleaseSchedule {
        every: 0,
        reverse: false,
        release_in_save: true,
    };
    caller.configure(release_in_save, RETAIN).await?;
    assert_eq!(caller.run(3).await?, 3);
    let oplog = caller.oplog().await?;
    assert_eq!(
        caller.released_spans().await?,
        0,
        "a release during snapshot save appends no hint"
    );
    assert_settled_history(&oplog);

    let caller = caller
        .restart(deps, &context, snapshot_every_invocation())
        .await?;
    let report = caller.inspect().await?;
    assert_eq!(report.completed, 3);
    assert!(report.restored);
    assert_eq!(caller.run(1).await?, 4);
    assert_eq!(caller.target_count(&caller.target).await?, 4);
    Ok(())
}

/// After a restore the agent makes one call more than the one recorded: replay must still reject
/// it. Releasing a client is no longer a durable call, but every real call still is.
#[test]
#[timeout("3 minutes")]
async fn negative_control_wrong_ordinal_still_diverges(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        snapshot_every_invocation(),
        "diverge",
    )
    .await?;
    let promise = caller.create_promise().await?;
    let key = IdempotencyKey::fresh();
    caller
        .start_and_suspend(
            &key,
            "misbehave_then_await",
            data_value!("extra", promise.clone()),
        )
        .await?;
    let caller = caller
        .restart(deps, &context, snapshot_every_invocation())
        .await?;
    caller.executor.complete_promise(&promise, vec![]).await?;
    let result = tokio::time::timeout(
        REPLAY_TIMEOUT,
        caller.executor.invoke_and_await_agent_with_key(
            &caller.component,
            &caller.id,
            &key,
            "misbehave_then_await",
            data_value!("extra", promise.clone()),
        ),
    )
    .await?;
    let error = result.expect_err("a replay that makes an extra call must not complete");
    assert!(
        format!("{error:#}").contains("Unexpected oplog entry"),
        "the replay reports the divergence: {error:#}"
    );
    Ok(())
}

/// Runs `abandon_then_await` (an un-awaited RPC future dropped `live` before or after the next
/// call) up to its promise, restarts, and replays it with the future dropped `after_restore`.
async fn abandon_while_awaiting(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    fixture: &PrecompiledComponent,
    name: &str,
    live: AbandonSchedule,
    after_restore: AbandonSchedule,
) -> anyhow::Result<Caller> {
    let caller = Caller::start(deps, context, fixture, snapshot_every_invocation(), name).await?;
    caller
        .invoke("configure_abandon", data_value!(live, after_restore))
        .await?;
    let gate = caller.create_promise().await?;
    let promise = caller.create_promise().await?;
    let blocking = format!("{name}-blocking");
    caller
        .start_and_suspend(
            &IdempotencyKey::fresh(),
            "abandon_then_await",
            data_value!(blocking, gate, false, promise.clone()),
        )
        .await?;
    let live_cancelled = usize::from(live.drop_at != NEVER.drop_at);
    assert_eq!(
        cancelled_count(&caller.oplog().await?),
        live_cancelled,
        "the live run cancels the future once if it drops it"
    );

    let caller = caller
        .restart(deps, context, snapshot_every_invocation())
        .await?;
    caller.executor.complete_promise(&promise, vec![]).await?;
    let report = caller.inspect().await?;
    assert_eq!(report.completed, 1);
    assert_eq!(report.checksum, 1);
    assert!(report.restored, "the agent was restored from a snapshot");
    assert_eq!(caller.target_count(&caller.target).await?, 1);
    let oplog = caller.oplog().await?;
    assert_settled_history(&oplog);
    assert_eq!(
        cancelled_count(&oplog),
        1,
        "the future is cancelled exactly once: replay claims a recorded Cancelled, or records it live"
    );
    Ok(caller)
}

fn cancelled_count(oplog: &[PublicOplogEntryWithIndex]) -> usize {
    oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(_)))
        .count()
}

/// The restored heap drops the un-awaited future before the next call; the live run dropped it
/// after, so the recorded `Cancelled` follows that call.
#[test]
#[timeout("3 minutes")]
async fn replay_drops_future_earlier_than_recorded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    abandon_while_awaiting(
        deps,
        &context,
        fixture,
        "future-earlier",
        AFTER_NEXT_CALL,
        BEFORE_NEXT_CALL,
    )
    .await?;
    Ok(())
}

/// The restored heap drops the un-awaited future after the next call; the live run dropped it
/// before.
#[test]
#[timeout("3 minutes")]
async fn replay_drops_future_later_than_recorded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    abandon_while_awaiting(
        deps,
        &context,
        fixture,
        "future-later",
        BEFORE_NEXT_CALL,
        AFTER_NEXT_CALL,
    )
    .await?;
    Ok(())
}

/// The restored heap never drops the un-awaited future; the agent keeps it in its state and
/// continues live.
#[test]
#[timeout("3 minutes")]
async fn replay_never_drops_future(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = abandon_while_awaiting(
        deps,
        &context,
        fixture,
        "future-never",
        BEFORE_NEXT_CALL,
        NEVER,
    )
    .await?;
    assert_eq!(caller.inspect().await?.retained_futures, 1);
    assert_eq!(caller.run(1).await?, 2);
    assert_eq!(caller.target_count(&caller.target).await?, 2);
    Ok(())
}

/// The restored heap drops an un-awaited future the live run kept, so the oplog has no
/// `Cancelled` for it: replay finishes and the cancellation is recorded live.
#[test]
#[timeout("3 minutes")]
async fn replay_drops_future_the_original_never_dropped(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = abandon_while_awaiting(
        deps,
        &context,
        fixture,
        "future-unrecorded",
        NEVER,
        BEFORE_NEXT_CALL,
    )
    .await?;
    assert_eq!(caller.inspect().await?.retained_futures, 0);
    assert_eq!(caller.run(1).await?, 2);
    Ok(())
}

/// The restored heap drops an un-awaited future one invocation earlier than the live run, whose
/// `Cancelled` belongs to the next invocation. The earlier invocation must finish without
/// waiting for it.
#[test]
#[timeout("3 minutes")]
async fn replay_drops_future_an_invocation_earlier_than_recorded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = || snapshot_every_nth_invocation(2);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        overrides(),
        "future-next-invocation",
    )
    .await?;
    caller
        .invoke("configure_abandon", data_value!(NEVER, BEFORE_NEXT_CALL))
        .await?;
    let gate = caller.create_promise().await?;
    let completed = caller.create_promise().await?;
    caller.executor.complete_promise(&completed, vec![]).await?;
    caller
        .invoke(
            "abandon_then_await",
            data_value!("future-next-invocation-blocking", gate, false, completed),
        )
        .await?;
    let promise = caller.create_promise().await?;
    caller
        .start_and_suspend(
            &IdempotencyKey::fresh(),
            "drop_retained_futures_then_await",
            data_value!(promise.clone()),
        )
        .await?;
    let oplog = caller.oplog().await?;
    assert_eq!(cancelled_count(&oplog), 1);
    let cancelled_at = oplog
        .iter()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(_)))
        .map(|entry| entry.oplog_index)
        .unwrap();
    let invocations = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    let [.., abandon_started, drop_started] = invocations[..] else {
        panic!("the history has the abandoning and the dropping invocations");
    };
    assert!(
        cancelled_at > drop_started,
        "the live run records the Cancelled in the dropping invocation"
    );
    let last_snapshot = oplog
        .iter()
        .rev()
        .find(|entry| is_snapshot(entry))
        .map(|entry| entry.oplog_index)
        .expect("a snapshot precedes the abandoning invocation");
    assert!(
        last_snapshot < abandon_started,
        "the restart restores from a snapshot taken before the abandoning invocation"
    );

    let caller = caller.restart(deps, &context, overrides()).await?;
    caller.executor.complete_promise(&promise, vec![]).await?;
    let report = caller.inspect().await?;
    assert_eq!(report.completed, 1);
    assert!(report.restored, "the agent was restored from a snapshot");
    assert_eq!(report.retained_futures, 0);
    let oplog = caller.oplog().await?;
    assert_settled_history(&oplog);
    assert_eq!(
        cancelled_count(&oplog),
        1,
        "replay claims the recorded Cancelled and appends none"
    );
    Ok(())
}

#[test]
#[timeout("5 minutes")]
async fn cache_futures_replay_with_changed_drop_schedule(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (name, live, replay) in [
        ("cache-earlier", AFTER_NEXT_CALL, BEFORE_NEXT_CALL),
        ("cache-later", BEFORE_NEXT_CALL, AFTER_NEXT_CALL),
        ("cache-never", BEFORE_NEXT_CALL, NEVER),
        ("cache-unrecorded", NEVER, BEFORE_NEXT_CALL),
    ] {
        let context = TestContext::new(last_unique_id);
        let caller =
            Caller::start(deps, &context, fixture, snapshot_every_invocation(), name).await?;
        caller
            .invoke("configure_abandon", data_value!(live, replay))
            .await?;
        let promise = caller.create_promise().await?;
        caller
            .start_and_suspend(
                &IdempotencyKey::fresh(),
                "abandon_cache_then_await",
                data_value!(promise.clone()),
            )
            .await?;
        assert_eq!(
            cancelled_count(&caller.oplog().await?),
            if live.drop_at == 2 { 0 } else { 5 }
        );
        let caller = caller
            .restart(deps, &context, snapshot_every_invocation())
            .await?;
        caller.executor.complete_promise(&promise, vec![]).await?;
        assert!(
            caller
                .invoke("inspect_cache", data_value!())
                .await?
                .into_typed::<bool>()?,
            "{name}: abandoned set/delete must not change the cache"
        );
        assert!(caller.inspect().await?.restored);
        let history = caller.oplog().await?;
        assert_eq!(cancelled_count(&history), 5, "{name}");
        assert_settled_history(&history);
    }
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn cache_futures_replay_drop_in_an_earlier_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let caller = Caller::start(
        deps,
        &context,
        fixture,
        snapshot_every_nth_invocation(2),
        "cache-cross-invocation",
    )
    .await?;
    caller
        .invoke("configure_abandon", data_value!(NEVER, BEFORE_NEXT_CALL))
        .await?;
    let completed = caller.create_promise().await?;
    caller.executor.complete_promise(&completed, vec![]).await?;
    let promise = caller.create_promise().await?;
    caller
        .invoke("abandon_cache_then_await", data_value!(completed))
        .await?;
    caller
        .start_and_suspend(
            &IdempotencyKey::fresh(),
            "drop_retained_cache_then_await",
            data_value!(promise.clone()),
        )
        .await?;
    let history = caller.oplog().await?;
    assert_eq!(cancelled_count(&history), 5);
    let starts = history
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    let [.., abandon, dropping] = starts[..] else {
        panic!("two invocations required")
    };
    assert!(
        history
            .iter()
            .rev()
            .find(|entry| is_snapshot(entry))
            .unwrap()
            .oplog_index
            < abandon
    );
    assert!(
        history
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(_)))
            .all(|entry| entry.oplog_index > dropping)
    );
    let caller = caller
        .restart(deps, &context, snapshot_every_nth_invocation(2))
        .await?;
    caller.executor.complete_promise(&promise, vec![]).await?;
    assert!(
        caller
            .invoke("inspect_cache", data_value!())
            .await?
            .into_typed::<bool>()?
    );
    assert!(caller.inspect().await?.restored);
    let history = caller.oplog().await?;
    assert_eq!(cancelled_count(&history), 5);
    assert_settled_history(&history);
    Ok(())
}

#[test]
#[timeout("8 minutes")]
async fn cache_fill_replay_with_changed_drop_schedule(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (name, live, replay, cross_invocation) in [
        ("fill-earlier", AFTER_NEXT_CALL, BEFORE_NEXT_CALL, false),
        ("fill-later", BEFORE_NEXT_CALL, AFTER_NEXT_CALL, false),
        ("fill-never", BEFORE_NEXT_CALL, NEVER, false),
        ("fill-unrecorded", NEVER, BEFORE_NEXT_CALL, false),
        ("fill-cross-invocation", NEVER, BEFORE_NEXT_CALL, true),
    ] {
        let context = TestContext::new(last_unique_id);
        let frequency = if cross_invocation { 2 } else { 1 };
        let caller = Caller::start(
            deps,
            &context,
            fixture,
            snapshot_every_nth_invocation(frequency),
            name,
        )
        .await?;
        caller
            .invoke("configure_abandon", data_value!(live, replay))
            .await?;
        let promise = caller.create_promise().await?;
        if cross_invocation {
            let completed = caller.create_promise().await?;
            caller.executor.complete_promise(&completed, vec![]).await?;
            caller
                .invoke("fill_cache_then_await", data_value!(completed))
                .await?;
            caller
                .start_and_suspend(
                    &IdempotencyKey::fresh(),
                    "drop_retained_fills_then_await",
                    data_value!(promise.clone()),
                )
                .await?;
        } else {
            caller
                .start_and_suspend(
                    &IdempotencyKey::fresh(),
                    "fill_cache_then_await",
                    data_value!(promise.clone()),
                )
                .await?;
        }
        let history = caller.oplog().await?;
        let fill_starts =
            starts_named(&history, "keyvalue::cache::vacancy::vacancy-fill").collect::<Vec<_>>();
        assert_eq!(
            fill_starts.len(),
            1,
            "{name}: the fill has one durable Start"
        );
        let fill_start = fill_starts[0];
        let recorded = live.drop_at != 2 || cross_invocation;
        assert_eq!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == fill_start)).count(), usize::from(recorded));
        if cross_invocation {
            let last_started = history
                .iter()
                .rev()
                .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
                .unwrap()
                .oplog_index;
            assert!(history.iter().any(|entry| entry.oplog_index > last_started && matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == fill_start)));
            assert!(
                history
                    .iter()
                    .rev()
                    .find(|entry| is_snapshot(entry))
                    .unwrap()
                    .oplog_index
                    < fill_start
            );
        }
        let key = format!("{name}-fill");
        let observed = caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key.clone(), Option::<String>::None),
            )
            .await?
            .into_typed::<Option<String>>()?;
        assert_eq!(observed, recorded.then(|| "filled".to_string()));
        if recorded {
            caller
                .executor
                .invoke_and_await_agent(
                    &caller.component,
                    &caller.target,
                    "cache_value",
                    data_value!(key.clone(), Some("sentinel".to_string())),
                )
                .await?;
        }
        let caller = caller
            .restart(deps, &context, snapshot_every_nth_invocation(frequency))
            .await?;
        caller.executor.complete_promise(&promise, vec![]).await?;
        assert!(caller.inspect().await?.restored);
        let observed = caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key, Option::<String>::None),
            )
            .await?
            .into_typed::<Option<String>>()?;
        assert_eq!(
            observed.as_deref(),
            Some(if recorded { "sentinel" } else { "filled" }),
            "{name}: completed replay must not set again; an incomplete fill must set at the tail"
        );
        let history = caller.oplog().await?;
        assert_eq!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == fill_start)).count(), 1);
        assert!(
            !history.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::CompletionDelivered(marker) => marker.start_index == fill_start,
                PublicOplogEntry::CompletionDiscarded(marker) => marker.start_index == fill_start,
                _ => false,
            }),
            "{name}: destructor completion is internal, not a guest delivery"
        );
        assert_eq!(cancelled_count(&history), 0);
        assert_settled_history(&history);
    }
    Ok(())
}

#[test]
#[timeout("5 minutes")]
async fn completed_invocation_cache_drops_repair_inside_next_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (name, method, suffix, expected_value, expected_pending) in [
        (
            "completed-fill",
            "fill_cache_then_await",
            "fill",
            "filled",
            1,
        ),
        (
            "completed-futures",
            "abandon_cache_then_await",
            "cache",
            "original",
            5,
        ),
    ] {
        let context = TestContext::new(last_unique_id);
        let caller =
            Caller::start(deps, &context, fixture, snapshot_every_invocation(), name).await?;
        caller
            .invoke("configure_abandon", data_value!(NEVER, BEFORE_NEXT_CALL))
            .await?;
        let completed = caller.create_promise().await?;
        caller.executor.complete_promise(&completed, vec![]).await?;
        caller.invoke(method, data_value!(completed)).await?;
        let original = caller.oplog().await?;
        let a_started = original
            .iter()
            .rev()
            .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .unwrap()
            .oplog_index;
        let a_finished = original
            .iter()
            .rev()
            .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .unwrap()
            .oplog_index;
        let baseline = original
            .iter()
            .rev()
            .find(|entry| is_snapshot(entry))
            .unwrap()
            .oplog_index;
        assert!(baseline < a_started && a_started < a_finished);
        let pending = original
            .iter()
            .filter_map(|entry| {
                if matches!(&entry.entry, PublicOplogEntry::Start(_))
                    && !original.iter().any(|terminal| match &terminal.entry {
                        PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                        PublicOplogEntry::Cancelled(cancelled) => {
                            cancelled.start_index == entry.oplog_index
                        }
                        _ => false,
                    })
                {
                    Some(entry.oplog_index)
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        assert_eq!(pending.len(), expected_pending);
        assert!(
            pending
                .iter()
                .all(|start| a_started < *start && *start < a_finished)
        );
        let key = format!("{name}-{suffix}");
        let before = caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key.clone(), Option::<String>::None),
            )
            .await?
            .into_typed::<Option<String>>()?;
        assert_eq!(
            before.as_deref(),
            if suffix == "fill" {
                None
            } else {
                Some("original")
            }
        );

        // Keep the original snapshot as the baseline so the second restart also replays A.
        let caller = caller
            .restart(deps, &context, snapshot_every_nth_invocation(1_000))
            .await?;
        assert!(caller.inspect().await?.restored);
        let repaired = caller.oplog().await?;
        let b_started = repaired
            .iter()
            .filter(|entry| {
                entry.oplog_index > a_finished
                    && matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>();
        let b_finished = repaired
            .iter()
            .filter(|entry| {
                entry.oplog_index > a_finished
                    && matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>();
        assert_eq!(b_started.len(), 1);
        assert_eq!(b_finished.len(), 1);
        for start in &pending {
            let terminals = repaired
                .iter()
                .filter(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) => end.start_index == *start,
                    PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == *start,
                    _ => false,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                terminals.len(),
                1,
                "{name}: exactly one terminal for original Start {start}"
            );
            assert!(
                b_started[0] < terminals[0].oplog_index && terminals[0].oplog_index < b_finished[0],
                "{name}: fresh B drives the parked cleanup between its Started and Finished"
            );
            assert!(if suffix == "fill" {
                matches!(&terminals[0].entry, PublicOplogEntry::End(_))
            } else {
                matches!(&terminals[0].entry, PublicOplogEntry::Cancelled(_))
            });
        }
        assert_eq!(
            repaired
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
                .count(),
            original
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
                .count(),
            "inspect opens no calls and repair must retain the original Starts"
        );
        assert_settled_history(&repaired);
        let after = caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key.clone(), Option::<String>::None),
            )
            .await?
            .into_typed::<Option<String>>()?;
        assert_eq!(
            after.as_deref(),
            Some(expected_value),
            "fill must execute; abandoned set/delete must not execute"
        );
        caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key.clone(), Some("sentinel".to_string())),
            )
            .await?;
        assert_eq!(
            repaired
                .iter()
                .rev()
                .find(|entry| is_snapshot(entry))
                .unwrap()
                .oplog_index,
            baseline
        );

        let caller = caller
            .restart(deps, &context, snapshot_every_nth_invocation(1_000))
            .await?;
        assert!(caller.inspect().await?.restored);
        let second = caller.oplog().await?;
        for start in &pending {
            assert_eq!(
                second
                    .iter()
                    .filter(|entry| match &entry.entry {
                        PublicOplogEntry::End(end) => end.start_index == *start,
                        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == *start,
                        _ => false,
                    })
                    .count(),
                1,
                "{name}: the second replay must not append another terminal"
            );
        }
        let after_second = caller
            .executor
            .invoke_and_await_agent(
                &caller.component,
                &caller.target,
                "cache_value",
                data_value!(key, Option::<String>::None),
            )
            .await?
            .into_typed::<Option<String>>()?;
        assert_eq!(
            after_second.as_deref(),
            Some("sentinel"),
            "{name}: the second replay must not repeat any cache write"
        );
        assert_settled_history(&second);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, FromSchema)]
struct TsGcSnapshotReport {
    completed: f64,
    checksum: f64,
    restored: bool,
}

/// A TypeScript agent whose RPC clients are owned by reference cycles, so the QuickJS garbage
/// collector releases them. The agent snapshots every second invocation. Clients created before
/// a snapshot are collected by the next invocation's collection; the restored heap has none of
/// them, so its replay of that invocation releases fewer clients than the original run did.
#[test]
#[timeout("3 minutes")]
async fn ts_snapshot_restore_replays_gc_released_clients(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let id = agent_id!("TsGcSnapshotDriver", "ts-gc-snapshot");
    let target = agent_id!("TsCounter", "ts-gc-snapshot-target");
    let agent = executor.start_agent(&component.id, id.clone()).await?;
    let last_index = |oplog: &[PublicOplogEntryWithIndex], pred: fn(&PublicOplogEntry) -> bool| {
        oplog
            .iter()
            .rev()
            .find(|entry| pred(&entry.entry))
            .map(|entry| entry.oplog_index)
    };

    // Run invocations that leave uncollected clients until a snapshot follows the last one.
    let completed = loop {
        let completed = executor
            .invoke_and_await_agent(&component, &id, "run", data_value!(5_f64, -1_f64))
            .await?
            .into_typed::<f64>()?;
        let history = ts_oplog(&executor, &agent).await?;
        let snapshot = last_index(&history, |entry| {
            matches!(entry, PublicOplogEntry::Snapshot(_))
        });
        let finished = last_index(&history, |entry| {
            matches!(entry, PublicOplogEntry::AgentInvocationFinished(_))
        });
        if snapshot > finished {
            break completed;
        }
        assert!(
            completed < 20.0,
            "the agent snapshots every second invocation"
        );
    };

    // The next invocation collects the clients created before the snapshot.
    let expected = completed + 5.0;
    let observed = executor
        .invoke_and_await_agent(&component, &id, "run", data_value!(5_f64, 2_f64))
        .await?
        .into_typed::<f64>()?;
    assert_eq!(observed, expected);
    let history = ts_oplog(&executor, &agent).await?;
    let snapshot = last_index(&history, |entry| {
        matches!(entry, PublicOplogEntry::Snapshot(_))
    })
    .expect("the agent took a snapshot");
    let collecting = last_index(&history, |entry| {
        matches!(entry, PublicOplogEntry::AgentInvocationStarted(_))
    })
    .expect("the collecting invocation started");
    assert!(
        snapshot < collecting,
        "a restart restores the snapshot taken before the collecting invocation"
    );
    assert!(
        history.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::SpanClosed(params)
                if entry.oplog_index > collecting
                    && params.resource_start_index.is_some_and(|start| start < snapshot)
        )),
        "the collecting invocation releases clients created before the snapshot"
    );

    drop(executor);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let report = tokio::time::timeout(
        REPLAY_TIMEOUT,
        executor.invoke_and_await_agent(&component, &id, "inspect", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("inspect did not complete within {REPLAY_TIMEOUT:?}"))??
    .into_typed::<TsGcSnapshotReport>()?;
    assert_eq!(
        report,
        TsGcSnapshotReport {
            completed: expected,
            checksum: expected * (expected + 1.0) / 2.0,
            restored: true,
        }
    );
    let target_count = executor
        .invoke_and_await_agent(&component, &target, "getValue", data_value!())
        .await?
        .into_typed::<f64>()?;
    assert_eq!(target_count, expected);
    assert_settled_history(&ts_oplog(&executor, &agent).await?);
    let next = executor
        .invoke_and_await_agent(&component, &id, "run", data_value!(1_f64, -1_f64))
        .await?
        .into_typed::<f64>()?;
    assert_eq!(next, expected + 1.0);
    Ok(())
}

async fn ts_oplog(
    executor: &TestWorkerExecutor,
    agent: &AgentId,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    executor.commit_oplog(agent).await?;
    executor.get_oplog(agent, OplogIndex::INITIAL).await
}
