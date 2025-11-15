use std::sync::Arc;
use std::time::Duration;

use crate::command::shared_args::{AppOptionalComponentNames, DeployArgs, ForceBuildArg};
use crate::command_handler::app::AppCommandHandler;
use crate::context::Context;
use crate::log::{set_log_output, Output};
use crate::mcp::handler::GolemMcpServerHandler;
use rust_mcp_sdk::event_store::InMemoryEventStore;
use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions};
use rust_mcp_sdk::schema::{
    Implementation, InitializeResult, ServerCapabilities, ServerCapabilitiesTools,
    LATEST_PROTOCOL_VERSION,
};
use rust_mcp_sdk::{error::SdkResult, mcp_server::ServerHandler};

pub mod handler;
pub mod tools;
pub async fn start_mcp_server(handler: Arc<Context>, port: u16) -> SdkResult<()> {
    let server_details = InitializeResult {
        server_info: Implementation {
            name: "Golem CLI MCP Server".to_string(),
            version: crate::version().to_string(),
            title: Some("Golem CLI MCP Server".to_string()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        meta: None,
        protocol_version: LATEST_PROTOCOL_VERSION.to_string(),
        instructions: Some(
            "Use this MCP server to manage Golem applications via MCP protocol.".to_string(),
        ),
    };

    let handler = GolemMcpServerHandler {
        ctx: handler.clone(),
    };

    let server = hyper_server::create_server(
        server_details,
        handler,
        HyperServerOptions {
            port,
            host: "127.0.0.1".to_string(),
            ping_interval: Duration::from_secs(5),
            event_store: Some(Arc::new(InMemoryEventStore::default())),
            ..Default::default()
        },
    );

    server.start().await?;

    return Ok(());
}
