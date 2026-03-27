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

//! MCP (Model Context Protocol) server for golem-cli.
//!
//! Implements an MCP server using `rust-mcp-sdk` that exposes Golem CLI commands
//! as MCP tools and golem.yaml manifests as MCP resources.
//!
//! ## Architecture
//!
//! Tool calls are dispatched by re-invoking the same golem-cli binary with
//! appropriate arguments and `--format json`. This ensures:
//! - Full compatibility with authentication, profiles, and all CLI features
//! - No need to duplicate the internal command dispatch logic
//! - Consistent behavior between CLI and MCP usage
//!
//! ## Supported Tools
//!
//! - `app_deploy` - Build and deploy WASM components
//! - `app_build` - Build WASM components
//! - `component_list` - List deployed components
//! - `worker_new` - Create a new worker instance
//! - `worker_invoke` - Invoke a function on a worker
//! - `worker_list` - List workers for a component

use crate::context::Context;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

// ─── Tool Definitions ────────────────────────────────────────────────

fn tool_definitions() -> serde_json::Value {
    json!({
        "tools": [
            {
                "name": "app_deploy",
                "description": "Build and deploy a Golem application. Equivalent to `golem-cli app deploy`. Builds WASM components, uploads them to Golem Cloud, and deploys HTTP APIs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Optional component name to deploy. If omitted, deploys all components."
                        },
                        "force_build": {
                            "type": "boolean",
                            "description": "Force rebuild even if components are up-to-date.",
                            "default": false
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Working directory containing golem.yaml. Defaults to current directory."
                        }
                    }
                }
            },
            {
                "name": "app_build",
                "description": "Build a Golem application's WASM components. Equivalent to `golem-cli app build`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Optional component name to build."
                        },
                        "force_build": {
                            "type": "boolean",
                            "description": "Force rebuild even if up-to-date.",
                            "default": false
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Working directory containing golem.yaml."
                        }
                    }
                }
            },
            {
                "name": "component_list",
                "description": "List all deployed Golem components. Returns component names, versions, and metadata.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Optional filter by component name."
                        }
                    }
                }
            },
            {
                "name": "worker_new",
                "description": "Create a new Golem worker instance from a deployed component.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Component name to create the worker from."
                        },
                        "worker_name": {
                            "type": "string",
                            "description": "Name for the new worker instance."
                        },
                        "env": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Environment variables as key=value pairs."
                        },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Arguments to pass to the worker."
                        }
                    },
                    "required": ["component_name", "worker_name"]
                }
            },
            {
                "name": "worker_invoke",
                "description": "Invoke a function on a Golem worker instance. Arguments are passed as JSON-encoded WAVE values.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "worker_name": {
                            "type": "string",
                            "description": "Worker name (format: <WORKER> or <COMPONENT>/<WORKER>)."
                        },
                        "function_name": {
                            "type": "string",
                            "description": "Function name to invoke on the worker."
                        },
                        "arguments": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Function arguments as WAVE values."
                        }
                    },
                    "required": ["worker_name", "function_name"]
                }
            },
            {
                "name": "worker_list",
                "description": "List workers for a Golem component. Shows worker names, status, and metadata.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Component name to list workers for."
                        }
                    },
                    "required": ["component_name"]
                }
            }
        ]
    })
}

// ─── Resource Discovery ──────────────────────────────────────────────

fn discover_manifests() -> Vec<serde_json::Value> {
    let mut resources = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_default();

    // Check current directory and immediate children for golem.yaml
    for dir in std::iter::once(cwd.clone()).chain(
        std::fs::read_dir(&cwd)
            .into_iter()
            .flat_map(|entries| entries.flatten())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path()),
    ) {
        let manifest = dir.join("golem.yaml");
        if manifest.exists() {
            let name = if dir == cwd {
                "golem.yaml (root)".to_string()
            } else {
                format!(
                    "golem.yaml ({})",
                    dir.file_name().unwrap_or_default().to_string_lossy()
                )
            };
            resources.push(json!({
                "uri": format!("file://{}", manifest.display()),
                "name": name,
                "description": format!("Golem application manifest at {}", manifest.display()),
                "mimeType": "application/yaml"
            }));
        }
    }

    resources
}

fn read_manifest(uri: &str) -> Result<String, String> {
    let path = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("Invalid resource URI: {}", uri))?;
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path, e))
}

// ─── CLI Command Execution ──────────────────────────────────────────

async fn execute_cli_command(args: &[&str], working_dir: Option<&str>) -> String {
    let current_exe = std::env::current_exe().unwrap_or_else(|_| "golem-cli".into());

    let mut cmd = TokioCommand::new(&current_exe);
    cmd.args(args).arg("--format").arg("json");

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    match cmd.output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                if stdout.is_empty() {
                    format!("Command completed successfully.\n{}", stderr)
                } else {
                    stdout.into_owned()
                }
            } else {
                format!(
                    "Command failed (exit code: {}).\nStdout: {}\nStderr: {}",
                    output.status.code().unwrap_or(-1),
                    stdout,
                    stderr
                )
            }
        }
        Err(e) => format!("Failed to execute golem-cli: {}", e),
    }
}

// ─── Tool Call Handler ──────────────────────────────────────────────

async fn handle_tool_call(name: &str, arguments: &serde_json::Value) -> Result<String, String> {
    match name {
        "app_deploy" => {
            let mut args = vec!["app", "deploy"];
            if arguments
                .get("force_build")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                args.push("--force-build");
            }
            let component_name = arguments
                .get("component_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(ref cn) = component_name {
                args.push(cn);
            }
            let wd = arguments
                .get("working_directory")
                .and_then(|v| v.as_str());
            Ok(execute_cli_command(&args, wd).await)
        }

        "app_build" => {
            let mut args = vec!["app", "build"];
            if arguments
                .get("force_build")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                args.push("--force-build");
            }
            let component_name = arguments
                .get("component_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(ref cn) = component_name {
                args.push(cn);
            }
            let wd = arguments
                .get("working_directory")
                .and_then(|v| v.as_str());
            Ok(execute_cli_command(&args, wd).await)
        }

        "component_list" => {
            let mut args = vec!["component", "list"];
            let component_name = arguments
                .get("component_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(ref cn) = component_name {
                args.push(cn);
            }
            Ok(execute_cli_command(&args, None).await)
        }

        "worker_new" => {
            let component_name = arguments
                .get("component_name")
                .and_then(|v| v.as_str())
                .ok_or("component_name is required")?
                .to_string();
            let worker_name = arguments
                .get("worker_name")
                .and_then(|v| v.as_str())
                .ok_or("worker_name is required")?
                .to_string();

            let mut args = vec!["worker", "new", &component_name, &worker_name];

            let env_vars: Vec<String> = arguments
                .get("env")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            for env_var in &env_vars {
                args.push("--env");
                args.push(env_var);
            }

            let worker_args: Vec<String> = arguments
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            for arg in &worker_args {
                args.push("--arg");
                args.push(arg);
            }

            Ok(execute_cli_command(&args, None).await)
        }

        "worker_invoke" => {
            let worker_name = arguments
                .get("worker_name")
                .and_then(|v| v.as_str())
                .ok_or("worker_name is required")?
                .to_string();
            let function_name = arguments
                .get("function_name")
                .and_then(|v| v.as_str())
                .ok_or("function_name is required")?
                .to_string();

            let mut args = vec!["worker", "invoke", &worker_name, &function_name];

            let invoke_args: Vec<String> = arguments
                .get("arguments")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            for arg in &invoke_args {
                args.push(arg);
            }

            Ok(execute_cli_command(&args, None).await)
        }

        "worker_list" => {
            let component_name = arguments
                .get("component_name")
                .and_then(|v| v.as_str())
                .ok_or("component_name is required")?
                .to_string();
            Ok(execute_cli_command(
                &["worker", "list", &component_name],
                None,
            ).await)
        }

        _ => Err(format!("Unknown tool: {}", name)),
    }
}

// ─── MCP JSON-RPC Server Loop ────────────────────────────────────────

pub async fn run_stdio_loop(_ctx: Arc<Context>) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = stdout;

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(_) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Parse error" },
                    "id": serde_json::Value::Null
                });
                let mut out = serde_json::to_string(&err_resp)?;
                out.push('\n');
                writer.write_all(out.as_bytes()).await?;
                writer.flush().await?;
                continue;
            }
        };

        let response = handle_jsonrpc_request(request).await;
        if let Some(mut resp_str) = response {
            resp_str.push('\n');
            writer.write_all(resp_str.as_bytes()).await?;
            writer.flush().await?;
        }
    }

    Ok(())
}

async fn handle_jsonrpc_request(req: serde_json::Value) -> Option<String> {
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = req
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // Notifications (no id) don't get responses
    let is_notification = req.get("id").is_none();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-03-26",
            "serverInfo": {
                "name": "golem-cli",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "instructions": "Golem CLI MCP Server. Use tools to build, deploy, and manage WebAssembly components and workers on Golem Cloud."
        })),

        "notifications/initialized" | "initialized" => {
            // Client ready — no response needed
            return None;
        }

        "tools/list" => Ok(tool_definitions()),

        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let arguments = params.get("arguments").unwrap_or(&json!({}));

            match handle_tool_call(name, arguments).await {
                Ok(output) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": output
                    }]
                })),
                Err(err) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": err
                    }],
                    "isError": true
                })),
            }
        }

        "resources/list" => Ok(json!({
            "resources": discover_manifests()
        })),

        "resources/read" => {
            let uri = params
                .get("uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");

            match read_manifest(uri) {
                Ok(content) => Ok(json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/yaml",
                        "text": content
                    }]
                })),
                Err(err) => Err(json!({
                    "code": -32603,
                    "message": err
                })),
            }
        }

        _ => Err(json!({
            "code": -32601,
            "message": format!("Method not found: {}", method)
        })),
    };

    if is_notification {
        return None;
    }

    let response = match result {
        Ok(res) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": res
        }),
        Err(err) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": err
        }),
    };

    serde_json::to_string(&response).ok()
}
