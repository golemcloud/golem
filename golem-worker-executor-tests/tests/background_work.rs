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

//! Background work that outlives its method's result: the caller gets the result at the
//! export's return, the background work continues, and a real executor restart in each crash
//! window neither computes the result again nor repeats a counted effect.
//! Exercises the `BackgroundWork` and `Tally` agents of the shared `agent_sdk_rust` component.

use crate::Tracing;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId, PromiseId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// How long a caller waits for a result that must not wait for background work. The
/// background work in these tests waits for a promise that the test completes only after the
/// result arrived, so a result that waits for it never arrives.
const RESULT_WAIT: Duration = Duration::from_secs(30);

struct Agents {
    component: ComponentDto,
    worker: ParsedAgentKey,
    worker_id: OwnedAgentId,
    tally: ParsedAgentKey,
    tally_id: OwnedAgentId,
    tally_name: String,
}

impl Agents {
    async fn new(
        executor: &(impl TestDsl + Sync),
        component: ComponentDto,
        context: &TestContext,
        case: &str,
    ) -> anyhow::Result<Self> {
        let tally_name = format!("{case}-tally-{}", context.redis_prefix());
        let worker = agent_key!(
            "BackgroundWork",
            format!("{case}-{}", context.redis_prefix())
        );
        let tally = agent_key!("Tally", tally_name.clone());
        let worker_id = executor.start_agent(&component.id, worker.clone()).await?;
        let tally_id = executor.start_agent(&component.id, tally.clone()).await?;
        Ok(Self {
            worker_id,
            tally_id,
            component,
            worker,
            tally,
            tally_name,
        })
    }

    /// Completes `promise`, which an agent of the component created.
    async fn complete(
        &self,
        executor: &(impl TestDsl + Sync),
        promise: &PromiseId,
    ) -> anyhow::Result<()> {
        executor
            .complete_promise(&self.component.environment_id, promise, vec![1])
            .await
    }

    async fn create_release(
        &self,
        executor: &(impl TestDsl + Sync),
        agent: &ParsedAgentKey,
    ) -> anyhow::Result<PromiseId> {
        executor
            .invoke_and_await_agent(&self.component, agent, "create_release", data_value!())
            .await?
            .into_typed::<PromiseId>()
    }

    /// Invokes `add_after_release` and returns its result. Fails when the result does not
    /// arrive within [`RESULT_WAIT`], which is what happens when the result waits for the
    /// background work.
    async fn add_after_release(
        &self,
        executor: &(impl TestDsl + Sync),
        key: &IdempotencyKey,
        release: &PromiseId,
        tally_release: Option<PromiseId>,
        amount: u64,
    ) -> anyhow::Result<String> {
        let result = tokio::time::timeout(
            RESULT_WAIT,
            executor.invoke_and_await_agent_with_key(
                &self.component,
                &self.worker,
                key,
                "add_after_release",
                data_value!(
                    release.clone(),
                    self.tally_name.clone(),
                    tally_release,
                    amount
                ),
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the result of add_after_release did not arrive within {RESULT_WAIT:?}: \
                 it must not wait for the background work"
            )
        })??;
        result.into_typed::<String>()
    }

    /// Invokes `trap_after_release` with `key` and returns its result within [`RESULT_WAIT`].
    async fn trap_after_release(
        &self,
        executor: &(impl TestDsl + Sync),
        key: &IdempotencyKey,
        release: &PromiseId,
    ) -> anyhow::Result<String> {
        let result = tokio::time::timeout(
            RESULT_WAIT,
            executor.invoke_and_await_agent_with_key(
                &self.component,
                &self.worker,
                key,
                "trap_after_release",
                data_value!(release.clone()),
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the result of trap_after_release did not arrive within {RESULT_WAIT:?}"
            )
        })??;
        result.into_typed::<String>()
    }

    /// The number of completed background runs. The worker admits this invocation only after
    /// the background work of the earlier invocations ended.
    async fn completions(&self, executor: &(impl TestDsl + Sync)) -> anyhow::Result<u64> {
        executor
            .invoke_and_await_agent(&self.component, &self.worker, "completions", data_value!())
            .await?
            .into_typed::<u64>()
    }

    async fn total(&self, executor: &(impl TestDsl + Sync)) -> anyhow::Result<u64> {
        executor
            .invoke_and_await_agent(&self.component, &self.tally, "total", data_value!())
            .await?
            .into_typed::<u64>()
    }

    /// Checks the oplogs after the background work ended: the worker started and finished
    /// `method` once, the tally executed `add_when_released` once, and every invocation of both
    /// agents settled once, after its result.
    async fn assert_executed_once(
        &self,
        executor: &(impl TestDsl + Sync),
        method: &str,
    ) -> anyhow::Result<()> {
        let oplog = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        let started = method_starts(&oplog, method);
        assert_eq!(started.len(), 1, "{method} is started once");
        let finished = oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::AgentInvocationFinished(params)
                        if params.invocation_id == started[0]
                )
            })
            .count();
        assert_eq!(finished, 1, "{method} records its result once");

        let tally_oplog = executor
            .get_oplog(&self.tally_id, OplogIndex::INITIAL)
            .await?;
        assert_eq!(
            method_starts(&tally_oplog, "add_when_released").len(),
            1,
            "the tally executes the background call once"
        );
        assert_each_settled_once(executor, &self.worker_id).await?;
        assert_each_settled_once(executor, &self.tally_id).await?;
        Ok(())
    }

    /// Checks the oplog of the worker before a restart in a window after the result of
    /// `method`: the result is recorded, the invocation is not settled, and a durable call whose
    /// function name contains `open_call` has no terminal. With `open_after_result`, that call
    /// starts after the result.
    async fn assert_unsettled_after_result(
        &self,
        executor: &(impl TestDsl + Sync),
        method: &str,
        open_call: &str,
        open_after_result: bool,
    ) -> anyhow::Result<()> {
        let oplog = executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        let started = method_starts(&oplog, method);
        assert_eq!(started.len(), 1, "{method} is started once");
        let finished = finished_of(&oplog, &started[0])
            .unwrap_or_else(|| panic!("the result of {method} is recorded before the restart"));
        assert_eq!(
            settled_of(&oplog, &started[0]),
            Vec::<OplogIndex>::new(),
            "{method} is not settled before the restart"
        );
        let open = open_starts(&oplog, open_call);
        assert!(
            open.iter()
                .any(|start| !open_after_result || *start > finished),
            "a {open_call} call of {method} is open{} before the restart; open: {open:?}, \
             result at {finished}",
            if open_after_result {
                " after the result"
            } else {
                ""
            }
        );
        Ok(())
    }
}

/// The index of the `AgentInvocationFinished` of `invocation`.
fn finished_of(
    oplog: &[PublicOplogEntryWithIndex],
    invocation: &InvocationId,
) -> Option<OplogIndex> {
    oplog
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.invocation_id == *invocation
            )
        })
        .map(|entry| entry.oplog_index)
}

/// The indexes of every `AgentInvocationSettled` of `invocation`.
fn settled_of(oplog: &[PublicOplogEntryWithIndex], invocation: &InvocationId) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationSettled(params)
                    if params.invocation_id == *invocation
            )
        })
        .map(|entry| entry.oplog_index)
        .collect()
}

/// The indexes of the `Start`s whose function name contains `function` and that have no `End`
/// or `Cancelled`.
fn open_starts(oplog: &[PublicOplogEntryWithIndex], function: &str) -> Vec<OplogIndex> {
    let terminated = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) => Some(params.start_index),
            PublicOplogEntry::Cancelled(params) => Some(params.start_index),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params) if params.function_name.contains(function)
            ) && !terminated.contains(&entry.oplog_index)
        })
        .map(|entry| entry.oplog_index)
        .collect()
}

/// Waits until every invocation with a recorded result in `agent_id`'s oplog settled, then
/// checks that each one settled exactly once, after its own result. The last invocation can
/// settle after its caller got the result, so the check waits for it.
async fn assert_each_settled_once(
    executor: &(impl TestDsl + Sync),
    agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let oplog = executor.get_oplog(agent_id, OplogIndex::INITIAL).await?;
        let results = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationFinished(params) => {
                    Some((params.invocation_id, entry.oplog_index))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let all_settled = results
            .iter()
            .all(|(invocation, _)| !settled_of(&oplog, invocation).is_empty());
        if all_settled || tokio::time::Instant::now() >= deadline {
            for (invocation, finished) in results {
                let settled = settled_of(&oplog, &invocation);
                assert_eq!(
                    settled.len(),
                    1,
                    "the invocation {invocation} of {agent_id} settles exactly once"
                );
                assert!(
                    settled[0] > finished,
                    "the invocation {invocation} of {agent_id} settles at {} after its result at \
                     {finished}",
                    settled[0]
                );
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The invocation IDs of every started `method` invocation, in order.
fn method_starts(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    method: &str,
) -> Vec<golem_common::model::InvocationId> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
                PublicAgentInvocation::AgentMethodInvocation(call)
                    if call.method_name == method =>
                {
                    Some(call.invocation_id)
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Waits until `agent_id`'s oplog records a started `method` invocation.
async fn await_method_start(
    executor: &(impl TestDsl + Sync),
    agent_id: &OwnedAgentId,
    method: &str,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let oplog = executor.get_oplog(agent_id, OplogIndex::INITIAL).await?;
        if !method_starts(&oplog, method).is_empty() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("{agent_id} did not start {method}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn the_result_arrives_before_the_background_work_ends(
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
    let agents = Agents::new(&executor, component, &context, "result-first").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;

    let result = agents
        .add_after_release(&executor, &IdempotencyKey::fresh(), &release, None, 5)
        .await?;
    assert_eq!(result, "returned");
    assert_eq!(
        agents.total(&executor).await?,
        0,
        "the background work has not added yet"
    );

    agents.complete(&executor, &release).await?;
    assert_eq!(
        agents.completions(&executor).await?,
        1,
        "the next invocation runs after the background work ended"
    );
    assert_eq!(agents.total(&executor).await?, 5);
    agents
        .assert_executed_once(&executor, "add_after_release")
        .await?;

    drop(executor);
    Ok(())
}

/// Crash window: the method waits for its release, so its result is not recorded yet.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_restart_before_the_result_continues_the_method(
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
    let agents = Agents::new(&executor, component, &context, "before-result").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(release.clone(), agents.tally_name.clone(), 3u64);

    executor
        .invoke_agent_with_key(
            &agents.component,
            &agents.worker,
            &key,
            "add_before_return",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(
            &agents.worker_id,
            AgentStatus::Suspended,
            Duration::from_secs(30),
        )
        .await?;
    let oplog = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    let started = method_starts(&oplog, "add_before_return");
    assert_eq!(started.len(), 1);
    assert_eq!(
        finished_of(&oplog, &started[0]),
        None,
        "the result is not recorded before the restart"
    );
    assert_eq!(settled_of(&oplog, &started[0]), Vec::<OplogIndex>::new());
    assert!(
        !open_starts(&oplog, "promise").is_empty(),
        "the method's wait for its release is open before the restart"
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    agents.complete(&executor, &release).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &agents.component,
            &agents.worker,
            &key,
            "add_before_return",
            params,
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!(result, 3, "the method continues and returns its result");
    assert_eq!(agents.total(&executor).await?, 3);
    agents
        .assert_executed_once(&executor, "add_before_return")
        .await?;

    drop(executor);
    Ok(())
}

/// Crash window: the result is recorded, and the background work waits for its release.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_restart_after_the_result_completes_the_pending_background_work(
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
    let agents = Agents::new(&executor, component, &context, "pending-work").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let key = IdempotencyKey::fresh();

    let result = agents
        .add_after_release(&executor, &key, &release, None, 5)
        .await?;
    assert_eq!(result, "returned");
    executor
        .wait_for_status(
            &agents.worker_id,
            AgentStatus::Suspended,
            Duration::from_secs(30),
        )
        .await?;
    agents
        .assert_unsettled_after_result(&executor, "add_after_release", "promise", false)
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;
    agents.complete(&executor, &release).await?;
    assert_eq!(
        agents.completions(&executor).await?,
        1,
        "the background work completes after the restart"
    );
    assert_eq!(agents.total(&executor).await?, 5);
    assert_eq!(
        agents
            .add_after_release(&executor, &key, &release, None, 5)
            .await?,
        "returned",
        "the same key returns the recorded result"
    );
    agents
        .assert_executed_once(&executor, "add_after_release")
        .await?;

    drop(executor);
    Ok(())
}

/// Crash window: the background work's call to the tally is started and not completed, because
/// the tally waits for its own release.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_restart_inside_a_background_call_completes_it_once(
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
    let agents = Agents::new(&executor, component, &context, "inside-call").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let tally_release = agents.create_release(&executor, &agents.tally).await?;

    let result = agents
        .add_after_release(
            &executor,
            &IdempotencyKey::fresh(),
            &release,
            Some(tally_release.clone()),
            7,
        )
        .await?;
    assert_eq!(result, "returned");
    agents.complete(&executor, &release).await?;
    await_method_start(&executor, &agents.tally_id, "add_when_released").await?;
    executor
        .wait_for_status(
            &agents.tally_id,
            AgentStatus::Suspended,
            Duration::from_secs(30),
        )
        .await?;
    agents
        .assert_unsettled_after_result(&executor, "add_after_release", "rpc", true)
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;
    agents.complete(&executor, &tally_release).await?;
    assert_eq!(
        agents.completions(&executor).await?,
        1,
        "the background call completes after the restart"
    );
    assert_eq!(
        agents.total(&executor).await?,
        7,
        "the background call adds once"
    );
    agents
        .assert_executed_once(&executor, "add_after_release")
        .await?;

    drop(executor);
    Ok(())
}

/// Crash window: the background work ended, so nothing of the invocation runs again.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_restart_after_the_background_work_ended_repeats_nothing(
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
    let agents = Agents::new(&executor, component, &context, "work-ended").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;

    let result = agents
        .add_after_release(&executor, &IdempotencyKey::fresh(), &release, None, 5)
        .await?;
    assert_eq!(result, "returned");
    agents.complete(&executor, &release).await?;
    assert_eq!(agents.completions(&executor).await?, 1);
    assert_each_settled_once(&executor, &agents.worker_id).await?;
    let oplog_before = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;
    assert_eq!(
        agents.completions(&executor).await?,
        1,
        "replay restores the completed background run"
    );
    assert_eq!(agents.total(&executor).await?, 5);
    let oplog_after = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    let durable_calls = |oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex]| {
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Start(_)))
            .count()
    };
    assert_eq!(
        durable_calls(&oplog_after),
        durable_calls(&oplog_before),
        "the replayed background work makes no new durable call"
    );
    agents
        .assert_executed_once(&executor, "add_after_release")
        .await?;

    drop(executor);
    Ok(())
}

/// A trap of the background work after the result is an ordinary trap of the agent: the caller
/// keeps the recorded success, the method does not run again, and the retry policy decides what
/// happens to the agent. A Rust panic is permanent, so the agent fails. A failed agent refuses
/// every new external call before it looks up a result, so the test checks that the oplog keeps
/// the success as the outcome of the invocation, before and after a reconstruction.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_trap_after_the_result_keeps_the_result(
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
    let agents = Agents::new(&executor, component, &context, "trap-after-result").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let key = IdempotencyKey::fresh();

    assert_eq!(
        agents.trap_after_release(&executor, &key, &release).await?,
        "returned"
    );
    agents.complete(&executor, &release).await?;
    executor
        .wait_for_status(
            &agents.worker_id,
            AgentStatus::Failed,
            Duration::from_secs(30),
        )
        .await?;

    let oplog = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    let started = method_starts(&oplog, "trap_after_release");
    assert_eq!(started.len(), 1, "the method runs once");
    let finished = finished_of(&oplog, &started[0]).expect("the result is recorded");
    let errors = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Error(params) => Some((entry.oplog_index, params)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !errors.is_empty()
            && errors.iter().all(|(index, params)| *index > finished
                && params.invocation_id.is_none()
                && params.error.contains("trap_after_release")),
        "the trap is an error of the agent after the result, attributed to no invocation: \
         {errors:?}"
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::CancelPendingInvocation(_))),
        "the trap cancels no invocation"
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    let error = agents
        .trap_after_release(&executor, &key, &release)
        .await
        .expect_err("the failed agent refuses the call");
    assert!(
        format!("{error:#}").contains("background work trapped after the result"),
        "the agent stays failed with the trap of its background work: {error:#}"
    );
    let oplog = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    let started = method_starts(&oplog, "trap_after_release");
    assert_eq!(started.len(), 1, "the method does not run again");
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.invocation_id == started[0]
            ))
            .count(),
        1,
        "the reconstruction records no other result"
    );

    drop(executor);
    Ok(())
}

/// An interrupt while the background work drains is not the outcome of the invocation: the
/// caller keeps the recorded success, before and after a reconstruction, and the background
/// work completes once after the agent resumes. The background call to the tally is open when
/// the interrupt arrives, so the agent is running its drain.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn an_interrupt_during_the_drain_keeps_the_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // The worker keeps running while its background call waits, so the interrupt reaches the
    // drain instead of a suspended agent.
    let running = || TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.suspend_after = Duration::from_secs(600);
            config.suspend.rpc_suspend_after = Duration::from_secs(600);
            config.suspend.wait_suspend_grace = Duration::from_secs(600);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, running()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let agents = Agents::new(&executor, component, &context, "interrupt-drain").await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let tally_release = agents.create_release(&executor, &agents.tally).await?;
    let key = IdempotencyKey::fresh();

    assert_eq!(
        agents
            .add_after_release(&executor, &key, &release, Some(tally_release.clone()), 7)
            .await?,
        "returned"
    );
    agents.complete(&executor, &release).await?;
    await_method_start(&executor, &agents.tally_id, "add_when_released").await?;
    agents
        .assert_unsettled_after_result(&executor, "add_after_release", "rpc", true)
        .await?;
    executor.interrupt(&agents.worker_id).await?;
    executor
        .wait_for_status(
            &agents.worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(30),
        )
        .await?;
    assert_eq!(
        agents
            .add_after_release(&executor, &key, &release, Some(tally_release.clone()), 7)
            .await?,
        "returned",
        "the same key returns the recorded success after the interrupt"
    );

    drop(executor);
    let executor = start_with_overrides(deps, &context, running()).await?;
    assert_eq!(
        agents
            .add_after_release(&executor, &key, &release, Some(tally_release.clone()), 7)
            .await?,
        "returned",
        "the same key returns the recorded success after a reconstruction"
    );
    executor.resume(&agents.worker_id, false).await?;
    agents.complete(&executor, &tally_release).await?;
    assert_eq!(
        agents.completions(&executor).await?,
        1,
        "the background work completes once after the agent resumes"
    );
    assert_eq!(agents.total(&executor).await?, 7);
    agents
        .assert_executed_once(&executor, "add_after_release")
        .await?;

    drop(executor);
    Ok(())
}

/// A method whose export can no longer make progress before it returns fails with the async
/// deadlock of its event loop instead of hanging.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_method_that_deadlocks_before_its_return_fails(
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
    let agents = Agents::new(&executor, component, &context, "deadlock").await?;

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        executor.invoke_and_await_agent(
            &agents.component,
            &agents.worker,
            "deadlock_before_return",
            data_value!(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the deadlocked method neither failed nor returned"))?;
    let error = result.expect_err("the deadlocked method fails");
    assert!(
        format!("{error:#}").contains("deadlock detected"),
        "the method fails with the async deadlock: {error:#}"
    );

    drop(executor);
    Ok(())
}

/// Every cancelled `get_promise_result` `Start`, which is the wait that `drop_unfinished_wait`
/// dropped, with the indexes of all of its `Cancelled` entries.
fn cancelled_promise_waits(
    oplog: &[PublicOplogEntryWithIndex],
) -> Vec<(OplogIndex, Vec<OplogIndex>)> {
    oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params) if params.function_name.contains("get_promise_result")
            )
        })
        .map(|start| {
            let cancelled = oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::Cancelled(params) if params.start_index == start.oplog_index
                    )
                })
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>();
            (start.oplog_index, cancelled)
        })
        .filter(|(_, cancelled)| !cancelled.is_empty())
        .collect()
}

/// Runs `drop_unfinished_wait` and restarts the executor for real after its result. The method
/// drops an unfinished durable call and returns with no further host call, so its `Cancelled`
/// lies before its result. Replay must record that `Cancelled` before it reads the result: the
/// agent then replays, the next invocation runs, and the call keeps exactly one `Cancelled`.
/// With `with_background_work`, background work of the method waits for a promise that the test
/// completes only after the restart.
async fn a_dropped_call_replays_before_the_result(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    agent_sdk_rust: &PrecompiledComponent,
    case: &str,
    with_background_work: bool,
) -> anyhow::Result<()> {
    let executor = start(deps, context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let agents = Agents::new(&executor, component, context, case).await?;
    let release = agents.create_release(&executor, &agents.worker).await?;
    let background = if with_background_work {
        Some(agents.create_release(&executor, &agents.worker).await?)
    } else {
        None
    };

    let result = tokio::time::timeout(
        RESULT_WAIT,
        executor.invoke_and_await_agent(
            &agents.component,
            &agents.worker,
            "drop_unfinished_wait",
            data_value!(release.clone(), background.clone()),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the result of drop_unfinished_wait did not arrive"))??
    .into_typed::<String>()?;
    assert_eq!(result, "timed out");

    let oplog = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    let started = method_starts(&oplog, "drop_unfinished_wait");
    assert_eq!(started.len(), 1, "drop_unfinished_wait is started once");
    let finished = finished_of(&oplog, &started[0]).expect("the result is recorded");
    let dropped = cancelled_promise_waits(&oplog);
    assert_eq!(
        dropped.len(),
        1,
        "one dropped wait is cancelled: {dropped:?}"
    );
    let (dropped_start, cancelled) = dropped[0].clone();
    assert_eq!(cancelled.len(), 1, "the dropped wait is cancelled once");
    assert!(
        cancelled[0] < finished,
        "the dropped wait is cancelled at {} before the result at {finished}",
        cancelled[0]
    );
    if !with_background_work {
        assert_each_settled_once(&executor, &agents.worker_id).await?;
    }

    drop(executor);
    let executor = start(deps, context).await?;
    if let Some(background) = &background {
        agents.complete(&executor, background).await?;
    }
    let completions = tokio::time::timeout(RESULT_WAIT, agents.completions(&executor))
        .await
        .map_err(|_| {
            anyhow::anyhow!("the agent did not replay drop_unfinished_wait within {RESULT_WAIT:?}")
        })??;
    assert_eq!(completions, u64::from(with_background_work));

    assert_each_settled_once(&executor, &agents.worker_id).await?;
    let oplog = executor
        .get_oplog(&agents.worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        cancelled_promise_waits(&oplog),
        vec![(dropped_start, cancelled)],
        "replay records no other cancellation of the dropped wait"
    );
    assert_eq!(
        method_starts(&oplog, "drop_unfinished_wait"),
        started,
        "replay does not run drop_unfinished_wait again"
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_call_dropped_before_the_return_replays_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    a_dropped_call_replays_before_the_result(deps, &context, agent_sdk_rust, "dropped-call", false)
        .await
}

#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn a_call_dropped_before_the_return_replays_while_background_work_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    a_dropped_call_replays_before_the_result(
        deps,
        &context,
        agent_sdk_rust,
        "dropped-call-background",
        true,
    )
    .await
}
