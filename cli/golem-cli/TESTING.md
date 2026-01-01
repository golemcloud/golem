# MCP Server Testing Guide

## Test Suites

### 1. Unit Tests (`tests/mcp_server.rs`)
Tests individual tool implementations with mocked dependencies.

**Run:**
```bash
cargo test --package golem-cli --test mcp_server
```

**Tests:**
- `test_list_components` - Verifies component listing
- `test_list_agent_types` - Verifies agent type listing
- `test_get_component` - Tests component retrieval
- `test_get_component_with_revision` - Tests versioned component retrieval
- `test_describe_component` - Tests component description
- `test_get_golem_yaml` - Tests manifest file reading
- `test_list_components_error` - Error handling for component listing
- `test_get_component_error` - Error handling for component retrieval

### 2. Integration Tests (`tests/mcp_integration.rs`)
Tests the actual MCP server running over HTTP.

**Prerequisites:**
- MCP server must be running on port 13337

**Start server for tests:**
```bash
cargo run --package golem-cli -- mcp-server start --port 13337
```

**Run tests (in another terminal):**
```bash
cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1
```

**Tests:**
- `test_server_health_endpoint` - Verifies health check
- `test_mcp_initialize` - Tests MCP protocol initialization
- `test_mcp_list_tools` - Verifies tool discovery
- `test_mcp_call_list_agent_types` - Tests agent type listing tool
- `test_mcp_call_list_components` - Tests component listing tool
- `test_mcp_call_nonexistent_tool` - Error handling for invalid tools
- `test_mcp_invalid_json_rpc` - Tests invalid JSON-RPC handling
- `test_mcp_concurrent_requests` - Tests concurrent request handling
- `test_mcp_tool_schemas` - Validates tool JSON schemas

## Running All Tests

### Quick Test (Unit Tests Only)
```bash
cargo test --package golem-cli --test mcp_server
```

### Full Test Suite

Terminal 1:
```bash
# Start MCP server for integration tests
cargo run --package golem-cli -- mcp-server start --port 13337
```

Terminal 2:
```bash
# Run unit tests
cargo test --package golem-cli --test mcp_server

# Run integration tests
cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1
```

## Automated Test Script

### PowerShell (`run_all_tests.ps1`)
```powershell
# Start server in background
$serverJob = Start-Job -ScriptBlock {
    Set-Location "C:\Users\matias.magni2\Documents\dev\mine\Algora\golem"
    cargo run --package golem-cli -- mcp-server start --port 13337
}

# Wait for server to start
Start-Sleep -Seconds 5

try {
    # Run unit tests
    Write-Host "Running unit tests..." -ForegroundColor Cyan
    cargo test --package golem-cli --test mcp_server
    
    # Run integration tests
    Write-Host "Running integration tests..." -ForegroundColor Cyan
    cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1
} finally {
    # Stop server
    Stop-Job $serverJob
    Remove-Job $serverJob
}
```

### Bash (`run_all_tests.sh`)
```bash
#!/bin/bash

# Start server in background
cargo run --package golem-cli -- mcp-server start --port 13337 &
SERVER_PID=$!

# Wait for server to start
sleep 5

# Run unit tests
echo "Running unit tests..."
cargo test --package golem-cli --test mcp_server

# Run integration tests
echo "Running integration tests..."
cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1

# Stop server
kill $SERVER_PID
```

## Test Coverage

### What's Tested

#### Unit Tests (with Mocks)
- ✅ Tool implementations
- ✅ Success cases
- ✅ Error handling
- ✅ Data transformation
- ✅ Mock client behavior

#### Integration Tests (Real HTTP)
- ✅ HTTP server startup
- ✅ Health endpoint
- ✅ MCP protocol compliance
- ✅ Tool discovery
- ✅ Tool execution
- ✅ Error responses
- ✅ Invalid requests
- ✅ Concurrent requests
- ✅ JSON Schema validation

### What's NOT Tested
- ❌ Actual Golem backend integration (requires running Golem services)
- ❌ Authentication/authorization (not yet implemented)
- ❌ Performance under load (use separate load testing tools)
- ❌ Network failures (use chaos engineering tools)

## Continuous Integration

### GitHub Actions Example
```yaml
name: MCP Server Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      
      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Run unit tests
        run: cargo test --package golem-cli --test mcp_server
      
      - name: Start MCP server
        run: cargo run --package golem-cli -- mcp-server start --port 13337 &
        
      - name: Wait for server
        run: sleep 5
      
      - name: Run integration tests
        run: cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1
```

## Debugging Failed Tests

### Unit Test Failures

**Check:**
1. Mock setup is correct
2. Expected data structures match actual
3. Error messages are accurate

**Debug with:**
```bash
RUST_LOG=debug cargo test --package golem-cli --test mcp_server -- --nocapture
```

### Integration Test Failures

**Check:**
1. Server is actually running
2. Correct port (13337)
3. No firewall blocking
4. Server has time to start (increase sleep if needed)

**Debug with:**
```bash
# Server with logging
RUST_LOG=debug cargo run --package golem-cli -- mcp-server start --port 13337

# Tests with output
cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1 --nocapture
```

### Common Issues

**"Server should be running" assertion fails:**
- Server not started or crashed
- Wrong port
- Firewall blocking

**Solution:** Check server is running: `curl http://127.0.0.1:13337`

**"Tool call should succeed" with error:**
- This is expected if Golem backend not configured
- Tests handle this gracefully
- Verify error is structured correctly

**Concurrent test failures:**
- Reduce concurrency in test
- Check server handles concurrent requests
- Look for race conditions

## Test Output Examples

### Successful Unit Test
```
running 8 tests
test test_describe_component ... ok
test test_get_component ... ok
test test_get_component_error ... ok
test test_get_component_with_revision ... ok
test test_get_golem_yaml ... ok
test test_list_agent_types ... ok
test test_list_components ... ok
test test_list_components_error ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### Successful Integration Test
```
running 9 tests
test test_mcp_call_list_agent_types ... ok
test test_mcp_call_list_components ... ok
test test_mcp_call_nonexistent_tool ... ok
test test_mcp_concurrent_requests ... ok
test test_mcp_initialize ... ok
test test_mcp_invalid_json_rpc ... ok
test test_mcp_list_tools ... ok
test test_mcp_tool_schemas ... ok
test test_server_health_endpoint ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Writing New Tests

### Add Unit Test
```rust
#[tokio::test]
async fn test_my_new_tool() {
    // Create mock clients
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client, mock_environment_client);
    
    // Create tools
    let tools = Tools::new(ctx);
    
    // Make request
    let req = Request {
        tool_name: "my_new_tool".to_string(),
        parameters: serde_json::json!({"param": "value"}),
    };
    
    // Execute and assert
    let result = tools.call(req).await.unwrap();
    // Add assertions
}
```

### Add Integration Test
```rust
#[tokio::test]
#[ignore]
async fn test_my_new_endpoint() {
    assert!(wait_for_server(50).await);
    
    let params = json!({
        "name": "my_new_tool",
        "arguments": {"param": "value"}
    });
    
    let response = mcp_request("tools/call", params, 10).await;
    assert!(response.is_ok());
    
    // Add assertions
}
```

## Best Practices

1. **Isolate Tests**: Each test should be independent
2. **Use Mocks**: Unit tests should use mocks, not real services
3. **Test Error Cases**: Don't just test happy paths
4. **Clear Assertions**: Make failure messages helpful
5. **Fast Tests**: Unit tests should be < 1s, integration < 5s
6. **Cleanup**: Always cleanup resources (files, processes)
7. **Document**: Explain what each test validates

## Performance Benchmarks

Expected test durations:
- **Unit tests**: ~2-5 seconds total
- **Integration tests**: ~10-15 seconds total
- **Full suite**: ~20-25 seconds total

If tests are slower, investigate:
- Network latency
- Server startup time
- Resource contention
- Too much logging
