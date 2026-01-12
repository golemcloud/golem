# Golem MCP Server - Cursor Integration Guide

## ✅ Configuration Complete

The Golem MCP server has been configured in Cursor and is ready to use!

### Configuration Location
```
C:\Users\matias.magni2\AppData\Roaming\Cursor\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json
```

### Current Configuration
```json
{
  "golem-cli": {
    "command": "C:\\Users\\matias.magni2\\Documents\\dev\\mine\\Algora\\golem\\target\\debug\\golem-cli.exe",
    "args": [
      "mcp-server",
      "start",
      "--host",
      "127.0.0.1",
      "--port",
      "3000"
    ]
  }
}
```

## 🚀 Activation Steps

1. **Restart Cursor** - Close and reopen Cursor to load the MCP configuration
2. **Verify Connection** - Cursor will automatically start the MCP server when needed
3. **Test the Integration** - Ask Cursor to use Golem tools

## 🧪 Manual Testing

### Test 1: Health Check
```bash
curl http://127.0.0.1:3000/
```
Expected: `Hello from Golem CLI MCP Server!`

### Test 2: MCP Initialize
```bash
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
```

### Test 3: List Tools
After initializing, you can list available tools:
```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Connection: keep-alive" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }'
```

## 📋 Available Tools

Once connected, Cursor can use these Golem tools:

1. **list_agent_types** - List all available agent types in Golem
2. **list_components** - List all available components in Golem

## 🔧 Troubleshooting

### Server Not Starting
- Check if port 3000 is available: `netstat -an | findstr :3000`
- Verify binary exists: `target\debug\golem-cli.exe`
- Check Cursor logs for MCP connection errors

### Connection Issues
- Ensure the server is running: Check Task Manager for `golem-cli.exe`
- Verify configuration: Check `cline_mcp_settings.json`
- Restart Cursor after configuration changes

### Testing Manually
Run the test script:
```bash
python test_mcp_cursor_integration.py
```

## 📝 Notes

- **LocalSessionManager**: The server uses connection-based sessions. Cursor handles this automatically when using the command-based configuration.
- **Port**: Default port is 3000. Change in config if needed.
- **Auto-start**: Cursor will start the server automatically when connecting.

## ✅ Verification

The MCP server has been:
- ✅ Configured in Cursor settings
- ✅ Tested for health endpoint
- ✅ Tested for MCP initialize
- ✅ Ready for Cursor integration

**Next Step**: Restart Cursor and start using Golem tools!
