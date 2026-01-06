use anyhow::anyhow;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use uuid::Uuid;

// INTERNAL GOLEM IMPORTS
use crate::context::Context;
use crate::command_handler::Handlers;
use crate::command::component::ComponentSubcommand;

// DOMAIN MODELS
use golem_client::api::WorkerClient;
use golem_client::model::WorkerCreationRequest;
use golem_common::model::component::{ComponentId, ComponentName};
use golem_common::model::worker::WasiConfigVars;

/// Defines the available MCP tools and their JSON Schemas.
pub fn list_tools() -> Vec<Value> {
    vec![
        // TOOL: Create New Component (Scaffolding)
        // Maps to: `golem component new`
        json!({
            "name": "golem_new_component",
            "description": "Create a new Golem component from a template in the current directory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "string",
                        "description": "Name for the new component (e.g. 'my-app')."
                    },
                    "template_name": {
                        "type": "string",
                        "description": "Template ID to use (e.g. 'tier1-service', 'wasi-rust-service'). Defaults to 'default'."
                    }
                },
                "required": ["component_name"]
            }
        }),
        // TOOL: Launch Worker (Remote Execution)
        // Maps to: `golem worker add` (API level)
        json!({
            "name": "golem_worker_launch",
            "description": "Launch a persistent worker instance on Golem Cloud.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "component_id": {
                        "type": "string",
                        "description": "UUID of the component to launch a worker for."
                    },
                    "worker_name": {
                        "type": "string",
                        "description": "Unique name for the worker instance."
                    },
                    "env": {
                        "type": "object",
                        "description": "Environment variables for the worker (key-value pairs).",
                        "additionalProperties": { "type": "string" }
                    },
                    "args": {
                        "type": "array",
                        "description": "Command line arguments for the worker.",
                        "items": { "type": "string" }
                    }
                },
                "required": ["component_id", "worker_name"]
            }
        }),
    ]
}

/// Handles MCP tool calls by routing them to internal Golem logic.
pub async fn handle_tool_call(
    ctx: &Arc<Context>,
    params: Option<Value>,
) -> anyhow::Result<Value> {
    let params = params.ok_or_else(|| anyhow!("Missing parameters"))?;
    let name = params["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing tool name"))?;
    let args = &params["arguments"];

    // Security: Hard timeout to prevent zombie processes during CLI/API calls
    let execution_limit = Duration::from_secs(30);

    timeout(execution_limit, async {
        match name {
            "golem_new_component" => execute_new_component(ctx, args).await,
            "golem_worker_launch" => execute_worker_launch(ctx, args).await,
            _ => Err(anyhow!("Unknown tool: {}", name)),
        }
    })
    .await
    .map_err(|_| anyhow!("Tool execution timed out after 30s"))?
}

// --- INTERNAL LOGIC: LOCAL SCAFFOLDING ---

async fn execute_new_component(ctx: &Arc<Context>, args: &Value) -> anyhow::Result<Value> {
    // 1. Parse Inputs
    let component_name_str = args["component_name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing 'component_name'"))?;

    let template_name = args["template_name"].as_str().map(|s| s.to_string());

    // 2. Construct Internal Command
    // We map this to `golem component new` which handles template downloading and hydration
    let cmd = ComponentSubcommand::New {
        component_name: Some(ComponentName(component_name_str.to_string())),
        component_template: template_name.clone(),
    };

    // 3. Execute via ComponentHandler
    // This will perform file I/O relative to the current directory.
    // We use the handler because it encapsulates complex logic for template fetching.
    ctx.component_handler().handle_command(cmd).await?;

    // 4. Return Confirmation
    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!(
                "Successfully created component '{}'{}",
                component_name_str,
                template_name.map(|t| format!(" using template '{}'", t)).unwrap_or_default()
            )
        }]
    }))
}

// --- INTERNAL LOGIC: REMOTE WORKER LAUNCH ---

async fn execute_worker_launch(ctx: &Arc<Context>, args: &Value) -> anyhow::Result<Value> {
    // 1. Parse Inputs
    let component_id_str = args["component_id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing 'component_id'"))?;

    let worker_name_str = args["worker_name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing 'worker_name'"))?;

    let component_id = ComponentId(
        Uuid::parse_str(component_id_str)
            .map_err(|_| anyhow!("Invalid UUID for component_id"))?,
    );

    let env_vars: HashMap<String, String> = match args["env"].as_object() {
        Some(obj) => obj
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect(),
        None => HashMap::new(),
    };

    // 2. Resolve Client
    // We use the direct client here instead of CommandHandler to capture the returned Worker ID structure
    let clients = ctx.golem_clients().await?;

    let request = WorkerCreationRequest {
        name: worker_name_str.to_string(),
        env: env_vars,
        config_vars: WasiConfigVars::default(),
    };

    // 3. Execute Remote Launch
    // This calls the Golem Cloud API directly
    let _ = clients
        .worker
        .launch_new_worker(&component_id.0, &request)
        .await?;

    let uri = format!("urn:worker:{}/{}", component_id.0, worker_name_str);

    // 4. Return Structured Data
    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!(
                "Successfully launched worker '{}' (URN: {})",
                worker_name_str,
                uri
            )
        }],
        "data": {
            "worker_id": worker_name_str,
            "component_id": component_id.0.to_string(),
            "uri": uri
        }
    }))
}
