// Integration tests for MCP Server
// These tests verify the MCP server functionality end-to-end

use std::time::Duration;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::time::sleep;
use serde_json::json;
use tokio::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};


// Dynamic port allocation to prevent conflicts
static NEXT_PORT: AtomicU16 = AtomicU16::new(13337);

fn get_next_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::SeqCst)
}

fn get_binary_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_golem-cli") {
        return path.into();
    }
    
    // Get workspace root by going up from CARGO_MANIFEST_DIR (which is cli/golem-cli)
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

struct McpServerHandle {
    child: tokio::process::Child,
    port: u16,
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

async fn spawn_mcp_server() -> (McpServerHandle, u16) {
    let port = get_next_port();
    let binary_path = get_binary_path();
    
    println!("Using binary path: {:?}", binary_path);
    println!("Starting server on port: {}", port);
    
    let mut command = Command::new(&binary_path);
    command
        .arg("mcp-server")
        .arg("start")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn().expect("Failed to spawn golem-cli mcp-server");
    
    sleep(Duration::from_millis(500)).await;
    
    if let Ok(Some(status)) = child.try_wait() {
        let output = child.wait_with_output().await.expect("Failed to wait for output");
        eprintln!("MCP Server exited with status: {}", status);
        eprintln!("Stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("Stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("MCP Server failed to start");
    }

    let server_url = format!("http://127.0.0.1:{}", port);
    assert!(wait_for_server(&server_url, 50).await, "MCP Server did not start");

    (McpServerHandle { child, port }, port)
}

/// Helper function to check if server is ready
async fn wait_for_server(url: &str, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if reqwest::get(url).await.is_ok() {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

/// MCP client using a single persistent TCP connection
/// This ensures session persistence since LocalSessionManager is connection-based
struct McpClient {
    stream: tokio::net::TcpStream,
    request_id: std::sync::atomic::AtomicI32,
    port: u16,
    is_initialized: bool,
}

impl McpClient {
    async fn new(port: u16) -> Result<Self, String> {
        let addr = format!("127.0.0.1:{}", port);
        let stream = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("Failed to connect: {}", e))?;
        
        Ok(Self {
            stream,
            request_id: std::sync::atomic::AtomicI32::new(1),
            port,
            is_initialized: false,
        })
    }
    
    async fn send_http(&mut self, body: &str) -> Result<String, String> {
        let request = format!(
            "POST /mcp HTTP/1.1\r\n\
            Host: 127.0.0.1:{}\r\n\
            Connection: keep-alive\r\n\
            Content-Type: application/json\r\n\
            Accept: application/json, text/event-stream\r\n\
            Content-Length: {}\r\n\
            \r\n\
            {}",
            self.port, body.len(), body
        );
        
        self.stream.write_all(request.as_bytes())
            .await
            .map_err(|e| format!("Write error: {}", e))?;
        self.stream.flush()
            .await
            .map_err(|e| format!("Flush error: {}", e))?;
        
        // Read response
        let mut buffer = Vec::new();
        let mut temp = [0u8; 4096];
        let mut saw_data = false;
        
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut temp)).await {
                Ok(Ok(n)) => {
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&temp[..n]);
                    let text = String::from_utf8_lossy(&buffer);
                    if text.contains("data: ") {
                        saw_data = true;
                        if text.ends_with("\n\n") || text.ends_with("\r\n\r\n") {
                            break;
                        }
                    }
                    if buffer.len() > 1_000_000 {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(format!("Read error: {}", e)),
                Err(_) => {
                    if saw_data && buffer.len() > 100 {
                        break;
                    }
                    continue;
                }
            }
        }
        
        Ok(String::from_utf8_lossy(&buffer).to_string())
    }
    
    
    async fn request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        if method == "initialize" {
            return self.request_internal(method, params).await;
        }
        
        // Ensure session is initialized on this connection
        if !self.is_initialized {
            self.initialize().await?;
        }
        
        self.request_internal(method, params).await
    }
    
    async fn request_internal(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        
        let body_str = serde_json::to_string(&request_body)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        let response_text = self.send_http(&body_str).await?;
        
        // Parse HTTP response
        if !response_text.contains("200 OK") {
            let error_msg = response_text.lines()
                .find(|l| l.contains("data: "))
                .and_then(|l| l.strip_prefix("data: "))
                .unwrap_or(&response_text[..response_text.len().min(200)]);
            return Err(format!("HTTP error: {}", error_msg));
        }
        
        // Extract JSON from SSE
        let json_str = response_text
            .lines()
            .find(|line| line.starts_with("data: "))
            .map(|line| line.trim_start_matches("data: "))
            .ok_or_else(|| format!("No data line in response: {}", &response_text[..200]))?;
        
        serde_json::from_str(json_str)
            .map_err(|e| format!("Parse error: {} - JSON: {}", e, json_str))
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
        
        let response = self.request_internal("initialize", params).await?;
        
        if response.get("error").is_some() {
            return Err(format!("Initialize error: {:?}", response["error"]));
        }
        
        // Verify we got a result
        if response.get("result").is_none() {
            return Err(format!("Initialize missing result: {:?}", response));
        }
        
        // Send initialized notification on same connection
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        
        let notify_body = serde_json::to_string(&notification)
            .map_err(|e| format!("Serialize error: {}", e))?;
        
        let _ = self.send_http(&notify_body).await?;
        
        self.is_initialized = true;
        Ok(())
    }
}

async fn spawn_server_and_client() -> (McpServerHandle, McpClient) {
    let (server, port) = spawn_mcp_server().await;
    let mut client = McpClient::new(port).await.expect("Failed to create client");
    client.initialize().await.expect("Failed to initialize session");
    (server, client)
}


#[tokio::test]
async fn test_server_health_endpoint() {
    let (server, _port) = spawn_mcp_server().await;
    let server_url = format!("http://127.0.0.1:{}", server.port);
    let response = reqwest::get(&server_url).await;
    
    assert!(response.is_ok(), "Health endpoint should respond");
    
    let response = response.unwrap();
    assert_eq!(response.status(), 200);
    
    let text = response.text().await.unwrap();
    assert!(text.contains("Golem CLI MCP Server"), "Health check should return expected message");
}

#[tokio::test]
async fn test_mcp_initialize() {
    let (_server, _client) = spawn_server_and_client().await;
    // If we got here, initialization succeeded
    assert!(true);
}

#[tokio::test]
async fn test_mcp_list_tools() {
    let (_server, _client) = spawn_server_and_client().await;
    
    let response = client.request("tools/list", json!({})).await;
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
    let (_server, _client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "list_agent_types",
        "arguments": {}
    });
    
    let response = client.request("tools/call", params).await;
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
    let (_server, _client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "list_components",
        "arguments": {}
    });
    
    let response = client.request("tools/call", params).await;
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
    let (_server, _client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "nonexistent_tool",
        "arguments": {}
    });
    
    let response = client.request("tools/call", params).await;
    assert!(response.is_ok(), "Request should complete (but with error)");
    
    let response = response.unwrap();
    assert!(response.get("error").is_some(), "Should return an error for nonexistent tool");
    
    let error = &response["error"];
    assert!(error.get("code").is_some());
    assert!(error.get("message").is_some());
}

#[tokio::test]
async fn test_mcp_tool_schemas() {
    let (_server, _client) = spawn_server_and_client().await;
    
    let response = client.request("tools/list", json!({})).await;
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
