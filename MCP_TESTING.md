# MCP Server Testing Guide

## Quick Test

To run all MCP integration tests:

```powershell
.\run_all_mcp_tests.ps1
```

This script will:
1. Build the MCP integration tests
2. Run all tests with proper output
3. Show a summary of results
4. Save full output to `mcp_test_results.txt`

## Manual Testing

### 1. Start the MCP Server

```powershell
.\target\debug\golem-cli.exe mcp-server start --host 127.0.0.1 --port 13337
```

Or use the helper script:
```powershell
.\start_server.ps1
```

### 2. Test the Health Endpoint

```powershell
Invoke-WebRequest -Uri "http://127.0.0.1:13337" -UseBasicParsing
```

Should return: "Hello from Golem CLI MCP Server!"

### 3. Test MCP Protocol

The MCP protocol requires a specific handshake:

1. **Initialize**: Send an `initialize` request
2. **Initialized Notification**: Send `notifications/initialized` 
3. **Use Tools**: Now you can call `tools/list`, `tools/call`, etc.

Example PowerShell test:

```powershell
# Initialize
$body = @{
    jsonrpc = "2.0"
    id = 1
    method = "initialize"
    params = @{
        protocolVersion = "2024-11-05"
        capabilities = @{}
        clientInfo = @{
            name = "test-client"
            version = "1.0.0"
        }
    }
} | ConvertTo-Json -Depth 10

$headers = @{
    "Content-Type" = "application/json"
    "Accept" = "application/json, text/event-stream"
}

$response = Invoke-WebRequest -Uri "http://127.0.0.1:13337/mcp" -Method Post -Body $body -Headers $headers -UseBasicParsing -SessionVariable session
Write-Host $response.Content

# Send initialized notification
$notify = @{
    jsonrpc = "2.0"
    method = "notifications/initialized"
    params = @{}
} | ConvertTo-Json

Invoke-WebRequest -Uri "http://127.0.0.1:13337/mcp" -Method Post -Body $notify -Headers $headers -UseBasicParsing -WebSession $session

# List tools
$listTools = @{
    jsonrpc = "2.0"
    id = 2
    method = "tools/list"
    params = @{}
} | ConvertTo-Json

$response = Invoke-WebRequest -Uri "http://127.0.0.1:13337/mcp" -Method Post -Body $listTools -Headers $headers -UseBasicParsing -WebSession $session
Write-Host $response.Content
```

## Important Notes

1. **Accept Header Required**: All MCP requests must include:
   ```
   Accept: application/json, text/event-stream
   ```

2. **SSE Response Format**: Responses are in Server-Sent Events format:
   ```
   data: {"jsonrpc":"2.0","id":1,"result":{...}}
   
   
   ```
   
   Extract the JSON from the line starting with `data: `.

3. **Session Management**: Use cookie-based sessions (WebSession in PowerShell) to maintain state between requests.

4. **Initialization is Required**: You must initialize before calling any other methods, or you'll get an error: "Unexpected message, expect initialize request"

## Available Tools

The MCP server currently provides these tools:

- `list_agent_types`: List all available agent types
- `list_components`: List all available components

## Troubleshooting

### Server won't start
- Check if port 13337 is already in use
- Kill any existing golem-cli processes: `Get-Process | Where-Object {$_.ProcessName -like "*golem-cli*"} | Stop-Process -Force`

### Tests timeout
- The tests spawn their own server instance
- Each test runs independently with a fresh server
- Tests use `--test-threads=1` to avoid port conflicts

### "Unexpected message" error
- This means you didn't send the `initialize` request first
- Always initialize before calling any other methods
- Use the `initialize_mcp_session()` helper in tests

## Test Structure

The integration tests are located at:
```
cli/golem-cli/tests/mcp_integration.rs
```

Key test helpers:
- `spawn_mcp_server()`: Starts a test server instance
- `initialize_mcp_session()`: Performs the MCP handshake and returns an initialized client
- `mcp_request_with_client()`: Sends requests using an initialized client

Each test:
1. Spawns its own server
2. Initializes an MCP session
3. Performs test operations
4. Server is automatically killed when test completes
