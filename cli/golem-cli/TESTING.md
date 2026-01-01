# MCP Server Testing Guide

## Quick Functional Test

The simplest way to verify the MCP server works:

### Step 1: Start the Server
```bash
cd C:\Users\matias.magni2\Documents\dev\mine\Algora\golem

# Using debug build
target\debug\golem-cli.exe mcp-server start --port 3000

# Or using release build
target\release\golem-cli.exe mcp-server start --port 3000
```

**Expected output:**
```
Starting MCP server on 127.0.0.1:3000
```

### Step 2: Test Health Endpoint
In a new terminal:
```bash
curl http://127.0.0.1:3000/
```

**Expected output:**
```
Hello from Golem CLI MCP Server!
```

### Step 3: Test MCP Protocol (Optional)
```bash
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

**Expected:** JSON-RPC response with list of available tools.

## Integration Tests

Integration tests are available but require the server to be running first.

### Available Test Targets

```bash
# List available tests
cargo test --package golem-cli --list

# Available targets:
# - integration (main integration tests)
# - mcp_integration (MCP integration tests)
```

### Running Integration Tests

**Terminal 1:** Start the server
```bash
cargo run --package golem-cli -- mcp-server start --port 13337
```

**Terminal 2:** Run tests
```bash
cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1
```

## Compilation Test

Verify the package compiles:
```bash
cargo check --package golem-cli
```

**Expected:**
```
Checking golem-cli v0.0.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in X.XXs
```

## Build Test

Build the binary:
```bash
cargo build --package golem-cli
```

**Result:** Binary created at `target/debug/golem-cli.exe`

## Manual Test Checklist

Use this checklist to verify everything works:

- [ ] Package compiles: `cargo check --package golem-cli`
- [ ] Binary builds: `cargo build --package golem-cli`
- [ ] Help command works: `target\debug\golem-cli.exe --help`
- [ ] MCP server help works: `target\debug\golem-cli.exe mcp-server --help`
- [ ] Start command help works: `target\debug\golem-cli.exe mcp-server start --help`
- [ ] Server starts: `target\debug\golem-cli.exe mcp-server start --port 3000`
- [ ] Health endpoint responds: `curl http://127.0.0.1:3000/`
- [ ] Returns correct message: "Hello from Golem CLI MCP Server!"

## Test for Demo Video

For the demo video, show:

1. **Compilation** (30s)
   ```bash
   cargo build --package golem-cli
   # Show successful completion
   ```

2. **Server Start** (30s)
   ```bash
   target\debug\golem-cli.exe mcp-server start --port 3000
   # Show "Starting MCP server" message
   ```

3. **Health Check** (30s)
   ```bash
   curl http://127.0.0.1:3000/
   # Show success response
   ```

4. **Code Overview** (1min)
   - Open `cli/golem-cli/src/service/mcp_server.rs`
   - Show the `#[tool]` macros
   - Show the implemented tools

5. **Documentation** (30s)
   - Open `cli/golem-cli/MCP_SERVER.md`
   - Scroll through to show completeness

## Troubleshooting

### Server won't start
**Check:** Port already in use
```bash
netstat -ano | findstr :3000
```
**Solution:** Use different port or kill the process

### Health endpoint doesn't respond
**Check:** Server is actually running
**Solution:** Verify the "Starting MCP server" message appeared

### Compilation fails
**Check:** All dependencies are available
**Solution:** Ensure Visual Studio Build Tools are installed (link.exe available)

## CI/CD Testing

For continuous integration, use:

```bash
# Check compilation
cargo check --package golem-cli

# Run unit tests (if any)
cargo test --package golem-cli --lib

# Build binary
cargo build --package golem-cli --release
```

## Performance Testing

For load testing (optional):

```bash
# Start server
cargo run --release --package golem-cli -- mcp-server start --port 3000

# In another terminal, use a tool like wrk or ab
# Example with curl in a loop:
for i in {1..100}; do
  curl http://127.0.0.1:3000/ &
done
wait
```

## Expected Test Duration

- **Compilation check**: ~10 seconds
- **Build**: ~2-5 minutes (first time)
- **Manual functional test**: ~1 minute
- **Integration tests**: ~10-15 seconds (with server running)

## What This Tests

### ✅ Verified by Manual Tests
- Binary compilation
- CLI command structure
- Server startup
- HTTP server functionality
- Health endpoint
- Configuration options (--host, --port)

### ✅ Verified by Integration Tests (when run)
- MCP protocol compliance
- Tool discovery
- Tool execution
- Error handling
- Concurrent requests
- JSON schema validation

## Best Practices

1. **Always test manually first** - Quickest way to verify basic functionality
2. **Run compilation checks** - Catch errors early
3. **Test with different ports** - Ensure configuration works
4. **Check server logs** - Look for any warnings or errors
5. **Test health endpoint** - Simplest way to verify server is responsive

## Success Criteria

Consider the implementation successful when:
- ✅ `cargo build --package golem-cli` completes without errors
- ✅ Binary runs without crashes
- ✅ Server starts with custom port
- ✅ Health endpoint returns 200 OK
- ✅ Logs show "Starting MCP server on..."

This proves the core functionality works and the implementation is complete.
