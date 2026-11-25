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

//! The two RPC cancel methods of a never-live fork (`agent-rpc-rust`): the cancellation token of a
//! scheduled invocation, and the cancel of a future invoke result. Neither sends a remote cancel.

use super::{settled_update_outcomes_after, tip};
use crate::Tracing;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, RecordingRpc, TestContext, TestExecutorOverrides,
    TestWorkerExecutor, WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

struct Cancels {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    /// Every call that reaches the executor's RPC service.
    rpc_calls: Arc<AtomicUsize>,
    /// The remote invocation cancellations that reach the worker proxy.
    evidence: Arc<crate::fork::RemoteRpcEvidence>,
}

impl Cancels {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        agent_rpc_rust: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let rpc_calls = Arc::new(Mutex::new(None::<Arc<AtomicUsize>>));
        let (executor, evidence) = crate::fork::start_with_local_resume_and_evidence(
            deps,
            &context,
            TestExecutorOverrides {
                wrap_rpc: Some(Arc::new({
                    let rpc_calls = rpc_calls.clone();
                    move |inner| {
                        let recording = RecordingRpc::new(inner, "", Arc::default());
                        *rpc_calls.lock().unwrap() = Some(recording.call_count());
                        Arc::new(recording)
                    }
                })),
                ..Default::default()
            },
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let rpc_calls = rpc_calls
            .lock()
            .unwrap()
            .clone()
            .expect("the executor wraps its RPC service");
        Ok(Self {
            executor,
            component,
            rpc_calls,
            evidence,
        })
    }

    fn rpc_calls(&self) -> usize {
        self.rpc_calls.load(Ordering::SeqCst)
    }

    fn cancel_invocations(&self) -> usize {
        self.evidence.cancel_invocations.load(Ordering::SeqCst)
    }

    /// Forks `source_id` at `cut` with the never-live mark, applies an automatic update to its own
    /// revision, and returns the update outcomes of the fork.
    async fn marked_update(
        &self,
        source_id: &OwnedAgentId,
        cut: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let fork = self.executor.fork_worker_never_live(source_id, cut).await?;
        self.executor
            .auto_update_worker(&fork, self.component.revision, false)
            .await?;
        settled_update_outcomes_after(&self.executor, &fork, cut).await
    }
}

fn assert_one_success(outcomes: &[PublicOplogEntryWithIndex]) {
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
}

/// GOL-2143 P10, the cancellation token: a source schedules an invocation and cancels it through
/// its cancellation token. A never-live fork cut after the `Start` of the cancel, so that the
/// cancel is incomplete, and one cut after its `End`, each write one `SuccessfulUpdate` and send no
/// remote cancel. The control is the unmarked twin of the incomplete cut: it repairs the cancel,
/// which reaches the RPC service again.
#[test]
#[timeout("4m")]
async fn the_cancellation_token_of_a_never_live_fork_sends_no_remote_cancel(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let cancels = Cancels::start(last_unique_id, deps, agent_rpc_rust).await?;
    let (name, server) = ("p10-token", "p10-token-server");
    let agent = agent_key!("ScheduledInvocationClient", name);
    let source_id = cancels
        .executor
        .start_agent(&cancels.component.id, agent.clone())
        .await?;
    let due = chrono::Utc::now() + chrono::Duration::hours(1);
    cancels
        .executor
        .invoke_and_await_agent(
            &cancels.component,
            &agent,
            "schedule_at",
            data_value!(server.to_string(), due.timestamp() as u64, 0u32),
        )
        .await?;
    cancels
        .executor
        .invoke_and_await_agent(
            &cancels.component,
            &agent,
            "cancel_scheduled",
            data_value!(),
        )
        .await?;
    let incomplete = cancels
        .executor
        .get_oplog(&source_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .find(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
                if start.function_name == "golem::rpc::cancellation-token::cancel")
        })
        .map(|entry| entry.oplog_index)
        .expect("the cancel has a Start");
    let complete = tip(&cancels.executor, &source_id).await?;
    let (rpc_calls, cancel_invocations) = (cancels.rpc_calls(), cancels.cancel_invocations());
    for cut in [incomplete, complete] {
        let outcomes = cancels.marked_update(&source_id, cut).await?;
        assert_one_success(&outcomes);
        assert_eq!(cancels.rpc_calls(), rpc_calls, "cut {cut}");
        assert_eq!(
            cancels.cancel_invocations(),
            cancel_invocations,
            "cut {cut}"
        );
    }

    cancels.executor.fork_worker(&source_id, incomplete).await?;
    tokio::time::timeout(Duration::from_secs(60), async {
        while cancels.rpc_calls() == rpc_calls {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the unmarked twin did not repair its cancel"))?;
    Ok(())
}

/// GOL-2143 P10, the future invoke result: a source starts an RPC call and cancels its future, so
/// the live cancel sends one remote invocation cancellation. A never-live fork of the whole
/// history replays the cancel, writes one `SuccessfulUpdate`, and sends no further cancellation
/// and no RPC call.
#[test]
#[timeout("4m")]
async fn the_future_cancel_of_a_never_live_fork_sends_no_remote_cancel(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let cancels = Cancels::start(last_unique_id, deps, agent_rpc_rust).await?;
    let name = "p10-future";
    let agent = agent_key!("CancelTester", name);
    let source_id = cancels
        .executor
        .start_agent(&cancels.component.id, agent.clone())
        .await?;
    cancels
        .executor
        .invoke_and_await_agent(
            &cancels.component,
            &agent,
            "test_cancel_before_await",
            data_value!("p10-future-counter"),
        )
        .await?;
    assert_eq!(cancels.cancel_invocations(), 1, "the source cancels live");
    let cut = tip(&cancels.executor, &source_id).await?;
    let (rpc_calls, cancel_invocations) = (cancels.rpc_calls(), cancels.cancel_invocations());

    let outcomes = cancels.marked_update(&source_id, cut).await?;
    assert_one_success(&outcomes);
    assert_eq!(cancels.rpc_calls(), rpc_calls);
    assert_eq!(cancels.cancel_invocations(), cancel_invocations);
    Ok(())
}
