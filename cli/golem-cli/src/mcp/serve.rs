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

use super::{start_mcp_server, McpTransport};
use std::process::ExitCode;

const DEFAULT_PORT: u16 = 9090;
const DEFAULT_TRANSPORT: McpTransport = McpTransport::Stdio;

/// Parse --serve-port and --serve-transport from raw CLI args and start the MCP server.
pub async fn run_mcp_from_args(args: &[String]) -> Result<ExitCode, anyhow::Error> {
    let port = parse_arg_value(args, "--serve-port")
        .map(|s| {
            s.parse::<u16>()
                .map_err(|e| anyhow::anyhow!("Invalid port '{}': {}", s, e))
        })
        .transpose()?
        .unwrap_or(DEFAULT_PORT);

    let transport = parse_arg_value(args, "--serve-transport")
        .map(|s| {
            s.parse::<McpTransport>()
                .map_err(|e| anyhow::anyhow!("{}", e))
        })
        .transpose()?
        .unwrap_or(DEFAULT_TRANSPORT);

    start_mcp_server(transport, port).await
}

fn parse_arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .or_else(|| {
            // Also support --flag=value syntax
            args.iter().find_map(|a| {
                a.strip_prefix(flag)
                    .and_then(|rest| rest.strip_prefix('='))
            })
        })
}
