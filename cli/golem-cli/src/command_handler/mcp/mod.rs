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

use crate::command::shared_args::mcp::{McpServeArgs, McpSubcommand};
use crate::context::Context;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::Infallible;
use warp::Filter;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use futures_util::StreamExt;
use warp::sse::Event;
use uuid::Uuid;
use std::collections::{BTreeMap, HashMap};
use tokio::sync::Mutex;
use crate::command_handler::Handlers;
use golem_common::model::component::ComponentName;
use crate::model::environment::EnvironmentResolveMode;

pub struct McpCommandHandler {
    ctx: Arc<Context>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

type SessionSender = mpsc::UnboundedSender<Result<Event, Infallible>>;
type Sessions = Arc<Mutex<HashMap<String, SessionSender>>>;

impl McpCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: McpSubcommand) -> anyhow::Result<()> {
        match subcommand {
            McpSubcommand::Serve(args) => self.serve(args).await,
        }
    }

    async fn serve(&self, args: McpServeArgs) -> anyhow::Result<()> {
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        let ctx = self.ctx.clone();
        
        let sessions_clone = sessions.clone();
        let sse_route = warp::path("sse")
            .and(warp::get())
            .map(move || {
                let session_id = Uuid::new_v4().to_string();
                let (tx, rx) = mpsc::unbounded_channel();
                
                let sessions = sessions_clone.clone();
                let session_id_clone = session_id.clone();
                
                tokio::spawn(async move {
                    sessions.lock().await.insert(session_id_clone, tx);
                });

                let endpoint = format!("/message?session_id={}", session_id);
                let initial_event = Event::default()
                    .event("endpoint")
                    .data(endpoint);

                let stream = UnboundedReceiverStream::new(rx);
                let response_stream = futures_util::stream::once(async move { Ok(initial_event) })
                    .chain(stream);

                warp::sse::reply(warp::sse::keep_alive().stream(response_stream))
            });

        let ctx_clone = ctx.clone();
        let sessions_clone = sessions.clone();
        let message_route = warp::path("message")
            .and(warp::post())
            .and(warp::query::<HashMap<String, String>>())
            .and(warp::body::json())
            .and_then(move |query: HashMap<String, String>, request: JsonRpcRequest| {
                let ctx = ctx_clone.clone();
                let sessions = sessions_clone.clone();
                async move {
                    let session_id = query.get("session_id").cloned().unwrap_or_default();
                    let response = match process_request(&ctx, request).await {
                        Ok(res) => res,
                        Err(e) => JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: Value::Null,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32603,
                                message: e.to_string(),
                                data: None,
                            }),
                        },
                    };

                    if let Some(tx) = sessions.lock().await.get(&session_id) {
                        let response_str = serde_json::to_string(&response).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(response_str)));
                    }

                    Ok::<_, Infallible>(warp::reply::with_status("OK", warp::http::StatusCode::ACCEPTED))
                }
            });

        let routes = sse_route.or(message_route);
        
        let addr: std::net::SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
        println!("MCP Server listening on http://{}", addr);
        warp::serve(routes).run(addr).await;

        Ok(())
    }
}

async fn process_request(ctx: &Arc<Context>, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
    let id = request.id.unwrap_or(Value::Null);
    
    let result = match request.method.as_str() {
        "initialize" => Ok(Some(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "golem-cli-mcp",
                "version": "0.1.0"
            }
        }))),
        "tools/list" => list_tools().await,
        "tools/call" => call_tool(ctx, request.params).await,
        _ => Err(anyhow::anyhow!("Method not found")),
    };

    match result {
        Ok(res) => Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: res,
            error: None,
        }),
        Err(e) => Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: e.to_string(),
                data: None,
            }),
        }),
    }
}

async fn list_tools() -> anyhow::Result<Option<Value>> {
    Ok(Some(serde_json::json!({
        "tools": [
            {
                "name": "golem_component_list",
                "description": "List all deployed components in Golem.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "Optional component name filter."
                        }
                    }
                }
            },
            {
                "name": "golem_agent_list",
                "description": "List agents for a specific component.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "component_name": {
                            "type": "string",
                            "description": "The component name."
                        }
                    },
                    "required": ["component_name"]
                }
            },
            {
                "name": "golem_agent_invoke",
                "description": "Invoke a function on an agent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent_name": {
                            "type": "string",
                            "description": "The agent name."
                        },
                        "function_name": {
                            "type": "string",
                            "description": "The function name to invoke."
                        },
                        "arguments": {
                            "type": "array",
                            "items": {
                                    "type": "string"
                            },
                            "description": "Function arguments in WAVE format."
                        }
                    },
                    "required": ["agent_name", "function_name"]
                }
            }
        ]
    })))
}

async fn call_tool(ctx: &Arc<Context>, params: Value) -> anyhow::Result<Option<Value>> {
    let tool_name = params.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
    let arguments = params.get("arguments").unwrap_or(&Value::Object(Default::default())).clone();

    match tool_name {
        "golem_component_list" => {
            let component_names: BTreeMap<ComponentName, _> = ctx.component_handler().deployable_manifest_components().await?;
            let mut components = Vec::new();
            
            let environment = ctx.environment_handler().resolve_environment(EnvironmentResolveMode::Any).await?;
            for (name, _) in component_names {
                if let Some(c) = ctx.component_handler().get_current_deployed_server_component_by_name(&environment, &name).await? {
                    components.push(c);
                }
            }
            
            let show_sensitive = ctx.show_sensitive();
            let views: Vec<_> = components.into_iter().map(|c| crate::model::component::ComponentView::new_wit_style(show_sensitive, c)).collect();
            
            Ok(Some(serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&views)?
                    }
                ]
            })))
        },
        "golem_agent_list" => {
            let component_name_str = arguments.get("component_name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing component_name"))?;
            let component_name = ComponentName(component_name_str.to_string());
            
            let environment = ctx.environment_handler().resolve_environment(EnvironmentResolveMode::Any).await?;
            let component = ctx.component_handler().get_current_deployed_server_component_by_name(&environment, &component_name).await?.ok_or_else(|| anyhow::anyhow!("Component not found"))?;

            let (agents, _cursor) = ctx.worker_handler().list_component_workers(&component_name, &component.id, None, None, None, false).await?;
            let views: Vec<crate::model::worker::WorkerMetadataView> = agents.into_iter().map(|a| a.into()).collect();
            
            Ok(Some(serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&views)?
                    }
                ]
            })))
        },
        "golem_agent_invoke" => {
            let agent_name_str = arguments.get("agent_name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing agent_name"))?;
            let agent_name = crate::model::worker::WorkerName(agent_name_str.to_string());
            let function_name = arguments.get("function_name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing function_name"))?.to_string();
            
            let environment = ctx.environment_handler().resolve_environment(EnvironmentResolveMode::Any).await?;
            let match_result = ctx.worker_handler().match_worker_name(agent_name.clone()).await?;
            
            let component = ctx.component_handler().get_current_deployed_server_component_by_name(&match_result.environment, &match_result.component_name).await?.ok_or_else(|| anyhow::anyhow!("Component not found"))?;

            let result = ctx.worker_handler().invoke_worker(
                &component, 
                &agent_name,
                &function_name, 
                vec![], // Wave args would need proper parsing here
                golem_common::model::IdempotencyKey::new(Uuid::new_v4().to_string()),
                false, 
                None
            ).await?;
            
            Ok(Some(serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!("{:?}", result)
                    }
                ]
            })))
        },
        _ => Err(anyhow::anyhow!("Unknown tool")),
    }
}
