// Golem CLI MCP Server Implementation
// Provides Model Context Protocol server functionality via HTTP/SSE

pub mod server;
pub mod tools;
pub mod resources;
pub mod security;

pub use server::{GolemMcpServer, serve, serve_with_shutdown};

use crate::context::Context;
use std::sync::Arc;

/// Initialize and start the MCP server
pub async fn start_mcp_server(
    context: Arc<Context>,
    port: u16,
) -> anyhow::Result<()> {
    serve(context, port).await
}
