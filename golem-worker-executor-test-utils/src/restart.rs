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

//! Restarting a test executor on the same storage, so that a test reconstructs its agents.

use crate::{
    TestContext, TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies,
    start_customized, start_with_overrides, start_with_redis_oplog_config,
};
use anyhow::anyhow;
use golem_common::model::RetryConfig;
use golem_worker_executor::services::golem_config::{
    HttpClientConfig, OplogConfig, SnapshotPolicy,
};

/// How [`TestWorkerExecutor::restart`] tears the old executor down before it starts the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// Drops the executor. This only asks its tasks to stop; the next start waits until the
    /// invocation loops of the dropped executor have exited.
    Drop,
    /// [`TestWorkerExecutor::shutdown_and_wait_for_invocation_loops`], then drops the executor.
    Shutdown,
    /// [`TestWorkerExecutor::release`]: evicts the cached agents and waits until the service
    /// graph is released. Tests on managed XFS roots need it.
    Release,
}

/// The start function, and its arguments, that created an executor.
#[derive(Clone)]
pub(crate) enum StartedWith {
    Customized {
        system_memory_override: Option<u64>,
        retry_override: Option<RetryConfig>,
        snapshot_policy_override: Option<SnapshotPolicy>,
        http_client_override: Option<HttpClientConfig>,
        oplog_config_override: Option<OplogConfig>,
    },
    Overrides(Box<TestExecutorOverrides>),
    RedisOplog(Option<OplogConfig>),
}

impl StartedWith {
    async fn start(
        self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
    ) -> anyhow::Result<TestWorkerExecutor> {
        match self {
            StartedWith::Customized {
                system_memory_override,
                retry_override,
                snapshot_policy_override,
                http_client_override,
                oplog_config_override,
            } => {
                start_customized(
                    deps,
                    context,
                    system_memory_override,
                    retry_override,
                    snapshot_policy_override,
                    http_client_override,
                    oplog_config_override,
                )
                .await
            }
            StartedWith::Overrides(overrides) => {
                start_with_overrides(deps, context, *overrides).await
            }
            StartedWith::RedisOplog(oplog_config_override) => {
                start_with_redis_oplog_config(deps, context, oplog_config_override).await
            }
        }
    }
}

impl TestWorkerExecutor {
    /// Tears this executor down as `teardown` says, then starts a new executor on the same
    /// dependencies and test context, with the start function and arguments that created this
    /// one. The new executor uses the same storage, so its agents are reconstructed from their
    /// oplogs.
    ///
    /// Only the executor is torn down: gates, servers and other handles the test holds stay
    /// alive. The new executor authenticates as the test account, also when this handle was
    /// made by [`TestWorkerExecutor::as_caller`].
    ///
    /// Supported for executors created by `start`, `start_with_snapshot_policy`,
    /// `start_with_http_client_config`, `start_with_oplog_config`, `start_customized`,
    /// `start_with_overrides`, `start_with_redis_storage` and `start_with_redis_oplog_config`.
    /// For the others, `restart` fails before it tears anything down.
    pub async fn restart(self, teardown: Teardown) -> anyhow::Result<Self> {
        let started_with = self.started_with.clone().ok_or_else(|| {
            anyhow!(
                "restart does not know the start function of this executor; \
                 tear it down and call that function again"
            )
        })?;
        let deps = self.deps.clone();
        let context = self.context.clone();
        self.tear_down(teardown).await?;
        Box::pin(started_with.start(&deps, &context)).await
    }

    /// Tears this executor down as `teardown` says, then starts a new executor with
    /// `start_with_overrides(deps, context, overrides)`. Use it when the restart changes the
    /// overrides on purpose; the new executor remembers `overrides` for a later
    /// [`TestWorkerExecutor::restart`].
    pub async fn restart_with(
        self,
        teardown: Teardown,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let deps = self.deps.clone();
        let context = self.context.clone();
        self.tear_down(teardown).await?;
        Box::pin(start_with_overrides(&deps, &context, overrides)).await
    }

    async fn tear_down(self, teardown: Teardown) -> anyhow::Result<()> {
        match teardown {
            Teardown::Drop => drop(self),
            Teardown::Shutdown => {
                self.shutdown_and_wait_for_invocation_loops().await?;
                drop(self);
            }
            Teardown::Release => self.release().await?,
        }
        Ok(())
    }
}
