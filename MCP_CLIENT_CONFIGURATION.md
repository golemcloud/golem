# MCP Client Configuration Guide

This guide shows how to configure the Golem CLI MCP server for different clients.

## Quick Setup Scripts

We provide Python scripts to automatically configure each client:

- **Cursor**: `configure_mcp_cursor.py` (HTTP/SSE mode)
- **Claude Desktop**: `configure_mcp_claude.py` (stdio mode)
- **Gemini CLI**: `configure_mcp_gemini.py` (stdio mode)

Run the appropriate script for your client:

```bash
# For Cursor
python configure_mcp_cursor.py

# For Claude Desktop
python configure_mcp_claude.py

# For Gemini CLI
python configure_mcp_gemini.py
```

---

## Manual Configuration

### 1. Cursor (HTTP/SSE Mode)

Cursor uses HTTP/SSE transport for MCP servers.

**Configuration File**: `%APPDATA%\Cursor\User\globalStorage\mcp.json`

**Configuration**:
```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

**Steps**:
1. Start the MCP server in HTTP/SSE mode (default):
   ```bash
   golem-cli mcp-server start
   ```
   
   Or with custom host/port:
   ```bash
   golem-cli mcp-server start --host 127.0.0.1 --port 3000
   ```

2. Add the configuration to Cursor's MCP config file

3. Restart Cursor

**Notes**:
- The server must be running before Cursor can connect
- Uses HTTP/SSE (Streamable HTTP) transport
- Server endpoint: `http://127.0.0.1:3000/mcp`

---

### 2. Claude Desktop (Stdio Mode)

Claude Desktop uses stdio transport for MCP servers.

**Configuration File**: `%APPDATA%\Claude\claude_desktop_config.json`

**Configuration**:
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

**Or with full path**:
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "C:\\path\\to\\golem-cli.exe",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

**Steps**:
1. Ensure `golem-cli` is in your PATH, or use the full path to the executable

2. Add the configuration to Claude Desktop's config file

3. Restart Claude Desktop

**Notes**:
- Uses stdio transport (stdin/stdout)
- Claude Desktop automatically starts the server process
- The server runs as a subprocess managed by Claude Desktop
- No need to manually start the server

---

### 3. Gemini CLI (Stdio Mode)

Gemini CLI uses stdio transport for MCP servers (if supported).

**Configuration File**: `%USERPROFILE%\.gemini\mcp_config.json`

*Note: The actual configuration path may vary depending on your Gemini CLI installation. Please verify the correct path.*

**Configuration**:
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

**Or with full path**:
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "C:\\path\\to\\golem-cli.exe",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

**Steps**:
1. Verify the configuration path for your Gemini CLI installation

2. Ensure `golem-cli` is in your PATH, or use the full path

3. Add the configuration to Gemini CLI's config file

4. Restart Gemini CLI

**Notes**:
- Uses stdio transport (stdin/stdout)
- Gemini CLI automatically starts the server process
- Configuration path may vary - check Gemini CLI documentation

---

## Transport Modes

### HTTP/SSE Mode (Default)

Used by: **Cursor**, **Claude Code**, and other HTTP-based clients

- **Transport**: HTTP/SSE (Streamable HTTP)
- **Command**: `golem-cli mcp-server start` (or with `--transport http`)
- **Endpoint**: `http://127.0.0.1:3000/mcp`
- **Requirements**: Server must be running before client connects
- **Configuration**: Uses `url` field in config

### Stdio Mode

Used by: **Claude Desktop**, **Gemini CLI**, and other stdio-based clients

- **Transport**: stdin/stdout
- **Command**: `golem-cli mcp-server start --transport stdio`
- **Endpoint**: N/A (uses stdin/stdout)
- **Requirements**: Client manages the server process
- **Configuration**: Uses `command` and `args` fields in config

---

## Building Golem CLI

Before configuring any client, ensure `golem-cli` is built:

```bash
# Release build (recommended)
cargo build --release --package golem-cli

# Debug build (for development)
cargo build --package golem-cli
```

The executable will be at:
- Release: `target\release\golem-cli.exe`
- Debug: `target\debug\golem-cli.exe`

**Note**: Make sure `golem-cli` is in your PATH, or use the full path in configuration files.

---

## Verifying Configuration

### For HTTP/SSE Clients (Cursor)

1. Start the server:
   ```bash
   golem-cli mcp-server start
   ```

2. Test the health endpoint:
   ```bash
   curl http://127.0.0.1:3000/
   ```
   
   Should return: `Hello from Golem CLI MCP Server!`

3. Test the MCP endpoint:
   ```bash
   curl -X POST http://127.0.0.1:3000/mcp \
     -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
   ```

4. Check that Cursor can connect (restart Cursor and check MCP status)

### For Stdio Clients (Claude Desktop, Gemini CLI)

1. Test the executable manually:
   ```bash
   golem-cli mcp-server start --transport stdio
   ```
   
   Should start without errors (will wait for input on stdin)

2. Check the configuration file format is valid JSON

3. Restart the client and check MCP status/logs

---

## Troubleshooting

### Cursor

**Problem**: Cursor can't connect to the MCP server

**Solutions**:
1. Verify the server is running: `curl http://127.0.0.1:3000/`
2. Check the configuration file path and format
3. Verify the URL in the config matches the server address
4. Check Cursor's logs for connection errors
5. Restart Cursor after configuration changes

### Claude Desktop

**Problem**: Claude Desktop can't start the MCP server

**Solutions**:
1. Verify `golem-cli` is in PATH or use full path in config
2. Test the command manually: `golem-cli mcp-server start --transport stdio`
3. Check the configuration file format is valid JSON
4. Verify the executable path is correct
5. Check Claude Desktop logs for process start errors
6. Restart Claude Desktop after configuration changes

### Gemini CLI

**Problem**: Gemini CLI can't start the MCP server

**Solutions**:
1. Verify the configuration file path is correct
2. Verify `golem-cli` is in PATH or use full path
3. Test the command manually: `golem-cli mcp-server start --transport stdio`
4. Check the configuration file format is valid JSON
5. Check Gemini CLI documentation for the correct config path
6. Restart Gemini CLI after configuration changes

---

## Configuration Examples

### Complete Cursor Configuration
```json
{
  "mcpServers": {
    "golem-cli": {
      "url": "http://127.0.0.1:3000/mcp"
    },
    "other-server": {
      "url": "http://127.0.0.1:3001/mcp"
    }
  }
}
```

### Complete Claude Desktop Configuration
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "golem-cli",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    },
    "other-server": {
      "command": "other-server",
      "args": ["--arg", "value"]
    }
  }
}
```

---

## Additional Resources

- [MCP Server Documentation](./cli/golem-cli/MCP_SERVER.md)
- [MCP Server Development Guide](./cli/golem-cli/MCP_SERVER_DEV_GUIDE.md)
- [MCP Transport Guide](./MCP_STDIO_VS_HTTP.md)
- [Manual Testing Prompts](./MCP_MANUAL_TESTING_PROMPTS.md)

---

## Summary

| Client | Transport | Config File | Script |
|--------|-----------|-------------|--------|
| **Cursor** | HTTP/SSE | `%APPDATA%\Cursor\User\globalStorage\mcp.json` | `configure_mcp_cursor.ps1` |
| **Claude Desktop** | stdio | `%APPDATA%\Claude\claude_desktop_config.json` | `configure_mcp_claude.ps1` |
| **Gemini CLI** | stdio | `%USERPROFILE%\.gemini\mcp_config.json` | `configure_mcp_gemini.ps1` |
