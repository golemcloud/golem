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

//! MCP (Model Context Protocol) server implementation for Golem CLI.
//!
//! This module exposes Golem CLI commands as MCP tools and manifest files as MCP resources,
//! allowing AI agents (e.g. Claude Code) to interact with Golem through the MCP protocol.

use async_trait::async_trait;
use rust_mcp_sdk::mcp_server::{HyperServerOptions, McpServerOptions, ServerHandler, ServerRuntime, ToMcpServerHandler};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, CallToolResult, ContentBlock, InitializeResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResult, ReadResourceContent, Resource, ServerCapabilities,
    ServerCapabilitiesResources, ServerCapabilitiesTools, TextContent, TextResourceContents,
    Tool, ToolInputSchema,
};
use rust_mcp_sdk::schema::schema_utils::CallToolError;
use rust_mcp_sdk::{McpServer, StdioTransport, TransportOptions};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tracing::info;
use walkdir::WalkDir;

/// The MCP server handler for Golem CLI.
pub struct GolemMcpHandler {
    /// The path to the golem-cli binary to invoke.
    cli_binary: String,
    /// Working directory for CLI invocations.
    work_dir: PathBuf,
    /// Global flags to forward to every CLI invocation (e.g. --profile, --format json).
    global_flags: Vec<String>,
}

impl GolemMcpHandler {
    pub fn new(cli_binary: String, work_dir: PathBuf, global_flags: Vec<String>) -> Self {
        Self {
            cli_binary,
            work_dir,
            global_flags,
        }
    }

    /// Execute a golem-cli command and return stdout/stderr.
    async fn exec_cli(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new(&self.cli_binary);
        cmd.current_dir(&self.work_dir);

        // Always output JSON for machine-readable results
        cmd.arg("--format").arg("json");

        // Forward global flags
        for flag in &self.global_flags {
            cmd.arg(flag);
        }

        for arg in args {
            cmd.arg(arg);
        }

        info!("Executing: {} {:?}", &self.cli_binary, args);

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.status.success() {
                    Ok(stdout)
                } else {
                    Err(format!(
                        "Command failed (exit code {:?}):\nstdout: {}\nstderr: {}",
                        output.status.code(),
                        stdout,
                        stderr
                    ))
                }
            }
            Err(e) => Err(format!("Failed to execute golem-cli: {}", e)),
        }
    }

    /// Find all golem.yaml manifest files in the working directory and ancestors/children.
    fn find_manifest_files(&self) -> Vec<PathBuf> {
        let mut manifests = Vec::new();

        // Search ancestors
        let mut dir = self.work_dir.clone();
        loop {
            let candidate = dir.join("golem.yaml");
            if candidate.exists() {
                manifests.push(candidate);
            }
            if !dir.pop() {
                break;
            }
        }

        // Search children (max depth 5)
        for entry in WalkDir::new(&self.work_dir)
            .max_depth(5)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name() == "golem.yaml" {
                let path = entry.path().to_path_buf();
                if !manifests.contains(&path) {
                    manifests.push(path);
                }
            }
        }

        manifests
    }
}

/// Defines all CLI tools exposed via MCP.
fn define_tools() -> Vec<Tool> {
    vec![
        // ── App commands ──
        make_tool(
            "app_new",
            "Create a new Golem application",
            json!({
                "type": "object",
                "properties": {
                    "application_name": { "type": "string", "description": "Application folder name" },
                    "language": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Languages the application should support (e.g. rust, go, typescript)"
                    }
                }
            }),
        ),
        make_tool(
            "app_build",
            "Build all or selected components in the application",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional component names to build"
                    },
                    "force_build": { "type": "boolean", "description": "Skip modification time checks" },
                    "step": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Select specific build step(s)"
                    }
                }
            }),
        ),
        make_tool(
            "app_deploy",
            "Deploy all or selected components and HTTP APIs (includes building)",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional component names to deploy"
                    },
                    "force_build": { "type": "boolean" },
                    "update_workers": { "type": "string", "description": "Update mode: auto or manual" },
                    "redeploy_workers": { "type": "boolean" },
                    "redeploy_http_api": { "type": "boolean" },
                    "redeploy_all": { "type": "boolean" }
                }
            }),
        ),
        make_tool(
            "app_clean",
            "Clean all components in the application or by selection",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional component names to clean"
                    }
                }
            }),
        ),
        make_tool(
            "app_diagnose",
            "Diagnose possible tooling problems in the application",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }),
        ),
        // ── Component commands ──
        make_tool(
            "component_new",
            "Create a new component in the current application",
            json!({
                "type": "object",
                "properties": {
                    "component_template": { "type": "string", "description": "Template to use" },
                    "component_name": { "type": "string", "description": "Name in 'package:name' form" }
                }
            }),
        ),
        make_tool(
            "component_templates",
            "List or search component templates",
            json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Filter for language or template name" }
                }
            }),
        ),
        make_tool(
            "component_build",
            "Build component(s) based on the current directory or by selection",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "force_build": { "type": "boolean" }
                }
            }),
        ),
        make_tool(
            "component_deploy",
            "Deploy component(s) and dependent HTTP APIs",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "force_build": { "type": "boolean" },
                    "update_workers": { "type": "string" },
                    "redeploy_all": { "type": "boolean" }
                }
            }),
        ),
        make_tool(
            "component_clean",
            "Clean component(s)",
            json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }),
        ),
        make_tool(
            "component_list",
            "List deployed component versions' metadata",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string", "description": "Optional component name" }
                }
            }),
        ),
        make_tool(
            "component_get",
            "Get latest or selected version of deployed component metadata",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string" },
                    "version": { "type": "integer", "description": "Optional component version" }
                }
            }),
        ),
        make_tool(
            "component_add_dependency",
            "Add or update a component dependency",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string" },
                    "target_component_name": { "type": "string" },
                    "target_component_path": { "type": "string" },
                    "target_component_url": { "type": "string" },
                    "dependency_type": { "type": "string" }
                }
            }),
        ),
        make_tool(
            "component_update_workers",
            "Try to automatically update all existing workers of the component",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string" },
                    "update_mode": { "type": "string", "description": "auto or manual" },
                    "await": { "type": "boolean" }
                }
            }),
        ),
        make_tool(
            "component_redeploy_workers",
            "Redeploy all workers of the selected component",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string" }
                }
            }),
        ),
        // ── Worker commands ──
        make_tool(
            "worker_new",
            "Create a new worker",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string", "description": "Worker name (formats: WORKER, COMPONENT/WORKER, etc.)" },
                    "arg": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Worker arguments"
                    },
                    "env": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Environment variables as KEY=VALUE"
                    }
                },
                "required": ["worker_name"]
            }),
        ),
        make_tool(
            "worker_invoke",
            "Invoke a function on a worker",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string", "description": "Worker name" },
                    "function_name": { "type": "string", "description": "Function to invoke" },
                    "arg": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Function arguments in WAVE format"
                    },
                    "idempotency_key": { "type": "string" },
                    "use_stdio": { "type": "boolean" }
                },
                "required": ["worker_name", "function_name"]
            }),
        ),
        make_tool(
            "worker_list",
            "List workers of a component",
            json!({
                "type": "object",
                "properties": {
                    "component_name": { "type": "string" },
                    "filter": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter expressions"
                    },
                    "count": { "type": "integer", "description": "Maximum number of results" }
                }
            }),
        ),
        make_tool(
            "worker_get",
            "Get worker metadata",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string" }
                },
                "required": ["worker_name"]
            }),
        ),
        make_tool(
            "worker_delete",
            "Delete a worker",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string" }
                },
                "required": ["worker_name"]
            }),
        ),
        make_tool(
            "worker_update",
            "Update a worker to a different component version",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string" },
                    "target_version": { "type": "integer" },
                    "mode": { "type": "string", "description": "auto or manual" }
                },
                "required": ["worker_name", "target_version", "mode"]
            }),
        ),
        make_tool(
            "worker_connect",
            "Connect to a worker's event stream",
            json!({
                "type": "object",
                "properties": {
                    "worker_name": { "type": "string" }
                },
                "required": ["worker_name"]
            }),
        ),
        // ── API commands ──
        make_tool(
            "api_definition_list",
            "List API definitions",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Optional name filter" }
                }
            }),
        ),
        make_tool(
            "api_definition_get",
            "Get an API definition",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" }
                },
                "required": ["name", "version"]
            }),
        ),
        make_tool(
            "api_definition_add",
            "Add or update an API definition from file",
            json!({
                "type": "object",
                "properties": {
                    "definition_file": { "type": "string", "description": "Path to the definition JSON/YAML file" }
                },
                "required": ["definition_file"]
            }),
        ),
        make_tool(
            "api_definition_delete",
            "Delete an API definition",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" }
                },
                "required": ["name", "version"]
            }),
        ),
        make_tool(
            "api_deployment_list",
            "List API deployments",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            }),
        ),
        make_tool(
            "api_deployment_deploy",
            "Deploy an API",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" },
                    "host": { "type": "string" },
                    "subdomain": { "type": "string" }
                },
                "required": ["name", "version"]
            }),
        ),
        make_tool(
            "api_deployment_delete",
            "Undeploy an API",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "host": { "type": "string" },
                    "subdomain": { "type": "string" }
                },
                "required": ["name"]
            }),
        ),
        // ── Plugin commands ──
        make_tool(
            "plugin_list",
            "List available plugins",
            json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "description": "global, project, or component scope" }
                }
            }),
        ),
        make_tool(
            "plugin_get",
            "Get a plugin's details",
            json!({
                "type": "object",
                "properties": {
                    "plugin_name": { "type": "string" },
                    "plugin_version": { "type": "string" }
                },
                "required": ["plugin_name", "plugin_version"]
            }),
        ),
        make_tool(
            "plugin_register",
            "Register a new plugin",
            json!({
                "type": "object",
                "properties": {
                    "plugin_file": { "type": "string", "description": "Path to the plugin manifest" }
                },
                "required": ["plugin_file"]
            }),
        ),
        // ── Profile commands ──
        make_tool(
            "profile_list",
            "List configured CLI profiles",
            json!({ "type": "object", "properties": {} }),
        ),
        make_tool(
            "profile_switch",
            "Switch active CLI profile",
            json!({
                "type": "object",
                "properties": {
                    "profile_name": { "type": "string" }
                },
                "required": ["profile_name"]
            }),
        ),
        make_tool(
            "profile_show",
            "Show current profile configuration",
            json!({ "type": "object", "properties": {} }),
        ),
        // ── Cloud commands ──
        make_tool(
            "cloud_account_get",
            "Get current cloud account info",
            json!({ "type": "object", "properties": {} }),
        ),
        make_tool(
            "cloud_project_list",
            "List cloud projects",
            json!({
                "type": "object",
                "properties": {
                    "account_id": { "type": "string" }
                }
            }),
        ),
        make_tool(
            "cloud_project_get",
            "Get cloud project details",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "PROJECT or ACCOUNT/PROJECT" }
                },
                "required": ["project"]
            }),
        ),
        make_tool(
            "cloud_project_new",
            "Create a new cloud project",
            json!({
                "type": "object",
                "properties": {
                    "project_name": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["project_name"]
            }),
        ),
        make_tool(
            "cloud_token_list",
            "List cloud auth tokens",
            json!({ "type": "object", "properties": {} }),
        ),
    ]
}

fn make_tool(name: &str, description: &str, schema: Value) -> Tool {
    // Convert JSON properties into HashMap<String, Map<String, Value>>
    let properties = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| {
                    let prop_map = match v.as_object() {
                        Some(m) => m.clone(),
                        None => serde_json::Map::new(),
                    };
                    (k.clone(), prop_map)
                })
                .collect::<std::collections::HashMap<String, serde_json::Map<String, Value>>>()
        });

    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    Tool {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema: ToolInputSchema::new(required, properties, None),
        annotations: None,
        execution: None,
        icons: vec![],
        meta: None,
        output_schema: None,
        title: None,
    }
}

/// Maps an MCP tool call to CLI arguments.
fn tool_to_cli_args(tool_name: &str, params: &Map<String, Value>) -> Result<Vec<String>, String> {
    let mut args = Vec::new();

    // Map tool name to CLI subcommand path
    let cmd_parts = match tool_name {
        "app_new" => vec!["app", "new"],
        "app_build" => vec!["app", "build"],
        "app_deploy" => vec!["app", "deploy"],
        "app_clean" => vec!["app", "clean"],
        "app_diagnose" => vec!["app", "diagnose"],
        "component_new" => vec!["component", "new"],
        "component_templates" => vec!["component", "templates"],
        "component_build" => vec!["component", "build"],
        "component_deploy" => vec!["component", "deploy"],
        "component_clean" => vec!["component", "clean"],
        "component_list" => vec!["component", "list"],
        "component_get" => vec!["component", "get"],
        "component_add_dependency" => vec!["component", "add-dependency"],
        "component_update_workers" => vec!["component", "update-workers"],
        "component_redeploy_workers" => vec!["component", "redeploy-workers"],
        "worker_new" => vec!["worker", "new"],
        "worker_invoke" => vec!["worker", "invoke"],
        "worker_list" => vec!["worker", "list"],
        "worker_get" => vec!["worker", "get"],
        "worker_delete" => vec!["worker", "delete"],
        "worker_update" => vec!["worker", "update"],
        "worker_connect" => vec!["worker", "connect"],
        "api_definition_list" => vec!["api", "definition", "list"],
        "api_definition_get" => vec!["api", "definition", "get"],
        "api_definition_add" => vec!["api", "definition", "add"],
        "api_definition_delete" => vec!["api", "definition", "delete"],
        "api_deployment_list" => vec!["api", "deployment", "list"],
        "api_deployment_deploy" => vec!["api", "deployment", "deploy"],
        "api_deployment_delete" => vec!["api", "deployment", "delete"],
        "plugin_list" => vec!["plugin", "list"],
        "plugin_get" => vec!["plugin", "get"],
        "plugin_register" => vec!["plugin", "register"],
        "profile_list" => vec!["profile", "list"],
        "profile_switch" => vec!["profile", "switch"],
        "profile_show" => vec!["profile", "show"],
        "cloud_account_get" => vec!["cloud", "account", "get"],
        "cloud_project_list" => vec!["cloud", "project", "list"],
        "cloud_project_get" => vec!["cloud", "project", "get"],
        "cloud_project_new" => vec!["cloud", "project", "new"],
        "cloud_token_list" => vec!["cloud", "token", "list"],
        _ => return Err(format!("Unknown tool: {}", tool_name)),
    };

    for part in cmd_parts {
        args.push(part.to_string());
    }

    // Positional args are tool-specific; flags use --name value pattern
    let positional_params = match tool_name {
        "app_new" => vec!["application_name"],
        "component_new" => vec!["component_template", "component_name"],
        "component_templates" => vec!["filter"],
        "component_get" => vec!["component_name", "version"],
        "worker_new" => vec!["worker_name"],
        "worker_invoke" => vec!["worker_name", "function_name"],
        "worker_get" | "worker_delete" | "worker_connect" => vec!["worker_name"],
        "worker_update" => vec!["worker_name", "target_version", "mode"],
        "profile_switch" => vec!["profile_name"],
        "api_definition_get" | "api_definition_delete" => vec!["name", "version"],
        "cloud_project_get" => vec!["project"],
        _ => vec![],
    };

    // Add positional args first
    for pos in &positional_params {
        if let Some(val) = params.get(*pos) {
            match val {
                Value::String(s) => args.push(s.clone()),
                Value::Number(n) => args.push(n.to_string()),
                Value::Bool(b) => args.push(b.to_string()),
                _ => {}
            }
        }
    }

    // Add remaining params as flags
    for (key, value) in params {
        if positional_params.contains(&key.as_str()) {
            continue;
        }

        let flag_name = format!("--{}", key.replace('_', "-"));

        match value {
            Value::Bool(true) => {
                args.push(flag_name);
            }
            Value::Bool(false) => {}
            Value::String(s) if !s.is_empty() => {
                args.push(flag_name);
                args.push(s.clone());
            }
            Value::Number(n) => {
                args.push(flag_name);
                args.push(n.to_string());
            }
            Value::Array(arr) => {
                for item in arr {
                    match item {
                        Value::String(s) => {
                            args.push(flag_name.clone());
                            args.push(s.clone());
                        }
                        Value::Number(n) => {
                            args.push(flag_name.clone());
                            args.push(n.to_string());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    Ok(args)
}

#[async_trait]
impl ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, rust_mcp_sdk::schema::RpcError> {
        Ok(ListToolsResult {
            tools: define_tools(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        let tool_name = &params.name;
        let arguments = params.arguments.unwrap_or_default();

        let cli_args = match tool_to_cli_args(tool_name, &arguments) {
            Ok(args) => args,
            Err(e) => {
                return Ok(CallToolResult {
                    content: vec![ContentBlock::TextContent(TextContent::new(
                        format!("Error mapping tool to CLI args: {}", e),
                        None,
                        None,
                    ))],
                    is_error: Some(true),
                    meta: None,
                    structured_content: None,
                });
            }
        };

        let args_refs: Vec<&str> = cli_args.iter().map(|s| s.as_str()).collect();

        match self.exec_cli(&args_refs).await {
            Ok(output) => Ok(CallToolResult {
                content: vec![ContentBlock::TextContent(TextContent::new(output, None, None))],
                is_error: Some(false),
                meta: None,
                structured_content: None,
            }),
            Err(e) => Ok(CallToolResult {
                content: vec![ContentBlock::TextContent(TextContent::new(e, None, None))],
                is_error: Some(true),
                meta: None,
                structured_content: None,
            }),
        }
    }

    async fn handle_list_resources_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListResourcesResult, rust_mcp_sdk::schema::RpcError> {
        let manifests = self.find_manifest_files();

        let resources: Vec<Resource> = manifests
            .into_iter()
            .map(|path| {
                let uri = format!("file://{}", path.display());
                let name = path
                    .strip_prefix(&self.work_dir)
                    .unwrap_or(&path)
                    .display()
                    .to_string();

                Resource {
                    uri,
                    name,
                    description: Some("Golem application manifest".to_string()),
                    mime_type: Some("application/yaml".to_string()),
                    annotations: None,
                    size: None,
                    icons: vec![],
                    meta: None,
                    title: None,
                }
            })
            .collect();

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ReadResourceResult, rust_mcp_sdk::schema::RpcError> {
        let uri = &params.uri;

        // Extract file path from file:// URI
        let path = if let Some(stripped) = uri.strip_prefix("file://") {
            PathBuf::from(stripped)
        } else {
            return Err(
                rust_mcp_sdk::schema::RpcError::invalid_params()
                    .with_message(format!("Invalid resource URI: {}", uri)),
            );
        };

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(ReadResourceResult {
                contents: vec![ReadResourceContent::TextResourceContents(TextResourceContents {
                    uri: uri.clone(),
                    mime_type: Some("application/yaml".to_string()),
                    text: content,
                    meta: None,
                })],
                meta: None,
            }),
            Err(e) => Err(
                rust_mcp_sdk::schema::RpcError::internal_error()
                    .with_message(format!("Failed to read resource: {}", e)),
            ),
        }
    }
}

/// Build the MCP server InitializeResult (server info + capabilities).
fn server_info() -> InitializeResult {
    InitializeResult {
        protocol_version: ProtocolVersion::V2025_03_26.into(),
        server_info: rust_mcp_sdk::schema::Implementation {
            name: "golem-cli-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            title: Some("Golem CLI MCP Server".to_string()),
            description: Some("MCP server for interacting with Golem Cloud via CLI commands".to_string()),
            icons: vec![],
            website_url: Some("https://golem.cloud".to_string()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            resources: Some(ServerCapabilitiesResources {
                subscribe: Some(false),
                list_changed: Some(false),
            }),
            completions: None,
            experimental: None,
            logging: None,
            prompts: None,
            tasks: None,
        },
        instructions: Some(
            "Golem CLI MCP Server — interact with Golem Cloud through CLI commands exposed as tools. \
             Manifest files (golem.yaml) are available as resources."
                .to_string(),
        ),
        meta: None,
    }
}

/// Start the MCP server with stdio transport (for integration with Claude Code, etc.)
pub async fn start_stdio_server(
    cli_binary: String,
    work_dir: PathBuf,
    global_flags: Vec<String>,
) -> anyhow::Result<()> {
    let handler = GolemMcpHandler::new(cli_binary, work_dir, global_flags);

    let transport = StdioTransport::new(TransportOptions::default())
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let server: Arc<ServerRuntime> = rust_mcp_sdk::mcp_server::server_runtime::create_server(McpServerOptions {
        server_details: server_info(),
        transport,
        handler: handler.to_mcp_server_handler(),
        task_store: None,
        client_task_store: None,
    });

    info!("Golem CLI MCP server starting on stdio...");
    server.start().await.map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}

/// Start the MCP server with HTTP (Streamable HTTP) transport on the given port.
pub async fn start_http_server(
    port: u16,
    cli_binary: String,
    work_dir: PathBuf,
    global_flags: Vec<String>,
) -> anyhow::Result<()> {
    let handler = GolemMcpHandler::new(cli_binary, work_dir, global_flags);

    let hyper_options = HyperServerOptions {
        host: "0.0.0.0".to_string(),
        port,
        ..Default::default()
    };

    let server = rust_mcp_sdk::mcp_server::hyper_server::create_server(
        server_info(),
        handler.to_mcp_server_handler(),
        hyper_options,
    );

    eprintln!("golem-cli running MCP Server at port {}", port);
    info!("Golem CLI MCP server starting on port {}...", port);

    server.start().await.map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}
