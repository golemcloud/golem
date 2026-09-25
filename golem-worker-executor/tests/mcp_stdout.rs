// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::Tracing;
use crate::tool_discovery::deployment_state;
use golem_common::agent_id;
use golem_common::data_value;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::mcp_import::{McpImport, McpImportSource};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, OwnedAgentId};
use golem_mcp_import::tool::{Limits, ProjectedTool};
use golem_service_base::clients::registry::McpRuntimeCredential;
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

fn projected_tool(name: &str) -> ProjectedTool {
    ProjectedTool::new(
        &json!({
            "name": name,
            "description": name,
            "inputSchema": {"type":"object", "properties":{}, "additionalProperties":false}
        }),
        name,
        Limits::default(),
    )
    .unwrap()
}

fn observation(source: McpImportSource, names: &[&str]) -> McpImportObservation {
    McpImportObservation {
        source,
        protocol_version: golem_mcp_import::transport::PROTOCOL_VERSION.to_string(),
        tools: names.iter().map(|name| projected_tool(name)).collect(),
        diagnostics: Vec::new(),
    }
}

#[test]
#[timeout("2m")]
async fn mcp_stdout_is_durable_before_consumption_and_projects_only_streamable_content(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const ATTACHMENT_BYTES: usize = 64 * 1024;
    const NAMES: [&str; 9] = [
        "crash",
        "text",
        "binary",
        "mixed",
        "empty",
        "error",
        "cancel",
        "cancel-presence",
        "cancel-output",
    ];
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let mcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mcp_port = mcp_listener.local_addr()?.port();
    let large = (0..65_537).map(|i| format!("{i:06}\n")).collect::<String>();
    let expected_large = large.as_bytes().to_vec();
    let mcp_handler = axum::routing::post({
        let calls = calls.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let calls = calls.clone();
            let large = large.clone();
            async move {
                let name = body["params"]["name"].as_str().unwrap().to_string();
                calls.lock().unwrap().push(name.clone());
                if name == "cancel" {
                    std::future::pending::<()>().await;
                }
                if name == "cancel-presence" {
                    return axum::Json(json!({"jsonrpc":"2.0", "id":body["id"],
                        "error":{"code":-32602,"message":"invalid parameters"}}));
                }
                let result = match name.as_str() {
                    "crash" | "cancel-output" => json!({"content":[{"type":"text","text":large}]}),
                    "text" => json!({"content":[{"type":"text","text":"hello stdout"}]}),
                    "binary" => {
                        json!({"content":[{"type":"image","data":"AP8B","mimeType":"application/octet-stream"}]})
                    }
                    "mixed" => {
                        json!({"content":[{"type":"text","text":"not bytes"},{"type":"image","data":"AA==","mimeType":"application/octet-stream"}]})
                    }
                    "empty" => json!({"content":[{"type":"text","text":""}]}),
                    "error" => {
                        json!({"isError":true,"content":[{"type":"text","text":"upstream failed"}]})
                    }
                    "cancel" => unreachable!(),
                    _ => unreachable!(),
                };
                axum::Json(json!({"jsonrpc":"2.0", "id":body["id"], "result":result}))
            }
        }
    });
    let mcp_server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(mcp_listener, axum::Router::new().route("/mcp", mcp_handler))
            .await
            .unwrap();
    }));

    let checkpoint_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let checkpoint_port = checkpoint_listener.local_addr()?.port();
    let (arrived_tx, mut arrived_rx) =
        tokio::sync::mpsc::unbounded_channel::<tokio::sync::oneshot::Sender<()>>();
    let checkpoint_handler = axum::routing::get(move || {
        let arrived_tx = arrived_tx.clone();
        async move {
            let (release, wait) = tokio::sync::oneshot::channel();
            arrived_tx.send(release).unwrap();
            let _ = wait.await;
            "ok"
        }
    });
    let checkpoint_server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(
            checkpoint_listener,
            axum::Router::new().route("/checkpoint/{name}", checkpoint_handler),
        )
        .await
        .unwrap();
    }));

    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        configure: Some(Arc::new(|config| {
            config.limits.max_tool_attachment_bytes = ATTACHMENT_BYTES;
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let mut deployment = deployment_state(
        &AgentTypeName("GolemHostApi".into()),
        70,
        component.revision,
        &[],
    );
    deployment.mcp_imports.push(McpImport {
        url: format!("http://127.0.0.1:{mcp_port}/mcp"),
        auth: None,
        security_scheme: None,
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    });
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let source = McpImportSource {
        environment_id: context.default_environment_id,
        deployment_revision: 70_u64.try_into().unwrap(),
        import_index: 0,
        upstream_tool_name: String::new(),
    };
    service.set_mcp_observation(source.clone(), Ok(observation(source.clone(), &NAMES)));
    for name in NAMES {
        let mut credential_source = source.clone();
        credential_source.upstream_tool_name = name.into();
        service.set_mcp_credential(
            credential_source,
            McpRuntimeCredential {
                credential: None,
                oauth_grant_generation: None,
            },
        );
    }
    let agent_id = agent_id!("GolemHostApi", "mcp-stdout");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([(
                "MCP_STDOUT_CHECKPOINT_PORT".to_string(),
                checkpoint_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let key = IdempotencyKey::fresh();
    let waiter = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "tool_rpc_collect_stdout",
                    data_value!("crash", Some("before-read".to_string())),
                )
                .await
        })
    };
    let release = tokio::time::timeout(Duration::from_secs(30), arrived_rx.recv())
        .await?
        .expect("checkpoint server stopped");
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await.unwrap();
            let start = oplog.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call" => Some(entry.oplog_index),
                _ => None,
            });
            if start.is_some_and(|start| oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start))) {
                let entity = oplog.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke")).expect("outer entity Start");
                assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == entity.oplog_index)), "crash must precede entity completion while stdout is backpressured");
                if let Some(stdout) = executor.active_entity_metadata(&owned_agent_id).await
                    .and_then(|active| active.tool_operations.operations.into_iter().next())
                    .and_then(|operation| operation.stdout)
                    && stdout.buffered_bytes == ATTACHMENT_BYTES
                {
                    assert_eq!(stdout.delivered_bytes, 0);
                    assert!(!stdout.terminal_selected);
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("MCP response was not committed before stdout consumption");
    assert_eq!(calls.lock().unwrap().as_slice(), ["crash"]);
    waiter.abort();
    let _ = waiter.await;
    drop(executor);
    drop(release);

    executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let recovery = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "tool_rpc_collect_stdout",
                    data_value!("crash", Some("before-read".to_string())),
                )
                .await
        })
    };
    let release = tokio::time::timeout(Duration::from_secs(30), arrived_rx.recv())
        .await?
        .expect("replayed checkpoint server stopped");
    release.send(()).expect("replayed checkpoint disconnected");
    let recovered = recovery
        .await??
        .into_typed::<(Result<(), String>, Result<Vec<u8>, String>)>()?;
    assert_eq!(recovered.0, Ok(()));
    assert_eq!(recovered.1.map_err(anyhow::Error::msg)?, expected_large);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["crash"],
        "replay resent tools/call"
    );

    for (name, expected, succeeds) in [
        ("text", b"hello stdout".to_vec(), true),
        ("binary", vec![0, 255, 1], true),
        ("mixed", Vec::new(), true),
        ("empty", Vec::new(), true),
        ("error", Vec::new(), false),
    ] {
        let outcome = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "tool_rpc_collect_stdout",
                data_value!(name, Option::<String>::None),
            )
            .await?
            .into_typed::<(Result<(), String>, Result<Vec<u8>, String>)>()?;
        assert_eq!(
            outcome.0.is_ok(),
            succeeds,
            "{name} result: {:?}",
            outcome.0
        );
        assert_eq!(
            outcome.1.map_err(anyhow::Error::msg)?,
            expected,
            "{name} stdout"
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let starts = oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")).count();
    let ends = oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if oplog.iter().any(|candidate| candidate.oplog_index == end.start_index && matches!(&candidate.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")))).count();
    assert_eq!((starts, ends), (6, 6));

    service.set_mcp_refresh_gate(Arc::new(tokio::sync::Notify::new()));
    for name in ["cancel", "cancel-presence", "cancel-output"] {
        let cancelled = {
            let executor = executor.clone();
            let component = component.clone();
            let agent_id = agent_id.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "tool_rpc_cancel_and_collect_stdout",
                        data_value!(name),
                    )
                    .await
            })
        };
        let release = tokio::time::timeout(Duration::from_secs(30), arrived_rx.recv())
            .await?
            .expect("cancellation checkpoint server stopped");
        tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let ready = match name {
                "cancel-presence" => service.mcp_observation_refreshes().contains(&true),
                "cancel-output" => {
                    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await.unwrap();
                    let start = oplog.iter().rev().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call"));
                    let called = calls.lock().unwrap().iter().any(|called| called == name);
                    called
                        && start.is_some_and(|start| oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start.oplog_index)))
                        && executor.active_entity_metadata(&owned_agent_id).await
                            .and_then(|active| active.tool_operations.operations.into_iter().next())
                            .and_then(|operation| operation.stdout)
                            .is_some_and(|stdout| stdout.buffered_bytes == ATTACHMENT_BYTES
                                && stdout.delivered_bytes == 0 && !stdout.terminal_selected)
                }
                _ => calls.lock().unwrap().iter().any(|called| called == name),
            };
            if ready {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("MCP invocation did not reach its cancellation checkpoint");
        release
            .send(())
            .expect("cancellation checkpoint disconnected");
        let cancelled = cancelled
            .await??
            .into_typed::<(Result<(), String>, Result<Vec<u8>, String>)>()?;
        assert!(
            cancelled
                .0
                .as_ref()
                .is_err_and(|error| error.contains("Cancelled")),
            "{name}: ordinary ToolRpc cancellation must settle the result: {:?}",
            cancelled.0
        );
        assert_eq!(
            cancelled.1,
            Err("ByteStreamFailure::Cancelled".to_string()),
            "cancelled stdout must settle with the ordinary attachment cancellation"
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let cancel_start = oplog
        .iter()
        .rev()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call"))
        .expect("cancelled MCP Start");
        assert!(oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == cancel_start.oplog_index)), "cancelled MCP call must settle durably");
        if name == "cancel-presence" {
            let presence = oplog.iter().rev().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::presence")).expect("presence Start");
            assert!(oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == presence.oplog_index)), "cancelled presence must settle durably");
        }
    }
    let call_count = calls.lock().unwrap().len();
    let credential_request_count = service.mcp_credential_requests().len();
    let observation_request_count = service.mcp_observation_requests().len();
    service.clear_mcp_observations();
    service.clear_mcp_credentials();
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(replayed, Ok(agent_id.to_string()));
    assert_eq!(
        calls.lock().unwrap().len(),
        call_count,
        "completed cancellation replay repeated upstream HTTP"
    );
    assert_eq!(
        service.mcp_credential_requests().len(),
        credential_request_count,
        "completed cancellation replay reacquired OAuth credentials"
    );
    assert_eq!(
        service.mcp_observation_requests().len(),
        observation_request_count,
        "completed presence cancellation replay refreshed metadata"
    );
    drop(mcp_server);
    drop(checkpoint_server);
    Ok(())
}
