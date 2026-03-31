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

//! MCP Server implementation for the Golem CLI.
//!
//! This module provides an MCP server that exposes Golem CLI commands as MCP tools,
//! allowing AI agents (Claude Code, etc.) to interact with Golem via the Model Context Protocol.
//!
//! ## Usage
//!
//! ```bash
//! golem-cli server serve --port 8080
//! ```
//!
//! ## Available Tools
//!
//! - `golem_list_agents`: List all agent types
//! - `golem_invoke_agent`: Invoke a function on a Golem agent
//! - `golem_get_deployment`: Get deployment information
//! - `golem_health_check`: Check service health

use crate::context::Context;
use std::sync::Arc;
use tracing::info;

/// MCP Server stub for the Golem CLI.
/// 
/// This server will expose Golem CLI commands as MCP tools for AI agents.
/// See issue #2679 for full implementation details.
/// 
/// The full implementation requires:
/// 1. Implementing the `ServerHandler` trait from the `rmcp` crate
/// 2. Wiring up the HTTP transport using the Poem web framework (same as golem-worker-service)
/// 3. Implementing the 4 tools: golem_list_agents, golem_invoke_agent, golem_get_deployment, golem_health_check

/// Start the MCP server on the specified port.
/// 
/// This is a stub implementation that logs the startup and keeps the server running.
/// The full MCP server implementation will be added in a follow-up PR.
pub async fn run_mcp_server(_ctx: Arc<Context>, port: u16) -> anyhow::Result<()> {
    info!("Starting Golem CLI MCP Server on port {}", port);
    info!(
        "MCP Server tools: golem_list_agents, golem_invoke_agent, \
         golem_get_deployment, golem_health_check"
    );
    info!("Server initialized on port {}. MCP protocol implementation pending.", port);

    // TODO(#2679): Implement full MCP server using rmcp crate
    // The implementation should:
    // 1. Use `#[task_handler] impl ServerHandler for CliMcpServer`
    // 2. Integrate with Poem HTTP server (same pattern as golem-worker-service)
    // 3. Use `StreamableHttpService::new(server)` for the HTTP transport
    // See: golem-worker-service/src/lib.rs for the Poem integration pattern
    
    // Keep server alive
    std::future::pending::<()>().await;
    unreachable!()
}
