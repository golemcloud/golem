# MCP Server Comprehensive Testing Guide

This guide covers all testing approaches for the Golem CLI MCP server.

## Test Types

### 1. Unit Tests
**Location:** `cli/golem-cli/tests/mcp_server.rs`

Tests individual MCP server functions in isolation using mocks.

**Run:**
```bash
cargo test --package golem-cli --lib mcp_server
```

**What it tests:**
- Tool execution logic
- Error handling
- Response formatting
- Mock client interactions

### 2. Integration Tests - HTTP Mode
**Location:** `cli/golem-cli/tests/mcp_integration.rs`

Tests the full MCP server over HTTP/SSE transport.

**Run:**
```bash
cargo test --package golem-cli --test mcp_integration -- --test-threads=1
```

**What it tests:**
- Server startup and shutdown
- HTTP/SSE transport
- MCP protocol handshake
- Tool listing and execution
- Session management
- Error responses

### 3. Integration Tests - Stdio Mode
**Location:** `cli/golem-cli/tests/mcp_stdio_integration.rs`

Tests the full MCP server over stdio transport.

**Run:**
```bash
cargo test --package golem-cli --test mcp_stdio_integration -- --test-threads=1
```

**What it tests:**
- Stdio transport communication
- Line-based JSON-RPC protocol
- Request/response handling
- Multiple sequential requests
- Error handling in stdio mode

### 4. End-to-End Tests
**Location:** `test_mcp_e2e.py`

Tests both HTTP and stdio modes end-to-end.

**Run:**
```bash
python test_mcp_e2e.py
```

**What it tests:**
- Complete server lifecycle
- Both transport modes
- Health endpoints
- Full MCP protocol flow
- Error scenarios

### 5. Manual Tests
**Location:** `test_mcp_manual.md`

Step-by-step manual testing instructions.

**Run:**
Follow the instructions in `test_mcp_manual.md`

**What it tests:**
- Interactive testing
- Real-world usage scenarios
- Edge cases
- User experience

### 6. Playwright Exploratory Tests
**Location:** `test_mcp_playwright.py`

Exploratory testing using Playwright MCP tools.

**Run:**
```bash
python test_mcp_playwright.py
```

**What it tests:**
- Server capabilities discovery
- Tool exploration
- Error handling exploration
- Concurrent request handling
- Protocol compliance

## Running All Tests

### Quick Run
```bash
python run_all_mcp_tests.py
```

This will run all test suites in sequence and provide a summary.

### Individual Test Suites

#### Unit Tests Only
```bash
cargo test --package golem-cli --lib mcp_server
```

#### HTTP Integration Tests Only
```bash
cargo test --package golem-cli --test mcp_integration
```

#### Stdio Integration Tests Only
```bash
cargo test --package golem-cli --test mcp_stdio_integration
```

#### Python E2E Tests Only
```bash
python test_mcp_e2e.py
```

#### Stdio Manual Test Only
```bash
python test_mcp_stdio.py
```

#### Playwright Exploratory Test Only
```bash
python test_mcp_playwright.py
```

## Test Coverage

### Transport Modes
- ✅ HTTP/SSE transport
- ✅ Stdio transport
- ✅ Transport mode switching

### MCP Protocol
- ✅ Initialize handshake
- ✅ Initialized notification
- ✅ Tools listing
- ✅ Tool execution
- ✅ Error handling
- ✅ Invalid requests

### Tools
- ✅ list_agent_types
- ✅ list_components
- ✅ list_workers
- ✅ Error responses for invalid tools

### Error Scenarios
- ✅ Uninitialized requests
- ✅ Invalid tool names
- ✅ Malformed requests
- ✅ Network errors
- ✅ Timeout handling

### Performance
- ✅ Concurrent requests
- ✅ Sequential requests
- ✅ Multiple tool calls
- ✅ Session persistence

## Prerequisites

### Build Requirements
```bash
cargo build --package golem-cli
```

### Python Requirements
```bash
pip install requests colorama
```

### Test Environment
- Rust toolchain (for Rust tests)
- Python 3.7+ (for Python tests)
- Network access to localhost (for HTTP tests)

## Test Results Interpretation

### Success Indicators
- ✅ All tests pass
- ✅ No crashes or panics
- ✅ Valid JSON-RPC responses
- ✅ Proper error handling
- ✅ Correct protocol compliance

### Failure Indicators
- ❌ Tests fail or timeout
- ❌ Server crashes
- ❌ Invalid JSON responses
- ❌ Protocol violations
- ❌ Memory leaks

## Troubleshooting

### Tests Timeout
- Check if server is starting correctly
- Verify port is not in use
- Increase timeout values if needed
- Check system resources

### Server Won't Start
- Verify binary exists: `target/debug/golem-cli.exe`
- Check for port conflicts
- Review error messages
- Ensure dependencies are installed

### Integration Tests Fail
- Run with `--test-threads=1` to avoid port conflicts
- Check that server starts before tests run
- Verify network connectivity
- Review test logs for details

### Python Tests Fail
- Verify Python version (3.7+)
- Install required packages: `pip install requests colorama`
- Check that server binary exists
- Review Python error messages

## Continuous Integration

For CI/CD pipelines:

```bash
# Build first
cargo build --package golem-cli

# Run all tests
python run_all_mcp_tests.py

# Or run individually for better error reporting
cargo test --package golem-cli --lib mcp_server
cargo test --package golem-cli --test mcp_integration -- --test-threads=1
cargo test --package golem-cli --test mcp_stdio_integration -- --test-threads=1
python test_mcp_e2e.py
```

## Test Maintenance

### Adding New Tests

1. **Unit Tests:** Add to `cli/golem-cli/tests/mcp_server.rs`
2. **Integration Tests:** Add to `cli/golem-cli/tests/mcp_integration.rs` or `mcp_stdio_integration.rs`
3. **E2E Tests:** Add to `test_mcp_e2e.py`
4. **Manual Tests:** Add to `test_mcp_manual.md`

### Updating Tests

When MCP server changes:
- Update unit tests for new functionality
- Add integration tests for new transport modes
- Update E2E tests for new features
- Document new manual test scenarios

## Best Practices

1. **Run tests before committing**
2. **Keep tests independent** - each test should be able to run alone
3. **Use descriptive test names** - clearly indicate what is being tested
4. **Test both success and failure paths**
5. **Verify error messages are helpful**
6. **Test edge cases** - empty inputs, large inputs, etc.
7. **Maintain test documentation** - keep this guide updated

## References

- [MCP Specification](https://spec.modelcontextprotocol.io/)
- [Rust Testing Guide](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Python Testing Best Practices](https://docs.python.org/3/library/unittest.html)
