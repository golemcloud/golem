# Cursor MCP Integration Setup

## Quick Start

### 1. Start the MCP Server

```bash
# From the golem directory
target\debug\golem-cli.exe mcp-server start --host 127.0.0.1 --port 3000
```

Or use the Python script:
```bash
python test_mcp_cursor.py
```

### 2. Verify Server is Running

The server should respond at:
- **Health endpoint**: http://127.0.0.1:3000/
- **MCP endpoint**: http://127.0.0.1:3000/mcp

### 3. Configure Cursor

Add to your Cursor MCP settings (usually in `.cursor/mcp.json` or Cursor settings):

```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

### 4. Available MCP Tools

Once connected, Cursor will have access to:

- **`list_agent_types`** - List all available agent types in Golem
- **`list_components`** - List all available components in Golem

### 5. Test the Connection

Run the test script:
```bash
python test_mcp_cursor.py
```

Expected output:
```
[PASS] Health check: Hello from Golem CLI MCP Server!
[PASS] Initialize: SUCCESS
[PASS] MCP Server is ready for Cursor!
```

## Troubleshooting

### Server not responding
- Check if port 3000 is available
- Verify the binary exists: `target\debug\golem-cli.exe`
- Check firewall settings

### Connection refused
- Ensure server is running
- Verify the URL in Cursor settings matches: `http://127.0.0.1:3000/mcp`
- Check that host is `127.0.0.1` (not `localhost`)

### Tools not available
- Ensure you've initialized the MCP session
- Check server logs for errors
- Verify Golem environment is configured (for tools that need it)

## Server Status

The server is currently running in the background. To stop it:
```bash
Get-Process -Name "golem-cli" | Stop-Process -Force
```
