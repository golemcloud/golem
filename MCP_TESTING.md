# MCP Server Testing Guide

## Quick Test

To run the full MCP test suite (unit, integration, E2E, manual, Playwright exploratory):

```bash
python run_all_mcp_tests.py
```

This script runs:
1. **Unit tests** – `cargo test --test mcp_server_unit` (7 tests: DTOs, request/response serde)
2. **Integration HTTP** – `cargo test --test mcp_integration_test` (7 tests: health, initialize, list tools, tool schemas, list_agent_types, list_components, nonexistent tool)
3. **Integration Stdio** – `cargo test --test mcp_stdio_integration` (6 tests)
4. **E2E** – `test_mcp_e2e.py` (HTTP + stdio)
5. **Stdio manual** – `test_mcp_stdio.py`
6. **Playwright exploratory** – `test_mcp_playwright.py`

## Unit Tests

MCP-specific unit tests live in `cli/golem-cli/tests/mcp_server_unit.rs`:

```bash
cargo test --package golem-cli --test mcp_server_unit
```

They cover `ListAgentTypesResponse`, `ListComponentsResponse`, `McpComponentDto`, `McpWorkerDto`, `GetComponentRequest`, and related serde roundtrips.

## Manual Testing

### 1. Start the MCP Server

```powershell
.\target\debug\golem-cli.exe mcp-server start --host 127.0.0.1 --port 13337
```

Or use the cargo command directly:
```powershell
cargo run -p golem-cli -- mcp-server start --port 13337
```

### 2. Test the Health Endpoint

```powershell
Invoke-WebRequest -Uri "http://127.0.0.1:13337" -UseBasicParsing
```

Should return: "Hello from Golem CLI MCP Server!"

### 3. Verification Script

We have provided a robust Python script to verify the MCP protocol end-to-end:

```bash
python verify_mcp.py
```

This script will:
1. Initialize the session using `curl`.
2. Handle the `mcp-session-id` header and cookies automatically.
3. Perform the full handshake.
4. Verify tool listing and execution.


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

- **Unit:** `cli/golem-cli/tests/mcp_server_unit.rs`
- **Integration HTTP:** `cli/golem-cli/tests/mcp_integration.rs`
- **Integration Stdio:** `cli/golem-cli/tests/mcp_stdio_integration.rs`

Key test helpers:
- `spawn_mcp_server()`: Starts a test server instance
- `initialize_mcp_session()`: Performs the MCP handshake and returns an initialized client
- `mcp_request_with_client()`: Sends requests using an initialized client

Each test:
1. Spawns its own server
2. Initializes an MCP session
3. Performs test operations
4. Server is automatically killed when test completes
