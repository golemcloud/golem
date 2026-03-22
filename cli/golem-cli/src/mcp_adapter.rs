// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::command::GolemCliCommand;
use crate::context::Context;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt as _;

#[derive(Clone)]
pub struct McpServer {
    ctx: Arc<Context>,
}

impl McpServer {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn run(self, port: u16) -> anyhow::Result<()> {
        let state = McpState::from_context(self.ctx.clone());
        let app = router(state);
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;

        eprintln!("golem-cli MCP server listening on http://127.0.0.1:{port}");
        eprintln!("SSE endpoint: http://127.0.0.1:{port}/sse");
        eprintln!("JSON-RPC endpoint: http://127.0.0.1:{port}/mcp");

        axum::serve(listener, app).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct McpState {
    command_metadata: crate::model::cli_command_metadata::CliCommandMetadata,
    resources: Vec<ResourceEntry>,
}

impl McpState {
    fn from_context(_ctx: Arc<Context>) -> Self {
        Self {
            command_metadata: GolemCliCommand::collect_metadata_for_repl(),
            resources: discover_manifest_resources(std::env::current_dir().ok().as_deref()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceEntry {
    uri: String,
    name: String,
    description: String,
    mime_type: String,
    path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

pub fn router(state: McpState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/sse", get(sse))
        .route("/mcp", post(mcp_rpc))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "transport": ["http", "sse"]}))
}

async fn sse() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = IntervalStream::new(tokio::time::interval(std::time::Duration::from_secs(15)))
        .map(|_| Ok(Event::default().event("ping").data("{}")));

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn mcp_rpc(
    State(state): State<McpState>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match request.method.as_str() {
        "initialize" => ok(
            request.id,
            json!({
                "protocolVersion": "2025-03-26",
                "serverInfo": {
                    "name": "golem-cli",
                    "version": crate::version(),
                },
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false}
                }
            }),
        ),
        "tools/list" => ok(
            request.id,
            json!({ "tools": build_tools(&state.command_metadata) }),
        ),
        "resources/list" => ok(request.id, json!({ "resources": state.resources })),
        "resources/read" => match request.params.get("uri").and_then(Value::as_str) {
            Some(uri) => match read_resource(&state.resources, uri) {
                Ok(resource) => ok(request.id, json!({ "contents": [resource] })),
                Err(error) => err(request.id, StatusCode::NOT_FOUND, &error),
            },
            None => err(request.id, StatusCode::BAD_REQUEST, "missing uri parameter"),
        },
        "tools/call" => match handle_tool_call(&state, &request.params) {
            Ok(value) => ok(request.id, value),
            Err(error) => err(request.id, StatusCode::BAD_REQUEST, &error),
        },
        _ => err(request.id, StatusCode::NOT_FOUND, "unsupported MCP method"),
    };

    (StatusCode::OK, Json(response))
}

fn ok(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Option<Value>, code: StatusCode, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(json!({
            "code": i64::from(code.as_u16()),
            "message": message,
        })),
    }
}

fn build_tools(metadata: &crate::model::cli_command_metadata::CliCommandMetadata) -> Vec<Value> {
    vec![
        json!({
            "name": "cli.metadata",
            "description": "Return the filtered golem-cli command metadata tree suitable for agent tool discovery.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "manifest.resources",
            "description": "List manifest resources discovered from the current working directory, its ancestors, and direct child directories.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "command.search",
            "description": "Search the command metadata tree by command name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "command.examples",
            "description": "Return lightweight examples for a command path from the metadata tree.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "array",
                        "items": {"type": "string"}
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "metadataSummary": metadata.name
        }),
    ]
}

fn handle_tool_call(state: &McpState, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .or_else(|| params.get("tool"))
        .and_then(Value::as_str)
        .ok_or_else(|| "missing tool name".to_string())?;

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let payload = match name {
        "cli.metadata" => json!(state.command_metadata),
        "manifest.resources" => json!(state.resources),
        "command.search" => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing query".to_string())?
                .to_lowercase();
            json!(search_commands(&state.command_metadata, &query))
        }
        "command.examples" => {
            let path = arguments
                .get("path")
                .and_then(Value::as_array)
                .ok_or_else(|| "missing path".to_string())?
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            json!(command_examples(&state.command_metadata, &path))
        }
        _ => return Err(format!("unsupported tool: {name}")),
    };

    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
            }
        ],
        "structuredContent": payload
    }))
}

fn search_commands(
    metadata: &crate::model::cli_command_metadata::CliCommandMetadata,
    query: &str,
) -> Vec<Value> {
    let mut out = Vec::new();
    visit_commands(metadata, &mut |command| {
        if command.name.to_lowercase().contains(query)
            || command
                .path
                .iter()
                .any(|segment| segment.to_lowercase().contains(query))
        {
            out.push(json!({
                "path": command.path,
                "name": command.name,
                "about": command.about,
            }));
        }
    });
    out
}

fn command_examples(
    metadata: &crate::model::cli_command_metadata::CliCommandMetadata,
    path: &[String],
) -> Value {
    let mut found = None;
    visit_commands(metadata, &mut |command| {
        if command.path == path {
            let command_path = if path.is_empty() {
                vec![metadata.name.clone()]
            } else {
                let mut p = vec![metadata.name.clone()];
                p.extend(path.to_vec());
                p
            };
            let flags = command
                .args
                .iter()
                .filter_map(|arg| arg.long.first().map(|name| format!("--{name}")))
                .take(5)
                .collect::<Vec<_>>();
            found = Some(json!({
                "command": command_path.join(" "),
                "sampleFlags": flags,
            }));
        }
    });
    found.unwrap_or_else(|| json!({"error": "command path not found"}))
}

fn visit_commands<F>(
    metadata: &crate::model::cli_command_metadata::CliCommandMetadata,
    visitor: &mut F,
) where
    F: FnMut(&crate::model::cli_command_metadata::CliCommandMetadata),
{
    visitor(metadata);
    for sub in &metadata.subcommands {
        visit_commands(sub, visitor);
    }
}

fn read_resource(resources: &[ResourceEntry], uri: &str) -> Result<Value, String> {
    let resource = resources
        .iter()
        .find(|resource| resource.uri == uri)
        .ok_or_else(|| format!("resource not found: {uri}"))?;
    let text = std::fs::read_to_string(&resource.path)
        .map_err(|error| format!("failed to read {}: {error}", resource.path.display()))?;

    Ok(json!({
        "uri": resource.uri,
        "mimeType": resource.mime_type,
        "text": text,
    }))
}

pub fn discover_manifest_resources(root: Option<&Path>) -> Vec<ResourceEntry> {
    let Some(root) = root else {
        return Vec::new();
    };

    let mut resources = Vec::new();
    let mut push_manifest = |path: PathBuf, kind: &str| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("golem.yaml")
            .to_string();
        resources.push(ResourceEntry {
            uri: format!("golem://manifest/{kind}/{}", path.display()),
            name,
            description: format!("Manifest discovered from {kind} scope"),
            mime_type: "application/yaml".to_string(),
            path,
        });
    };

    for ancestor in root.ancestors() {
        for candidate in manifest_candidates(ancestor) {
            if candidate.exists() {
                push_manifest(candidate, "ancestor");
            }
        }
    }

    if let Ok(children) = std::fs::read_dir(root) {
        for child in children.flatten() {
            let path = child.path();
            if path.is_dir() {
                for candidate in manifest_candidates(&path) {
                    if candidate.exists() {
                        push_manifest(candidate, "child");
                    }
                }
            }
        }
    }

    resources.sort_by(|a, b| a.uri.cmp(&b.uri));
    resources.dedup_by(|a, b| a.uri == b.uri);
    resources
}

fn manifest_candidates(dir: &Path) -> [PathBuf; 2] {
    [dir.join("golem.yaml"), dir.join("golem.yml")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use http::Request;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn discover_manifest_resources_finds_current_and_child() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("golem.yaml"), "name: root\n").unwrap();
        std::fs::create_dir(root.join("child")).unwrap();
        std::fs::write(root.join("child").join("golem.yml"), "name: child\n").unwrap();

        let resources = discover_manifest_resources(Some(root));
        assert!(resources
            .iter()
            .any(|resource| resource.path.ends_with("golem.yaml")));
        assert!(resources
            .iter()
            .any(|resource| resource.path.ends_with("golem.yml")));
    }

    #[tokio::test]
    async fn mcp_router_supports_initialize_and_tools_list() {
        let state = McpState {
            command_metadata: GolemCliCommand::collect_metadata_for_repl(),
            resources: Vec::new(),
        };
        let app = router(state);

        let init_response = app
            .clone()
            .oneshot(
                Request::post("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "initialize",
                            "params": {}
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(init_response.status(), StatusCode::OK);

        let tools_response = app
            .oneshot(
                Request::post("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "jsonrpc": "2.0",
                            "id": 2,
                            "method": "tools/list",
                            "params": {}
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tools_response.status(), StatusCode::OK);
        let body = to_bytes(tools_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let tools = value["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "cli.metadata"));
    }
}
