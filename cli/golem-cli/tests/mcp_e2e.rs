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

//! End-to-end tests for the MCP server functionality.
//!
//! These tests start the CLI in serve mode and verify that MCP tools and resources
//! work correctly via HTTP.
//!
//! @ai_prompt These tests verify MCP server functionality without requiring Golem Cloud credentials
//! @context_boundary E2E test module for MCP server

use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

/// Test port for MCP server (use a high port to avoid conflicts)
const TEST_PORT: u16 = 19232;

/// Timeout for waiting for server to start
const SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for individual tool calls
const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Helper struct to manage the MCP server process
struct McpServerProcess {
    child: Child,
    port: u16,
    _temp_dir: TempDir,
}

impl McpServerProcess {
    /// Start the MCP server in a temporary directory with test manifests
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let workdir = temp_dir.path().join("workdir");
        fs::create_dir_all(&workdir)?;

        // Create test manifest files
        Self::create_test_manifests(&temp_dir)?;

        // Get the path to the golem-cli binary
        let bin_path = Self::get_binary_path()?;

        // Start the server
        let child = Command::new(&bin_path)
            .args(["--serve", "--serve-port", &TEST_PORT.to_string()])
            .current_dir(&workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let server = Self {
            child,
            port: TEST_PORT,
            _temp_dir: temp_dir,
        };

        // Wait for server to be ready
        server.wait_for_ready()?;

        Ok(server)
    }

    /// Create test manifest files in the temp directory
    fn create_test_manifests(temp_dir: &TempDir) -> Result<(), Box<dyn std::error::Error>> {
        let root_manifest = temp_dir.path().join("golem.yaml");
        let mut file = fs::File::create(&root_manifest)?;
        writeln!(file, "# Root manifest for testing")?;
        writeln!(file, "name: test-app")?;
        writeln!(file, "version: 1.0.0")?;

        let child_dir = temp_dir.path().join("workdir").join("child-component");
        fs::create_dir_all(&child_dir)?;
        let child_manifest = child_dir.join("golem.yaml");
        let mut file = fs::File::create(&child_manifest)?;
        writeln!(file, "# Child manifest for testing")?;
        writeln!(file, "name: child-component")?;
        writeln!(file, "version: 0.1.0")?;

        let workdir_manifest = temp_dir.path().join("workdir").join("golem.yaml");
        let mut file = fs::File::create(&workdir_manifest)?;
        writeln!(file, "# Workdir manifest for testing")?;
        writeln!(file, "name: workdir-component")?;
        writeln!(file, "version: 0.2.0")?;

        Ok(())
    }

    /// Get the path to the golem-cli binary
    fn get_binary_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        // Try CARGO_BIN_EXE first (set during cargo test)
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_golem-cli") {
            return Ok(PathBuf::from(path));
        }

        // Fallback to finding in target directory
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
        let target_dir = PathBuf::from(manifest_dir)
            .parent()
            .and_then(|p| p.parent())
            .ok_or("Cannot find target directory")?
            .join("target")
            .join("debug")
            .join("golem-cli");

        if target_dir.exists() {
            Ok(target_dir)
        } else {
            Err("golem-cli binary not found".into())
        }
    }

    /// Wait for the server to be ready to accept connections
    fn wait_for_ready(&self) -> Result<(), Box<dyn std::error::Error>> {
        let start = std::time::Instant::now();
        let addr = format!("127.0.0.1:{}", self.port);

        while start.elapsed() < SERVER_STARTUP_TIMEOUT {
            if TcpStream::connect(&addr).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Err(format!("Server did not start within {:?}", SERVER_STARTUP_TIMEOUT).into())
    }

    /// Get the MCP endpoint URL
    fn endpoint_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

impl Drop for McpServerProcess {
    fn drop(&mut self) {
        // Kill the server process
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Simple HTTP client for MCP JSON-RPC requests
struct McpClient {
    endpoint: String,
    client: reqwest::blocking::Client,
    request_id: i64,
}

impl McpClient {
    fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            client: reqwest::blocking::Client::builder()
                .timeout(TOOL_CALL_TIMEOUT)
                .build()
                .expect("Failed to create HTTP client"),
            request_id: 0,
        }
    }

    /// Send a JSON-RPC request and get the result
    fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.request_id += 1;

        let request_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.request_id,
            "method": method,
            "params": params.unwrap_or(serde_json::json!({}))
        });

        let response = self
            .client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .body(request_body.to_string())
            .send()?;

        let response_body: serde_json::Value = response.json()?;

        if let Some(error) = response_body.get("error") {
            return Err(format!("JSON-RPC error: {}", error).into());
        }

        Ok(response_body
            .get("result")
            .cloned()
            .unwrap_or(serde_json::json!(null)))
    }

    /// Initialize the MCP session
    fn initialize(&mut self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.request(
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            })),
        )
    }

    /// List available tools
    fn list_tools(&mut self) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
        let result = self.request("tools/list", None)?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools)
    }

    /// Call a tool with the given arguments
    fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.request(
            "tools/call",
            Some(serde_json::json!({
                "name": name,
                "arguments": args
            })),
        )
    }

    /// List available resources
    fn list_resources(&mut self) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
        let result = self.request("resources/list", None)?;
        let resources = result
            .get("resources")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(resources)
    }

    /// Read a resource by URI
    fn read_resource(
        &mut self,
        uri: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.request(
            "resources/read",
            Some(serde_json::json!({
                "uri": uri
            })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the MCP server starts and responds to initialize
    #[test]
    #[ignore = "requires compiled binary"]
    fn test_mcp_server_initialize() {
        let server = McpServerProcess::start().expect("Failed to start MCP server");
        let mut client = McpClient::new(server.endpoint_url());

        let result = client.initialize().expect("Failed to initialize");

        assert!(result.get("protocolVersion").is_some());
        assert!(result.get("capabilities").is_some());
        assert!(result.get("serverInfo").is_some());
    }

    /// Test that tools/list returns a list of tools
    #[test]
    #[ignore = "requires compiled binary"]
    fn test_mcp_tools_list() {
        let server = McpServerProcess::start().expect("Failed to start MCP server");
        let mut client = McpClient::new(server.endpoint_url());

        client.initialize().expect("Failed to initialize");
        let tools = client.list_tools().expect("Failed to list tools");

        // Should have at least some tools
        assert!(!tools.is_empty(), "Expected at least one tool");

        // Each tool should have name and description
        for tool in &tools {
            assert!(
                tool.get("name").is_some(),
                "Tool should have a name: {:?}",
                tool
            );
        }
    }

    /// Test that each tool can be called with --help
    #[test]
    #[ignore = "requires compiled binary"]
    fn test_mcp_tools_help() {
        let server = McpServerProcess::start().expect("Failed to start MCP server");
        let mut client = McpClient::new(server.endpoint_url());

        client.initialize().expect("Failed to initialize");
        let tools = client.list_tools().expect("Failed to list tools");

        // Test a subset of tools to keep test time reasonable
        let tools_to_test: Vec<_> = tools.iter().take(5).collect();

        for tool in tools_to_test {
            let name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .expect("Tool should have a name");

            let result = client
                .call_tool(name, serde_json::json!({"args": ["--help"]}))
                .expect(&format!("Failed to call tool: {}", name));

            // Tool should return content
            let content = result.get("content");
            assert!(
                content.is_some(),
                "Tool {} should return content: {:?}",
                name,
                result
            );
        }
    }

    /// Test that resources/list returns manifests
    #[test]
    #[ignore = "requires compiled binary"]
    fn test_mcp_resources_list() {
        let server = McpServerProcess::start().expect("Failed to start MCP server");
        let mut client = McpClient::new(server.endpoint_url());

        client.initialize().expect("Failed to initialize");
        let resources = client.list_resources().expect("Failed to list resources");

        // Should have manifest files
        assert!(
            !resources.is_empty(),
            "Expected at least one manifest resource"
        );

        // Each resource should have uri and name
        for resource in &resources {
            assert!(
                resource.get("uri").is_some(),
                "Resource should have a uri: {:?}",
                resource
            );
            assert!(
                resource.get("name").is_some(),
                "Resource should have a name: {:?}",
                resource
            );
        }
    }

    /// Test that resources can be read
    #[test]
    #[ignore = "requires compiled binary"]
    fn test_mcp_resources_read() {
        let server = McpServerProcess::start().expect("Failed to start MCP server");
        let mut client = McpClient::new(server.endpoint_url());

        client.initialize().expect("Failed to initialize");
        let resources = client.list_resources().expect("Failed to list resources");

        // Read each resource
        for resource in &resources {
            let uri = resource
                .get("uri")
                .and_then(|u| u.as_str())
                .expect("Resource should have a uri");

            let result = client
                .read_resource(uri)
                .expect(&format!("Failed to read resource: {}", uri));

            // Should return contents
            let contents = result.get("contents");
            assert!(
                contents.is_some(),
                "Resource {} should return contents: {:?}",
                uri,
                result
            );
        }
    }
}
