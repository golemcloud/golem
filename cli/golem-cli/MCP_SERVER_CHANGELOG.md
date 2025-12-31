# MCP Server Changelog

## [Unreleased]

### Added
- MCP Server implementation for Golem CLI (#1926)
  - `mcp-server start` command to run CLI as MCP server
  - Two initial tools: `list_agent_types` and `list_components`
  - HTTP server with health check endpoint
  - Integration with rmcp crate and rmcp_macros for tool routing
  - Comprehensive documentation (MCP_SERVER.md, MCP_SERVER_DEV_GUIDE.md, MCP_SERVER_QUICK_REF.md)
  - End-to-end integration tests
  - Support for AI agent integration (Claude Code, etc.)

### Technical Details
- Built on Axum HTTP server framework
- Uses StreamableHttpService from rmcp for MCP protocol handling
- Async/await with Tokio runtime
- JSON schema generation for tool parameters
- Proper error handling with MCP-compliant error codes

### Configuration
- `--host` flag to specify bind address (default: 127.0.0.1)
- `--port` flag to specify port (default: 3000)
- MCP endpoint available at `/mcp`
- Health check endpoint at `/`

### Dependencies Added
- `rmcp` - Rust MCP SDK
- `rmcp_macros` - Procedural macros for tool routing
- `axum` - HTTP server framework
- `schemars` - JSON schema generation

## Future Enhancements

Planned improvements for future releases:
- Additional tool implementations for more Golem CLI commands
- Resource support for manifest files
- Streaming responses for long-running operations
- Pagination for large result sets
- Custom authentication mechanisms
- WebSocket transport option
- Metrics and observability
