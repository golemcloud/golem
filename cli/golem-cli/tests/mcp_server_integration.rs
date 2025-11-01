// MCP Server Integration Tests
// End-to-end tests for MCP protocol implementation

use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Helper to start MCP server in background
fn start_mcp_server(port: u16, temp_dir: &TempDir) -> Child {
    Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--",
            &format!("--serve={}", port),
            "--component-dir",
            temp_dir.path().to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start MCP server")
}

/// Test that MCP server starts successfully
#[test_r::test]
fn test_mcp_server_starts() {
    let temp_dir = TempDir::new().unwrap();
    let mut child = start_mcp_server(8090, &temp_dir);

    // Give server time to start
    thread::sleep(Duration::from_secs(3));

    // Check if process is still running
    match child.try_wait() {
        Ok(None) => {
            // Still running - success!
            child.kill().expect("Failed to kill server");
        }
        Ok(Some(status)) => {
            panic!("Server exited unexpectedly with status: {}", status);
        }
        Err(e) => {
            panic!("Failed to check server status: {}", e);
        }
    }
}

/// Test MCP protocol: initialize session
#[test_r::test]
fn test_mcp_initialize() {
    let temp_dir = TempDir::new().unwrap();
    let port = 8091;
    let mut child = start_mcp_server(port, &temp_dir);

    thread::sleep(Duration::from_secs(3));

    // Send initialize request
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(format!("http://localhost:{}/mcp", port))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0"
                }
            }
        }))
        .send();

    child.kill().expect("Failed to kill server");

    assert!(response.is_ok(), "Initialize request failed");
    let resp = response.unwrap();
    assert!(resp.status().is_success(), "Initialize returned error status");
}

/// Test tools/list returns expected number of tools
#[test_r::test]
fn test_mcp_tools_list() {
    let temp_dir = TempDir::new().unwrap();
    let port = 8092;
    let mut child = start_mcp_server(port, &temp_dir);

    thread::sleep(Duration::from_secs(3));

    let client = reqwest::blocking::Client::new();

    // Initialize session first
    let init_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .expect("Initialize failed");

    // Extract session ID from headers
    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .expect("No session ID in response")
        .to_str()
        .unwrap();

    // Send initialized notification
    client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .expect("Initialized notification failed");

    // List tools
    let tools_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .expect("Tools list failed");

    child.kill().expect("Failed to kill server");

    assert!(tools_response.status().is_success());

    let body: serde_json::Value = tools_response.json().expect("Failed to parse JSON");
    let tools = body["result"]["tools"]
        .as_array()
        .expect("No tools array in response");

    // Bounty requirement: at least 90 tools
    assert!(
        tools.len() >= 90,
        "Expected at least 90 tools, got {}",
        tools.len()
    );

    println!("✅ MCP server exposes {} tools", tools.len());
}

/// Test that tools have proper structure
#[test_r::test]
fn test_mcp_tool_structure() {
    let temp_dir = TempDir::new().unwrap();
    let port = 8093;
    let mut child = start_mcp_server(port, &temp_dir);

    thread::sleep(Duration::from_secs(3));

    let client = reqwest::blocking::Client::new();

    // Initialize and get session
    let init_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .unwrap();

    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap();

    // Initialized notification
    client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .unwrap();

    // List tools
    let tools_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .unwrap();

    child.kill().unwrap();

    let body: serde_json::Value = tools_response.json().unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();

    // Check first tool has required fields
    let first_tool = &tools[0];
    assert!(first_tool["name"].is_string(), "Tool should have name");
    assert!(first_tool["description"].is_string(), "Tool should have description");
    assert!(first_tool["inputSchema"].is_object(), "Tool should have inputSchema");

    println!("✅ Tools have proper MCP structure");
}

/// Test that sensitive commands are filtered
#[test_r::test]
fn test_mcp_sensitive_commands_filtered() {
    let temp_dir = TempDir::new().unwrap();
    let port = 8094;
    let mut child = start_mcp_server(port, &temp_dir);

    thread::sleep(Duration::from_secs(3));

    let client = reqwest::blocking::Client::new();

    // Initialize session
    let init_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .unwrap();

    let session_id = init_response.headers().get("mcp-session-id").unwrap().to_str().unwrap();

    client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .unwrap();

    // List tools
    let tools_response = client
        .post(format!("http://localhost:{}/mcp", port))
        .header("mcp-session-id", session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .unwrap();

    child.kill().unwrap();

    let body: serde_json::Value = tools_response.json().unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();

    let tool_names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    // Verify sensitive commands are NOT present
    assert!(!tool_names.iter().any(|n| n.starts_with("profile")),
            "Profile commands should be filtered");
    assert!(!tool_names.iter().any(|n| n.contains("token")),
            "Token commands should be filtered");
    assert!(!tool_names.iter().any(|n| n.contains("grant")),
            "Grant commands should be filtered");

    println!("✅ Sensitive commands properly filtered");
}
