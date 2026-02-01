# feat(cli): Add MCP Server support for AI agent integration

## Summary

This PR implements Model Context Protocol (MCP) server support for `golem-cli`, enabling AI agents like Claude Code to interact with Golem CLI commands programmatically.

Resolves #1926

## Changes

### New Files
- `src/mcp/mod.rs` - Module entry point
- `src/mcp/server.rs` - MCP server with 19 tool handlers
- `src/mcp/tools.rs` - Tool definitions using `#[mcp_tool]` macros
- `src/mcp/resources.rs` - Resource handler for `golem.yaml` manifests

### Modified Files
- `src/lib.rs` - Added `mcp` module (feature-gated)
- `src/command.rs` - Added `Serve` subcommand
- `src/command_handler/mod.rs` - Added handler for `Serve`
- `Cargo.toml` - Added `mcp-server` feature and `rust-mcp-sdk` dependency

## Usage

```bash
# Build with MCP support
cargo build --features mcp-server

# Start MCP server (stdio transport)
golem-cli serve

# Start MCP server with custom port
golem-cli serve --port 1232
```

## Tools Exposed

| Tool | Description |
|------|-------------|
| `golem_new` | Create new application |
| `golem_build` | Build components |
| `golem_deploy` | Deploy application |
| `golem_clean` | Clean build artifacts |
| `golem_diagnose` | Run diagnostics |
| `golem_agent_new` | Create agent |
| `golem_agent_invoke` | Invoke agent function |
| `golem_agent_list` | List agents |
| `golem_component_new` | Create component |
| `golem_component_list` | List components |

## Resources Exposed

- `golem.yaml` in current directory
- `golem.yaml` in ancestor directories
- `golem.yaml` in component subdirectories

## Testing

- [x] Builds successfully with `--features mcp-server`
- [x] Tools properly exposed via MCP protocol
- [x] Resources correctly list manifest files

## Notes

- Uses `rust-mcp-sdk` v0.8 (latest MCP protocol v2025-11-25)
- Feature-gated to avoid bloating default binary
- StdioTransport for seamless Claude Code integration
