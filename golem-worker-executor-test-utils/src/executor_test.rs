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

//! The setup most executor tests start with, background invocations, oplog polling and
//! waits for unloading.

use crate::{
    LastUniqueId, OplogView, PrecompiledComponent, TestContext, TestExecutorOverrides,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start, start_with_overrides,
};
use golem_common::model::agent::{OwnedAgentId, ParsedAgentKey};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntryWithIndex};
use golem_common::model::{AgentStatus, IdempotencyKey};
use golem_common::schema::TypedSchemaValue;
use golem_test_framework::dsl::{AgentResult, TestDsl, within};
use std::fmt::Display;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::Instrument;

/// How often [`TestWorkerExecutor::wait_for_oplog`] reads the oplog again, and
/// [`TestWorkerExecutor::wait_for_suspended_unload`] checks whether the agent is loaded.
const OPLOG_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A fresh context, an executor on it and a component stored in its default environment.
pub struct ExecutorTest {
    pub context: TestContext,
    pub executor: TestWorkerExecutor,
    pub component: ComponentDto,
}

impl ExecutorTest {
    /// Starts the executor with [`start`] and stores `component` unchanged.
    pub async fn start(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        component: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        Self::store(context, executor, component).await
    }

    /// Starts the executor with [`start_with_overrides`] and stores `component` unchanged.
    pub async fn start_with_overrides(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        component: &PrecompiledComponent,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        Self::store(context, executor, component).await
    }

    async fn store(
        context: TestContext,
        executor: TestWorkerExecutor,
        component: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let component = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        Ok(Self {
            context,
            executor,
            component,
        })
    }
}

impl TestWorkerExecutor {
    /// Starts [`TestDsl::invoke_and_await_agent`] in a background task, so that the test can
    /// interrupt, crash or contend with the invocation while it runs.
    pub fn spawn_invoke_and_await(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentKey,
        method_name: &str,
        params: TypedSchemaValue,
    ) -> JoinHandle<anyhow::Result<AgentResult>> {
        let executor = self.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let method_name = method_name.to_string();
        tokio::spawn(
            async move {
                executor
                    .invoke_and_await_agent(&component, &agent_id, &method_name, params)
                    .await
            }
            .in_current_span(),
        )
    }

    /// As [`TestWorkerExecutor::spawn_invoke_and_await`], with the idempotency key of the
    /// invocation.
    pub fn spawn_invoke_and_await_with_key(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentKey,
        idempotency_key: &IdempotencyKey,
        method_name: &str,
        params: TypedSchemaValue,
    ) -> JoinHandle<anyhow::Result<AgentResult>> {
        let executor = self.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.to_string();
        tokio::spawn(
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent_id,
                        &idempotency_key,
                        &method_name,
                        params,
                    )
                    .await
            }
            .in_current_span(),
        )
    }

    /// Reads the agent's public oplog until `predicate` holds, and returns that oplog. Fails
    /// after `timeout` with `what` and the last oplog read.
    pub async fn wait_for_oplog(
        &self,
        agent_id: &OwnedAgentId,
        timeout: Duration,
        what: impl Display,
        predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        self.poll_oplog(agent_id, false, timeout, what, predicate)
            .await
    }

    /// As [`TestWorkerExecutor::wait_for_oplog`], but commits the agent's oplog before each
    /// read, so that the `Start` of a call that is still running becomes visible.
    pub async fn wait_for_committed_oplog(
        &self,
        agent_id: &OwnedAgentId,
        timeout: Duration,
        what: impl Display,
        predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        self.poll_oplog(agent_id, true, timeout, what, predicate)
            .await
    }

    /// Waits until the agent's status is `Suspended` and the executor has unloaded it. Fails
    /// after `timeout`, which covers both waits.
    pub async fn wait_for_suspended_unload(
        &self,
        agent_id: &OwnedAgentId,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        within(
            timeout,
            format!("{agent_id} to suspend and unload"),
            async {
                self.wait_for_status(agent_id, AgentStatus::Suspended, timeout)
                    .await?;
                while self.worker_is_loaded(agent_id).await {
                    tokio::time::sleep(OPLOG_POLL_INTERVAL).await;
                }
                anyhow::Ok(())
            },
        )
        .await?
    }

    async fn poll_oplog(
        &self,
        agent_id: &OwnedAgentId,
        commit: bool,
        timeout: Duration,
        what: impl Display,
        predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let deadline = Instant::now() + timeout;
        loop {
            if commit {
                self.commit_oplog(&agent_id.agent_id).await?;
            }
            let oplog = self.get_oplog(agent_id, OplogIndex::INITIAL).await?;
            if predicate(&oplog) {
                return Ok(oplog);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "timed out after {timeout:?} waiting for {what}; the oplog: {}",
                oplog.describe()
            );
            tokio::time::sleep(OPLOG_POLL_INTERVAL).await;
        }
    }
}
