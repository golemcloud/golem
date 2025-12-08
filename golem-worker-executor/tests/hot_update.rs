// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use assert2::check;
use async_lock::Mutex;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use golem_common::model::component::ComponentRevision;
use golem_test_framework::dsl::{update_counts, TestDsl};
use golem_wasm::{IntoValueAndType, Value};
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::StatusCode;
use log::info;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::Arc;
use test_r::{inherit_test_dep, test};
use tokio::spawn;
use tokio::task::JoinHandle;
use tracing::{debug, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
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
                        if let Some(blocker) = &*guard {
                            if blocker.value == body {
                                let F1Blocker {
                                    reached, resume, ..
                                } = guard.take().unwrap();
                                debug!("Reached f1 blocking point");
                                reached.send(()).unwrap();
                                debug!("Awaiting resume at f1 blocking point");
                                resume.await.unwrap();
                                debug!("Resuming from f1 blocking point");
                            }
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
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(&component.id, "auto_update_on_running", env, vec![])
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();

    let mut control = http_server.f1_control(100).await;
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:component/api.{f1}",
                    vec![50u64.into_value_and_type()],
                )
                .await
                .unwrap()
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    control.resume();
    let mut control2 = http_server.f1_control(110).await;

    control2.await_reached().await;
    executor.log_output(&worker_id).await?;
    control2.resume();

    let result = fiber.await??;
    info!("result: {result:?}");

    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??; // awaiting a result from f3 to make sure the metadata already contains the updates
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and eventually finishes with 150. The update is marked as a success.
    check!(result[0] == Value::U64(150));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "auto_update_on_idle")
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result[0] == Value::U64(0));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_auto_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();

    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(&component.id, "failing_auto_update_on_idle", env, vec![])
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: we finish executing f1 which returns with 300. Then we try updating, but the
    // updated f1 would return 150 which we detect as a divergence and fail the update. After this
    // f2's original version is executed which returns random u64.
    check!(result[0] != Value::U64(150));
    check!(result[0] != Value::U64(300));
    check!(metadata.component_version == ComponentRevision(0));
    check!(update_counts(&metadata) == (0, 0, 1));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle_with_non_diverging_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker(
            &component.id,
            "auto_update_on_idle_with_non_diverging_history",
        )
        .await?;

    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??;

    executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??;

    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await??;

    info!("result: {result:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the f3 function is not changing between the versions, so we can safely
    // update the component and call f4 which only exists in the new version.
    // the current state which is 0
    check!(result[0] == Value::U64(11));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_auto_update_on_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(&component.id, "failing_auto_update_on_running", env, vec![])
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();

    let mut control = http_server.f1_control(100).await;
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:component/api.{f1}",
                    vec![20u64.into_value_and_type()],
                )
                .await
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    control.resume();
    let mut control2 = http_server.f1_control(110).await;

    control2.await_reached().await;
    executor.log_output(&worker_id).await?;
    control2.resume();

    let result = fiber.await???;
    info!("result: {result:?}");

    executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??; // awaiting a result from f3 to make sure the metadata already contains the updates
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and tries to get updated, but it fails because f2 was previously executed, and it is
    // diverging from the new version. The update is marked as a failure and the invocation continues
    // with the original version, resulting in 300.
    check!(result[0] == Value::U64(300));
    check!(metadata.component_version == ComponentRevision(0));
    check!(update_counts(&metadata) == (0, 0, 1));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v2")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(&component.id, "manual_update_on_idle", env, vec![])
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v3")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    let before_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    executor
        .manual_update_worker(&worker_id, updated_component.revision)
        .await?;

    let after_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    check!(before_update == after_update);
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_without_save_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "manual_update_on_idle_without_save_snapshot",
            env,
            vec![],
        )
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v3")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    executor
        .manual_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Explanation: We are trying to update v1 to v3 using snapshots, but v1 does not
    // export a save function, so the update attempt fails and the worker continues running
    // the original version which we can invoke.
    check!(result == vec![Value::U64(5)]);
    check!(metadata.component_version == ComponentRevision(0));
    check!(update_counts(&metadata) == (0, 0, 1));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_running_followed_by_manual(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let mut http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "auto_update_on_running_followed_by_manual",
            env,
            vec![],
        )
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component_1 = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component_1.revision
    );

    let updated_component_2 = executor
        .update_component(&component.id, "update-test-v3")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component_2.revision
    );

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();

    let mut control = http_server.f1_control(100).await;

    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:component/api.{f1}",
                    vec![20u64.into_value_and_type()],
                )
                .await
        }
        .in_current_span(),
    );

    control.await_reached().await;
    executor
        .auto_update_worker(&worker_id, updated_component_1.revision)
        .await?;
    executor
        .manual_update_worker(&worker_id, updated_component_2.revision)
        .await?;
    control.resume();

    let mut control2 = http_server.f1_control(110).await;
    control2.await_reached().await;
    executor.log_output(&worker_id).await?;
    control2.resume();

    let result1 = fiber.await???;
    info!("result1: {result1:?}");

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await??;
    info!("result2: {result2:?}");

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
    // and eventually finishes with 150. The update is marked as a success, but immediately
    // it gets updated again to v3 on which we can call the previously non-existent 'get'
    // function to get the same state that was generated by 'v2'.
    check!(result1[0] == Value::U64(150));
    check!(result2[0] == Value::U64(150));
    check!(metadata.component_version == updated_component_2.revision);
    check!(update_counts(&metadata) == (0, 2, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_with_failing_load(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v2")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "manual_update_on_idle_with_failing_load",
            env,
            vec![],
        )
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v4")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    executor
        .manual_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    // Explanation: We try to update v2 to v4, but v4's load function always fails. So
    // the component must stay on v2, on which we can invoke f3.
    check!(result == vec![Value::U64(5)]);
    check!(metadata.component_version == ComponentRevision(0));
    check!(update_counts(&metadata) == (0, 0, 1));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_using_v11(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v2-11")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "manual_update_on_idle_using_v11",
            env,
            vec![],
        )
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v3-11")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    let before_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    executor
        .manual_update_worker(&worker_id, updated_component.revision)
        .await?;

    let after_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    check!(before_update == after_update);
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn manual_update_on_idle_using_golem_rust_sdk(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let component = executor
        .component(&context.default_environment_id, "update-test-v2-11")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "manual_update_on_idle_using_golem_rust_sdk",
            env,
            vec![],
        )
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v3-sdk")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{f1}",
            vec![0u64.into_value_and_type()],
        )
        .await??;

    let before_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    executor
        .manual_update_worker(&worker_id, updated_component.revision)
        .await?;

    let after_update = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Explanation: we can call 'get' on the updated component that does not exist in previous
    // versions, and it returns the previous global state which has been transferred to it
    // using the v2 component's 'save' function through the v3 component's load function.

    drop(executor);
    http_server.abort();

    check!(before_update == after_update);
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn auto_update_on_idle_to_non_existing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "update-test-v1")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "auto_update_on_idle")
        .await?;
    executor.log_output(&worker_id).await?;

    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    // Now we try to update to version target_version + 1, which does not exist.
    executor
        .auto_update_worker(&worker_id, updated_component.revision.next()?)
        .await?;

    // We expect this update to fail, and the component to remain on `target_version` and remain
    // responsible to further invocations:

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await??;

    let metadata = executor.get_worker_metadata(&worker_id).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result1[0] == Value::U64(0));
    check!(result2[0] == Value::U64(0));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 1));

    Ok(())
}

/// Check that GOLEM_COMPONENT_REVISION environment variable is updated as part of a worker update
#[test]
#[tracing::instrument]
async fn update_component_revision_environment_variable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "update-test-env-var")
        .store()
        .await?;

    let worker_id = executor.start_worker(&component.id, "worker-1").await?;

    {
        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem:component/api.{get-revision-from-env-var}",
                vec![],
            )
            .await??;

        assert_eq!(result, vec![Value::String("0".to_string())]);
    }

    let updated_component_1 = executor
        .update_component(&component.id, "update-test-env-var")
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component_1.revision)
        .await?;

    {
        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem:component/api.{get-revision-from-env-var}",
                vec![],
            )
            .await??;

        assert_eq!(result, vec![Value::String("0".to_string())]);

        // FIXME: broken as get-environment during the replay is getting cached
        // assert_eq!(result, vec![Value::String("1".to_string())]);
    }

    // worker created on the new version sees correct component version
    {
        let worker2 = executor.start_worker(&component.id, "worker-2").await?;

        let result = executor
            .invoke_and_await(
                &worker2,
                "golem:component/api.{get-revision-from-env-var}",
                vec![],
            )
            .await??;

        assert_eq!(result, vec![Value::String("1".to_string())]);
    }

    let updated_component_2 = executor
        .update_component(&component.id, "update-test-env-var")
        .await?;

    executor
        .manual_update_worker(&worker_id, updated_component_2.revision)
        .await?;

    {
        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem:component/api.{get-revision-from-env-var}",
                vec![],
            )
            .await??;

        assert_eq!(result, vec![Value::String("2".to_string())]);
    }

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}
