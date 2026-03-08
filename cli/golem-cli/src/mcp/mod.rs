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

mod handler;
pub mod resources;
pub mod serve;
pub mod tools;

use handler::GolemMcpHandler;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServiceExt;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::info;

/// Transport mode for the MCP server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

impl std::str::FromStr for McpTransport {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stdio" => Ok(McpTransport::Stdio),
            "streamable-http" | "streamablehttp" | "http" | "sse" => {
                Ok(McpTransport::StreamableHttp)
            }
            _ => Err(format!(
                "Unknown transport '{}'. Use 'stdio' or 'streamable-http'.",
                s
            )),
        }
    }
}

/// Start the MCP server with the specified transport and port.
pub async fn start_mcp_server(
    transport: McpTransport,
    port: u16,
) -> Result<ExitCode, anyhow::Error> {
    match transport {
        McpTransport::Stdio => {
            info!("Starting Golem CLI MCP server (stdio transport)");
            eprintln!("golem-cli running MCP Server (stdio)");

            let handler = GolemMcpHandler::new();
            let service = handler
                .serve(rmcp::transport::io::stdio())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start MCP server: {}", e))?;

            // Wait for the service to complete
            service
                .waiting()
                .await
                .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;

            Ok(ExitCode::SUCCESS)
        }
        McpTransport::StreamableHttp => {
            info!(
                "Starting Golem CLI MCP server (streamable HTTP on port {})",
                port
            );
            eprintln!(
                "golem-cli running MCP Server at http://127.0.0.1:{}/mcp",
                port
            );

            let config = StreamableHttpServerConfig::default();
            let ct = config.cancellation_token.clone();

            let service = StreamableHttpService::new(
                move || Ok(GolemMcpHandler::new()),
                Arc::new(rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()),
                config,
            );

            let app = axum::Router::new().route(
                "/mcp",
                axum::routing::any_service(service),
            );

            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to bind to port {}: {}", port, e))?;

            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    ct.cancelled().await;
                })
                .await
                .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))?;

            Ok(ExitCode::SUCCESS)
        }
    }
}
