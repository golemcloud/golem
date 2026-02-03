# Golem CLI MCP Tools - Quick Reference

Quick reference guide for using Golem CLI MCP tools from Claude Desktop and Gemini CLI.

## Current Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `list_components` | List all available components | None |
| `list_agent_types` | List all available agent types | None |

---

## Example Prompts for Claude Desktop

### List Components

```
Use the list_components tool to show me all my deployed Golem components
```

```
What components do I have in Golem Cloud? Use the MCP tool to check.
```

```
Please use the list_components MCP tool and show me the results
```

### List Agent Types

```
What agent types can I use? Please use the list_agent_types MCP tool
```

```
Use the MCP tool to list all available agent types in my Golem deployment
```

```
Show me my agent types using the list_agent_types tool
```

### Combined Queries

```
I want to understand my Golem deployment. First, list all components, then show me the available agent types.
```

```
Please use the MCP tools to:
1. List all components
2. List all agent types
And then provide a summary of my deployment
```

---

## Testing Tools Directly

### Using Python Script

```bash
# List all tools
python list_mcp_tools.py

# Run full test suite
python test_mcp_connections.py
```

### Manual MCP Protocol Testing

See `MCP_TOOLS_DOCUMENTATION.md` for detailed curl examples.

---

## Quick Setup Commands

### Start MCP Server (HTTP/SSE)

```bash
golem-cli mcp-server start --host 127.0.0.1 --port 3000
```

### Start MCP Server for Claude Desktop (Stdio)

```bash
golem-cli mcp-server start --transport stdio
```

**Note:** Claude Desktop will automatically start the server via stdio when configured correctly, so you typically don't need to run this manually.

---

## Configuration Files

### Claude Desktop Configuration

Add to `claude_desktop_config.json`:

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

**Configuration file locations:**
- **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
- **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Linux:** `~/.config/Claude/claude_desktop_config.json`

---

## Troubleshooting

### Empty Results

If tools return empty arrays:
- Ensure you're authenticated: `golem-cli cloud login`
- Check if you have components/agents deployed
- Verify your current profile: `golem-cli profile list`

### Connection Issues

**Claude Desktop:**
- Ensure `golem-cli` is in your PATH
- Verify configuration uses `--transport stdio`
- Check Claude Desktop logs for errors

---

## Next Steps

For more detailed information, see:
- [MCP Tools Documentation](MCP_TOOLS_DOCUMENTATION.md) - Full documentation
- [MCP Manual Testing Prompts](MCP_MANUAL_TESTING_PROMPTS.md) - Testing guide
- [MCP Server Development Guide](cli/golem-cli/MCP_SERVER_DEV_GUIDE.md) - Development guide
