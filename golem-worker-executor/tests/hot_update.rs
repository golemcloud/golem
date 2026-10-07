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

use crate::Tracing;
use crate::durability::{assert_snapshot_recovery_failed, assert_snapshot_recovery_loaded};
use async_lock::Mutex;
use axum::Router;
use axum::routing::post;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use golem_common::base_model::oplog::PublicUpdateDescription;
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::oplog::{
    OplogErrorKind, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
};
use golem_common::model::worker::{
    AgentUpdateMode, RevertToOplogIndex, RevertWorkerTarget, UpdateRecord,
};
use golem_common::model::{AgentEvent, AgentId, AgentStatus, OwnedAgentId, ScanCursor};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::dsl::{TestDsl, update_counts};

use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::filesystem_snapshot_testing::{
    TestFilesystemSnapshotStore, with_snapshot_store,
};
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::golem_config::{
    FilesystemSnapshotUploadConfig, OplogConfig, SnapshotPolicy,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_customized, start_with_overrides,
    start_with_snapshot_policy,
};
use http::StatusCode;
use log::info;
use pretty_assertions::{assert_eq, assert_ne};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v2")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

pub struct F1Blocker {
    pub value: u64,
    pub reached: tokio::sync::oneshot::Sender<()>,
    pub resume: tokio::sync::oneshot::Receiver<()>,
}

pub struct F1Control {
    reached: Option<tokio::sync::oneshot::Receiver<()>>,
    resume: tokio::sync::oneshot::Sender<()>,
}

impl F1Control {
    pub async fn await_reached(&mut self) {
        self.reached.take().unwrap().await.unwrap();
        debug!("F1 control reached blocking point");
    }

    pub fn resume(self) {
        let _ = self.resume.send(());
        debug!("F1 control resumed from blocking point");
    }
}

pub struct TestHttpServer {
    handle: JoinHandle<()>,
    f1_blocker: Arc<Mutex<Option<F1Blocker>>>,
    port: u16,
}

impl TestHttpServer {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

        let port = listener.local_addr().unwrap().port();

        let f1_blocker = Arc::new(Mutex::new(None::<F1Blocker>));
        let f1_blocker_clone = f1_blocker.clone();

        let handle = spawn(async move {
            let route = Router::new().route(
                "/f1",
                post(move |body: Bytes| {
                    async move {
                        let body: u64 = String::from_utf8(body.to_vec()).unwrap().parse().unwrap();
                        debug!("f1: {}", body);

                        let mut guard = f1_blocker_clone.lock().await;
                        if let Some(blocker) = &*guard
                            && blocker.value == body
                        {
                            let F1Blocker {
                                reached, resume, ..
                            } = guard.take().unwrap();
                            debug!("Reached f1 blocking point");
                            reached.send(()).unwrap();
                            debug!("Awaiting resume at f1 blocking point");
                            resume.await.unwrap();
                            debug!("Resuming from f1 blocking point");
                        }

                        StatusCode::OK
                    }
                    .in_current_span()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        });
        Self {
            handle,
            f1_blocker,
            port,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn abort(&self) {
        self.handle.abort()
    }

    pub async fn f1_control(&mut self, value: u64) -> F1Control {
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let blocker = F1Blocker {
            value,
            reached: reached_tx,
            resume: resume_rx,
        };
        let mut guard = self.f1_blocker.lock().await;
        *guard = Some(blocker);
        F1Control {
            reached: Some(reached_rx),
            resume: resume_tx,
        }
    }
}

/// The files of the agent of a snapshot-assisted update test.
#[derive(Clone)]
enum AgentFiles {
    /// The agent writes no file, and the executor keeps no filesystem snapshots.
    Untouched,
    /// The agent writes files before and after the selected snapshot record, and the executor
    /// keeps filesystem snapshots in the store.
    Snapshotted(TestFilesystemSnapshotStore),
}

impl AgentFiles {
    fn snapshotted(&self) -> bool {
        matches!(self, Self::Snapshotted(_))
    }

    /// Holds the next save of a filesystem snapshot when the executor keeps filesystem
    /// snapshots, so the snapshot record of that save is not usable until the release.
    fn hold_next_save(
        &self,
    ) -> Option<golem_worker_executor::filesystem_snapshot_testing::HeldSave> {
        match self {
            Self::Untouched => None,
            Self::Snapshotted(store) => Some(store.hold_next_save()),
        }
    }

    /// The overrides of an executor with the snapshot policy `policy`.
    fn overrides(&self, policy: SnapshotPolicy) -> TestExecutorOverrides {
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.oplog.default_snapshotting = policy.clone();
            })),
            filesystem_snapshot_store: match self {
                Self::Untouched => None,
                Self::Snapshotted(store) => {
                    Some((store.clone(), FilesystemSnapshotUploadConfig::default()))
                }
            },
            ..Default::default()
        }
    }

    async fn start(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        policy: SnapshotPolicy,
    ) -> anyhow::Result<TestWorkerExecutor> {
        start_with_overrides(deps, context, self.overrides(policy)).await
    }

    async fn start_with_local_resume(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        policy: SnapshotPolicy,
    ) -> anyhow::Result<TestWorkerExecutor> {
        crate::fork::start_with_local_resume_and(deps, context, self.overrides(policy)).await
    }

    /// Writes the file `path` with `path` as its content, when the agent writes files.
    async fn write(
        &self,
        executor: &TestWorkerExecutor,
        component: &ComponentDto,
        agent_id: &golem_common::model::agent::ParsedAgentId,
        path: &str,
    ) -> anyhow::Result<()> {
        if self.snapshotted() {
            executor
                .invoke_and_await_agent(
                    component,
                    agent_id,
                    "write_file",
                    data_value!(path.to_string(), path.to_string()),
                )
                .await?;
        }
        Ok(())
    }

    /// Checks that the agent holds each file of `paths` with its path as its content, when the
    /// agent writes files.
    async fn assert_files(
        &self,
        executor: &TestWorkerExecutor,
        component: &ComponentDto,
        agent_id: &golem_common::model::agent::ParsedAgentId,
        paths: &[&str],
    ) -> anyhow::Result<()> {
        if self.snapshotted() {
            let read = futures::stream::iter(paths)
                .then(|path| async move {
                    executor
                        .invoke_and_await_agent(
                            component,
                            agent_id,
                            "read_file",
                            data_value!(path.to_string()),
                        )
                        .await?
                        .into_typed::<Option<String>>()
                })
                .try_collect::<Vec<_>>()
                .await?;
            assert_eq!(
                read,
                paths
                    .iter()
                    .map(|path| Some(path.to_string()))
                    .collect::<Vec<_>>()
            );
        }
        Ok(())
    }

    /// The snapshot record that a snapshot-assisted update selects: the first one after
    /// `untouched_after` when the agent writes no file, and the first one after the last start
    /// of `method` when it writes files, as the writes move the records.
    async fn selected_record(
        &self,
        executor: &TestWorkerExecutor,
        worker_id: &AgentId,
        untouched_after: OplogIndex,
        method: &str,
    ) -> anyhow::Result<OplogIndex> {
        let after = if self.snapshotted() {
            last_start_of(executor, worker_id, method).await?
        } else {
            untouched_after
        };
        wait_for_snapshot_after(executor, worker_id, after).await
    }
}

/// The index of the last start of an invocation of `method`.
async fn last_start_of(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    method: &str,
) -> anyhow::Result<OplogIndex> {
    executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(
                    &params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(invoked)
                        if invoked.method_name == method
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("no invocation of {method} started"))
}

async fn wait_for_snapshot_after(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    after: OplogIndex,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            if let Some(snapshot) = oplog.iter().find(|entry| {
                entry.oplog_index > after && matches!(entry.entry, PublicOplogEntry::Snapshot(_))
            }) {
                break anyhow::Ok(snapshot.oplog_index);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

async fn wait_for_update_counts(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    expected: (usize, usize, usize),
) -> anyhow::Result<golem_common::model::worker::AgentMetadataDto> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(worker_id).await?;
            if update_counts(&metadata) == expected {
                break anyhow::Ok(metadata);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

#[test]
#[tracing::instrument]
async fn auto_update_on_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();

    let mut control = http_server.f1_control(100).await;
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "f1", data_value!(50u64))
                .await
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    control.resume();
    let mut control2 = http_server.f1_control(110).await;

    control2.await_reached().await;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);
    control2.resume();

    let result = fiber.await??;
    info!("result: {result:?}");

    executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?; // awaiting a result from f3 to make sure the metadata already contains the updates
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and eventually finishes with 150. The update is marked as a success.
    assert_eq!(result.into_typed::<u64>()?, 150);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result.into_typed::<u64>()?, 0);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn automatic_update_promotes_snapshot_from_previous_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let initial = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(initial.into_typed::<u32>()?, 0);

    let snapshots_before_update = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
    assert!(snapshots_before_update > 0);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    let snapshots_after_update = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
    assert_eq!(snapshots_after_update, snapshots_before_update);

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let loaded_snapshot_revision = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    assert_eq!(loaded_snapshot_revision.into_typed::<u32>()?, 1);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_after_auto_update_recovers_with_updated_component_context(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    let snapshots_before_invocation = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
    let before_snapshot = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(before_snapshot.into_typed::<u32>()?, 0);

    // Automatic snapshot creation is queued after the invocation result is published.
    let snapshot_count = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let count = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await?
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
                .count();
            if count > snapshots_before_invocation {
                break Ok::<_, anyhow::Error>(count);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(snapshot_count, snapshots_before_invocation + 1);

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let revision = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;
    let loaded_snapshot_revision = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    assert_eq!(revision.into_typed::<u32>()?, 2);
    assert_eq!(loaded_snapshot_revision.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_assisted_update_skips_pre_snapshot_history_and_replays_in_flight_tail(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    skips_pre_snapshot_history_and_replays_in_flight_tail(
        last_unique_id,
        deps,
        agent_update_v1,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_update_with_filesystem_snapshots_skips_pre_snapshot_history_and_keeps_the_files(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    with_snapshot_store(|store| async move {
        skips_pre_snapshot_history_and_replays_in_flight_tail(
            last_unique_id,
            deps,
            agent_update_v1,
            AgentFiles::Snapshotted(store),
        )
        .await
    })
    .await
}

async fn skips_pre_snapshot_history_and_replays_in_flight_tail(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    files: AgentFiles,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = files
        .start_with_local_resume(
            deps,
            &context,
            SnapshotPolicy::EveryNInvocation { count: 2 },
        )
        .await?;
    let mut http_server = TestHttpServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;
    files
        .write(&executor, &component, &agent_id, "/before.txt")
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "pre_snapshot_value", data_value!())
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 1);
    let untouched_snapshot = if files.snapshotted() {
        None
    } else {
        Some(wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?)
    };
    let post_snapshot = executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    assert_eq!(post_snapshot.into_typed::<u32>()?, 7);
    let snapshot_index = match untouched_snapshot {
        Some(snapshot_index) => snapshot_index,
        None => {
            files
                .selected_record(&executor, &worker_id, OplogIndex::INITIAL, "stable_value")
                .await?
        }
    };
    files
        .write(&executor, &component, &agent_id, "/tail.txt")
        .await?;
    if files.snapshotted() {
        assert!(snapshot_index < last_start_of(&executor, &worker_id, "write_file").await?);
    }
    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    let mut control = http_server.f1_control(900).await;
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "blocking_stable",
                    data_value!(900u64),
                )
                .await
        }
        .in_current_span(),
    );
    control.await_reached().await;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let pending_index = executor
        .get_oplog(&worker_id, snapshot_index.next())
        .await?
        .into_iter()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)).then_some(entry.oplog_index)
        })
        .expect("assisted update must append PendingUpdate before source work completes");

    assert!(!invocation.is_finished());
    control.resume();
    assert_eq!(invocation.await??.into_typed::<u32>()?, 111);
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(
                    &params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "blocking_stable"
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("blocking agent invocation must have a start");
    let success_index = oplog
        .iter()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_))
                .then_some(entry.oplog_index)
        })
        .expect("assisted update must record SuccessfulUpdate");
    let invocation_end = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(params)
                if params.method_name.as_deref() == Some("blocking_stable") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("blocking invocation must finish after the update handoff");
    assert!(
        invocation_start < pending_index
            && pending_index < success_index
            && success_index < invocation_end,
        "blocking invocation must span P and finish only after U: start={invocation_start}, P={pending_index}, U={success_index}, end={invocation_end}"
    );
    drop(executor);

    let executor = files
        .start_with_local_resume(
            deps,
            &context,
            SnapshotPolicy::EveryNInvocation { count: 2 },
        )
        .await?;
    let loaded = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(loaded.into_typed::<u32>()?, 1);
    let accumulated = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?;
    assert_eq!(accumulated.into_typed::<u32>()?, 111);

    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    let success = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => Some(update),
            _ => None,
        })
        .expect("assisted update must record a successful terminal outcome");
    assert_eq!(success.mode, AgentUpdateMode::Automatic);
    let details = success
        .snapshot_assisted_details
        .as_ref()
        .expect("assisted success must expose snapshot provenance");
    assert_eq!(details.snapshot_index, snapshot_index);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_)))
            .count(),
        1
    );
    assert!(!oplog.iter().any(|entry| matches!(
        entry.entry,
        PublicOplogEntry::FailedUpdate(_) | PublicOplogEntry::Error(_)
    )));

    let fork = phantom_agent_id!("SnapshotUpdateTest", uuid::Uuid::new_v4());
    executor
        .fork_worker(&worker_id, &fork.to_string(), success_index)
        .await?;
    let fork_value = executor
        .invoke_and_await_agent(&component, &fork, "accumulated_value", data_value!())
        .await?;
    assert_eq!(fork_value.into_typed::<u32>()?, 111);
    files
        .assert_files(&executor, &component, &fork, &["/before.txt", "/tail.txt"])
        .await?;

    let later_snapshot = wait_for_snapshot_after(&executor, &worker_id, success_index).await?;
    executor
        .return_empty_snapshot_payload(&worker_id, later_snapshot)
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !executor.stop_worker_if_idle(&owned).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let mut events = executor.capture_output(&worker_id).await?;
    let recovered = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?;
    assert_eq!(recovered.into_typed::<u32>()?, 111);
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut rejected_later_snapshot = false;
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoveryFailed {
                    snapshot_index,
                    error,
                    ..
                }) if snapshot_index == later_snapshot => {
                    assert!(error.contains("Snapshot is empty"), "{error}");
                    rejected_later_snapshot = true;
                }
                Ok(AgentEvent::SnapshotRecoverySucceeded {
                    snapshot_index: recovered_index,
                    ..
                }) if rejected_later_snapshot && recovered_index == snapshot_index => break,
                _ => {}
            }
        }
        assert!(rejected_later_snapshot);
    })
    .await?;
    files
        .assert_files(
            &executor,
            &component,
            &agent_id,
            &["/before.txt", "/tail.txt"],
        )
        .await?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn automatic_update_without_snapshot_falls_back_to_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let stable = executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    assert_eq!(stable.into_typed::<u32>()?, 7);
    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(metadata.retry_count, 0);
    assert_eq!(metadata.last_error, None);
    let success = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => Some(update),
            _ => None,
        })
        .expect("automatic fallback must record a successful terminal outcome");
    assert_eq!(success.mode, AgentUpdateMode::Automatic);
    assert!(success.snapshot_assisted_details.is_none());

    let source = executor
        .invoke_and_await_agent(&component, &agent_id, "pre_snapshot_value", data_value!())
        .await?;
    assert_eq!(source.into_typed::<u32>()?, 2);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_)))
            .count(),
        1
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

async fn assert_automatic_update_rejects_agent_mode_change(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    snapshot_policy: SnapshotPolicy,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let expect_snapshot = !matches!(snapshot_policy, SnapshotPolicy::Disabled);
    let executor = start_with_snapshot_policy(deps, &context, snapshot_policy).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let source_value = executor
        .invoke_and_await_agent(&component, &agent_id, "replay_revision", data_value!())
        .await?;
    assert_eq!(source_value.into_typed::<u32>()?, 0);
    if expect_snapshot {
        wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    }

    let target = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, target.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    let failure = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => Some(update),
            _ => None,
        })
        .expect("mode-changing automatic update must fail");
    assert!(
        failure
            .details
            .as_deref()
            .is_some_and(|details| details.contains("mode Ephemeral"))
    );
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(metadata.status, AgentStatus::Idle);
    assert_eq!(metadata.last_error, None);
    if expect_snapshot {
        assert!(
            failure.snapshot_assisted_details.as_ref().is_some(),
            "snapshot-assisted mode rejection must preserve selected snapshot provenance"
        );
    } else {
        assert!(failure.snapshot_assisted_details.is_none());
    }
    let source_value = executor
        .invoke_and_await_agent(&component, &agent_id, "replay_revision", data_value!())
        .await?;
    assert_eq!(source_value.into_typed::<u32>()?, 0);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::FailedUpdate(_)))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn automatic_full_replay_rejects_agent_mode_change(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_automatic_update_rejects_agent_mode_change(
        last_unique_id,
        deps,
        agent_update_v1,
        SnapshotPolicy::Disabled,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn automatic_snapshot_assisted_rejects_agent_mode_change(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_automatic_update_rejects_agent_mode_change(
        last_unique_id,
        deps,
        agent_update_v1,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_assisted_replay_mismatch_fails_once_and_preserves_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 3 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..2 {
        let stable = executor
            .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
            .await?;
        assert_eq!(stable.into_typed::<u32>()?, 7);
    }
    let snapshot_index =
        wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    let source_value = executor
        .invoke_and_await_agent(&component, &agent_id, "pre_snapshot_value", data_value!())
        .await?;
    assert_eq!(source_value.into_typed::<u32>()?, 1);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(metadata.retry_count, 0);
    assert_eq!(metadata.last_error, None);
    let failure = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => Some(update),
            _ => None,
        })
        .expect("suffix mismatch must record one failed update");
    assert_eq!(failure.mode, AgentUpdateMode::Automatic);
    let details = failure
        .snapshot_assisted_details
        .as_ref()
        .expect("suffix replay failure must expose attempted provenance");
    assert_eq!(details.snapshot_index, snapshot_index);

    let source = executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    assert_eq!(source.into_typed::<u32>()?, 7);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::FailedUpdate(_)))
            .count(),
        1
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum SnapshotAssistedSuffixFailure {
    HostCallDivergence,
    Trap,
    Exit,
}

async fn assert_snapshot_assisted_suffix_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    failure: SnapshotAssistedSuffixFailure,
    files: AgentFiles,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = files
        .start(
            deps,
            &context,
            SnapshotPolicy::EveryNInvocation { count: 2 },
        )
        .await?;
    let http_server = TestHttpServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;
    // Two writes keep the records where the agent without files has them.
    files
        .write(&executor, &component, &agent_id, "/before.txt")
        .await?;
    files
        .write(&executor, &component, &agent_id, "/before-2.txt")
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    let snapshot_index = files
        .selected_record(&executor, &worker_id, OplogIndex::INITIAL, "stable_value")
        .await?;
    // The record after the tail write and the diverging call stays unusable while its save is
    // held, so the update selects the record before them and replays both.
    let held = files.hold_next_save();
    files
        .write(&executor, &component, &agent_id, "/tail.txt")
        .await?;

    let (function, input, expected_cause) = match failure {
        SnapshotAssistedSuffixFailure::HostCallDivergence => {
            ("suffix_host_value", data_value!(), "unexpected oplog entry")
        }
        SnapshotAssistedSuffixFailure::Trap => ("suffix_trap", data_value!(), "component trapped"),
        SnapshotAssistedSuffixFailure::Exit => ("suffix_exit", data_value!(), "exit"),
    };
    let source_result = executor
        .invoke_and_await_agent(&component, &agent_id, function, input)
        .await?;
    assert_eq!(source_result.into_typed::<u32>()?, 7);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(metadata.retry_count, 0);
    assert_eq!(metadata.last_error, None);
    let failed = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => Some(update),
            _ => None,
        })
        .expect("assisted suffix failure must record FailedUpdate");
    let details = failed
        .details
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        details.contains(expected_cause),
        "unexpected assisted failure: {details}"
    );
    assert_eq!(
        failed
            .snapshot_assisted_details
            .as_ref()
            .map(|details| details.snapshot_index),
        Some(snapshot_index)
    );

    let healthy = executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    assert_eq!(healthy.into_typed::<u32>()?, 7);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::FailedUpdate(_)))
            .count(),
        1
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
    );
    if let Some(held) = held {
        held.release();
    }
    files
        .assert_files(
            &executor,
            &component,
            &agent_id,
            &["/before.txt", "/before-2.txt", "/tail.txt"],
        )
        .await?;
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_host_call_divergence_preserves_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_snapshot_assisted_suffix_failure(
        last_unique_id,
        deps,
        agent_update_v1,
        SnapshotAssistedSuffixFailure::HostCallDivergence,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_host_call_divergence_with_filesystem_snapshots_preserves_source_and_its_files(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    with_snapshot_store(|store| async move {
        assert_snapshot_assisted_suffix_failure(
            last_unique_id,
            deps,
            agent_update_v1,
            SnapshotAssistedSuffixFailure::HostCallDivergence,
            AgentFiles::Snapshotted(store),
        )
        .await
    })
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_guest_trap_preserves_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_snapshot_assisted_suffix_failure(
        last_unique_id,
        deps,
        agent_update_v1,
        SnapshotAssistedSuffixFailure::Trap,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_guest_exit_preserves_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_snapshot_assisted_suffix_failure(
        last_unique_id,
        deps,
        agent_update_v1,
        SnapshotAssistedSuffixFailure::Exit,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_schema_rejection_fails_once_and_allows_later_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;

    let rejecting = executor
        .update_component(&component.id, "it_agent_update_v4_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, rejecting.revision, false)
        .await?;
    let failed = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    assert_eq!(failed.component_revision, component.revision);
    let cause = failed
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => update.details.as_deref(),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        cause.contains("Invalid snapshot - simulating failure"),
        "{cause}"
    );

    let compatible = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, compatible.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, compatible.revision, Duration::from_secs(30))
        .await?;
    let revision = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    assert_eq!(revision.into_typed::<u32>()?, 2);
    Ok(())
}

/// A payload of the selected record that does not download once is a transient failure: the
/// frozen attempt runs again with the same record, and the update succeeds without a failed
/// update.
#[test]
#[timeout("120s")]
async fn snapshot_assisted_payload_download_failure_retries_the_same_record(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = OplogConfig {
        max_payload_size: 0,
        default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 2 },
        ..Default::default()
    };
    let executor = start_customized(deps, &context, None, None, None, None, Some(config)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("ExternalSnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let snapshot_index =
        wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    executor.fail_snapshot_download_once(&worker_id, snapshot_index);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    let success = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => Some(update),
            _ => None,
        })
        .expect("the update succeeds");

    assert_eq!(metadata.component_revision, updated_component.revision);
    assert!(success.snapshot_assisted_details.is_some());
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    Ok(())
}

/// A payload of the selected record that is missing from its store is a lost record: the update
/// fails once with `UPDATE_SNAPSHOT_UNAVAILABLE`, and the agent runs on its source revision.
#[test]
#[timeout("120s")]
async fn snapshot_assisted_missing_payload_fails_the_update_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = OplogConfig {
        max_payload_size: 0,
        default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 2 },
        ..Default::default()
    };
    let executor = start_customized(deps, &context, None, None, None, None, Some(config)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("ExternalSnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let snapshot_index =
        wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    executor.lose_snapshot_payload(&worker_id, snapshot_index);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    let failed = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => Some(update),
            _ => None,
        })
        .expect("the update fails");
    let details = failed.details.as_deref().unwrap_or_default();
    assert!(
        details.starts_with("UPDATE_SNAPSHOT_UNAVAILABLE"),
        "{details}"
    );
    assert_eq!(metadata.component_revision, component.revision);
    drop(executor);

    // A fresh start folds the failed update from the oplog: it never selects the lost record
    // again, also where its payload downloads, and replays the whole history.
    let executor = start_customized(
        deps,
        &context,
        None,
        None,
        None,
        None,
        Some(OplogConfig {
            max_payload_size: 0,
            default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 2 },
            ..Default::default()
        }),
    )
    .await?;
    let loaded = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?
        .into_typed::<u32>()?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    assert_eq!(loaded, 0);
    assert_eq!(metadata.component_revision, component.revision);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn automatic_and_manual_update_contracts_remain_distinct(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let strict_context = TestContext::new(last_unique_id);
    let strict = start_with_snapshot_policy(
        deps,
        &strict_context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let strict_component = strict
        .component_dep(&strict_context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let strict_worker = strict
        .start_agent(&strict_component.id, agent_id.clone())
        .await?;
    strict
        .invoke_and_await_agent(
            &strict_component,
            &agent_id,
            "pre_snapshot_value",
            data_value!(),
        )
        .await?;
    wait_for_snapshot_after(&strict, &strict_worker, OplogIndex::INITIAL).await?;
    let strict_target = strict
        .update_component(&strict_component.id, "it_agent_update_v2_release")
        .await?;
    strict
        .auto_update_worker(&strict_worker, strict_target.revision, false)
        .await?;
    let automatic_metadata = wait_for_update_counts(&strict, &strict_worker, (0, 1, 0)).await?;
    assert_eq!(
        automatic_metadata.component_revision,
        strict_target.revision
    );
    let automatic_success = automatic_metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => Some(update),
            _ => None,
        })
        .expect("automatic update must complete");
    assert!(automatic_success.snapshot_assisted_details.is_some());
    drop(strict);

    let manual_context = TestContext::new(last_unique_id);
    let manual = start(deps, &manual_context).await?;
    let mut http_server = TestHttpServer::start().await;
    let manual_component = manual
        .component_dep(&manual_context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let manual_worker = manual
        .start_agent_with(
            &manual_component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;
    let manual_target = manual
        .update_component(&manual_component.id, "it_agent_update_v2_release")
        .await?;
    let mut control = http_server.f1_control(700).await;
    let manual_clone = manual.clone();
    let component_clone = manual_component.clone();
    let agent_clone = agent_id.clone();
    let invocation = spawn(async move {
        manual_clone
            .invoke_and_await_agent(
                &component_clone,
                &agent_clone,
                "blocking_stable",
                data_value!(700u64),
            )
            .await
    });
    control.await_reached().await;
    manual
        .manual_update_worker(&manual_worker, manual_target.revision, false)
        .await?;
    let pending = wait_for_update_counts(&manual, &manual_worker, (1, 0, 0)).await?;
    assert_eq!(pending.component_revision, manual_component.revision);
    assert!(!invocation.is_finished());
    control.resume();
    assert_eq!(invocation.await??.into_typed::<u32>()?, 100);
    manual
        .wait_for_component_revision(
            &manual_worker,
            manual_target.revision,
            Duration::from_secs(30),
        )
        .await?;
    assert_eq!(
        update_counts(&manual.get_worker_metadata(&manual_worker).await?),
        (0, 1, 0)
    );
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_restart_before_attempt_and_later_update_modes_succeed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    restart_before_attempt_and_later_update_modes_succeed(
        last_unique_id,
        deps,
        agent_update_v1,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_restart_before_attempt_and_later_update_modes_with_filesystem_snapshots_keep_the_files(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    with_snapshot_store(|store| async move {
        restart_before_attempt_and_later_update_modes_succeed(
            last_unique_id,
            deps,
            agent_update_v1,
            AgentFiles::Snapshotted(store),
        )
        .await
    })
    .await
}

async fn restart_before_attempt_and_later_update_modes_succeed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    files: AgentFiles,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = files.start(deps, &context, policy.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let mut http_server = TestHttpServer::start().await;
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;
    // Two writes keep the records where the agent without files has them.
    files
        .write(&executor, &component, &agent_id, "/before.txt")
        .await?;
    files
        .write(&executor, &component, &agent_id, "/before-2.txt")
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    let snapshot_index = files
        .selected_record(&executor, &worker_id, OplogIndex::INITIAL, "stable_value")
        .await?;
    files
        .write(&executor, &component, &agent_id, "/tail.txt")
        .await?;

    let mut control = http_server.f1_control(901).await;
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = spawn(async move {
        executor_clone
            .invoke_and_await_agent(
                &component_clone,
                &agent_id_clone,
                "blocking_stable",
                data_value!(901u64),
            )
            .await
    });
    control.await_reached().await;
    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let interrupt = spawn(async move { executor_clone.interrupt(&worker_id_clone).await });
    control.resume();
    interrupt.await??;
    let _ = invocation.await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let assisted = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, assisted.revision, true)
        .await?;
    let pending = wait_for_update_counts(&executor, &worker_id, (1, 0, 0)).await?;
    assert_eq!(pending.component_revision, component.revision);
    drop(executor);

    let executor = files.start(deps, &context, policy).await?;
    executor.resume(&worker_id, true).await?;
    executor
        .wait_for_component_revision(&worker_id, assisted.revision, Duration::from_secs(30))
        .await?;
    let first_update = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(update_counts(&first_update), (0, 1, 0));
    assert_eq!(
        first_update.updates.iter().find_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => update
                .snapshot_assisted_details
                .as_ref()
                .map(|details| details.snapshot_index),
            _ => None,
        }),
        Some(snapshot_index),
        "the update after the restart restores the record before the tail"
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    let automatic_without_new_snapshot = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, automatic_without_new_snapshot.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            automatic_without_new_snapshot.revision,
            Duration::from_secs(30),
        )
        .await?;

    let manual = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, manual.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, manual.revision, Duration::from_secs(30))
        .await?;
    let before_snapshot = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .unwrap()
        .oplog_index;
    for _ in 0..2 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
            .await?;
    }
    wait_for_snapshot_after(&executor, &worker_id, before_snapshot).await?;

    let assisted_again = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, assisted_again.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, assisted_again.revision, Duration::from_secs(30))
        .await?;
    assert_eq!(
        update_counts(&executor.get_worker_metadata(&worker_id).await?),
        (0, 4, 0)
    );
    files
        .assert_files(
            &executor,
            &component,
            &agent_id,
            &["/before.txt", "/before-2.txt", "/tail.txt"],
        )
        .await?;
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn queued_automatic_targets_select_strategy_when_each_reaches_queue_head(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let mut http_server = TestHttpServer::start().await;
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    let mut control = http_server.f1_control(902).await;
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = spawn(async move {
        executor_clone
            .invoke_and_await_agent(
                &component_clone,
                &agent_id_clone,
                "blocking_stable",
                data_value!(902u64),
            )
            .await
    });
    control.await_reached().await;
    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let interrupt = spawn(async move { executor_clone.interrupt(&worker_id_clone).await });
    control.resume();
    interrupt.await??;
    let _ = invocation.await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    let revision_two = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    let revision_three = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, revision_two.revision, true)
        .await?;
    executor
        .auto_update_worker(&worker_id, revision_three.revision, true)
        .await?;

    let pending = wait_for_update_counts(&executor, &worker_id, (2, 0, 0)).await?;
    assert_eq!(pending.component_revision, component.revision);
    let pending_attempts = pending
        .updates
        .iter()
        .filter_map(|record| match record {
            UpdateRecord::PendingUpdate(update) => update.pending_update_index,
            _ => None,
        })
        .collect::<HashSet<_>>();
    assert_eq!(pending_attempts.len(), 2);

    executor.resume(&worker_id, true).await?;
    executor
        .wait_for_component_revision(&worker_id, revision_three.revision, Duration::from_secs(30))
        .await?;
    let completed = wait_for_update_counts(&executor, &worker_id, (0, 2, 0)).await?;
    assert_eq!(completed.component_revision, revision_three.revision);
    let successes = completed
        .updates
        .iter()
        .filter_map(|record| match record {
            UpdateRecord::SuccessfulUpdate(update) => Some(update),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(successes.len(), 2);
    assert_eq!(successes[0].target_revision, revision_two.revision);
    assert_eq!(successes[1].target_revision, revision_three.revision);
    assert!(successes[0].snapshot_assisted_details.is_some());
    assert!(successes[1].snapshot_assisted_details.is_none());
    assert_eq!(
        successes
            .iter()
            .filter_map(|update| update.pending_update_index)
            .collect::<HashSet<_>>(),
        pending_attempts,
    );
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_revert_retries_retained_successful_and_failed_requests(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    revert_retries_retained_successful_and_failed_requests(
        last_unique_id,
        deps,
        agent_update_v1,
        AgentFiles::Untouched,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn snapshot_assisted_revert_with_filesystem_snapshots_retries_retained_requests_with_the_files(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    with_snapshot_store(|store| async move {
        revert_retries_retained_successful_and_failed_requests(
            last_unique_id,
            deps,
            agent_update_v1,
            AgentFiles::Snapshotted(store),
        )
        .await
    })
    .await
}

async fn revert_retries_retained_successful_and_failed_requests(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    files: AgentFiles,
) -> anyhow::Result<()> {
    for succeeds in [true, false] {
        let context = TestContext::new(last_unique_id);
        let executor = files
            .start(
                deps,
                &context,
                SnapshotPolicy::EveryNInvocation { count: 3 },
            )
            .await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_update_v1)
            .store()
            .await?;
        let agent_id = agent_id!("SnapshotUpdateTest");
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        // The write takes the place of one invocation, so the records stay where the agent
        // without files has them.
        files
            .write(&executor, &component, &agent_id, "/before.txt")
            .await?;
        let before_record = if files.snapshotted() { 1 } else { 2 };
        futures::stream::iter(0..before_record)
            .then(|_| {
                executor.invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "stable_value",
                    data_value!(),
                )
            })
            .try_collect::<Vec<_>>()
            .await?;
        files
            .selected_record(&executor, &worker_id, OplogIndex::INITIAL, "stable_value")
            .await?;
        files
            .write(&executor, &component, &agent_id, "/tail.txt")
            .await?;
        if !succeeds {
            executor
                .invoke_and_await_agent(&component, &agent_id, "pre_snapshot_value", data_value!())
                .await?;
        }
        let target = executor
            .update_component(&component.id, "it_agent_update_v2_release")
            .await?;
        executor
            .auto_update_worker(&worker_id, target.revision, false)
            .await?;
        let expected = if succeeds { (0, 1, 0) } else { (0, 0, 1) };
        let first_terminal = wait_for_update_counts(&executor, &worker_id, expected).await?;
        let (attempt_index, assisted_details) = first_terminal
            .updates
            .iter()
            .find_map(|record| match record {
                UpdateRecord::SuccessfulUpdate(update) => Some((
                    update.pending_update_index,
                    update.snapshot_assisted_details.as_ref(),
                )),
                UpdateRecord::FailedUpdate(update) => Some((
                    update.pending_update_index,
                    update.snapshot_assisted_details.as_ref(),
                )),
                UpdateRecord::PendingUpdate(_) => None,
            })
            .map(|(attempt, details)| {
                (
                    attempt.expect("terminal update must retain its admission identity"),
                    details
                        .expect("terminal update must retain assisted provenance")
                        .clone(),
                )
            })
            .expect("update must have a terminal record");
        let pending_entries = executor
            .get_oplog(&worker_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::PendingUpdate(params) => Some((
                    entry.oplog_index,
                    params.update_attempt_index,
                    params.description.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pending_entries.len(), 2);
        assert!(matches!(
            pending_entries[0].2,
            PublicUpdateDescription::Automatic(_)
        ));
        assert!(matches!(
            pending_entries[1].2,
            PublicUpdateDescription::SnapshotAssistedAutomatic(_)
        ));
        assert_eq!(pending_entries[0].1, attempt_index);
        assert_eq!(pending_entries[1].1, attempt_index);
        let selection_index = pending_entries[1].0;
        executor
            .revert(
                &worker_id,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: selection_index,
                }),
            )
            .await?;
        executor.resume(&worker_id, true).await?;
        let retried = wait_for_update_counts(&executor, &worker_id, expected).await?;
        assert_eq!(
            retried.component_revision,
            if succeeds {
                target.revision
            } else {
                component.revision
            }
        );
        let (retried_attempt, retried_details) = retried
            .updates
            .iter()
            .find_map(|record| match record {
                UpdateRecord::SuccessfulUpdate(update) => Some((
                    update.pending_update_index,
                    update.snapshot_assisted_details.as_ref(),
                )),
                UpdateRecord::FailedUpdate(update) => Some((
                    update.pending_update_index,
                    update.snapshot_assisted_details.as_ref(),
                )),
                UpdateRecord::PendingUpdate(_) => None,
            })
            .expect("retried update must have a terminal record");
        assert_eq!(retried_attempt, Some(attempt_index));
        assert_eq!(retried_details, Some(&assisted_details));
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)))
                .count(),
            2,
            "outcome-only revert must reuse the persisted strategy"
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
                .count(),
            0
        );
        files
            .assert_files(
                &executor,
                &component,
                &agent_id,
                &["/before.txt", "/tail.txt"],
            )
            .await?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AutomaticSnapshotLoadFailure {
    InvalidEntry,
    PayloadDownload,
}

async fn manual_update_with_periodic_snapshot(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    agent_update_v1: &PrecompiledComponent,
) -> anyhow::Result<(ComponentDto, AgentId, OplogIndex)> {
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let initial = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(initial.into_typed::<u32>()?, 0);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    // The manual snapshot contains v1's marker, even though the new component saves marker 2.
    let restored = executor
        .invoke_and_await_agent(
            &updated_component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(restored.into_typed::<u32>()?, 1);
    let result = executor
        .invoke_and_await_agent(
            &updated_component,
            &agent_id,
            "revision_two_only",
            data_value!(),
        )
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 2);

    let snapshot_index = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let update_position = oplog
                .iter()
                .rposition(|entry| matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_)))
                .unwrap();
            if let Some(snapshot) = oplog[update_position + 1..]
                .iter()
                .find(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
            {
                return anyhow::Ok(snapshot.oplog_index);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok((updated_component, worker_id, snapshot_index))
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_valid_tail_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let (component, worker_id, snapshot_index) =
        manual_update_with_periodic_snapshot(&executor, &context, agent_update_v1).await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let tail = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    assert_eq!(tail.into_typed::<u32>()?, 2);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .rfind(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
            .unwrap()
            .oplog_index,
        snapshot_index
    );
    assert!(oplog.iter().any(|entry| entry.oplog_index > snapshot_index
        && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))));
    drop(executor);

    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let mut events = executor.capture_output(&worker_id).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 2);
    assert_snapshot_recovery_loaded(&mut events).await;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_result_divergence_uses_manual_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let (component, worker_id, snapshot_index) =
        manual_update_with_periodic_snapshot(&executor, &context, agent_update_v1).await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let tail = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(tail.into_typed::<u32>()?, 1);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .rfind(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
            .unwrap()
            .oplog_index,
        snapshot_index
    );
    assert!(oplog.iter().any(|entry| entry.oplog_index > snapshot_index
        && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))));
    drop(executor);

    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let mut events = executor.capture_output(&worker_id).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await;
    let loaded_index = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    return snapshot_index;
                }
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                    panic!("The periodic snapshot must load before its result diverges: {error}");
                }
                _ => {}
            }
        }
        panic!("Missing snapshot load success before result divergence");
    })
    .await?;
    assert_eq!(loaded_index, snapshot_index);
    assert!(
        result.is_ok(),
        "Result divergence after snapshot {snapshot_index} must retry the valid manual baseline: {result:?}"
    );
    assert_eq!(result?.into_typed::<u32>()?, 1);
    let failed_index = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if let Ok(AgentEvent::SnapshotRecoveryFailed {
                snapshot_index,
                error,
                ..
            }) = AgentEvent::try_from(event)
            {
                assert!(error.contains("loaded_snapshot_revision"), "{error}");
                return snapshot_index;
            }
        }
        panic!("Missing periodic snapshot rejection");
    })
    .await?;
    assert_eq!(failed_index, snapshot_index);
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

async fn assert_manual_periodic_snapshot_rejection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    newer_snapshot: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = OplogConfig {
        default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 2 },
        ..Default::default()
    };
    let executor =
        start_customized(deps, &context, None, None, None, None, Some(config.clone())).await?;
    let (component, worker_id, rejected_index) =
        manual_update_with_periodic_snapshot(&executor, &context, agent_update_v1).await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    executor
        .return_empty_snapshot_payload(&worker_id, rejected_index)
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !executor.stop_worker_if_idle(&owned).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let mut events = executor.capture_output(&worker_id).await?;
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 2);
    assert_snapshot_recovery_failed(&mut events, "load-snapshot returned error").await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if let Ok(AgentEvent::InvocationFinished { function, .. }) = AgentEvent::try_from(event)
                && function == "revision_two_only"
            {
                return;
            }
        }
        panic!("Missing live invocation completion after fallback");
    })
    .await?;

    let expected_snapshot = if newer_snapshot {
        let result = executor
            .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
            .await?;
        assert_eq!(result.into_typed::<u32>()?, 2);
        let new_index = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                if let Some(entry) = oplog.iter().find(|entry| {
                    entry.oplog_index > rejected_index
                        && matches!(entry.entry, PublicOplogEntry::Snapshot(_))
                }) {
                    return anyhow::Ok(entry.oplog_index);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !executor.stop_worker_if_idle(&owned).await? {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await??;
        Some(new_index)
    } else {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            oplog
                .iter()
                .rfind(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
                .unwrap()
                .oplog_index,
            rejected_index
        );
        None
    };
    let executor = if newer_snapshot {
        executor
    } else {
        drop(executor);
        start_customized(deps, &context, None, None, None, None, Some(config)).await?
    };
    let mut events = if newer_snapshot {
        events
    } else {
        drop(events);
        executor.capture_output(&worker_id).await?
    };
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 2);
    let selected_index = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    return snapshot_index;
                }
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                    panic!("Unexpected recovery failure: {error}")
                }
                _ => {}
            }
        }
        panic!("Missing recovery event");
    })
    .await?;
    match expected_snapshot {
        Some(expected) => assert_eq!(
            selected_index, expected,
            "A newer snapshot must be eligible after successful live execution and unload"
        ),
        None => assert_ne!(
            selected_index, rejected_index,
            "Executor restart selected the rejected periodic snapshot again"
        ),
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_rejection_survives_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_manual_periodic_snapshot_rejection(last_unique_id, deps, agent_update_v1, false).await
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_rejection_allows_newer_snapshot_after_unload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_manual_periodic_snapshot_rejection(last_unique_id, deps, agent_update_v1, true).await
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_failed_manual_baseline_returns_error_without_looping(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = OplogConfig {
        default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 2 },
        ..Default::default()
    };
    let executor = start_customized(deps, &context, None, None, None, None, Some(config)).await?;
    let (component, worker_id, periodic_index) =
        manual_update_with_periodic_snapshot(&executor, &context, agent_update_v1).await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let manual_index = oplog
        .iter()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)))
        .unwrap()
        .oplog_index;
    executor
        .return_empty_snapshot_payload(&worker_id, periodic_index)
        .await?;
    executor
        .return_empty_snapshot_payload(&worker_id, manual_index)
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !executor.stop_worker_if_idle(&owned).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;

    let mut events = executor.capture_output(&worker_id).await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let invocation =
        executor.invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!());
    tokio::pin!(invocation);
    let mut manual_failures = 0;
    let mut periodic_failures = 0;
    let outcome = tokio::time::timeout(Duration::from_secs(15), async {
        let mut outcome = None;
        loop {
            // Invocation responses and captured events use independent transports.
            if periodic_failures > 0 && manual_failures > 0
                && let Some(result) = outcome.take()
            {
                break result;
            }
            tokio::select! {
                biased;
                event = events.recv() => {
                    let event = event.expect("Recovery event stream ended before both failures arrived");
                    if let Ok(AgentEvent::SnapshotRecoveryFailed { snapshot_index, error, .. }) = AgentEvent::try_from(event) {
                        if snapshot_index == periodic_index {
                            periodic_failures += 1;
                        } else {
                            assert_eq!(snapshot_index, manual_index);
                            manual_failures += 1;
                            assert!(manual_failures <= 1, "Manual baseline {manual_index} was retried after its load failed: {error}");
                        }
                    }
                }
                result = &mut invocation, if outcome.is_none() => outcome = Some(result),
            }
        }
    }).await.expect("Recovery must return the manual snapshot load failure rather than retry forever");
    let error = outcome.expect_err("Both snapshot payloads are invalid");
    let details = format!("{error:#}");
    assert!(details.contains("Snapshot is empty"), "{details}");
    assert!(details.contains(&manual_index.to_string()), "{details}");
    assert!(!details.contains("PreviousInvocationFailed"), "{details}");
    assert_eq!(periodic_failures, 1);
    assert_eq!(manual_failures, 1);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn manual_periodic_snapshot_temporary_download_failure_is_retryable_on_cached_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let (component, worker_id, periodic_index) =
        manual_update_with_periodic_snapshot(&executor, &context, agent_update_v1).await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let manual_index = oplog
        .iter()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)))
        .unwrap()
        .oplog_index;
    executor
        .return_empty_snapshot_payload(&worker_id, manual_index)
        .await?;
    executor.fail_snapshot_download_once(&worker_id, periodic_index);

    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !executor.stop_worker_if_idle(&owned).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;

    let agent_id = agent_id!("SnapshotUpdateTest");
    let mut events = executor.capture_output(&worker_id).await?;
    let first = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await;
    let details = format!(
        "{:#}",
        first.expect_err("The periodic download and manual snapshot load must both fail")
    );
    assert!(details.contains("Snapshot is empty"), "{details}");
    assert!(details.contains(&manual_index.to_string()), "{details}");
    assert_snapshot_recovery_failed(&mut events, "Failed to download snapshot payload").await;
    assert_snapshot_recovery_failed(&mut events, "load-snapshot returned error").await;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Error(params)
                    if params.kind == OplogErrorKind::Recovery
            ))
            .count(),
        1,
        "The failed startup must append one durable recovery error"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::RecoverySucceeded(_)))
            .count(),
        0,
        "Recovery must remain unresolved until a startup succeeds"
    );
    assert!(
        executor.worker_is_cached(&owned).await,
        "The second logical startup must reuse the same cached Worker"
    );

    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_loaded(&owned).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    // The unavailable periodic snapshot is retryable, but this startup ultimately failed on the
    // invalid manual baseline, so explicitly force the next attempt after fixing that baseline.
    executor.resume(&worker_id, true).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 2);
    let loaded_index = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    return snapshot_index;
                }
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                    panic!("The healthy periodic snapshot must be retried: {error}")
                }
                _ => {}
            }
        }
        panic!("Missing snapshot recovery success");
    })
    .await?;
    assert_eq!(loaded_index, periodic_index);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Error(params)
                    if params.kind == OplogErrorKind::Recovery
            ))
            .count(),
        1,
        "Successful recovery must retain the failure history"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::RecoverySucceeded(_)))
            .count(),
        1,
        "Successful startup must resolve the durable recovery error"
    );
    Ok(())
}

async fn assert_promoted_automatic_snapshot_load_failure_retries_required_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    failure: AutomaticSnapshotLoadFailure,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let oplog_config = OplogConfig {
        max_payload_size: 0,
        default_snapshotting: SnapshotPolicy::EveryNInvocation { count: 1 },
        ..Default::default()
    };
    let executor =
        start_customized(deps, &context, None, None, None, None, Some(oplog_config)).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Complete construction on the original revision before admitting the automatic update.
    let initial_revision = executor
        .invoke_and_await_agent(&component, &agent_id, "replay_revision", data_value!())
        .await?;
    assert_eq!(initial_revision.into_typed::<u32>()?, 0);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let snapshot_index = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let last_finished = oplog
                .iter()
                .rev()
                .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
                .expect("The invocation completed")
                .oplog_index;
            if let Some(snapshot) = oplog.iter().find(|entry| {
                entry.oplog_index > last_finished
                    && matches!(entry.entry, PublicOplogEntry::Snapshot(_))
            }) {
                break anyhow::Ok(snapshot.oplog_index);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;

    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !executor.stop_worker_if_idle(&owned).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;

    let expected_error = match failure {
        AutomaticSnapshotLoadFailure::InvalidEntry => {
            // Context initialization reads the snapshot boundary once before recovery loads it.
            executor.return_no_op_after_oplog_reads(&worker_id, snapshot_index, 1);
            "Expected Snapshot entry"
        }
        AutomaticSnapshotLoadFailure::PayloadDownload => {
            executor.fail_snapshot_download_once(&worker_id, snapshot_index);
            "Failed to download snapshot payload"
        }
    };
    let mut events = executor.capture_output(&worker_id).await?;

    let replay_revision = executor
        .invoke_and_await_agent(&component, &agent_id, "replay_revision", data_value!())
        .await?;
    assert_snapshot_recovery_failed(&mut events, expected_error).await;
    let revision_two_only = executor
        .invoke_and_await_agent(&component, &agent_id, "revision_two_only", data_value!())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    // The start from the promoted record replays its tail with the source revision, as the
    // update attempt did, and moves to the target at the replayed update.
    assert_eq!(replay_revision.into_typed::<u32>()?, 0);
    assert_eq!(revision_two_only.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn automatic_snapshot_invalid_entry_retries_promoted_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_promoted_automatic_snapshot_load_failure_retries_required_baseline(
        last_unique_id,
        deps,
        agent_update_v1,
        AutomaticSnapshotLoadFailure::InvalidEntry,
    )
    .await
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn automatic_snapshot_download_failure_retries_promoted_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_promoted_automatic_snapshot_load_failure_retries_required_baseline(
        last_unique_id,
        deps,
        agent_update_v1,
        AutomaticSnapshotLoadFailure::PayloadDownload,
    )
    .await
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn automatic_update_does_not_fallback_after_selected_snapshot_is_rejected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let revision_two = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, revision_two.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, revision_two.revision, Duration::from_secs(30))
        .await?;

    let snapshots_before_invocation = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
    let loaded_manual_snapshot = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(loaded_manual_snapshot.into_typed::<u32>()?, 1);
    let snapshots_after_invocation = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let count = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await?
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
                .count();
            if count > snapshots_before_invocation {
                break Ok::<_, anyhow::Error>(count);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(snapshots_after_invocation, snapshots_before_invocation + 1);

    let revision_four = executor
        .update_component(&component.id, "it_agent_update_v4_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, revision_four.revision, false)
        .await?;
    let failed = wait_for_update_counts(&executor, &worker_id, (0, 1, 1)).await?;
    failed
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update)
                if update.target_revision == revision_four.revision =>
            {
                update.snapshot_assisted_details.as_ref()
            }
            _ => None,
        })
        .expect("selected snapshot rejection must record assisted failure provenance");

    let loaded_snapshot_revision = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    // Once Automatic selects the periodic snapshot, target rejection fails that update without
    // falling back to full replay or an older snapshot. The source remains healthy.
    assert_eq!(loaded_snapshot_revision.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, revision_two.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 1));
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_auto_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();

    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: we finish executing f1 which returns with 300. Then we try updating, but the
    // updated f1 would return 150 which we detect as a divergence and fail the update. After this
    // f2's original version is executed which returns random u64.
    assert_ne!(result.clone().into_typed::<u64>()?, 150);
    assert_ne!(result.into_typed::<u64>()?, 300);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 1));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle_with_non_diverging_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f4", data_value!())
        .await?;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the f3 function is not changing between the versions, so we can safely
    // update the component and call f4 which only exists in the new version.
    // the current state which is 0
    assert_eq!(result.into_typed::<u64>()?, 11);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_auto_update_on_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();

    let mut control = http_server.f1_control(100).await;
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "f1", data_value!(20u64))
                .await
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    control.resume();
    let mut control2 = http_server.f1_control(110).await;

    control2.await_reached().await;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);
    control2.resume();

    let result = fiber.await??;
    info!("result: {result:?}");

    executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?; // awaiting a result from f3 to make sure the metadata already contains the updates
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and tries to get updated, but it fails because f2 was previously executed, and it is
    // diverging from the new version. The update is marked as a failure and the invocation continues
    // with the original version, resulting in 300.
    assert_eq!(result.into_typed::<u64>()?, 300);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 1));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    let before_update = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let after_update = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    assert_eq!(before_update, after_update);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));

    Ok(())
}

/// A stop arriving while a manual update is in flight must not deadlock either side.
///
/// The update is enqueued from the invocation loop, under the worker lifecycle lock, while an
/// interrupt takes the same worker down. The order is forced: the loop is held inside its
/// enqueue, the interrupt is issued against it, then the enqueue is released. Either outcome is
/// legal, a hang is not - and the timeout is the assertion.
#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn a_stop_racing_a_manual_update_never_deadlocks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    // The loop is held inside its enqueue of the update, under the worker lifecycle lock, and
    // the interrupt is issued while it waits there.
    let mut gate = executor.gate_next_pending_update_enqueue(&worker_id).await;
    let update = {
        let executor = executor.clone();
        let worker_id = worker_id.clone();
        let revision = updated_component.revision;
        spawn(
            async move {
                executor
                    .manual_update_worker(&worker_id, revision, false)
                    .await
            }
            .in_current_span(),
        )
    };
    tokio::time::timeout(Duration::from_secs(60), gate.entered())
        .await
        .map_err(|_| anyhow::anyhow!("the loop never reached the update's enqueue"))?;
    let stop = {
        let executor = executor.clone();
        let worker_id = worker_id.clone();
        spawn(async move { executor.interrupt(&worker_id).await }.in_current_span())
    };
    // The interrupt has to take the worker down while the loop holds the lock; a short wait
    // without a hook for "the stop is waiting on the loop", then the enqueue is released.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(gate);

    let (update, stop) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(update, stop)
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("a stop racing a manual update deadlocked: neither call came back")
    })?;
    // Either may fail on its own terms - the worker is being stopped - but neither may hang, and
    // the executor has to stay usable afterwards.
    let _ = update?;
    let _ = stop?;

    // The worker is still answerable, and on a revision that is one of the two legal outcomes -
    // the method set differs between them, so the probe is the metadata rather than a call.
    let metadata = tokio::time::timeout(
        Duration::from_secs(60),
        executor.get_worker_metadata(&worker_id),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the agent never answered again after the race"))??;
    assert!(
        metadata.component_revision == updated_component.revision
            || metadata.component_revision == ComponentRevision::INITIAL,
        "the update either landed or did not, but the revision must be one of the two, got {:?}",
        metadata.component_revision
    );
    assert!(
        !matches!(metadata.status, AgentStatus::Failed),
        "a stop racing an update must not fail the agent, got {:?}",
        metadata.status
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_without_save_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Explanation: We are trying to update v1 to v3 using snapshots, but v1 does not
    // export a save function, so the update attempt fails and the worker continues running
    // the original version which we can invoke.
    // f3 returns args.len() + env_vars.len(); agents get an extra GOLEM_AGENT_ID env var
    assert_eq!(result.into_typed::<u64>()?, 6);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 1));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_running_followed_by_manual(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component_1 = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component_1.revision
    );

    let updated_component_2 = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component_2.revision
    );

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();

    let mut control = http_server.f1_control(100).await;

    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "f1", data_value!(20u64))
                .await
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component_1.revision, false)
        .await?;
    executor
        .manual_update_worker(&worker_id, updated_component_2.revision, false)
        .await?;
    control.resume();

    let mut control2 = http_server.f1_control(110).await;
    control2.await_reached().await;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);
    control2.resume();

    let result1 = fiber.await??;
    info!("result1: {result1:?}");

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    info!("result2: {result2:?}");

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and eventually finishes with 150. The update is marked as a success, but immediately
    // it gets updated again to v3 on which we can call the previously non-existent 'get'
    // function to get the same state that was generated by 'v2'.
    assert_eq!(result1.into_typed::<u64>()?, 150);
    assert_eq!(result2.into_typed::<u64>()?, 150);
    assert_eq!(metadata.component_revision, updated_component_2.revision);
    assert_eq!(update_counts(&metadata), (0, 2, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_with_failing_load(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v4_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f3", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Explanation: We try to update v2 to v4, but v4's load function always fails. So
    // the component must stay on v2, on which we can invoke f3.
    // f3 returns args.len() + env_vars.len(); agents get an extra GOLEM_AGENT_ID env var
    assert_eq!(result.into_typed::<u64>()?, 6);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 1));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_using_v11(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    let before_update = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let after_update = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    assert_eq!(before_update, after_update);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_using_golem_rust_sdk(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await_agent(&component, &agent_id, "f1", data_value!(0u64))
        .await?;

    let before_update = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let after_update = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    assert_eq!(before_update, after_update);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle_to_non_existing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    // Now we try to update to version target_version + 1, which does not exist.
    executor
        .auto_update_worker(&worker_id, updated_component.revision.next()?, false)
        .await?;

    // We expect this update to fail, and the component to remain on `target_version` and remain
    // responsible to further invocations:

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result1.into_typed::<u64>()?, 0);
    assert_eq!(result2.into_typed::<u64>()?, 0);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 1));
    let failed = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => update.details.clone(),
            _ => None,
        })
        .unwrap_or_default();
    assert!(failed.starts_with("UPDATE_TARGET_NOT_FOUND: "), "{failed}");

    Ok(())
}

/// A component service that cannot give one component revision while it is down.
pub(crate) struct FlakyComponentService {
    pub(crate) inner: Arc<dyn ComponentService>,
    pub(crate) outage: Arc<Outage>,
}

/// The revision that a [`FlakyComponentService`] cannot give, whether it is down, and how many
/// fetches it refused. A revision whose metadata is lost gives its component, and after that no
/// metadata, as if the revision were deleted between the two fetches.
#[derive(Default)]
pub(crate) struct Outage {
    revision: std::sync::Mutex<Option<ComponentRevision>>,
    down: std::sync::atomic::AtomicBool,
    refused: std::sync::atomic::AtomicUsize,
    metadata_lost: std::sync::Mutex<Option<ComponentRevision>>,
    fetched: std::sync::atomic::AtomicBool,
}

impl Outage {
    pub(crate) fn begin(&self, revision: ComponentRevision) {
        *self.revision.lock().unwrap() = Some(revision);
        self.down.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn end(&self) {
        self.down.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn refused(&self) -> usize {
        self.refused.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn lose_metadata_after_get(&self, revision: ComponentRevision) {
        *self.metadata_lost.lock().unwrap() = Some(revision);
    }

    fn got(&self, revision: ComponentRevision) {
        if *self.metadata_lost.lock().unwrap() == Some(revision) {
            self.fetched
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Fails a fetch of the metadata of the revision after its component was fetched.
    fn check_metadata(
        &self,
        component_id: golem_common::model::component::ComponentId,
        revision: Option<ComponentRevision>,
    ) -> Result<(), WorkerExecutorError> {
        let lost = revision.is_some()
            && *self.metadata_lost.lock().unwrap() == revision
            && self.fetched.load(std::sync::atomic::Ordering::SeqCst);
        if lost {
            self.refused
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(WorkerExecutorError::ComponentNotFound { component_id })
        } else {
            self.check(component_id, revision)
        }
    }

    /// Fails a fetch of the revision while the service is down.
    fn check(
        &self,
        component_id: golem_common::model::component::ComponentId,
        revision: Option<ComponentRevision>,
    ) -> Result<(), WorkerExecutorError> {
        let affected = revision.is_some() && *self.revision.lock().unwrap() == revision;
        if affected && self.down.load(std::sync::atomic::Ordering::SeqCst) {
            self.refused
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(WorkerExecutorError::ComponentServiceUnavailable {
                component_id,
                component_revision: revision,
                reason: "the component service is down".to_string(),
            })
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl ComponentService for FlakyComponentService {
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        component_id: golem_common::model::component::ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        WorkerExecutorError,
    > {
        self.outage.check(component_id, Some(component_revision))?;
        let result = self
            .inner
            .get(engine, component_id, component_revision)
            .await?;
        self.outage.got(component_revision);
        Ok(result)
    }

    async fn get_metadata(
        &self,
        component_id: golem_common::model::component::ComponentId,
        forced_revision: Option<ComponentRevision>,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        self.outage.check_metadata(component_id, forced_revision)?;
        self.inner.get_metadata(component_id, forced_revision).await
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: golem_common::model::environment::EnvironmentId,
        resolving_application: golem_common::model::application::ApplicationId,
        resolving_account: golem_common::model::account::AccountId,
    ) -> Result<Option<golem_common::model::component::ComponentId>, WorkerExecutorError> {
        self.inner
            .resolve_component(
                component_reference,
                resolving_environment,
                resolving_application,
                resolving_account,
            )
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        self.inner.all_cached_metadata().await
    }

    async fn invalidate_current_deployed_metadata(&self) {
        self.inner.invalidate_current_deployed_metadata().await;
    }

    async fn invalidate_current_deployed_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_current_deployed_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_all_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all(&self) {
        self.inner.invalidate_all().await;
    }
}

/// While the component service cannot give the target of an update, the start of the agent fails
/// as a recovery that is retried, and no failed update is written; a start after the outage
/// applies the update.
#[test]
#[timeout("120s")]
async fn an_unavailable_component_service_delays_the_update_and_fails_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let outage = Arc::new(Outage::default());
    let wrapper_outage = outage.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(FlakyComponentService {
                    inner,
                    outage: wrapper_outage.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    outage.begin(updated_component.revision);
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let polled = (&executor, &worker_id, &outage);
    let during = tokio::time::timeout(
        Duration::from_secs(60),
        futures::StreamExt::into_future(futures::StreamExt::boxed(
            futures::TryStreamExt::try_skip_while(
                futures::StreamExt::then(
                    futures::stream::repeat(polled),
                    |(executor, worker_id, _)| async move {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        executor.get_worker_metadata(worker_id).await
                    },
                ),
                |metadata| {
                    futures::future::ready(Ok(!(polled.2.refused() > 0
                        && metadata.last_error.as_deref().is_some_and(|error| {
                            error.contains("component service is unavailable")
                        }))))
                },
            ),
        )),
    )
    .await?
    .0
    .expect("the metadata polls do not end")?;
    outage.end();
    executor.resume(&worker_id, true).await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    assert_eq!(during.component_revision, component.revision);
    assert_eq!(update_counts(&during), (1, 0, 0));
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

/// A target whose metadata is gone when the replay reaches the update point fails the update
/// there: one failed update with its code is written, and the agent runs on its source revision
/// with the state that the source built.
#[test]
#[timeout("120s")]
async fn a_failure_at_the_update_point_fails_the_update_once_and_keeps_the_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let outage = Arc::new(Outage::default());
    let wrapper_outage = outage.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(FlakyComponentService {
                    inner,
                    outage: wrapper_outage.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    for _ in 0..2 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
            .await?;
    }

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    outage.lose_metadata_after_get(updated_component.revision);
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    let details = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => update.details.clone(),
            _ => None,
        })
        .unwrap_or_default();

    assert!(details.starts_with("UPDATE_TARGET_NOT_FOUND"), "{details}");
    assert!(outage.refused() > 0);
    assert_eq!(metadata.component_revision, component.revision);

    let accumulated = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?;
    assert_eq!(accumulated.into_typed::<u32>()?, 20);
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    let accumulated = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    assert_eq!(accumulated.into_typed::<u32>()?, 30);
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(update_counts(&metadata), (0, 0, 1));
    Ok(())
}

/// Check that GOLEM_COMPONENT_REVISION environment variable is updated as part of a worker update
#[test]
#[tracing::instrument]
async fn update_component_revision_environment_variable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("RevisionEnvAgent");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_revision_from_env_var",
                data_value!(),
            )
            .await?;

        assert_eq!(result.into_typed::<String>()?, "0");
    }

    let updated_component_1 = executor
        .update_component(&component.id, "it_agent_update_v1_release")
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component_1.revision, false)
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_revision_from_env_var",
                data_value!(),
            )
            .await?;

        assert_eq!(result.into_typed::<String>()?, "0");

        // FIXME: broken as get-environment during the replay is getting cached
        // assert_eq!(result, data_value!("1"));
    }

    // agent created on the new version sees correct component version
    {
        let agent_id_2 = phantom_agent_id!("RevisionEnvAgent", uuid::Uuid::new_v4());
        let _worker2 = executor
            .start_agent(&component.id, agent_id_2.clone())
            .await?;

        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id_2,
                "get_revision_from_env_var",
                data_value!(),
            )
            .await?;

        assert_eq!(result.into_typed::<String>()?, "1");
    }

    let updated_component_2 = executor
        .update_component(&component.id, "it_agent_update_v1_release")
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component_2.revision, false)
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_revision_from_env_var",
                data_value!(),
            )
            .await?;

        assert_eq!(result.into_typed::<String>()?, "2");
    }

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_with_disable_wakeup_keeps_worker_interrupted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    // Invoke f1 with a blocking control point so the worker stays Running
    let mut control = http_server.f1_control(100).await;
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "f1", data_value!(50u64))
                .await
        }
        .in_current_span(),
    );

    // Wait until the worker reaches the blocking point (Running state)
    control.await_reached().await;

    // Interrupt and resume concurrently: interrupt() blocks until the worker is
    // actually interrupted, but the worker is waiting for the HTTP response,
    // so we must resume the HTTP server in parallel to avoid a deadlock.
    let executor_clone2 = executor.clone();
    let worker_id_clone2 = worker_id.clone();
    let interrupt_fiber = spawn(async move { executor_clone2.interrupt(&worker_id_clone2).await });
    control.resume();
    interrupt_fiber.await??;

    // The invoke should fail due to interruption
    let _ = fiber.await?;

    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // Upload an updated component
    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    // Request auto-update with disable_wakeup=true
    executor
        .auto_update_worker(&worker_id, updated_component.revision, true)
        .await?;

    // Give some time for any unintended wake-up to happen
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify the worker is still Interrupted (not woken up)
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // The worker should still be interrupted since disable_wakeup was true
    assert_eq!(metadata.status, AgentStatus::Interrupted);
    // The update should be pending, not yet applied
    assert_eq!(update_counts(&metadata), (1, 0, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_can_be_invoked_after_manual_snapshot_update_and_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v2")] agent_update_v2: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v2)
        .store()
        .await?;

    let agent_id = agent_id!("UpdateTest");

    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v3_release")
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    // restart and force the agent to reload the last snapshot
    drop(executor);
    let executor = start(deps, &context).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result.into_typed::<u64>()?, 0);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::RecoverySucceeded(_))),
        "routine recovery must not append a recovery-success marker"
    );

    Ok(())
}

/// How the manual-update snapshot is made unreadable on restart.
enum ManualSnapshotLoadFailure {
    /// The oplog entry at the snapshot index is not the update that wrote it.
    InvalidEntry,
    /// The snapshot payload's download fails. `ExternalSnapshotUpdateTest` writes a snapshot
    /// larger than the default `max_payload_size`, so it is stored outside the oplog.
    PayloadDownload,
}

/// A manual-update snapshot that cannot be read on restart. The oplog before
/// that update was recorded against a build the current one is incompatible
/// with, so there is no full replay to fall back to: the start attempt has to
/// fail and leave the baseline in place for the next one.
async fn assert_manual_snapshot_load_failure_fails_the_start_and_keeps_the_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_update_v1: &PrecompiledComponent,
    failure: ManualSnapshotLoadFailure,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("ExternalSnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let initial = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(initial.into_typed::<u32>()?, 0);

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    // Committed before the restart, so the update is complete and not re-run on recovery.
    let after_update = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(after_update.into_typed::<u32>()?, 1);
    let snapshot_index = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .find_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::PendingUpdate(_)).then_some(entry.oplog_index)
        })
        .expect("Expected the manual update's pending-update entry");

    drop(executor);
    let executor = start(deps, &context).await?;

    let expected_error = match failure {
        ManualSnapshotLoadFailure::InvalidEntry => {
            // The start reads the update entry for its filesystem baseline, and context
            // initialization reads the snapshot boundary, before recovery loads it.
            executor.return_no_op_after_oplog_reads(&worker_id, snapshot_index, 2);
            "Expected Snapshot entry"
        }
        ManualSnapshotLoadFailure::PayloadDownload => {
            executor.fail_next_oplog_download(&worker_id);
            "Failed to download snapshot payload"
        }
    };
    let mut events = executor.capture_output(&worker_id).await?;

    let failed_start = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await;
    assert!(
        failed_start.is_err(),
        "the start whose snapshot could not be read must not succeed: {failed_start:?}"
    );
    assert_snapshot_recovery_failed(&mut events, expected_error).await;

    let failed_metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(failed_metadata.status, AgentStatus::Failed);
    assert_eq!(
        failed_metadata.last_error_kind,
        Some(OplogErrorKind::Recovery)
    );
    assert!(
        failed_metadata
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains(expected_error)),
        "recovery failure metadata must retain the actionable cause: {failed_metadata:?}"
    );

    let (_, listed) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    let listed = listed
        .iter()
        .find(|metadata| metadata.agent_id == failed_metadata.agent_id)
        .expect("failed agent must be present in list metadata");
    assert_eq!(listed.status, AgentStatus::Failed);
    assert_eq!(listed.last_error_kind, failed_metadata.last_error_kind);
    assert_eq!(listed.last_error, failed_metadata.last_error);

    let repeated_failure = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await
        .expect_err("an unresolved recovery failure must reject invocation");
    let repeated_failure = repeated_failure.to_string();
    assert!(repeated_failure.contains("Failed to resume"));
    assert!(repeated_failure.contains(expected_error));

    // A failed start stays on the worker until it is resumed or unloaded, like any other
    // instance-creation failure; the resume is the next start attempt.
    executor.resume(&worker_id, true).await?;
    // Loading can still expose the cached idle status before recovery completes.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            if oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::RecoverySucceeded(_)))
            {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let recovered_metadata = executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    assert_eq!(recovered_metadata.status, AgentStatus::Idle);
    assert_eq!(recovered_metadata.last_error_kind, None);
    assert_eq!(recovered_metadata.last_error, None);

    let after_retry = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(after_retry.into_typed::<u32>()?, 1);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    assert_eq!(metadata.last_error_kind, None);
    assert_eq!(metadata.last_error, None);
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::RecoverySucceeded(_)))
            .count(),
        1,
        "successful recovery must append exactly one clearing marker"
    );

    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn manual_snapshot_invalid_entry_fails_the_start_and_keeps_the_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_manual_snapshot_load_failure_fails_the_start_and_keeps_the_baseline(
        last_unique_id,
        deps,
        agent_update_v1,
        ManualSnapshotLoadFailure::InvalidEntry,
    )
    .await
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn manual_snapshot_download_failure_fails_the_start_and_keeps_the_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_manual_snapshot_load_failure_fails_the_start_and_keeps_the_baseline(
        last_unique_id,
        deps,
        agent_update_v1,
        ManualSnapshotLoadFailure::PayloadDownload,
    )
    .await
}

/// A second manual update on the same agent. The first one leaves a snapshot
/// baseline behind, and everything after it still has to be replayed unless the
/// pending update's own override survives.
///
/// `SnapshotUpdateTest` stamps the build that wrote the snapshot into it, so
/// each update's own snapshot round-trip is visible: `1` after the update out of
/// v1, `2` after the one out of v2.
#[test]
#[tracing::instrument]
async fn manual_update_on_idle_twice(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // A suffix between the two snapshots for the second update to account for.
    let initial = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;

    // Both revisions carry the same build, so nothing depends on a behaviour change.
    let first = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, first.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, first.revision, Duration::from_secs(30))
        .await?;

    let after_first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;

    let second = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, second.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, second.revision, Duration::from_secs(30))
        .await?;

    let after_second = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert_ne!(second.revision, first.revision);
    assert_eq!(initial.into_typed::<u32>()?, 0);
    assert_eq!(after_first.into_typed::<u32>()?, 1);
    assert_eq!(after_second.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, second.revision);
    assert_eq!(update_counts(&metadata), (0, 2, 0));

    Ok(())
}

/// A manual update to a revision carrying an earlier build. Automatic update
/// cannot do this once any recorded invocation diverges, which is what makes
/// the snapshot path the one a rollback has to use.
#[test]
#[tracing::instrument]
async fn manual_update_on_idle_to_earlier_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;

    let forward = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, forward.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, forward.revision, Duration::from_secs(30))
        .await?;

    let after_forward = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;

    // Re-uploading the original build as a new revision is what a rollback is.
    let back = executor
        .update_component(&component.id, "it_agent_update_v1_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, back.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, back.revision, Duration::from_secs(30))
        .await?;

    let after_rollback = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    // The snapshot carries the agent's state across both builds.
    assert_eq!(after_forward.into_typed::<u32>()?, 1);
    assert_eq!(after_rollback.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, back.revision);
    assert_eq!(update_counts(&metadata), (0, 2, 0));

    Ok(())
}

/// An automatic update on an agent that has already had a manual one.
///
/// The manual update's snapshot is the authoritative replay baseline, so a later
/// automatic update must replay only the suffix after it. The prefix cannot
/// replay: it recorded a `component_version` of 1 under a build that now answers
/// 2, so divergence detection would fail the update.
///
/// This does not guard the `set_override` fix, which it survives. It goes red
/// when the status reducer stops folding the snapshot region into the skipped
/// regions.
#[test]
#[tracing::instrument]
async fn auto_update_on_idle_after_manual_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let counter_id = agent_id!("SnapshotCounter", "auto-after-manual");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;
    // Recorded under the original build, and unreplayable under any other one.
    let version_before = executor
        .invoke_and_await_agent(&component, &counter_id, "component_version", data_value!())
        .await?;

    let migrated = executor
        .update_component(&component.id, "it_agent_counters_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, migrated.revision, false)
        .await?;
    executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    // Same build again, so nothing after the snapshot can diverge.
    let later = executor
        .update_component(&component.id, "it_agent_counters_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, later.revision, false)
        .await?;
    let count = executor
        .invoke_and_await_agent(&component, &counter_id, "get", data_value!())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert_eq!(version_before.into_typed::<u32>()?, 1);
    assert_eq!(count.into_typed::<u32>()?, 2);
    assert_eq!(metadata.component_revision, later.revision);
    assert_eq!(update_counts(&metadata), (0, 2, 0));

    Ok(())
}

/// Stores agent-updates v1 and starts its `SnapshotUpdateTest` agent with the port of
/// `http_server`. The agent writes `/before.txt` with its path as its content before its first
/// snapshot boundary, so an executor without filesystem snapshots writes no snapshot record for
/// it.
async fn agent_with_a_written_file(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    agent_update_v1: &PrecompiledComponent,
    http_server: &TestHttpServer,
) -> anyhow::Result<(
    ComponentDto,
    golem_common::model::agent::ParsedAgentId,
    AgentId,
)> {
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), http_server.port().to_string())]),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/before.txt".to_string(), "/before.txt".to_string()),
        )
        .await?;
    Ok((component, agent_id, worker_id))
}

/// The content of the file `/before.txt` of the agent.
async fn written_file(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
) -> anyhow::Result<Option<String>> {
    executor
        .invoke_and_await_agent(
            component,
            agent_id,
            "read_file",
            data_value!("/before.txt".to_string()),
        )
        .await?
        .into_typed::<Option<String>>()
}

/// The descriptions of the `PendingUpdate` entries of the agent, and whether its oplog holds a
/// snapshot record.
async fn update_entries(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<(Vec<PublicUpdateDescription>, bool)> {
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    Ok((
        oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::PendingUpdate(params) => Some(params.description.clone()),
                _ => None,
            })
            .collect(),
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
    ))
}

/// Whether each description is a plain automatic update.
fn all_automatic(descriptions: &[PublicUpdateDescription]) -> bool {
    descriptions
        .iter()
        .all(|description| matches!(description, PublicUpdateDescription::Automatic(_)))
}

#[test]
#[timeout("120s")]
async fn an_automatic_update_after_a_file_write_without_filesystem_snapshots_replays_the_full_history_and_keeps_the_file(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor =
        crate::fork::start_with_local_resume_and_snapshot_policy(deps, &context, policy.clone())
            .await?;
    let http_server = TestHttpServer::start().await;
    let (component, agent_id, worker_id) =
        agent_with_a_written_file(&executor, &context, agent_update_v1, &http_server).await?;
    futures::stream::iter(0..2)
        .then(|_| {
            executor.invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        })
        .try_collect::<Vec<_>>()
        .await?;
    let updated = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    executor
        .auto_update_worker(&worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, updated.revision, Duration::from_secs(30))
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    let (descriptions, has_snapshot_record) = update_entries(&executor, &worker_id).await?;
    let file_after_update = written_file(&executor, &component, &agent_id).await?;
    let accumulated_after_update = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?
        .into_typed::<u32>()?;
    drop(executor);
    let restarted =
        crate::fork::start_with_local_resume_and_snapshot_policy(deps, &context, policy).await?;
    let file_after_restart = written_file(&restarted, &component, &agent_id).await?;

    assert!(!has_snapshot_record);
    assert_eq!(descriptions.len(), 2, "{descriptions:?}");
    assert!(all_automatic(&descriptions), "{descriptions:?}");
    assert!(metadata.updates.iter().all(|record| match record {
        UpdateRecord::SuccessfulUpdate(update) => update.snapshot_assisted_details.is_none(),
        _ => true,
    }));
    assert_eq!(accumulated_after_update, 20);
    assert_eq!(file_after_update, Some("/before.txt".to_string()));
    assert_eq!(file_after_restart, Some("/before.txt".to_string()));
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn a_host_call_divergence_of_an_automatic_update_after_a_file_write_without_filesystem_snapshots_fails_the_full_replay_and_keeps_the_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let http_server = TestHttpServer::start().await;
    let (component, agent_id, worker_id) =
        agent_with_a_written_file(&executor, &context, agent_update_v1, &http_server).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "suffix_host_value", data_value!())
        .await?;
    let updated = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    executor
        .auto_update_worker(&worker_id, updated.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    let failed = metadata
        .updates
        .iter()
        .find_map(|record| match record {
            UpdateRecord::FailedUpdate(update) => Some(update),
            _ => None,
        })
        .expect("the update fails");
    let (descriptions, has_snapshot_record) = update_entries(&executor, &worker_id).await?;
    let healthy = executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?
        .into_typed::<u32>()?;
    let file = written_file(&executor, &component, &agent_id).await?;

    assert!(!has_snapshot_record);
    assert!(all_automatic(&descriptions), "{descriptions:?}");
    assert!(
        failed
            .details
            .as_deref()
            .is_some_and(|details| details.starts_with("UPDATE_REPLAY_FAILED")),
        "{:?}",
        failed.details
    );
    assert!(failed.snapshot_assisted_details.is_none());
    assert_eq!(metadata.component_revision, component.revision);
    assert_eq!(healthy, 7);
    assert_eq!(file, Some("/before.txt".to_string()));
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn an_automatic_update_after_a_file_write_without_filesystem_snapshots_survives_a_restart_before_its_attempt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let mut http_server = TestHttpServer::start().await;
    let (component, agent_id, worker_id) =
        agent_with_a_written_file(&executor, &context, agent_update_v1, &http_server).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    let mut control = http_server.f1_control(902).await;
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = spawn(async move {
        executor_clone
            .invoke_and_await_agent(
                &component_clone,
                &agent_id_clone,
                "blocking_stable",
                data_value!(902u64),
            )
            .await
    });
    control.await_reached().await;
    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let interrupt = spawn(async move { executor_clone.interrupt(&worker_id_clone).await });
    control.resume();
    interrupt.await??;
    let _ = invocation.await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let updated = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, updated.revision, true)
        .await?;
    let pending = wait_for_update_counts(&executor, &worker_id, (1, 0, 0)).await?;
    drop(executor);

    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    executor.resume(&worker_id, true).await?;
    executor
        .wait_for_component_revision(&worker_id, updated.revision, Duration::from_secs(30))
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    let (descriptions, has_snapshot_record) = update_entries(&executor, &worker_id).await?;
    let file = written_file(&executor, &component, &agent_id).await?;

    assert_eq!(pending.component_revision, component.revision);
    assert!(!has_snapshot_record);
    assert!(all_automatic(&descriptions), "{descriptions:?}");
    assert_eq!(metadata.component_revision, updated.revision);
    assert_eq!(file, Some("/before.txt".to_string()));
    http_server.abort();
    Ok(())
}

#[test]
#[timeout("120s")]
async fn a_revert_of_an_automatic_update_after_a_file_write_without_filesystem_snapshots_retries_the_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    futures::stream::iter([true, false])
        .then(|succeeds| async move {
            let context = TestContext::new(last_unique_id);
            let executor = start_with_snapshot_policy(
                deps,
                &context,
                SnapshotPolicy::EveryNInvocation { count: 3 },
            )
            .await?;
            let http_server = TestHttpServer::start().await;
            let (component, agent_id, worker_id) =
                agent_with_a_written_file(&executor, &context, agent_update_v1, &http_server)
                    .await?;
            futures::stream::iter(0..2)
                .then(|_| {
                    executor.invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "stable_value",
                        data_value!(),
                    )
                })
                .try_collect::<Vec<_>>()
                .await?;
            if !succeeds {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "pre_snapshot_value",
                        data_value!(),
                    )
                    .await?;
            }
            let target = executor
                .update_component(&component.id, "it_agent_update_v2_release")
                .await?;
            executor
                .auto_update_worker(&worker_id, target.revision, false)
                .await?;
            let expected = if succeeds { (0, 1, 0) } else { (0, 0, 1) };
            wait_for_update_counts(&executor, &worker_id, expected).await?;
            let strategy_index = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await?
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)))
                .map(|entry| entry.oplog_index)
                .next_back()
                .ok_or_else(|| anyhow::anyhow!("the update has no strategy entry"))?;
            executor
                .revert(
                    &worker_id,
                    RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                        last_oplog_index: strategy_index,
                    }),
                )
                .await?;
            executor.resume(&worker_id, true).await?;
            let retried = wait_for_update_counts(&executor, &worker_id, expected).await?;
            let (descriptions, has_snapshot_record) = update_entries(&executor, &worker_id).await?;
            let file = written_file(&executor, &component, &agent_id).await?;

            assert!(!has_snapshot_record);
            assert_eq!(descriptions.len(), 2, "{descriptions:?}");
            assert!(all_automatic(&descriptions), "{descriptions:?}");
            assert_eq!(
                retried.component_revision,
                if succeeds {
                    target.revision
                } else {
                    component.revision
                }
            );
            assert!(retried.updates.iter().all(|record| match record {
                UpdateRecord::SuccessfulUpdate(update) =>
                    update.snapshot_assisted_details.is_none(),
                UpdateRecord::FailedUpdate(update) => update.snapshot_assisted_details.is_none(),
                UpdateRecord::PendingUpdate(_) => true,
            }));
            assert_eq!(file, Some("/before.txt".to_string()));
            http_server.abort();
            anyhow::Ok(())
        })
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}

/// The details of each failed update of the agent, and the description of each `PendingUpdate`
/// entry, in oplog order.
async fn failures_and_strategies(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<(Vec<String>, Vec<PublicUpdateDescription>)> {
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    Ok((
        oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::FailedUpdate(params) => {
                    Some(params.details.clone().unwrap_or_default())
                }
                _ => None,
            })
            .collect(),
        oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::PendingUpdate(params) => Some(params.description.clone()),
                _ => None,
            })
            .collect(),
    ))
}

#[test]
#[timeout("120s")]
async fn a_snapshot_the_target_cannot_load_makes_the_next_request_of_the_same_update_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    let snapshot_index =
        wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    // The target cannot load an empty snapshot; a full replay does not read it.
    executor
        .return_empty_snapshot_payload(&worker_id, snapshot_index)
        .await?;
    let target = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    executor
        .auto_update_worker(&worker_id, target.revision, false)
        .await?;
    wait_for_update_counts(&executor, &worker_id, (0, 0, 1)).await?;
    executor
        .auto_update_worker(&worker_id, target.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 1)).await?;
    let (failures, strategies) = failures_and_strategies(&executor, &worker_id).await?;
    let accumulated = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?
        .into_typed::<u32>()?;

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("UPDATE_SNAPSHOT_INCOMPATIBLE"),
        "{failures:?}"
    );
    assert_eq!(strategies.len(), 4, "{strategies:?}");
    assert!(matches!(
        strategies[1],
        PublicUpdateDescription::SnapshotAssistedAutomatic(_)
    ));
    assert!(matches!(
        strategies[3],
        PublicUpdateDescription::Automatic(_)
    ));
    assert!(metadata.updates.iter().all(|record| match record {
        UpdateRecord::SuccessfulUpdate(update) => update.snapshot_assisted_details.is_none(),
        _ => true,
    }));
    assert_eq!(metadata.component_revision, target.revision);
    assert_eq!(accumulated, 10);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn an_automatic_update_from_a_promoted_snapshot_that_the_target_cannot_load_fails_at_once_and_keeps_the_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stable_value", data_value!())
        .await?;
    wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    let assisted = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .auto_update_worker(&worker_id, assisted.revision, false)
        .await?;
    wait_for_update_counts(&executor, &worker_id, (0, 1, 0)).await?;
    let rejecting = executor
        .update_component(&component.id, "it_agent_update_v4_release")
        .await?;

    executor
        .auto_update_worker(&worker_id, rejecting.revision, false)
        .await?;
    let metadata = wait_for_update_counts(&executor, &worker_id, (0, 1, 1)).await?;
    let (failures, strategies) = failures_and_strategies(&executor, &worker_id).await?;
    let accumulated = executor
        .invoke_and_await_agent(&component, &agent_id, "accumulated_value", data_value!())
        .await?
        .into_typed::<u32>()?;

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("UPDATE_REPLAY_FAILED"),
        "{failures:?}"
    );
    assert_eq!(strategies.len(), 4, "{strategies:?}");
    assert!(matches!(
        strategies[1],
        PublicUpdateDescription::SnapshotAssistedAutomatic(_)
    ));
    assert!(matches!(
        strategies[3],
        PublicUpdateDescription::Automatic(_)
    ));
    assert_eq!(metadata.component_revision, assisted.revision);
    assert_eq!(accumulated, 10);
    Ok(())
}
