use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use axum::routing::get;
use axum::Router;
use golem_common::model::{WorkerId, WorkerStatus};
use golem_test_framework::dsl::{TestDslUnsafe};
use golem_wasm_rpc::Value;
use redis::Commands;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn fork_interrupted_worker_to_completion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    // We use poll functionality to gain better control over the worker lifecycle
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
            .await
            .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-parent-component-0", vec![], env)
        .await;

    let target = WorkerId {
        component_id: component_id.clone(),
        worker_name: "poll-loop-with-fork-component-0".to_string(),
    };

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
        )
        .await
        .unwrap();

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    executor
        .interrupt(&worker_id)
        .await;

    let (metadata, _) =
        executor.get_worker_metadata(&worker_id).await.unwrap();

    let last_index = metadata.last_known_status.oplog_idx;

    executor
        .fork_worker(&worker_id, &target, last_index)
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&target, WorkerStatus::Idle, Duration::from_secs(10))
        .await;

    drop(executor);
    http_server.abort();
}