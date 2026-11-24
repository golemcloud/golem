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

use super::{mcp_http_transport_config, stateless_transport};
use crate::mcp::McpDeployment;
use crate::mcp::serving::{McpServing, StatelessServing};
use futures::StreamExt;
use poem::endpoint::TowerCompatExt;
use poem::{
    Body, Endpoint, Request,
    http::{Method, StatusCode},
};
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use serde_json::json;
use std::sync::Arc;
use test_r::test;

#[derive(Clone)]
struct Server;
impl rmcp::ServerHandler for Server {}

#[test]
#[test_r::timeout("10s")]
async fn mcp_export_transport_accepts_deployment_domain_hosts() {
    let endpoint = StreamableHttpService::new(
        || Ok(Server),
        NeverSessionManager::default().into(),
        mcp_http_transport_config(&Default::default()),
    )
    .compat();
    let response = endpoint
        .call(
            Request::builder()
                .method(Method::POST)
                .uri("https://customer.example/mcp".parse().unwrap())
                .header("Host", "customer.example")
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .body(
                    Body::from_json(json!({
                        "jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                            "protocolVersion":"2025-03-26", "capabilities":{},
                            "clientInfo":{"name":"test", "version":"1"}
                        }
                    }))
                    .unwrap(),
                ),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// POSTs a native tool call that never answers through an idle proxy, with or without
/// keep-alive comments, and returns whether the response stream was closed within `SILENCE`
/// and what it carried.
async fn idle_mcp_call(keep_alive: bool) -> (bool, Vec<u8>) {
    use crate::config::McpConfig;
    use crate::mcp::invoke::test_support::NativeSessions;
    use crate::mcp::{GolemAgentMcpServer, InvocationHarness};
    use crate::transport_test_support::{INTERVAL, SILENCE, idle_proxy, server};
    use golem_common::model::agent::AgentMode;
    use golem_common::model::deployment::DeploymentRevision;
    use poem::EndpointExt;

    let config = McpConfig {
        sse_keep_alive_interval: if keep_alive {
            INTERVAL
        } else {
            std::time::Duration::from_secs(3600)
        },
        ..McpConfig::default()
    };
    let harness = InvocationHarness::with_run_method(AgentMode::Durable);
    harness.answer_native_sessions(NativeSessions::Hold);
    let deployment = Arc::new(McpDeployment::new(harness.compiled_mcp(
        "127.0.0.1",
        DeploymentRevision::INITIAL,
        true,
    )));
    let worker_service = harness.worker_service.clone();
    let endpoint = stateless_transport(
        move || GolemAgentMcpServer::new(Arc::new(super::tests::Lookup), worker_service.clone()),
        &config,
    )
    .before(move |mut request: Request| {
        let serving: Arc<dyn McpServing> = Arc::new(StatelessServing(deployment.clone()));
        request.extensions_mut().insert(deployment.clone());
        request.extensions_mut().insert(serving);
        async move { Ok(request) }
    })
    .map_to_response();
    let (address, _server) = server(endpoint).await;
    let (proxy, _proxy) = idle_proxy(address).await;
    let mut response = reqwest::Client::new()
        .post(format!("http://{proxy}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "native", "arguments": {}}
        }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    harness.native_sessions_opened(1).await;
    let mut bytes = Vec::new();
    let read = futures::stream::unfold(&mut response, |response| async move {
        match response.chunk().await {
            Ok(Some(chunk)) => Some((chunk, response)),
            _ => None,
        }
    })
    .for_each(|chunk| {
        bytes.extend_from_slice(&chunk);
        std::future::ready(())
    });
    let closed = tokio::time::timeout(SILENCE, read).await.is_ok();
    (closed, bytes)
}

#[test]
#[test_r::timeout("15s")]
async fn mcp_sse_without_keep_alive_loses_proxy_connection() {
    let (closed, bytes) = idle_mcp_call(false).await;
    assert!(closed, "the proxy must close an idle SSE connection");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("notifications/message"),
        "{bytes:?}"
    );
}

#[test]
#[test_r::timeout("15s")]
async fn mcp_sse_keep_alive_keeps_a_slow_call_open_through_an_idle_proxy() {
    let (closed, bytes) = idle_mcp_call(true).await;
    assert!(!closed, "the SSE stream must stay open");
    assert!(
        bytes.windows(3).any(|bytes| bytes == b":\n\n"),
        "expected rmcp keep-alive comments, got {bytes:?}"
    );
    assert!(
        !String::from_utf8_lossy(&bytes).contains("notifications/message"),
        "{bytes:?}"
    );
}
