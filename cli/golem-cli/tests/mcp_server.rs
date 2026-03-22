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
use std::time::Duration;
use test_r::{inherit_test_dep, sequential_suite, tag_suite, test};
use tokio::net::TcpListener;
use tokio::time::sleep;

tag_suite!(mcp_server, group4);
sequential_suite!(mcp_server);
inherit_test_dep!(Tracing);

async fn get_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
async fn mcp_server_starts_and_responds_to_health_check(_tracing: &Tracing) {
    let port = get_free_port().await;
    let exe = env!("CARGO_BIN_EXE_golem-cli");
    let mut child = tokio::process::Command::new(exe)
        .arg("--serve")
        .arg("--serve-port")
        .arg(port.to_string())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to start golem-cli in serve mode");

    // Wait for server to start
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/health", port);

    let mut success = false;
    for _ in 0..20 {
        sleep(Duration::from_millis(500)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                success = true;
                break;
            }
        }
    }

    assert!(
        success,
        "MCP server failed to start or respond to health check"
    );

    // Test tool list via MCP protocol
    let mcp_url = format!("http://127.0.0.1:{}/mcp", port);
    let tools_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let resp = client
        .post(&mcp_url)
        .json(&tools_req)
        .send()
        .await
        .expect("Failed to send tools/list request");

    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();

    // Check if cli.metadata tool is exposed
    let tools = body["result"]["tools"]
        .as_array()
        .expect("Tools list is not an array");
    let has_cli_metadata = tools.iter().any(|tool| tool["name"] == "cli.metadata");
    assert!(
        has_cli_metadata,
        "cli.metadata tool is missing from tools/list"
    );

    // Test SSE stream endpoint
    let sse_url = format!("http://127.0.0.1:{}/sse", port);
    let sse_resp = client
        .get(&sse_url)
        .send()
        .await
        .expect("Failed to connect to /sse");

    assert!(
        sse_resp.status().is_success(),
        "SSE endpoint returned non-success status"
    );
    let content_type = sse_resp
        .headers()
        .get("content-type")
        .expect("Missing content-type header")
        .to_str()
        .unwrap();
    assert!(
        content_type.starts_with("text/event-stream"),
        "SSE endpoint did not return text/event-stream"
    );

    child.kill().await.unwrap();
}
