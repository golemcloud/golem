# MCP Server Quick Reference

## Commands

### Start Server
```bash
golem-cli mcp-server start --host 127.0.0.1 --port 3000
```

## Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/` | GET | Health check |
| `/mcp` | POST | MCP protocol endpoint |

## MCP Protocol Messages

### Initialize
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {
      "name": "client-name",
      "version": "1.0.0"
    }
  }
}
```

### List Tools
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/list",
  "params": {}
}
```

### Call Tool
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "tool_name",
    "arguments": {
      "param1": "value1"
    }
  }
}
```

## Available Tools

### list_agent_types
Lists all available agent types.

**Arguments**: None

**Example**:
```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_agent_types","arguments":{}}}'
```

### list_components
Lists all available components.

**Arguments**: None

**Example**:
```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_components","arguments":{}}}'
```

## Error Codes

| Code | Description |
|------|-------------|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |

## Testing Commands

### Health Check
```bash
curl http://127.0.0.1:3000/
```

### Quick Test All Tools
```bash
# List tools
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

# Call list_agent_types
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_agent_types","arguments":{}}}'

# Call list_components
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_components","arguments":{}}}'
```

## Development

### Build
```bash
cargo build --package golem-cli
```

### Run
```bash
cargo run --package golem-cli -- mcp-server start --port 3000
```

### Test
```bash
cargo test --package golem-cli --test mcp_server
```

### Debug
```bash
RUST_LOG=debug cargo run --package golem-cli -- mcp-server start
```

## Common Issues

### Port in use
```bash
# Use different port
golem-cli mcp-server start --port 3001
```

### Connection refused
- Check server is running
- Verify port number
- Check firewall settings

### Tool not found
- Verify tool name spelling
- Check server logs
- List available tools first

## Files

| File | Purpose |
|------|---------|
| `src/command/mcp_server.rs` | CLI commands |
| `src/command_handler/mcp_server.rs` | Server startup |
| `src/service/mcp_server.rs` | Tool implementations |
| `tests/mcp_server.rs` | Integration tests |
