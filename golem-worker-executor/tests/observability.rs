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
use axum::routing::post;
use axum::{Json, Router};
use golem_common::model::IdempotencyKey;
use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
use golem_common::model::oplog::{
    HostRequest, HostRequestP3HttpSpanCleanup, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::dsl::debug_render::debug_render_oplog_entry;
use pretty_assertions::assert_eq;

use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use http::HeaderMap;
use log::info;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn get_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("GolemHostApi", "getoplog1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;

    executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key1,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;

    executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key2,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog2 = executor.get_oplog(&worker_id, OplogIndex::NONE).await?;

    assert_eq!(oplog[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(oplog[0].entry, PublicOplogEntry::Create(_)));

    assert_eq!(oplog2[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(oplog2[0].entry, PublicOplogEntry::Create(_)));

    let invoke_count = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                    invocation: PublicAgentInvocation::AgentMethodInvocation(_),
                    ..
                })
            )
        })
        .count();
    assert!(
        invoke_count >= 3,
        "Expected at least 3 AgentInvocationStarted entries with AgentMethodInvocation, got {invoke_count}"
    );

    Ok(())
}

#[test]
#[timeout("1m")]
async fn p3_unread_response_cleanup_reconstructs_before_next_accessor(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let route = Router::new().route(
            "/drop-unread",
            post(move || {
                let request = server_requests.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    http::StatusCode::from_u16(200 + request as u16)
                        .expect("test request count must fit an HTTP status")
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("RawWasiHttp", "p3-unread-cleanup-reconstruction");
    let worker = executor
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "drop_unread_response_then_wait",
            data_value!(),
        )
        .await?
        .into_typed::<u16>()?;
    assert_eq!(first, 201);
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    let initial_oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let send_index = initial_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == "http::client::send" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("P3 send Start");
    let cleanup_request: HostRequest = HostRequestP3HttpSpanCleanup {
        send_start_index: send_index,
    }
    .into();
    let cleanup_request = cleanup_request.into_typed_schema_value()?;
    let cleanup_index = initial_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "http::client::span-cleanup"
                    && start.request.as_ref() == Some(&cleanup_request) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("span cleanup Start referencing the P3 send");
    let cleanup_end = initial_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == cleanup_index => Some(end),
            _ => None,
        })
        .expect("span cleanup End");
    assert!(cleanup_end.span_finished.is_some());
    assert!(!initial_oplog.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::CompletionDelivered(marker) => marker.start_index == cleanup_index,
        PublicOplogEntry::CompletionDiscarded(marker) => marker.start_index == cleanup_index,
        _ => false,
    }));

    drop(executor);
    let executor = start(deps, &context).await?;
    let second = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "drop_unread_response_then_wait",
            data_value!(),
        )
        .await?
        .into_typed::<u16>()?;
    assert_eq!(second, 202);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "completed replay must not redispatch the first P3 send"
    );
    let reconstructed = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        reconstructed
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "http::client::span-cleanup" && start.request.as_ref() == Some(&cleanup_request)))
            .count(),
        1
    );
    assert_eq!(
        reconstructed
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == cleanup_index && end.span_finished.is_some()))
            .count(),
        1
    );
    server.abort();
    Ok(())
}

#[test]
#[timeout("2m")]
async fn guest_span_reconstructs_completed_and_incomplete_operations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::worker::{RevertToOplogIndex, RevertWorkerTarget};

    for cut in 0..7 {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let agent = agent_id!("InvocationContext", format!("span-prefix-{cut}"));
        let worker = executor.start_agent(&component.id, agent.clone()).await?;
        let original = executor
            .invoke_and_await_agent(&component, &agent, "span_roundtrip", data_value!())
            .await?
            .into_typed::<String>()?;
        let value: serde_json::Value = serde_json::from_str(&original)?;
        assert_eq!(value["x"], "last");
        assert_eq!(value["y"], "other");
        let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let starts: Vec<_> = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name.starts_with("golem::api::context") =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 3, "creation, attributes, and one finish only");
        let boundaries: Vec<_> = oplog.iter().filter(|entry| starts.contains(&entry.oplog_index) || matches!(&entry.entry, PublicOplogEntry::End(end) if starts.contains(&end.start_index))).map(|entry| entry.oplog_index).collect();
        assert_eq!(boundaries.len(), 6);
        let mut retained = None;
        if cut < boundaries.len() {
            let index = boundaries[cut];
            executor
                .revert(
                    &worker,
                    RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                        last_oplog_index: index,
                    }),
                )
                .await?;
            let reverted = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            retained = Some((index, reverted.last().unwrap().oplog_index));
        }
        drop(executor);

        let executor = start(deps, &context).await?;
        let reconstructed = executor
            .invoke_and_await_agent(&component, &agent, "last_span", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(
            reconstructed, original,
            "cut {cut} must preserve id, timestamp, parent, headers, and attributes"
        );
        let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let visible: Vec<_> = oplog
            .iter()
            .filter(|entry| {
                retained.is_none_or(|(cut, revert)| {
                    entry.oplog_index <= cut || entry.oplog_index > revert
                })
            })
            .collect();
        assert_eq!(visible.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.span_started.is_some())).count(), 1);
        assert_eq!(visible.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.span_finished.is_some())).count(), 1);
        for entry in &visible {
            if let PublicOplogEntry::Start(start) = &entry.entry
                && start.function_name.starts_with("golem::api::context")
            {
                assert_eq!(visible.iter().filter(|end| matches!(&end.entry, PublicOplogEntry::End(end) if end.start_index == entry.oplog_index)).count(), 1);
            }
        }
    }
    Ok(())
}

#[test]
#[timeout("1m")]
async fn finished_guest_span_attribute_error_does_not_open_operation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("InvocationContext", "finished-span-error");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent, "mutate_finished_span", data_value!())
            .await
            .is_err()
    );
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::api::context::span::set-attributes")));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn search_oplog_1(
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

    let repo_id = agent_id!("Repository", "search-oplog-1");
    let worker_id = executor.start_agent(&component.id, repo_id.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    executor
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    executor
        .invoke_and_await_agent(&component, &repo_id, "get", data_value!("G1002"))
        .await?;

    let result1 = executor.search_oplog(&worker_id, "G1002").await?;

    let result2 = executor
        .search_oplog(&worker_id, "imported-function")
        .await?;

    let result3 = executor
        .search_oplog(&worker_id, "id:G1001 OR id:G1000")
        .await?;

    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for entry in entries {
        println!(
            "{}\n{}",
            entry.oplog_index,
            debug_render_oplog_entry(&entry.entry)
        );
    }

    assert_eq!(result1.len(), 2, "G1002"); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog
    // Includes the core initializer's monotonic clock call.
    assert_eq!(result2.len(), 3, "imported-function");
    assert_eq!(result3.len(), 0, "id:G1001 OR id:G1000"); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_oplog_with_api_changing_updates(
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

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    // there might be a pending invocation entry before the update entry. Filter it out to make the test more robust
    let oplog = oplog
        .into_iter()
        .filter(|entry| !matches!(entry.entry, PublicOplogEntry::PendingAgentInvocation(_)))
        .filter(|entry| !matches!(entry.entry, PublicOplogEntry::GrowMemory(_)))
        .collect::<Vec<_>>();

    assert_eq!(result.into_typed::<u64>()?, 11);

    let _ = executor.check_oplog_is_queryable(&worker_id).await;

    assert_eq!(oplog.len(), 15);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_oplog_starting_with_updated_component(
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
    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let agent_id = agent_id!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "f4", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let oplog = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| !matches!(entry.entry, PublicOplogEntry::GrowMemory(_)))
        .collect::<Vec<_>>();

    assert_eq!(result.into_typed::<u64>()?, 11);
    assert_eq!(oplog.len(), 11);

    Ok(())
}

#[test]
#[tracing::instrument]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("InvocationContext", "w1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "test1", data_value!())
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
