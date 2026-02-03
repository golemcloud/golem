// Integration tests for MCP Server
// These tests verify the MCP server functionality end-to-end.

use std::sync::atomic::{AtomicI32, AtomicU16, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use serde_json::json;


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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = command.spawn().expect("Failed to spawn golem-cli mcp-server");
    
    sleep(Duration::from_millis(800)).await;  // Allow server to bind and accept
    
    if let Ok(Some(status)) = child.try_wait() {
        let _ = child.wait().await;
        eprintln!("MCP Server exited early with status: {}", status);
        panic!("MCP Server failed to start");
    }

    let server_url = format!("http://127.0.0.1:{}", port);
    assert!(wait_for_server(&server_url, 60).await, "MCP Server did not start");

    (McpServerHandle { child, port }, port)
}

/// Helper function to check if server is ready
async fn wait_for_server(url: &str, max_attempts: u32) -> bool {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(2))
        .build()
        .expect("reqwest client");
    for _ in 0..max_attempts {
        if let Ok(r) = client.get(url).send().await {
            if r.status().is_success() {
                return true;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

/// MCP client using reqwest. Uses mcp-session-id from initialize response for subsequent requests
/// since the server tracks session by connection and we use a new TCP connection per request.
struct ReqwestMcpClient {
    client: reqwest::Client,
    base_url: String,
    request_id: AtomicI32,
    session_id: std::sync::Mutex<Option<String>>,
    is_initialized: bool,
}

impl ReqwestMcpClient {
    fn new(port: u16) -> Self {
        let base_url = format!("http://127.0.0.1:{}/mcp", port);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self {
            client,
            base_url,
            request_id: AtomicI32::new(1),
            session_id: std::sync::Mutex::new(None),
            is_initialized: false,
        }
    }

    fn build_request(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.base_url)
            .json(body)
            .header("Accept", "application/json, text/event-stream");
        if let Ok(guard) = self.session_id.lock() {
            if let Some(ref id) = *guard {
                req = req.header("mcp-session-id", id.as_str());
            }
        }
        req
    }

    async fn post_mcp(&self, body: &serde_json::Value) -> Result<(Option<String>, String), String> {
        let resp = self
            .build_request(body)
            .send()
            .await
            .map_err(|e| format!("POST error: {}", e))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.map_err(|e| format!("body error: {}", e))?;
        if !status.is_success() {
            return Err(format!(
                "HTTP {}: {}",
                status,
                text.lines().next().unwrap_or("")
            ));
        }
        let sid = headers
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        Ok((sid, text))
    }

    async fn request_internal(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let (sid, text) = self.post_mcp(&body).await?;
        if let Some(s) = sid {
            if let Ok(mut g) = self.session_id.lock() {
                *g = Some(s);
            }
        }
        let json_str = text
            .lines()
            .find(|line| line.starts_with("data: "))
            .and_then(|line| {
                let s = line.trim_start_matches("data: ").trim();
                if s.is_empty() { None } else { Some(s) }
            })
            .ok_or_else(|| format!("No data line in response: {}", &text[..text.len().min(300)]))?;
        serde_json::from_str(json_str).map_err(|e| format!("Parse error: {} - {}", e, json_str))
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        });
        let response = self.request_internal("initialize", params).await?;
        if response.get("error").is_some() {
            return Err(format!("Initialize error: {:?}", response["error"]));
        }
        if response.get("result").is_none() {
            return Err(format!("Initialize missing result: {:?}", response));
        }
        let notify = json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} });
        let _ = self.post_mcp(&notify).await?;
        self.is_initialized = true;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        if method == "initialize" {
            return self.request_internal(method, params).await;
        }
        if !self.is_initialized {
            self.initialize().await?;
        }
        self.request_internal(method, params).await
    }
}

async fn spawn_server_and_client() -> (McpServerHandle, ReqwestMcpClient) {
    let (server, port) = spawn_mcp_server().await;
    let mut client = ReqwestMcpClient::new(port);
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
}

#[tokio::test]
async fn test_mcp_list_tools() {
    let (_server, mut client) = spawn_server_and_client().await;
    
    let response: Result<serde_json::Value, String> = client.request("tools/list", json!({})).await;
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
    let (_server, mut client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "list_agent_types",
        "arguments": {}
    });
    
    let response: Result<serde_json::Value, String> = client.request("tools/call", params).await;
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
    let (_server, mut client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "list_components",
        "arguments": {}
    });
    
    let response: Result<serde_json::Value, String> = client.request("tools/call", params).await;
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
    let (_server, mut client) = spawn_server_and_client().await;
    
    let params = json!({
        "name": "nonexistent_tool",
        "arguments": {}
    });
    
    let response: Result<serde_json::Value, String> = client.request("tools/call", params).await;
    assert!(response.is_ok(), "Request should complete (but with error)");
    
    let response = response.unwrap();
    assert!(response.get("error").is_some(), "Should return an error for nonexistent tool");
    
    let error = &response["error"];
    assert!(error.get("code").is_some());
    assert!(error.get("message").is_some());
}

#[tokio::test]
async fn test_mcp_tool_schemas() {
    let (_server, mut client) = spawn_server_and_client().await;
    
    let response: Result<serde_json::Value, String> = client.request("tools/list", json!({})).await;
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
