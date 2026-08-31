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
use golem_common::model::AgentStatus;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::dsl::{TestDsl, update_counts};

use golem_worker_executor::services::golem_config::{OplogConfig, SnapshotPolicy};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
    start_customized, start_with_snapshot_policy,
};
use http::StatusCode;
use log::info;
use pretty_assertions::{assert_eq, assert_ne};
use std::collections::HashMap;
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
async fn auto_update_invalidates_snapshot_from_previous_revision(
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

    assert_eq!(loaded_snapshot_revision.into_typed::<u32>()?, 0);
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

    let snapshot_count = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
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

#[derive(Clone, Copy)]
enum AutomaticSnapshotLoadFailure {
    InvalidEntry,
    PayloadDownload,
}

async fn assert_automatic_snapshot_load_failure_recreates_replay_context(
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
    let executor = start_customized(
        deps,
        &context,
        None,
        None,
        None,
        None,
        None,
        Some(oplog_config.clone()),
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

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let snapshot_index = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .rev()
        .find_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Snapshot(_)).then_some(entry.oplog_index)
        })
        .expect("Expected an automatic snapshot after the post-update invocation");

    drop(executor);
    let executor = start_customized(
        deps,
        &context,
        None,
        None,
        None,
        None,
        None,
        Some(oplog_config),
    )
    .await?;

    let expected_error = match failure {
        AutomaticSnapshotLoadFailure::InvalidEntry => {
            // Context initialization reads the snapshot boundary once before recovery loads it.
            executor.return_no_op_after_oplog_reads(&worker_id, snapshot_index, 1);
            "Expected Snapshot entry"
        }
        AutomaticSnapshotLoadFailure::PayloadDownload => {
            executor.fail_next_oplog_download(&worker_id);
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
async fn automatic_snapshot_invalid_entry_fallback_recreates_replay_context(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_automatic_snapshot_load_failure_recreates_replay_context(
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
async fn automatic_snapshot_download_failure_recreates_replay_context(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_automatic_snapshot_load_failure_recreates_replay_context(
        last_unique_id,
        deps,
        agent_update_v1,
        AutomaticSnapshotLoadFailure::PayloadDownload,
    )
    .await
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

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result.into_typed::<u64>()?, 0);
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));

    Ok(())
}
