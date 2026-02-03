# Golem CLI MCP Tools Documentation

This document describes all available MCP (Model Context Protocol) tools provided by the Golem CLI server and how to use them from various MCP clients.

## Available MCP Tools

The Golem CLI MCP server currently exposes **2 tools**:

### 1. `list_components`

**Description:** List all available components deployed to Golem Cloud.

**Parameters:** None

**Response Format:**
```json
{
  "components": [
    {
      "id": "component-uuid",
      "name": "namespace:component-name",
      "revision": 1,
      "size": 12345
    }
  ]
}
```

**Example Usage:**
- From Claude Desktop: "What components do I have deployed?"

**Notes:**
- Returns an empty list if no authentication is configured
- Returns an empty list if there are no components deployed
- Requires valid Golem Cloud credentials

---

### 2. `list_agent_types`

**Description:** List all available agent types that are deployed.

**Parameters:** None

**Response Format:**
```json
{
  "agent_types": [
    "agent-type-1",
    "agent-type-2"
  ]
}
```

**Example Usage:**
- From Claude Desktop: "What agent types are available in my Golem deployment?"

**Notes:**
- Returns an empty list if no environment/authentication is configured
- Agent types represent the different worker configurations available
- Requires valid Golem Cloud credentials

---

## Using MCP Tools from Claude Desktop

### Setup

1. **Configure Claude Desktop MCP settings:**
   - Locate Claude Desktop configuration file:
     - **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
     - **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
     - **Linux:** `~/.config/Claude/claude_desktop_config.json`

2. **Add Golem CLI MCP server configuration:**

```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": [
        "mcp-server",
        "start",
        "--transport",
        "stdio"
      ]
    }
  }
}
```

**Important:** Claude Desktop requires `stdio` transport mode, not HTTP/SSE.

3. **Restart Claude Desktop** for changes to take effect

### Example Prompts

**List Components:**
```
Use the list_components tool to show me all my deployed Golem components
```

**List Agent Types:**
```
What agent types can I use? Please use the list_agent_types MCP tool
```

**Interactive Queries:**
```
I want to understand my Golem deployment. First, list all components, then show me the available agent types.
```

---

## Using MCP Tools from Gemini CLI

### Setup

1. **Configure Gemini CLI MCP settings:**
   - Edit Gemini CLI configuration file (location varies by installation)

2. **Add Golem CLI MCP server:**

```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": [
        "mcp-server",
        "start",
        "--transport",
        "stdio"
      ]
    }
  }
}
```

**Note:** Gemini CLI also requires `stdio` transport mode.

---

## Testing MCP Tools

### Using Python Test Script

We provide a Python script to test MCP connections and list tools:

```bash
# Start the MCP server
golem-cli mcp-server start --host 127.0.0.1 --port 3000

# In another terminal, list tools
python list_mcp_tools.py

# Or run full connection tests
python test_mcp_connections.py
```

### Manual Testing with curl

For HTTP/SSE mode:

```bash
# Health check
curl http://127.0.0.1:3000/

# Initialize
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {
        "name": "test-client",
        "version": "1.0.0"
      }
    }
  }'

# List tools
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "mcp-session-id: <session-id-from-initialize>" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }'
```

---

## Transport Modes

The Golem CLI MCP server supports two transport modes:

### HTTP/SSE (Server-Sent Events)
- **Default mode** when no transport is specified
- Used by HTTP-based MCP clients
- Server listens on HTTP port (default: 3000)
- Maintains persistent TCP connections with session IDs

### Stdio (Standard Input/Output)
- Used by Claude Desktop and Gemini CLI
- Communication via stdin/stdout
- Specify with `--transport stdio` flag

**Starting the server:**
```bash
# HTTP/SSE mode (default)
golem-cli mcp-server start --host 127.0.0.1 --port 3000

# Stdio mode (for Claude Desktop/Gemini CLI)
golem-cli mcp-server start --transport stdio
```

---

## Troubleshooting

### Server Won't Start

**Error:** `Address already in use`
- **Solution:** Stop any existing MCP server process or use a different port:
  ```bash
  golem-cli mcp-server start --host 127.0.0.1 --port 3001
  ```

### Tools Return Empty Lists

**Symptom:** `list_components` or `list_agent_types` return empty arrays

**Possible Causes:**
1. **No authentication configured:**
   - Run: `golem-cli cloud login`
   - Or configure credentials in your environment

2. **No components/agents deployed:**
   - Deploy components first: `golem-cli app deploy`
   - This is expected if you haven't deployed anything yet

3. **Wrong environment selected:**
   - Check current profile: `golem-cli profile list`
   - Switch profile if needed: `golem-cli profile set <profile-name>`

### Connection Errors in Claude Desktop

**Error:** MCP server not found or not responding

**Solutions:**
1. Ensure `golem-cli` is in your PATH
2. Verify configuration uses `--transport stdio` (not HTTP URL)
3. Check Claude Desktop logs for detailed error messages
4. Test stdio mode manually:
   ```bash
   echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | golem-cli mcp-server start --transport stdio
   ```

---

## Future Enhancements

Potential additional MCP tools that could be added:

1. **`get_component`** - Get detailed information about a specific component
2. **`list_workers`** - List all running worker instances
3. **`create_worker`** - Create a new worker instance
4. **`invoke_worker`** - Invoke a function on a worker
5. **`deploy_component`** - Deploy a component to Golem Cloud
6. **`get_worker_status`** - Get status of a specific worker
7. **`list_environments`** - List all available environments

These would enable more comprehensive Golem Cloud management through MCP clients.

---

## Additional Resources

- [MCP Specification](https://spec.modelcontextprotocol.io/)
- [Golem CLI Documentation](../README.md)
- [MCP Server Development Guide](cli/golem-cli/MCP_SERVER_DEV_GUIDE.md)
- [MCP Manual Testing Prompts](MCP_MANUAL_TESTING_PROMPTS.md)

---

## Support

For issues or questions:
- Check the troubleshooting section above
- Review Golem CLI logs for detailed error messages
- Open an issue on the Golem repository
- Check MCP client logs (Claude Desktop) for connection issues
