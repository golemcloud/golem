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

use crate::app::TestContext;
use crate::Tracing;
use assert2::assert;
use serde_json::json;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[test_r::test]
async fn test_mcp_initialize(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let mut child = Command::new(&ctx.golem_cli_path)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn golem-cli --serve");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut lines = BufReader::new(stdout).lines();

    // Send initialize request
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });

    stdin
        .write_all(format!("{}\n", init_req).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    // Read response
    let line = lines
        .next_line()
        .await
        .unwrap()
        .expect("No response from MCP server");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert!(resp["id"] == 1);
    assert!(resp["result"]["capabilities"]["tools"].is_object());

    // Clean up
    let _ = child.kill().await;
}

#[test_r::test]
async fn test_mcp_list_tools(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let mut child = Command::new(&ctx.golem_cli_path)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn golem-cli --serve");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut lines = BufReader::new(stdout).lines();

    // Skip initialize for simplicity (or send it)
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });
    stdin
        .write_all(format!("{}\n", init_req).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let _ = lines.next_line().await;

    // Send tools/list request
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    stdin
        .write_all(format!("{}\n", list_req).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = lines
        .next_line()
        .await
        .unwrap()
        .expect("No response for tools/list");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert!(resp["id"] == 2);
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("Result tools should be an array");
    assert!(tools.iter().any(|t| t["name"] == "run_command"));
    assert!(tools.iter().any(|t| t["name"] == "get_info"));
    assert!(tools.iter().any(|t| t["name"] == "list_components"));

    // Clean up
    let _ = child.kill().await;
}

#[test_r::test]
async fn test_mcp_call_get_info(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let mut child = Command::new(&ctx.golem_cli_path)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn golem-cli --serve");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut lines = BufReader::new(stdout).lines();

    // Skip initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });
    stdin
        .write_all(format!("{}\n", init_req).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let _ = lines.next_line().await;

    // Send tools/call request for get_info
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "get_info",
            "arguments": {}
        }
    });

    stdin
        .write_all(format!("{}\n", call_req).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = lines
        .next_line()
        .await
        .unwrap()
        .expect("No response for tools/call");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert!(resp["id"] == 3);
    let content = resp["result"]["content"]
        .as_array()
        .expect("Result content should be an array");
    let text = content[0]["text"]
        .as_str()
        .expect("First content item should have text");
    
    // The get_info tool runs `golem --version`, so it should contain a version string
    assert!(text.contains("golem"));

    // Clean up
    let _ = child.kill().await;
}
