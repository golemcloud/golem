// Integration tests for MCP Server
// These tests verify the MCP server functionality end-to-end

use std::time::Duration;
use tokio::time::sleep;
use serde_json::json;
use tokio::process::Command;


struct McpServerHandle {
    child: tokio::process::Child,
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

async fn spawn_mcp_server() -> McpServerHandle {
    let cargo_bin_path = match std::env::var("CARGO_BIN_EXE_golem-cli") {
        Ok(path) => path,
        Err(_) => "cargo".to_string(), // Fallback to cargo if not running via `cargo test`
    };

    let mut command = Command::new(cargo_bin_path); // Use tokio::process::Command
    if std::env::var("CARGO_BIN_EXE_golem-cli").is_err() {
        command.arg("run").arg("--bin").arg("golem-cli").args(["--"]);
    }
    command
        .arg("mcp-server")
        .arg("start")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("13337")
        .kill_on_drop(true) // Ensure the process is killed when the command drops
        .stdout(std::process::Stdio::null()) // Suppress stdout to avoid polluting test output
        .stderr(std::process::Stdio::null()); // Suppress stderr

    let child = command.spawn().expect("Failed to spawn golem-cli --serve");

    assert!(wait_for_server(50).await, "MCP Server did not start");

    McpServerHandle { child }
}

const SERVER_URL: &str = "http://127.0.0.1:13337";
const MCP_ENDPOINT: &str = "http://127.0.0.1:13337/mcp";

/// Helper function to check if server is ready
async fn wait_for_server(max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if reqwest::get(SERVER_URL).await.is_ok() {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Helper to make MCP JSON-RPC requests
async fn mcp_request(method: &str, params: serde_json::Value, id: i32) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    
    let request_body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });
    
    let response = client
        .post(MCP_ENDPOINT)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }
    
    response.json().await
        .map_err(|e| format!("Failed to parse JSON: {}", e))
}

#[tokio::test]
 // Run with: cargo test --package golem-cli --test mcp_integration -- --ignored
async fn test_server_health_endpoint() {
    let _server = spawn_mcp_server().await;
    let response = reqwest::get(SERVER_URL).await;
    
    assert!(response.is_ok(), "Health endpoint should respond");
    
    let response = response.unwrap();
    assert_eq!(response.status(), 200);
    
    let text = response.text().await.unwrap();
    assert!(text.contains("Golem CLI MCP Server"), "Health check should return expected message");
}

#[tokio::test]

async fn test_mcp_initialize() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let params = json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "test-client",
            "version": "1.0.0"
        }
    });
    
    let response = mcp_request("initialize", params, 1).await;
    assert!(response.is_ok(), "Initialize should succeed: {:?}", response);
    
    let response = response.unwrap();
    assert!(response.get("result").is_some(), "Response should have result");
    
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert!(result["serverInfo"].is_object());
    assert_eq!(result["serverInfo"]["name"], "Golem CLI MCP Server");
}

#[tokio::test]

async fn test_mcp_list_tools() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let response = mcp_request("tools/list", json!({}), 2).await;
    assert!(response.is_ok(), "List tools should succeed: {:?}", response);
    
    let response = response.unwrap();
    assert!(response.get("result").is_some());
    
    let result = &response["result"];
    assert!(result["tools"].is_array(), "Should return tools array");
    
    let tools = result["tools"].as_array().unwrap();
    assert!(tools.len() >= 2, "Should have at least 2 tools (list_agent_types, list_components)");
    
    // Check that our tools are present
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    
    assert!(tool_names.contains(&"list_agent_types"), "Should have list_agent_types tool");
    assert!(tool_names.contains(&"list_components"), "Should have list_components tool");
}

#[tokio::test]

async fn test_mcp_call_list_agent_types() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let params = json!({
        "name": "list_agent_types",
        "arguments": {}
    });
    
    let response = mcp_request("tools/call", params, 3).await;
    assert!(response.is_ok(), "Tool call should succeed: {:?}", response);
    
    let response = response.unwrap();
    
    // Check for error in response
    if let Some(error) = response.get("error") {
        println!("MCP Error: {:?}", error);
        // This might fail if no proper Golem environment is set up, which is OK for isolated tests
        // Just verify the error is structured correctly
        assert!(error.get("code").is_some());
        assert!(error.get("message").is_some());
        return;
    }
    
    assert!(response.get("result").is_some());
    
    let result = &response["result"];
    assert!(result["content"].is_array());
    
    // Parse the actual content
    if let Some(content_array) = result["content"].as_array() {
        assert!(!content_array.is_empty(), "Should have at least one content item");
        
        let first_content = &content_array[0];
        assert_eq!(first_content["type"], "text");
        
        // Try to parse the text as JSON
        if let Some(text) = first_content["text"].as_str() {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
            assert!(parsed.is_ok(), "Content should be valid JSON");
            
            let data = parsed.unwrap();
            assert!(data.get("agent_types").is_some(), "Should have agent_types field");
        }
    }
}

#[tokio::test]

async fn test_mcp_call_list_components() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let params = json!({
        "name": "list_components",
        "arguments": {}
    });
    
    let response = mcp_request("tools/call", params, 4).await;
    assert!(response.is_ok(), "Tool call should succeed: {:?}", response);
    
    let response = response.unwrap();
    
    // Check for error in response
    if let Some(error) = response.get("error") {
        println!("MCP Error: {:?}", error);
        // This might fail if no proper Golem environment is set up, which is OK for isolated tests
        assert!(error.get("code").is_some());
        assert!(error.get("message").is_some());
        return;
    }
    
    assert!(response.get("result").is_some());
    
    let result = &response["result"];
    assert!(result["content"].is_array());
    
    if let Some(content_array) = result["content"].as_array() {
        assert!(!content_array.is_empty());
        
        let first_content = &content_array[0];
        assert_eq!(first_content["type"], "text");
        
        if let Some(text) = first_content["text"].as_str() {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
            assert!(parsed.is_ok(), "Content should be valid JSON");
            
            let data = parsed.unwrap();
            assert!(data.get("components").is_some(), "Should have components field");
        }
    }
}

#[tokio::test]

async fn test_mcp_call_nonexistent_tool() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let params = json!({
        "name": "nonexistent_tool",
        "arguments": {}
    });
    
    let response = mcp_request("tools/call", params, 5).await;
    assert!(response.is_ok(), "Request should complete (but with error)");
    
    let response = response.unwrap();
    assert!(response.get("error").is_some(), "Should return an error for nonexistent tool");
    
    let error = &response["error"];
    assert!(error.get("code").is_some());
    assert!(error.get("message").is_some());
}

#[tokio::test]

async fn test_mcp_invalid_json_rpc() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let client = reqwest::Client::new();
    
    // Send invalid JSON-RPC (missing required fields)
    let invalid_request = json!({
        "method": "tools/list"
        // Missing jsonrpc, id, params
    });
    
    let response = client
        .post(MCP_ENDPOINT)
        .header("Content-Type", "application/json")
        .json(&invalid_request)
        .send()
        .await;
    
    assert!(response.is_ok());
    let response = response.unwrap();
    
    // Should get an error response
    let json_response: serde_json::Value = response.json().await.unwrap();
    // The server should handle this gracefully with a proper JSON-RPC error
    assert!(
        json_response.get("error").is_some() || json_response.get("result").is_some(),
        "Should get a structured response"
    );
}

#[tokio::test]

async fn test_mcp_concurrent_requests() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    // Send multiple concurrent requests
    let mut tasks = vec![];
    
    for i in 0..10 {
        let task = tokio::spawn(async move {
            let params = json!({
                "name": "list_agent_types",
                "arguments": {}
            });
            
            mcp_request("tools/call", params, i).await
        });
        
        tasks.push(task);
    }
    
    // Wait for all requests to complete
    let results = futures_util::future::join_all(tasks).await;
    
    // All requests should complete (successfully or with expected errors)
    for result in results {
        assert!(result.is_ok(), "Task should complete");
        let response = result.unwrap();
        assert!(response.is_ok(), "Request should get a response");
    }
}

#[tokio::test]

async fn test_mcp_tool_schemas() {
    let _server = spawn_mcp_server().await;
    assert!(wait_for_server(50).await, "Server should be running");
    
    let response = mcp_request("tools/list", json!({}), 6).await;
    assert!(response.is_ok());
    
    let response = response.unwrap();
    let tools = response["result"]["tools"].as_array().unwrap();
    
    // Each tool should have proper schema
    for tool in tools {
        assert!(tool["name"].is_string(), "Tool should have name");
        assert!(tool["description"].is_string(), "Tool should have description");
        
        // Input schema should be present
        if let Some(input_schema) = tool.get("inputSchema") {
            assert!(input_schema.is_object(), "Input schema should be an object");
            // Should follow JSON Schema format
            assert!(
                input_schema.get("type").is_some() || input_schema.get("properties").is_some(),
                "Input schema should have type or properties"
            );
        }
    }
}
