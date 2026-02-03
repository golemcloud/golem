# MCP Server for Golem CLI

## Overview

The Golem CLI includes an MCP (Model Context Protocol) Server that exposes CLI commands as MCP tools, enabling AI agents like Claude Code to interact with Golem programmatically.

## Quick Start

### Starting the MCP Server

#### HTTP/SSE Mode (default)

By default, the server runs in HTTP/SSE (Streamable HTTP) mode:

```bash
golem-cli mcp-server start
```

Or explicitly:

```bash
golem-cli mcp-server start --host 127.0.0.1 --port 3000
```

The server will start and listen for MCP client connections at `http://127.0.0.1:3000/mcp` using Server-Sent Events (SSE) for streaming responses.

#### Stdio Mode (for Claude Desktop)

To use stdio transport (for Claude Desktop and other stdio-based clients):

```bash
golem-cli mcp-server start --transport stdio
```

The server will communicate via stdin/stdout, compatible with Claude Desktop and other stdio-based MCP clients.

### Command Options

- `--host` - Host address to bind to (HTTP/SSE mode only, default: `127.0.0.1`)
- `--port` - Port to bind to (HTTP/SSE mode only, default: `3000`)
- `--transport` - Transport mode: `http` (HTTP/SSE, default) or `stdio`

**Note:** If `--transport` is not specified, the server defaults to HTTP/SSE mode.

### Health Check

The server exposes a health endpoint at the root path:

```bash
curl http://127.0.0.1:3000/
```

Response: `Hello from Golem CLI MCP Server!`

## Available Tools

The MCP server currently exposes the following tools:

### 1. list_agent_types

Lists all available agent types in the Golem system.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "list_agent_types",
    "arguments": {}
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"agent_types\":[\"type1\",\"type2\",...]}"
      }
    ]
  }
}
```

### 2. list_components

Lists all available components in the Golem system.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "list_components",
    "arguments": {}
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"components\":[{\"id\":\"...\",\"name\":\"...\",\"revision\":0,\"size\":1024},...]}"
      }
    ]
  }
}
```

## Testing with curl

### List Available Tools

```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/list",
    "params": {}
  }'
```

### Call a Tool

```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/call",
    "params": {
      "name": "list_agent_types",
      "arguments": {}
    }
  }'
```

## Integration with AI Agents

### HTTP/SSE Clients (default)

Configure HTTP/SSE clients to use the Golem CLI MCP server by adding it to your MCP configuration:

```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

Start the server (defaults to HTTP/SSE mode):
```bash
golem-cli mcp-server start
```

Or explicitly:
```bash
golem-cli mcp-server start --host 127.0.0.1 --port 3000
```

The server uses Server-Sent Events (SSE) for streaming responses over HTTP.

Once configured, clients can:
- List available agent types
- List components
- Execute other Golem CLI commands exposed as tools

### Custom MCP Clients

Any MCP-compatible client can connect to the server using the standard MCP protocol over HTTP at the `/mcp` endpoint.

## Architecture

### Components

- **MCP Server Implementation**: `cli/golem-cli/src/service/mcp_server.rs`
  - Defines the MCP service and available tools
  - Uses `rmcp_macros` for tool routing
  - Implements proper error handling and JSON schema generation

- **Command Handler**: `cli/golem-cli/src/command_handler/mcp_server.rs`
  - Handles server startup and configuration
  - Sets up HTTP server with Axum
  - Manages MCP session lifecycle

- **Commands**: `cli/golem-cli/src/command/mcp_server.rs`
  - Defines CLI arguments for MCP server commands
  - Provides command-line interface

### Technology Stack

- **rmcp**: Rust MCP SDK for implementing MCP servers
- **rmcp_macros**: Procedural macros for tool routing
- **axum**: HTTP server framework
- **tokio**: Async runtime

## Adding New Tools

To add a new tool to the MCP server:

1. Define request/response types:
```rust
#[derive(JsonSchema, Deserialize, Serialize)]
pub struct MyToolRequest {
    pub param: String,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct MyToolResponse {
    pub result: String,
}
```

2. Add the tool method in `McpServerImpl`:
```rust
#[tool(
    name = "my_tool",
    description = "Description of what the tool does"
)]
async fn my_tool(
    &self,
    request: MyToolRequest,
) -> std::result::Result<CallToolResult, ErrorData> {
    // Implementation
    let result = self.ctx.some_handler().do_something(request.param).await
        .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
    
    let response = MyToolResponse { result };
    let content = serde_json::to_value(response)
        .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
    
    Ok(CallToolResult::success(vec![Content::json(content)?]))
}
```

3. The tool will automatically be available through the MCP server.

## Testing

### Unit Tests

Tests are located in `cli/golem-cli/tests/mcp_server.rs`.

Run tests:
```bash
cargo test --package golem-cli --test mcp_server
```

### Integration Testing

1. Start the server:
   ```bash
   golem-cli mcp-server start --port 3000
   ```

2. Use an MCP client or curl to test endpoints

3. Verify:
   - Server starts without errors
   - Health endpoint responds
   - Tools can be listed
   - Tools can be called and return expected responses

## Troubleshooting

### Server won't start

**Problem**: Port already in use
```
Error: Address already in use (os error 48)
```

**Solution**: Use a different port
```bash
golem-cli mcp-server start --port 3001
```

### Tools return errors

**Problem**: Tool execution fails with internal error

**Solution**: Check that:
- Golem environment is properly configured
- Required API credentials are set
- Backend services are accessible

### Connection refused

**Problem**: Client cannot connect to server

**Solution**: Verify:
- Server is actually running
- Firewall isn't blocking the port
- Using correct host and port in client

## Security Considerations

- The MCP server binds to `127.0.0.1` by default, making it accessible only from localhost
- For production use, consider:
  - Adding authentication/authorization
  - Using HTTPS/TLS
  - Rate limiting
  - Input validation and sanitization
  - Audit logging

## Performance

- The server uses async I/O with Tokio for efficient handling of concurrent requests
- Each tool call is executed asynchronously
- Connection pooling is handled by the underlying HTTP server

## Future Enhancements

Potential improvements to the MCP server:

- [ ] Expose more Golem CLI commands as tools
- [ ] Add resource support for manifest files
- [ ] Implement streaming responses for long-running operations
- [ ] Add pagination for large result sets
- [ ] Support for custom authentication mechanisms
- [ ] WebSocket transport in addition to HTTP
- [ ] Metrics and observability

## References

- [Model Context Protocol Specification](https://spec.modelcontextprotocol.io/)
- [rust-mcp-sdk Documentation](https://github.com/rust-mcp-stack/rust-mcp-sdk)
- [Golem CLI Documentation](../README.md)

## Related Issues

- Issue #1926: Incorporate MCP Server into Golem CLI
