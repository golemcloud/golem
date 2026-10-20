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

use super::{mcp_http_transport_config, mcp_transport};
use crate::mcp::serving::{McpServing, StatelessServing};
use crate::mcp::session::McpReplica;
use crate::mcp::session::store::SqliteMcpSessionStore;
use golem_service_base::mcp::CompiledMcp;
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

/// Opens a session's GET stream through an idle proxy, with or without keep-alive comments, and
/// returns whether the stream was closed within `SILENCE` and what it carried.
async fn idle_mcp_stream(keep_alive: bool) -> (bool, Vec<u8>) {
    use crate::config::McpConfig;
    use crate::transport_test_support::{INTERVAL, SILENCE, idle_proxy, server};
    use poem::EndpointExt;

    let config = McpConfig {
        sse_keep_alive_interval: if keep_alive {
            INTERVAL
        } else {
            std::time::Duration::from_secs(3600)
        },
        ..McpConfig::default()
    };
    let db = tempfile::NamedTempFile::new().unwrap();
    let pool = crate::custom_api::oidc::session_store::migrated_sqlite_pool(
        &golem_common::config::DbSqliteConfig {
            database: db.path().to_string_lossy().into_owned(),
            max_connections: 1,
            foreign_keys: false,
        },
    )
    .await
    .unwrap();
    let store = Arc::new(SqliteMcpSessionStore::new(
        pool,
        std::time::Duration::from_secs(3600),
    ));
    let replica = McpReplica::new(store, &config);
    let deployment = Arc::new(CompiledMcp {
        account_id: golem_common::model::account::AccountId::new(),
        account_email: golem_common::model::account::AccountEmail::new("owner@example.com"),
        application_name: golem_common::model::application::ApplicationName("app".into()),
        environment_id: golem_common::model::environment::EnvironmentId::new(),
        environment_name: golem_common::model::environment::EnvironmentName("env".into()),
        deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
        domain: golem_common::model::domain_registration::Domain("127.0.0.1".into()),
        security_scheme_name: None,
        security_scheme: None,
        registered_agent_types: vec![],
        tools: vec![],
    });
    let endpoint = mcp_transport(|| Server, replica, &config)
        .before(move |mut request: Request| {
            let serving: Arc<dyn McpServing> = Arc::new(StatelessServing(deployment.clone()));
            request.extensions_mut().insert(deployment.clone());
            request.extensions_mut().insert(serving);
            async move { Ok(request) }
        })
        .map_to_response();
    let (address, _server) = server(endpoint).await;
    let (proxy, _proxy) = idle_proxy(address).await;
    let url = format!("http://{proxy}/mcp");
    let client = reqwest::Client::new();
    let initialized = client
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "keep-alive-test", "version": "1"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(initialized.status().is_success());
    let session = initialized.headers()["Mcp-Session-Id"].clone();
    initialized.bytes().await.unwrap();
    let notified = client
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", session.clone())
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(notified.status().is_success());
    let mut response = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", session)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let mut bytes = Vec::new();
    let closed = tokio::time::timeout(SILENCE, async {
        while let Ok(Some(chunk)) = response.chunk().await {
            bytes.extend_from_slice(&chunk);
        }
    })
    .await
    .is_ok();
    (closed, bytes)
}

#[test]
#[test_r::timeout("15s")]
async fn mcp_sse_without_keep_alive_loses_proxy_connection() {
    let (closed, _) = idle_mcp_stream(false).await;
    assert!(closed, "the proxy must close an idle SSE connection");
    eprintln!("without MCP SSE keep-alive: stream closed by the 1s-idle proxy");
}

#[test]
#[test_r::timeout("15s")]
async fn mcp_sse_keep_alive_survives_idle_proxy() {
    let (closed, bytes) = idle_mcp_stream(true).await;
    assert!(!closed, "the SSE stream must stay open");
    assert!(
        bytes.windows(3).any(|bytes| bytes == b":\n\n"),
        "expected RMCP keep-alive comments, got {bytes:?}"
    );
    eprintln!("with MCP SSE keep-alive: stream stayed open for 3s through a 1s-idle proxy");
}
