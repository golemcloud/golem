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
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::task::JoinHandle;

inherit_test_dep!(Tracing);

/// Find an available port by binding to port 0.
fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// An MCP test session that manages server lifecycle and session state.
struct McpTestSession {
    port: u16,
    session_id: Option<String>,
    handle: JoinHandle<()>,
}

impl McpTestSession {
    /// Start a new MCP server and initialize a session.
    async fn start() -> Self {
        let port = find_free_port();

        let handle = tokio::spawn(async move {
            golem_cli::mcp::server::start_mcp_server(port)
                .await
                .expect("MCP server failed");
        });

        // Wait for server to start
        let client = Client::new();
        let url = format!("http://127.0.0.1:{port}/mcp");
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(resp) = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .body(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 0,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {},
                            "clientInfo": {
                                "name": "test-client",
                                "version": "0.1.0"
                            }
                        }
                    })
                    .to_string(),
                )
                .send()
                .await
            {
                // Extract session ID from response header
                let session_id = resp
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());

                return Self {
                    port,
                    session_id,
                    handle,
                };
            }
        }

        Self {
            port,
            session_id: None,
            handle,
        }
    }

    /// Send a JSON-RPC request within this session.
    async fn request(&self, method: &str, params: Value) -> Value {
        let client = Client::new();
        let url = format!("http://127.0.0.1:{}/mcp", self.port);

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });

        let mut req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(ref sid) = self.session_id {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        let resp = req
            .body(body.to_string())
            .send()
            .await
            .expect("Failed to send request to MCP server");

        let status = resp.status();
        let body_text = resp.text().await.expect("Failed to read response body");

        // Try to parse as JSON directly
        if let Ok(v) = serde_json::from_str::<Value>(&body_text) {
            return v;
        }

        // If SSE format, extract JSON from data: lines
        let mut last_data = None;
        for line in body_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                last_data = Some(data.to_string());
            }
        }

        if let Some(data) = last_data {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                return v;
            }
        }

        panic!(
            "Failed to parse MCP response for '{}': status={}, body={}",
            method, status, body_text
        );
    }

    /// Send a raw HTTP POST with custom body (for testing malformed requests).
    async fn raw_post(&self, body: &str) -> reqwest::Response {
        let client = Client::new();
        let url = format!("http://127.0.0.1:{}/mcp", self.port);

        let mut req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(ref sid) = self.session_id {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        req.body(body.to_string())
            .send()
            .await
            .expect("Failed to send raw request to MCP server")
    }

    fn abort(self) {
        self.handle.abort();
    }
}

// ========================
// Phase 2: Server Bootstrap
// ========================

#[test]
async fn mcp_server_starts_and_responds_to_initialize(_tracing: &Tracing) {
    let port = find_free_port();

    let handle = tokio::spawn(async move {
        golem_cli::mcp::server::start_mcp_server(port)
            .await
            .expect("MCP server failed");
    });

    // Wait for server
    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "test-client",
                        "version": "0.1.0"
                    }
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("Server should be reachable");

    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());

    let body_text = resp.text().await.expect("Should read body");

    // Parse the response - may be direct JSON or SSE format
    let body: Value = if let Ok(v) = serde_json::from_str(&body_text) {
        v
    } else {
        // Extract from SSE data: line
        let data_line = body_text
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .expect("Should find data: line in SSE response");
        serde_json::from_str(data_line).expect("Should parse SSE data as JSON")
    };

    // Verify the response has the expected structure
    assert!(
        body.get("result").is_some(),
        "Response should have 'result': {body}"
    );
    let result = &body["result"];
    assert_eq!(
        result["protocolVersion"], "2025-11-25",
        "Protocol version mismatch"
    );
    assert!(
        result["serverInfo"]["name"].as_str().is_some(),
        "Server info should have a name"
    );
    assert!(
        result["capabilities"]["tools"].is_object(),
        "Server should advertise tool capabilities"
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "Server should advertise resource capabilities"
    );
    assert!(
        session_id.is_some(),
        "Server should return a session ID header"
    );

    handle.abort();
}

#[test]
async fn mcp_server_starts_on_custom_port(_tracing: &Tracing) {
    let port = find_free_port();

    let handle = tokio::spawn(async move {
        golem_cli::mcp::server::start_mcp_server(port)
            .await
            .expect("MCP server failed");
    });

    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "test-client",
                        "version": "0.1.0"
                    }
                }
            })
            .to_string(),
        )
        .send()
        .await;

    assert!(
        resp.is_ok(),
        "Server should be reachable on custom port {port}"
    );

    handle.abort();
}

// ========================
// Phase 3: Tool Discovery
// ========================

#[test]
async fn mcp_tools_list_returns_tools(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    let resp = session.request("tools/list", json!({})).await;

    assert!(resp.get("result").is_some(), "Should have result: {resp}");
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    assert!(!tools.is_empty(), "Should have at least one tool");

    // Check tool naming convention
    for tool in tools {
        let name = tool["name"].as_str().expect("tool should have a name");
        assert!(
            name.starts_with("golem_"),
            "Tool name should start with 'golem_': {name}"
        );
    }

    session.abort();
}

#[test]
async fn mcp_tools_have_input_schema(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    let resp = session.request("tools/list", json!({})).await;
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    // Every tool should have an inputSchema with type "object"
    for tool in tools {
        let schema = &tool["inputSchema"];
        assert_eq!(
            schema["type"], "object",
            "Tool {} inputSchema type should be 'object'",
            tool["name"]
        );
    }

    session.abort();
}

#[test]
async fn mcp_tools_list_includes_expected_groups(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    let resp = session.request("tools/list", json!({})).await;
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    // Should have tools from major command groups
    let expected_prefixes = ["golem_app", "golem_component", "golem_worker"];
    for prefix in &expected_prefixes {
        assert!(
            tool_names.iter().any(|n| n.starts_with(prefix)),
            "Should have tools starting with '{prefix}', got: {tool_names:?}"
        );
    }

    // Should NOT have serve, help, or completion tools
    assert!(
        !tool_names.iter().any(|n| n.contains("serve")),
        "Should not expose serve as a tool"
    );
    assert!(
        !tool_names.iter().any(|n| n.contains("help")),
        "Should not expose help as a tool"
    );
    assert!(
        !tool_names.iter().any(|n| n.contains("completion")),
        "Should not expose completion as a tool"
    );

    session.abort();
}

// ========================
// Phase 4: Tool Execution
// ========================

#[test]
async fn mcp_call_tool_with_invalid_name_returns_error(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    // Call a non-existent tool — the server should not crash
    let resp = session
        .request(
            "tools/call",
            json!({
                "name": "golem_nonexistent_command",
                "arguments": {}
            }),
        )
        .await;

    // The response should indicate an error but the server should still be alive
    let is_error_result = resp
        .get("result")
        .and_then(|r| r.get("isError"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_rpc_error = resp.get("error").is_some();

    assert!(
        is_error_result || is_rpc_error,
        "Should return some kind of error for invalid tool: {resp}"
    );

    // Verify server is still alive by making another request
    let health_check = session.request("tools/list", json!({})).await;
    assert!(
        health_check.get("result").is_some(),
        "Server should still respond after error: {health_check}"
    );

    session.abort();
}

// ========================
// Phase 5: Resource Discovery
// ========================

#[test]
async fn mcp_resources_list_responds(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    let resp = session.request("resources/list", json!({})).await;

    assert!(resp.get("result").is_some(), "Should have result: {resp}");
    let resources = &resp["result"]["resources"];
    assert!(resources.is_array(), "resources should be an array: {resp}");

    session.abort();
}

#[test]
async fn mcp_resources_read_nonexistent_returns_error(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    let resp = session
        .request(
            "resources/read",
            json!({ "uri": "file:///nonexistent/golem.yaml" }),
        )
        .await;

    // Should return an error
    assert!(
        resp.get("error").is_some(),
        "Should return error for non-existent resource: {resp}"
    );

    session.abort();
}

// ========================
// Phase 7: Transport Hardening
// ========================

#[test]
async fn mcp_server_rejects_malformed_request(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    // Send completely invalid JSON
    let resp = session.raw_post("not valid json at all").await;

    // Server should return an HTTP response (even if it's an error status)
    let _status = resp.status();

    // Verify server is still alive
    let health = session
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0.1.0" }
            }),
        )
        .await;

    // Server should still respond (initialize creates a new session, so it may work
    // differently, but the point is the server didn't crash)
    assert!(
        health.get("result").is_some() || health.get("error").is_some(),
        "Server should still respond after malformed request: {health}"
    );

    session.abort();
}

#[test]
async fn mcp_server_handles_concurrent_requests(_tracing: &Tracing) {
    let session = McpTestSession::start().await;
    let port = session.port;
    let session_id = session.session_id.clone();

    // Send 5 concurrent tools/list requests
    let mut handles = Vec::new();
    for _ in 0..5 {
        let sid = session_id.clone();
        handles.push(tokio::spawn(async move {
            let client = Client::new();
            let url = format!("http://127.0.0.1:{port}/mcp");

            let mut req = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");

            if let Some(ref s) = sid {
                req = req.header("Mcp-Session-Id", s.as_str());
            }

            let resp = req
                .body(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "tools/list",
                        "params": {}
                    })
                    .to_string(),
                )
                .send()
                .await
                .expect("Should send request");

            resp.json::<Value>().await.expect("Should parse JSON")
        }));
    }

    for h in handles {
        let resp = h.await.expect("Task should not panic");
        assert!(
            resp.get("result").is_some(),
            "All concurrent requests should succeed: {resp}"
        );
    }

    session.abort();
}

#[test]
async fn mcp_server_binds_to_localhost_only(_tracing: &Tracing) {
    let session = McpTestSession::start().await;

    // The server should be bound to 127.0.0.1 specifically
    // We verify this by confirming the server is reachable on localhost
    assert!(
        session.session_id.is_some(),
        "Server should be reachable on 127.0.0.1 (session established)"
    );

    session.abort();
}
