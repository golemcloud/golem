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

//! MCP (Model Context Protocol) Server Implementation
//!
//! This module implements a JSON-RPC 2.0 server over STDIO that exposes
//! Golem CLI functionality as MCP tools and resources.

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::environment::EnvironmentResolveMode;
use anyhow::{anyhow, Result};
use golem_client::api::{ComponentClient, WorkerClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

pub struct McpCommandHandler {
    ctx: Arc<Context>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Option<Value>,
    id: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl McpCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    /// Run the MCP server over STDIO
    /// This blocks forever, reading JSON-RPC requests from stdin and writing responses to stdout
    pub async fn run_server(&self) -> Result<()> {
        // Log to stderr, never stdout (stdout is for MCP protocol only)
        eprintln!("golem-cli running MCP Server on stdio");
        eprintln!("Protocol: JSON-RPC 2.0 over STDIO");
        
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut reader = stdin.lock();
        let mut line = String::new();

        while reader.read_line(&mut line)? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
                Ok(req) => req,
                Err(e) => {
                    eprintln!("Failed to parse JSON-RPC request: {}", e);
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                        id: None,
                    };
                    writeln!(stdout, "{}", serde_json::to_string(&error_response)?)?;
                    stdout.flush()?;
                    line.clear();
                    continue;
                }
            };

            eprintln!("Received: {} (id: {:?})", request.method, request.id);

            if let Some(id) = request.id.clone() {
                let response = self.handle_request(&request.method, request.params, id).await;
                writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
                stdout.flush()?;
            } else {
                // Notification - no response needed
                self.handle_notification(&request.method, request.params).await;
            }

            line.clear();
        }

        Ok(())
    }

    async fn handle_request(&self, method: &str, params: Option<Value>, id: Value) -> JsonRpcResponse {
        match method {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(params, id).await,
            "resources/list" => self.handle_resources_list(id).await,
            "resources/read" => self.handle_resources_read(params, id).await,
            _ => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", method),
                    data: None,
                }),
                id: Some(id),
            },
        }
    }

    fn handle_initialize(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    },
                    "resources": {
                        "subscribe": false,
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "golem-cli",
                    "version": crate::version()
                }
            })),
            error: None,
            id: Some(id),
        }
    }

    fn handle_tools_list(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(json!({
                "tools": [
                    {
                        "name": "golem_component_list",
                        "description": "List all Golem components in the current environment",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        },
                        "outputSchema": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "component_id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "metadata": { "type": "object" }
                                },
                                "required": ["component_id", "name"]
                            }
                        }
                    },
                    {
                        "name": "golem_component_get",
                        "description": "Get metadata for a specific Golem component by ID",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "component_id": {
                                    "type": "string",
                                    "description": "The component UUID"
                                }
                            },
                            "required": ["component_id"]
                        },
                        "outputSchema": {
                            "type": "object",
                            "properties": {
                                "component_id": { "type": "string" },
                                "name": { "type": "string" },
                                "metadata": { "type": "object" }
                            },
                            "required": ["component_id", "name"]
                        }
                    },
                    {
                        "name": "golem_worker_list",
                        "description": "List all workers for a given component",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "component_id": {
                                    "type": "string",
                                    "description": "The component UUID"
                                }
                            },
                            "required": ["component_id"]
                        },
                        "outputSchema": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "worker_name": { "type": "string" },
                                    "status": { "type": "string" },
                                    "metadata": { "type": "object" }
                                },
                                "required": ["worker_name"]
                            }
                        }
                    },
                    {
                        "name": "golem_worker_get",
                        "description": "Get metadata for a specific worker",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "component_id": {
                                    "type": "string",
                                    "description": "The component UUID"
                                },
                                "worker_name": {
                                    "type": "string",
                                    "description": "The worker name"
                                }
                            },
                            "required": ["component_id", "worker_name"]
                        },
                        "outputSchema": {
                            "type": "object",
                            "properties": {
                                "worker_name": { "type": "string" },
                                "status": { "type": "string" },
                                "metadata": { "type": "object" }
                            },
                            "required": ["worker_name"]
                        }
                    }
                ]
            })),
            error: None,
            id: Some(id),
        }
    }

    async fn handle_tools_call(&self, params: Option<Value>, id: Value) -> JsonRpcResponse {
        let Some(params) = params else {
            return self.error_response(-32602, "Missing params", id);
        };

        let tool_name = params.get("name").and_then(|v| v.as_str());
        let arguments = params.get("arguments");

        match tool_name {
            Some("golem_component_list") => {
                match self.tool_component_list().await {
                    Ok(res) => self.success_response(res, id),
                    Err(e) => self.error_response(-32603, format!("{}", e), id),
                }
            }
            Some("golem_component_get") => {
                let component_id = arguments.and_then(|a| a.get("component_id")).and_then(|v| v.as_str());
                match component_id {
                    Some(cid) => match self.tool_component_get(cid).await {
                        Ok(res) => self.success_response(res, id),
                        Err(e) => self.error_response(-32603, format!("{}", e), id),
                    },
                    None => self.error_response(-32602, "Missing required parameter: component_id", id),
                }
            }
            Some("golem_worker_list") => {
                let component_id = arguments.and_then(|a| a.get("component_id")).and_then(|v| v.as_str());
                match component_id {
                    Some(cid) => match self.tool_worker_list(cid).await {
                        Ok(res) => self.success_response(res, id),
                        Err(e) => self.error_response(-32603, format!("{}", e), id),
                    },
                    None => self.error_response(-32602, "Missing required parameter: component_id", id),
                }
            }
            Some("golem_worker_get") => {
                let component_id = arguments.and_then(|a| a.get("component_id")).and_then(|v| v.as_str());
                let worker_name = arguments.and_then(|a| a.get("worker_name")).and_then(|v| v.as_str());
                match (component_id, worker_name) {
                    (Some(cid), Some(wn)) => match self.tool_worker_get(cid, wn).await {
                        Ok(res) => self.success_response(res, id),
                        Err(e) => self.error_response(-32603, format!("{}", e), id),
                    },
                    _ => self.error_response(-32602, "Missing required parameters: component_id and worker_name", id),
                }
            }
            Some(name) => self.error_response(-32602, format!("Unknown tool: {}", name), id),
            None => self.error_response(-32602, "Missing tool name", id),
        }
    }

    async fn handle_resources_list(&self, id: Value) -> JsonRpcResponse {
        match self.list_resources() {
            Ok(resources) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(json!({ 
                    "resources": resources,
                    "nextCursor": null
                })),
                error: None,
                id: Some(id),
            },
            Err(e) => self.error_response(-32603, format!("{}", e), id),
        }
    }

    async fn handle_resources_read(&self, params: Option<Value>, id: Value) -> JsonRpcResponse {
        let uri = params
            .as_ref()
            .and_then(|p| p.get("uri"))
            .and_then(|v| v.as_str());

        match uri {
            Some(uri) => match self.read_resource(uri) {
                Ok(content) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(content),
                    error: None,
                    id: Some(id),
                },
                Err(e) => self.error_response(-32603, format!("{}", e), id),
            },
            None => self.error_response(-32602, "Missing required parameter: uri", id),
        }
    }

    async fn handle_notification(&self, method: &str, _params: Option<Value>) {
        eprintln!("Notification: {}", method);
    }

    // --- Tool Implementations ---

    async fn tool_component_list(&self) -> Result<Value> {
        let clients = self.ctx.golem_clients().await?;
        let environment = self.ctx.environment_handler().resolve_environment(EnvironmentResolveMode::Any).await?;
        
        let page = clients.component.get_environment_components(&environment.environment_id.0).await
            .map_err(|e| anyhow!("{}", e))?;

        // Return array of components directly
        Ok(serde_json::to_value(&page.data)?)
    }

    async fn tool_component_get(&self, component_id: &str) -> Result<Value> {
        let clients = self.ctx.golem_clients().await?;
        let component_uuid = uuid::Uuid::parse_str(component_id)?;
        let component = clients.component.get_component(&component_uuid).await
            .map_err(|e| anyhow!("{}", e))?;

        // Return component object directly
        Ok(serde_json::to_value(&component)?)
    }

    async fn tool_worker_list(&self, component_id: &str) -> Result<Value> {
        let clients = self.ctx.golem_clients().await?;
        let component_uuid = uuid::Uuid::parse_str(component_id)?;
        
        let page = clients.worker.get_workers_metadata(&component_uuid, None, None, None, None).await
            .map_err(|e| anyhow!("{}", e))?;
        
        // Return array of workers directly
        Ok(serde_json::to_value(&page.data)?)
    }

    async fn tool_worker_get(&self, component_id: &str, worker_name: &str) -> Result<Value> {
        let clients = self.ctx.golem_clients().await?;
        let component_uuid = uuid::Uuid::parse_str(component_id)?;
        
        let worker = clients.worker.get_worker_metadata(&component_uuid, worker_name).await
            .map_err(|e| anyhow!("{}", e))?;
        
        // Return worker object directly
        Ok(serde_json::to_value(&worker)?)
    }

    // --- Resource Implementations ---

    /// Convert a file path to a canonical file:// URI
    fn path_to_uri(path: &std::path::Path) -> Result<String> {
        let canonical = path.canonicalize()?;
        // On Unix, use the path as-is. URL encoding handled by display
        Ok(format!("file://{}", canonical.display()))
    }

    fn list_resources(&self) -> Result<Value> {
        let mut resources = Vec::new();
        let cwd = std::env::current_dir()?;
        
        // Check current directory
        let manifest = cwd.join("golem.yaml");
        if manifest.exists() {
            if let Ok(uri) = Self::path_to_uri(&manifest) {
                resources.push(json!({
                    "uri": uri,
                    "name": "golem.yaml",
                    "description": "Golem application manifest (current directory)",
                    "mimeType": "application/yaml"
                }));
            }
        }
        
        // Check parent directories
        let mut parent = cwd.parent();
        while let Some(dir) = parent {
            let manifest = dir.join("golem.yaml");
            if manifest.exists() {
                let dir_name = dir.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "root".to_string());
                if let Ok(uri) = Self::path_to_uri(&manifest) {
                    resources.push(json!({
                        "uri": uri,
                        "name": format!("golem.yaml ({})", dir_name),
                        "description": "Golem application manifest (parent directory)",
                        "mimeType": "application/yaml"
                    }));
                }
            }
            parent = dir.parent();
        }
        
        // Check immediate child directories
        if let Ok(entries) = std::fs::read_dir(&cwd) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let manifest = entry.path().join("golem.yaml");
                    if manifest.exists() {
                        let dir_name = entry.file_name().to_string_lossy().to_string();
                        if let Ok(uri) = Self::path_to_uri(&manifest) {
                            resources.push(json!({
                                "uri": uri,
                                "name": format!("golem.yaml ({})", dir_name),
                                "description": "Golem application manifest (child directory)",
                                "mimeType": "application/yaml"
                            }));
                        }
                    }
                }
            }
        }
        
        Ok(json!(resources))
    }

    fn read_resource(&self, uri: &str) -> Result<Value> {
        // Parse file:// URI
        let path = uri.strip_prefix("file://")
            .ok_or_else(|| anyhow!("Invalid URI: must start with file://"))?;
        
        // Read file content - this will handle UTF-8 properly or error
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read file {}: {}", path, e))?;
        
        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/yaml",
                "text": content
            }]
        }))
    }

    // --- Response Helpers ---

    fn success_response(&self, result: Value, id: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id: Some(id),
        }
    }

    fn error_response(&self, code: i32, message: impl Into<String>, id: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
            id: Some(id),
        }
    }
}
