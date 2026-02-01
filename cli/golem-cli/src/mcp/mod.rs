// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://github.com/golemcloud/golem/blob/main/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MCP Server module for exposing Golem CLI as Model Context Protocol tools.
//!
//! This module provides an MCP server that allows AI agents like Claude Code
//! to interact with Golem CLI commands programmatically.

mod resources;
mod server;
mod tools;

pub use server::start_mcp_server;
pub use server::McpServerConfig;
