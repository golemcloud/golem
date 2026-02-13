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
use pretty_assertions::assert_eq;
use axum::routing::post;
use axum::{Json, Router};
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::oplog::public_oplog_entry::ExportedFunctionInvokedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::debug_render::debug_render_oplog_entry;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, Value};
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::HeaderMap;
use log::info;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn get_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog2 = executor.get_oplog(&worker_id, OplogIndex::NONE).await?;

    assert_eq!(oplog.len(), 16);
    assert_eq!(oplog[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(oplog[0].entry, PublicOplogEntry::Create(_)));

    assert_eq!(oplog2[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(oplog2[0].entry, PublicOplogEntry::Create(_)));

    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(&entry.entry, PublicOplogEntry::ExportedFunctionInvoked(
        ExportedFunctionInvokedParams { function_name, .. }
    ) if function_name == "golem:it/api.{generate-idempotency-keys}")
            )
            .count(),
        3
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn search_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "search-oplog-1");
    let worker_id = executor.start_agent(&component.id, repo_id.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    executor
        .invoke_and_await_agent(&component.id, &repo_id, "list", data_value!())
        .await?;

    executor
        .invoke_and_await_agent(&component.id, &repo_id, "get", data_value!("G1002"))
        .await?;

    let result1 = executor.search_oplog(&worker_id, "G1002").await?;

    let result2 = executor
        .search_oplog(&worker_id, "imported-function")
        .await?;

    let result3 = executor
        .search_oplog(&worker_id, "id:G1001 OR id:G1000")
        .await?;

    assert_eq!(result1.len(), 0); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog
    assert_eq!(result2.len(), 0);
    assert_eq!(result3.len(), 0); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_oplog_with_api_changing_updates(
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
        .start_worker(&component.id, "get_oplog_with_api_changing_updates")
        .await?;

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

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    // there might be a pending invocation entry before the update entry. Filter it out to make the test more robust
    let oplog = oplog
        .into_iter()
        .filter(|entry| !matches!(entry.entry, PublicOplogEntry::PendingWorkerInvocation(_)))
        .collect::<Vec<_>>();

    assert_eq!(result[0], Value::U64(11));

    let _ = executor.check_oplog_is_queryable(&worker_id).await;

    assert_eq!(oplog.len(), 11);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_oplog_starting_with_updated_component(
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
    let updated_component = executor
        .update_component(&component.id, "update-test-v2")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let worker_id = executor
        .start_worker(&component.id, "get_oplog_starting_with_updated_component")
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await??;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    assert_eq!(result[0], Value::U64(11));
    assert_eq!(oplog.len(), 4);

    Ok(())
}

#[test]
#[tracing::instrument]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let contexts = Arc::new(Mutex::new(Vec::new()));
    let contexts_clone = contexts.clone();

    let traceparents = Arc::new(Mutex::new(Vec::new()));
    let traceparents_clone = traceparents.clone();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/invocation-context",
                post(
                    move |headers: HeaderMap, body: Json<serde_json::Value>| async move {
                        contexts_clone.lock().unwrap().push(body.0);
                        traceparents_clone
                            .lock()
                            .unwrap()
                            .push(headers.get("traceparent").cloned());
                        "ok"
                    },
                ),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let component = executor
        .component(&context.default_environment_id, "golem_ictest")
        .with_dynamic_linking(&[(
            "golem:ictest-client/golem-ictest-client",
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                targets: HashMap::from_iter(vec![(
                    "golem-ictest-api".to_string(),
                    WasmRpcTarget {
                        interface_name: "golem:ictest-exports/golem-ictest-api".to_string(),
                        component_name: "golem_ictest".to_string(),
                    },
                )]),
            }),
        )])
        .store()
        .await?;

    let worker_id = executor
        .start_worker_with(&component.id, "w1", env.clone(), vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:ictest-exports/golem-ictest-api.{test1}",
            vec![],
        )
        .await?;

    let start = std::time::Instant::now();
    loop {
        let contexts = contexts.lock().unwrap();
        if contexts.len() == 3 {
            break;
        }
        drop(contexts);

        if start.elapsed().as_secs() > 30 {
            panic!("Timeout waiting for contexts");
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }

    let dump: Vec<_> = contexts.lock().unwrap().drain(..).collect();
    info!("{dump:#?}");

    executor.check_oplog_is_queryable(&worker_id).await?;

    http_server.abort();
    drop(executor);

    let traceparents = traceparents.lock().unwrap();

    assert!(result.is_ok());
    assert_eq!(traceparents.len(), 3);
    assert!(traceparents.iter().all(|tp| tp.is_some()));

    assert_eq!(
        dump[0]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    ); // root, invoke-exported-function
    assert_eq!(
        dump[1]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        5
    ); // + rpc-connection, rpc-invocation, invoke-exported-function
    assert_eq!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        10
    ); // + custom1, custom2, rpc-connection, rpc-invocation, invoke-exported-function

    Ok(())
}
