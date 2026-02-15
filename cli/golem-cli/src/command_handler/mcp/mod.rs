use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::mcp_server::server_runtime::create_server;
use rust_mcp_sdk::mcp_server::{McpServerOptions, ServerHandler, ToMcpServerHandler};
use rust_mcp_sdk::schema::schema_utils::CallToolError;
use rust_mcp_sdk::schema::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, InitializeResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceContent,
    ReadResourceRequestParams, ReadResourceResult, Resource, RpcError, ServerCapabilities,
    ServerCapabilitiesResources, ServerCapabilitiesTools, TextContent, TextResourceContents, Tool,
    ToolInputSchema,
};
use rust_mcp_sdk::McpServer as McpServerTrait;
use rust_mcp_sdk::StdioTransport;
use rust_mcp_sdk::TransportOptions;

use super::Context;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct McpHandler {
    #[allow(dead_code)]
    ctx: Arc<Context>,
}

impl McpHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn cmd_serve(self, _port: u16) -> anyhow::Result<()> {
        let handler = GolemMcpHandler;

        let server_details = InitializeResult {
            capabilities: ServerCapabilities {
                tools: Some(ServerCapabilitiesTools {
                    list_changed: Some(false),
                }),
                resources: Some(ServerCapabilitiesResources {
                    list_changed: Some(false),
                    subscribe: Some(false),
                }),
                completions: None,
                experimental: None,
                logging: None,
                prompts: None,
                tasks: None,
            },
            instructions: Some(
                "Golem Cloud CLI MCP server. Provides tools to manage \
                 components, workers, and API definitions on the Golem platform."
                    .into(),
            ),
            meta: None,
            protocol_version: PROTOCOL_VERSION.into(),
            server_info: Implementation {
                name: "golem-cli".into(),
                title: Some("Golem CLI MCP Server".into()),
                version: env!("CARGO_PKG_VERSION").into(),
                description: Some("MCP server for Golem Cloud CLI".into()),
                icons: vec![],
                website_url: None,
            },
        };

        let transport = StdioTransport::new(TransportOptions::default())
            .map_err(|e| anyhow::anyhow!("failed to create stdio transport: {e}"))?;

        let options = McpServerOptions {
            server_details,
            transport,
            handler: handler.to_mcp_server_handler(),
            task_store: None,
            client_task_store: None,
        };

        let server = create_server(options);

        server
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MCP handler implementing the ServerHandler trait from rust-mcp-sdk
// ---------------------------------------------------------------------------

struct GolemMcpHandler;

fn make_tool(
    name: &str,
    description: &str,
    required: Vec<String>,
    properties: Option<HashMap<String, serde_json::Map<String, serde_json::Value>>>,
) -> Tool {
    Tool {
        name: name.into(),
        description: Some(description.into()),
        input_schema: ToolInputSchema::new(required, properties, None),
        annotations: None,
        execution: None,
        icons: vec![],
        meta: None,
        output_schema: None,
        title: None,
    }
}

fn string_prop(desc: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), serde_json::Value::String("string".into()));
    m.insert("description".into(), serde_json::Value::String(desc.into()));
    m
}

fn text_result(text: String) -> CallToolResult {
    CallToolResult {
        content: vec![ContentBlock::TextContent(TextContent::new(
            text, None, None,
        ))],
        is_error: None,
        meta: None,
        structured_content: None,
    }
}

fn error_result(msg: String) -> CallToolResult {
    CallToolResult {
        content: vec![ContentBlock::TextContent(TextContent::new(msg, None, None))],
        is_error: Some(true),
        meta: None,
        structured_content: None,
    }
}

fn build_tool_list() -> Vec<Tool> {
    let mut tools = Vec::new();

    // component tools
    {
        let mut props = HashMap::new();
        props.insert("name".into(), string_prop("Name of the component"));
        props.insert("file".into(), string_prop("Path to the WASM file"));
        tools.push(make_tool(
            "component_add",
            "Add a new component from a WASM file",
            vec!["name".into(), "file".into()],
            Some(props),
        ));
    }
    tools.push(make_tool(
        "component_list",
        "List all registered components",
        vec![],
        None,
    ));
    {
        let mut props = HashMap::new();
        props.insert(
            "component".into(),
            string_prop("Component name or URN to inspect"),
        );
        tools.push(make_tool(
            "component_get",
            "Get metadata for a component",
            vec!["component".into()],
            Some(props),
        ));
    }

    // worker tools
    {
        let mut props = HashMap::new();
        props.insert("component".into(), string_prop("Component name or URN"));
        props.insert("worker".into(), string_prop("Worker name"));
        tools.push(make_tool(
            "worker_add",
            "Launch a new worker of a component",
            vec!["component".into(), "worker".into()],
            Some(props),
        ));
    }
    {
        let mut props = HashMap::new();
        props.insert("component".into(), string_prop("Component name or URN"));
        tools.push(make_tool(
            "worker_list",
            "List workers for a component",
            vec!["component".into()],
            Some(props),
        ));
    }
    {
        let mut props = HashMap::new();
        props.insert("component".into(), string_prop("Component name or URN"));
        props.insert("worker".into(), string_prop("Worker name"));
        props.insert(
            "function".into(),
            string_prop("Fully qualified function name"),
        );
        props.insert("args".into(), {
            let mut m = serde_json::Map::new();
            m.insert("type".into(), serde_json::Value::String("array".into()));
            m.insert(
                "description".into(),
                serde_json::Value::String("Function arguments as JSON strings".into()),
            );
            m
        });
        tools.push(make_tool(
            "worker_invoke",
            "Invoke a function on a worker",
            vec!["component".into(), "worker".into(), "function".into()],
            Some(props),
        ));
    }

    // api tools
    tools.push(make_tool(
        "api_definition_list",
        "List all API definitions",
        vec![],
        None,
    ));
    {
        let mut props = HashMap::new();
        props.insert("name".into(), string_prop("API definition name"));
        props.insert("version".into(), string_prop("API definition version"));
        tools.push(make_tool(
            "api_definition_get",
            "Get an API definition by name and version",
            vec!["name".into(), "version".into()],
            Some(props),
        ));
    }

    tools
}

#[async_trait]
impl ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServerTrait>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: build_tool_list(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServerTrait>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        let args = params.arguments.unwrap_or_default();
        match params.name.as_str() {
            "component_list" => Ok(text_result(
                "Use `golem-cli component list` to see all components.".into(),
            )),
            "component_add" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let file = args
                    .get("file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would add component '{name}' from file '{file}'."
                )))
            }
            "component_get" => {
                let component = args
                    .get("component")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would get metadata for component '{component}'."
                )))
            }
            "worker_add" => {
                let component = args
                    .get("component")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let worker = args
                    .get("worker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would launch worker '{worker}' for component '{component}'."
                )))
            }
            "worker_list" => {
                let component = args
                    .get("component")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would list workers for component '{component}'."
                )))
            }
            "worker_invoke" => {
                let component = args
                    .get("component")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let worker = args
                    .get("worker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let function = args
                    .get("function")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would invoke '{function}' on worker '{worker}' of component '{component}'."
                )))
            }
            "api_definition_list" => Ok(text_result(
                "Use `golem-cli api-definition list` to see all API definitions.".into(),
            )),
            "api_definition_get" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let version = args
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(text_result(format!(
                    "Would get API definition '{name}' version '{version}'."
                )))
            }
            other => Ok(error_result(format!("Unknown tool: {other}"))),
        }
    }

    async fn handle_list_resources_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServerTrait>,
    ) -> std::result::Result<ListResourcesResult, RpcError> {
        Ok(ListResourcesResult {
            resources: vec![Resource {
                uri: "golem://help".into(),
                name: "help".into(),
                description: Some("Overview of available Golem CLI capabilities".into()),
                mime_type: Some("text/plain".into()),
                annotations: None,
                icons: vec![],
                meta: None,
                size: None,
                title: Some("Golem CLI Help".into()),
            }],
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServerTrait>,
    ) -> std::result::Result<ReadResourceResult, RpcError> {
        let text = match params.uri.as_str() {
            "golem://help" => "Golem CLI MCP Server\n\n\
                 Available tool categories:\n\
                 - component_* : Manage Golem components (WASM modules)\n\
                 - worker_*    : Manage running worker instances\n\
                 - api_*       : Manage HTTP API definitions\n\n\
                 Use the list_tools capability to discover all available tools."
                .to_string(),
            other => format!("Unknown resource: {other}"),
        };

        Ok(ReadResourceResult {
            contents: vec![ReadResourceContent::TextResourceContents(
                TextResourceContents {
                    text,
                    uri: params.uri,
                    mime_type: Some("text/plain".into()),
                    meta: None,
                },
            )],
            meta: None,
        })
    }
}
