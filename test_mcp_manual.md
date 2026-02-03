# Manual MCP Server Testing Guide

This guide provides step-by-step instructions for manually testing the Golem CLI MCP server in both HTTP and stdio modes.

## Prerequisites

1. Build the project:
   ```bash
   cargo build --package golem-cli
   ```

2. Verify the binary exists:
   ```bash
   # Windows
   dir target\debug\golem-cli.exe
   
   # Linux/Mac
   ls target/debug/golem-cli
   ```

## Test 1: HTTP Mode - Health Endpoint

### Steps:

1. Start the server:
   ```bash
   target\debug\golem-cli.exe mcp-server start --host 127.0.0.1 --port 3000
   ```

2. In another terminal, test the health endpoint:
   ```bash
   # Windows PowerShell
   Invoke-WebRequest -Uri "http://127.0.0.1:3000" -UseBasicParsing
   
   # Linux/Mac
   curl http://127.0.0.1:3000
   ```

### Expected Result:
- Status: 200 OK
- Response: "Hello from Golem CLI MCP Server!"

## Test 2: HTTP Mode - MCP Initialize

### Steps:

1. Start the server (if not already running):
   ```bash
   target\debug\golem-cli.exe mcp-server start --port 3000
   ```

2. Send initialize request:
   ```bash
   # Windows PowerShell
   $body = @{
       jsonrpc = "2.0"
       id = 1
       method = "initialize"
       params = @{
           protocolVersion = "2024-11-05"
           capabilities = @{}
           clientInfo = @{
               name = "manual-test"
               version = "1.0.0"
           }
       }
   } | ConvertTo-Json -Depth 10
   
   Invoke-WebRequest -Uri "http://127.0.0.1:3000/mcp" `
       -Method POST `
       -ContentType "application/json" `
       -Headers @{"Accept"="application/json, text/event-stream"} `
       -Body $body
   
   # Linux/Mac
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
           "name": "manual-test",
           "version": "1.0.0"
         }
       }
     }'
   ```

### Expected Result:
- Response contains `data: {"jsonrpc":"2.0","id":1,"result":{...}}`
- Result includes `serverInfo` with name and version

## Test 3: HTTP Mode - List Tools

### Steps:

1. First initialize (see Test 2)

2. Send initialized notification:
   ```bash
   # Windows PowerShell
   $notify = @{
       jsonrpc = "2.0"
       method = "notifications/initialized"
       params = @{}
   } | ConvertTo-Json
   
   Invoke-WebRequest -Uri "http://127.0.0.1:3000/mcp" `
       -Method POST `
       -ContentType "application/json" `
       -Body $notify
   ```

3. List tools:
   ```bash
   # Windows PowerShell
   $body = @{
       jsonrpc = "2.0"
       id = 2
       method = "tools/list"
       params = @{}
   } | ConvertTo-Json
   
   Invoke-WebRequest -Uri "http://127.0.0.1:3000/mcp" `
       -Method POST `
       -ContentType "application/json" `
       -Headers @{"Accept"="application/json, text/event-stream"} `
       -Body $body
   ```

### Expected Result:
- Response contains list of tools
- Should include at least `list_agent_types` and `list_components`

## Test 4: HTTP Mode - Call Tool

### Steps:

1. Initialize and send initialized notification (see Tests 2-3)

2. Call a tool:
   ```bash
   # Windows PowerShell
   $body = @{
       jsonrpc = "2.0"
       id = 3
       method = "tools/call"
       params = @{
           name = "list_components"
           arguments = @{}
       }
   } | ConvertTo-Json -Depth 10
   
   Invoke-WebRequest -Uri "http://127.0.0.1:3000/mcp" `
       -Method POST `
       -ContentType "application/json" `
       -Headers @{"Accept"="application/json, text/event-stream"} `
       -Body $body
   ```

### Expected Result:
- Response contains tool execution result
- May return error if Golem environment not configured (this is OK)

## Test 5: Stdio Mode - Basic Communication

### Steps:

1. Start server in stdio mode:
   ```bash
   target\debug\golem-cli.exe mcp-server start --transport stdio
   ```

2. The server will wait for input on stdin

3. Send initialize request (type this and press Enter):
   ```json
   {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual-test","version":"1.0"}}}
   ```

4. Server should respond with initialization result

5. Send initialized notification:
   ```json
   {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
   ```

6. List tools:
   ```json
   {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
   ```

### Expected Result:
- Each request gets a JSON response on stdout
- Responses are valid JSON-RPC 2.0 format

## Test 6: Stdio Mode - Tool Execution

### Steps:

1. Start server in stdio mode (see Test 5)

2. Initialize and send initialized notification

3. Call a tool:
   ```json
   {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_agent_types","arguments":{}}}
   ```

### Expected Result:
- Tool execution result or error response
- Response is valid JSON

## Test 7: Error Handling

### Steps:

1. Start server in HTTP mode

2. Try calling a tool without initializing:
   ```bash
   curl -X POST http://127.0.0.1:3000/mcp \
     -H "Content-Type: application/json" \
     -H "Accept: application/json, text/event-stream" \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
   ```

### Expected Result:
- Error response indicating initialization required

## Test 8: Invalid Tool Name

### Steps:

1. Start server and initialize (see Tests 2-3)

2. Call non-existent tool:
   ```bash
   curl -X POST http://127.0.0.1:3000/mcp \
     -H "Content-Type: application/json" \
     -H "Accept: application/json, text/event-stream" \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nonexistent_tool","arguments":{}}}'
   ```

### Expected Result:
- Error response with appropriate error code and message

## Test 9: Concurrent Requests

### Steps:

1. Start server in HTTP mode

2. Initialize and send initialized notification

3. Send multiple tool calls simultaneously:
   ```bash
   # Run these in parallel (multiple terminals or background)
   curl -X POST http://127.0.0.1:3000/mcp \
     -H "Content-Type: application/json" \
     -H "Accept: application/json, text/event-stream" \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_components","arguments":{}}}' &
   
   curl -X POST http://127.0.0.1:3000/mcp \
     -H "Content-Type: application/json" \
     -H "Accept: application/json, text/event-stream" \
     -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_agent_types","arguments":{}}}' &
   ```

### Expected Result:
- Both requests complete successfully
- Responses are correct for each request

## Test 10: Server Shutdown

### Steps:

1. Start server in either mode

2. Send Ctrl+C to stop the server

### Expected Result:
- Server shuts down gracefully
- No error messages or crashes

## Troubleshooting

### Server won't start
- Check if port is already in use
- Verify binary exists and is executable
- Check for error messages in terminal

### No response from server
- Verify server is actually running
- Check firewall settings
- Ensure correct host and port

### Invalid JSON responses
- Verify request format is correct JSON
- Check Content-Type header is set
- Ensure Accept header includes text/event-stream for HTTP mode

### Timeout errors
- Increase timeout values
- Check server logs for errors
- Verify network connectivity

## Success Criteria

All tests should:
- ✅ Complete without errors
- ✅ Return valid JSON-RPC 2.0 responses
- ✅ Handle errors gracefully
- ✅ Maintain session state (HTTP mode)
- ✅ Process requests correctly (stdio mode)
