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

use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum McpServerSubcommand {
    /// Start Golem CLI as an MCP server
    Start(McpServerStartArgs),
}

#[derive(Debug, Clone, Args)]
pub struct McpServerStartArgs {
    /// Host address to bind to (HTTP mode only)
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port to bind to (HTTP mode only)
    #[arg(long, default_value_t = 3000)]
    pub port: u16,

    /// Transport mode: "http" (SSE/HTTP, default) or "stdio"
    #[arg(long, default_value = "http", value_parser = ["http", "stdio"])]
    pub transport: String,
}
