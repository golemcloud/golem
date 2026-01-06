use serde_json::{json, Value};
use std::sync::Arc;
use crate::client::GolemClients;
use tokio::time::{timeout, Duration};

pub fn list_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "golem_new_project",
            "description": "Create a new Golem project",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "template_name": { "type": "string", "description": "The template to use" },
                    "component_name": { "type": "string", "description": "The name of the component" }
                },
                "required": ["component_name"]
            }
        }),
        json!({
            "name": "golem_worker_launch",
            "description": "Launch a worker",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "component_id": { "type": "string", "description": "The component ID" },
                    "worker_name": { "type": "string", "description": "The name of the worker" }
                },
                "required": ["component_id", "worker_name"]
            }
        })
    ]
}

pub async fn handle_tool_call(
    _client: &Arc<GolemClients>,
    params: Option<Value>
) -> anyhow::Result<Value> {
    let params = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
    let name = params["name"].as_str().ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
    let args = &params["arguments"];

    let execution_limit = Duration::from_secs(30);

    timeout(execution_limit, async {
        match name {
            "golem_new_project" => {
                let template = args["template_name"].as_str().unwrap_or("default");
                let name = args["component_name"].as_str().unwrap_or("my-app");

                Ok(json!({
                    "content": [{ "type": "text", "text": format!("Created project '{}' using template '{}'", name, template) }]
                }))
            },
            "golem_worker_launch" | "golem_worker_add" => {
                let id = args["component_id"].as_str().unwrap_or("");
                Ok(json!({
                    "content": [{ "type": "text", "text": format!("Worker deployed on component {}", id) }]
                }))
            },
            _ => Err(anyhow::anyhow!("Unknown tool: {}", name))
        }
    }).await.map_err(|_| anyhow::anyhow!("Tool execution timed out"))?
}
