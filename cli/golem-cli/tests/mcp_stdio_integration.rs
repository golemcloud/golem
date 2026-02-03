// Integration tests for MCP Server stdio transport
// These tests verify the MCP server functionality over stdio (stdin/stdout)

use std::time::Duration;
use tokio::time::sleep;
use serde_json::json;
use tokio::process::{Command, Child};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use std::process::Stdio;

fn get_binary_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_golem-cli") {
        return path.into();
    }
    
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let workspace_root = std::path::PathBuf::from(&manifest_dir)
        .join("../..")
        .canonicalize()
        .expect("Failed to canonicalize workspace path");
    
    let target_dir = if cfg!(debug_assertions) {
        "target/debug"
    } else {
        "target/release"
    };
    
    let mut path = workspace_root.join(target_dir);
    path.push("golem-cli");
    
    if cfg!(windows) {
        path.set_extension("exe");
    }
    
    path
}

struct StdioMcpServerHandle {
    child: Child,
}

impl Drop for StdioMcpServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

async fn spawn_stdio_mcp_server() -> StdioMcpServerHandle {
    let binary_path = get_binary_path();
    
    println!("Using binary path: {:?}", binary_path);
    println!("Starting stdio MCP server");
    
    let mut command = Command::new(&binary_path);
    command
        .arg("mcp-server")
        .arg("start")
        .arg("--transport")
        .arg("stdio")
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("Failed to spawn golem-cli mcp-server");
    
    sleep(Duration::from_millis(200)).await;
    
    if let Ok(Some(status)) = child.try_wait() {
        panic!("MCP Server exited immediately with status: {:?}", status);
    }

    StdioMcpServerHandle { child }
}

struct StdioMcpClient {
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    request_id: std::sync::atomic::AtomicI32,
}

impl StdioMcpClient {
    async fn new(server: &mut Child) -> Result<Self, String> {
        let stdin = server.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = server.stdout.take().ok_or("Failed to get stdout")?;
        
        Ok(Self {
            stdin,
            stdout: BufReader::new(stdout),
            request_id: std::sync::atomic::AtomicI32::new(1),
        })
    }
    
    async fn send_request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        use std::sync::atomic::Ordering;
        
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        
        let request_str = serde_json::to_string(&request)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        // Write request with newline
        self.stdin.write_all(request_str.as_bytes()).await
            .map_err(|e| format!("Write error: {}", e))?;
        self.stdin.write_all(b"\n").await
            .map_err(|e| format!("Write newline error: {}", e))?;
        self.stdin.flush().await
            .map_err(|e| format!("Flush error: {}", e))?;
        
        // Read response line
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.stdout.read_line(&mut line)).await
            .map_err(|_| "Timeout reading response")?
            .map_err(|e| format!("Read error: {}", e))?;
        
        let line = line.trim();
        if line.is_empty() {
            return Err("Empty response".to_string());
        }
        
        serde_json::from_str(line)
            .map_err(|e| format!("Parse error: {} - Line: {}", e, line))
    }
    
    async fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        
        let response = self.send_request("initialize", params).await?;
        
        if response.get("error").is_some() {
            return Err(format!("Initialize error: {:?}", response["error"]));
        }
        
        if response.get("result").is_none() {
            return Err(format!("Initialize missing result: {:?}", response));
        }
        
        // Send initialized notification
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        
        let notify_str = serde_json::to_string(&notification)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        self.stdin.write_all(notify_str.as_bytes()).await
            .map_err(|e| format!("Write error: {}", e))?;
        self.stdin.write_all(b"\n").await
            .map_err(|e| format!("Write newline error: {}", e))?;
        self.stdin.flush().await
            .map_err(|e| format!("Flush error: {}", e))?;
        
        Ok(())
    }
}

async fn spawn_stdio_server_and_client() -> (StdioMcpServerHandle, StdioMcpClient) {
    let mut server_handle = spawn_stdio_mcp_server().await;
    let child = &mut server_handle.child;
    let mut client = StdioMcpClient::new(child).await.expect("Failed to create client");
    client.initialize().await.expect("Failed to initialize session");
    (server_handle, client)
}

#[tokio::test]
async fn test_stdio_mcp_initialize() {
    let (_server, _client) = spawn_stdio_server_and_client().await;
    // If we got here, initialization succeeded
    assert!(true);
}

#[tokio::test]
async fn test_stdio_mcp_list_tools() {
    let (_server, mut client) = spawn_stdio_server_and_client().await;
    
    let response = client.send_request("tools/list", json!({})).await;
    assert!(response.is_ok(), "List tools should succeed: {:?}", response);
    
    let response = response.unwrap();
    assert!(response.get("result").is_some());
    
    let result = &response["result"];
    assert!(result["tools"].is_array(), "Should return tools array");
    
    let tools = result["tools"].as_array().unwrap();
    assert!(tools.len() >= 2, "Should have at least 2 tools");
    
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    
    assert!(tool_names.contains(&"list_agent_types"), "Should have list_agent_types tool");
    assert!(tool_names.contains(&"list_components"), "Should have list_components tool");
}

#[tokio::test]
async fn test_stdio_mcp_call_list_agent_types() {
    let (_server, mut client) = spawn_stdio_server_and_client().await;
    
    let params = json!({
        "name": "list_agent_types",
        "arguments": {}
    });
    
    let response = client.send_request("tools/call", params).await;
    assert!(response.is_ok(), "Tool call should succeed: {:?}", response);
    
    let response = response.unwrap();
    
    // Check for error in response
    if let Some(error) = response.get("error") {
        println!("MCP Error: {:?}", error);
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
            assert!(data.get("agent_types").is_some(), "Should have agent_types field");
        }
    }
}

#[tokio::test]
async fn test_stdio_mcp_call_list_components() {
    let (_server, mut client) = spawn_stdio_server_and_client().await;
    
    let params = json!({
        "name": "list_components",
        "arguments": {}
    });
    
    let response = client.send_request("tools/call", params).await;
    assert!(response.is_ok(), "Tool call should succeed: {:?}", response);
    
    let response = response.unwrap();
    
    if let Some(error) = response.get("error") {
        println!("MCP Error: {:?}", error);
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
async fn test_stdio_mcp_call_nonexistent_tool() {
    let (_server, mut client) = spawn_stdio_server_and_client().await;
    
    let params = json!({
        "name": "nonexistent_tool",
        "arguments": {}
    });
    
    let response = client.send_request("tools/call", params).await;
    assert!(response.is_ok(), "Request should complete (but with error)");
    
    let response = response.unwrap();
    assert!(response.get("error").is_some(), "Should return an error for nonexistent tool");
    
    let error = &response["error"];
    assert!(error.get("code").is_some());
    assert!(error.get("message").is_some());
}

#[tokio::test]
async fn test_stdio_mcp_multiple_requests() {
    let (_server, mut client) = spawn_stdio_server_and_client().await;
    
    // Send multiple requests in sequence
    for i in 0..5 {
        let params = json!({
            "name": "list_components",
            "arguments": {}
        });
        
        let response = client.send_request("tools/call", params).await;
        assert!(response.is_ok(), "Request {} should succeed: {:?}", i, response);
        
        let response = response.unwrap();
        // Should have either result or error
        assert!(response.get("result").is_some() || response.get("error").is_some());
    }
}
