# Test Golem CLI MCP with Claude Desktop — Manual Guide

Quick steps to manually test the Golem CLI MCP server using Claude Desktop.

---

## 1. Build golem-cli

```bash
cd c:\Users\matias.magni2\Documents\dev\mine\Algora\golem
cargo build --release --package golem-cli
```

---

## 2. Configure Claude Desktop

Run the configuration script:

```bash
python configure_mcp_claude.py
```

Or configure manually: edit `%APPDATA%\Claude\claude_desktop_config.json` and add:

```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "C:\\Users\\matias.magni2\\Documents\\dev\\mine\\Algora\\golem\\target\\release\\golem-cli.exe",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

---

## 3. Restart Claude Desktop

Quit and restart Claude Desktop so it reloads MCP settings.

---

## 4. Prompts to Try in Claude Desktop

### Basic tool discovery
```
What MCP tools are available from the golem-cli server? List all tools and their descriptions.
```

### List agent types
```
Use the golem-cli MCP server to list all available agent types in Golem.
```

### List components
```
Use the golem-cli MCP server to list all available components in my Golem instance.
```

### Multi-step workflow
```
I want to understand my Golem setup. Please:
1. List all available agent types
2. List all available components
3. Give me a summary of what I can do with this setup
```

### If new to Golem
```
I'm new to Golem. Can you explain what agent types and components are, and then show me what I have available?
```

---

## 5. Optional: Login to Golem Cloud

For non-empty results:

```bash
golem-cli cloud login
```

---

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Tools not showing | Check MCP status in Claude Desktop settings |
| Server won't start | Ensure `golem-cli.exe` path is correct |
| Empty results | Run `golem-cli cloud login` and check profile |
| Config not loading | Restart Claude Desktop after config changes |

---

For more prompts, see [MCP_MANUAL_TESTING_PROMPTS.md](./MCP_MANUAL_TESTING_PROMPTS.md).
