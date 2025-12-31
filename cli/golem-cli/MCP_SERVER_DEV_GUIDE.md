# MCP Server Development Guide

This guide is for developers working on the MCP Server implementation in Golem CLI.

## Architecture Overview

### Component Structure

```
cli/golem-cli/
├── src/
│   ├── command/
│   │   └── mcp_server.rs          # CLI command definitions
│   ├── command_handler/
│   │   └── mcp_server.rs          # Server startup logic
│   └── service/
│       └── mcp_server.rs          # MCP service implementation & tools
└── tests/
    └── mcp_server.rs              # Integration tests
```

### Request Flow

```
Client Request
    ↓
HTTP Server (Axum)
    ↓
StreamableHttpService (rmcp)
    ↓
McpServerImpl
    ↓
Tool Router (rmcp_macros)
    ↓
Tool Implementation
    ↓
Golem CLI Handler
    ↓
Response
```

## Implementation Details

### Tool Definition

Tools are defined using the `#[tool]` macro from `rmcp_macros`:

```rust
#[tool(
    name = "tool_name",
    description = "What the tool does"
)]
async fn tool_name(
    &self,
    request: RequestType,
) -> std::result::Result<CallToolResult, ErrorData> {
    // Implementation
}
```

### Tool Routing

The `#[tool_router]` macro on `impl McpServerImpl` automatically:
- Generates routing logic for all `#[tool]` methods
- Creates JSON schemas from request/response types
- Handles tool discovery (tools/list)
- Routes tool calls to the appropriate method

### Error Handling

Use `ErrorData` for MCP-compliant errors:

```rust
.map_err(|e| ErrorData::new(
    ErrorCode::INTERNAL_ERROR,
    e.to_string(),
    None,
))?
```

Error codes:
- `INTERNAL_ERROR`: Server-side errors
- `INVALID_PARAMS`: Invalid request parameters
- `METHOD_NOT_FOUND`: Tool doesn't exist

### JSON Schema Generation

Request and response types must derive `JsonSchema`:

```rust
#[derive(JsonSchema, Deserialize, Serialize)]
pub struct MyRequest {
    pub field: String,
}
```

This allows automatic schema generation for MCP clients.

## Development Workflow

### 1. Set Up Development Environment

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone repository
git clone https://github.com/golemcloud/golem.git
cd golem

# Build
cargo build --package golem-cli
```

### 2. Run in Development Mode

```bash
# Build and run
cargo run --package golem-cli -- mcp-server start --port 3000

# Or with hot reload using cargo-watch
cargo watch -x 'run --package golem-cli -- mcp-server start --port 3000'
```

### 3. Testing During Development

#### Manual Testing

Terminal 1 (Server):
```bash
cargo run --package golem-cli -- mcp-server start --port 3000
```

Terminal 2 (Client):
```bash
# Health check
curl http://127.0.0.1:3000/

# List tools
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

# Call tool
curl -X POST http://127.0.0.1:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_agent_types","arguments":{}}}'
```

#### Automated Testing

```bash
# Run unit tests
cargo test --package golem-cli --test mcp_server

# Run with output
cargo test --package golem-cli --test mcp_server -- --nocapture

# Run specific test
cargo test --package golem-cli --test mcp_server test_list_tools
```

### 4. Debugging

#### Enable Logging

```rust
// Add to your code
tracing::info!("Processing request: {:?}", request);
tracing::error!("Error occurred: {}", error);
```

Run with logging:
```bash
RUST_LOG=debug cargo run --package golem-cli -- mcp-server start
```

#### Use Rust Analyzer

In VS Code with rust-analyzer:
- Set breakpoints
- Run debug configuration
- Step through code

## Adding a New Tool

### Step 1: Define Data Types

In `cli/golem-cli/src/service/mcp_server.rs`:

```rust
#[derive(JsonSchema, Deserialize, Serialize)]
pub struct CreateWorkerRequest {
    pub component_id: String,
    pub worker_name: String,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct CreateWorkerResponse {
    pub worker_id: String,
    pub status: String,
}
```

### Step 2: Implement Tool Method

```rust
#[tool(
    name = "create_worker",
    description = "Creates a new worker instance for a component"
)]
async fn create_worker(
    &self,
    request: CreateWorkerRequest,
) -> std::result::Result<CallToolResult, ErrorData> {
    // Get the appropriate handler
    let worker_handler = self.ctx.worker_handler();
    
    // Execute the command
    let worker = worker_handler
        .cmd_create_worker(request.component_id, request.worker_name)
        .await
        .map_err(|e| ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            format!("Failed to create worker: {}", e),
            None,
        ))?;
    
    // Prepare response
    let response = CreateWorkerResponse {
        worker_id: worker.id.to_string(),
        status: worker.status,
    };
    
    // Serialize and return
    let content = serde_json::to_value(response)
        .map_err(|e| ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            e.to_string(),
            None,
        ))?;
    
    Ok(CallToolResult::success(vec![Content::json(content)?]))
}
```

### Step 3: Add Tests

In `cli/golem-cli/tests/mcp_server.rs`:

```rust
#[tokio::test]
async fn test_create_worker() {
    let client = setup_test_client().await;
    
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "create_worker",
            "arguments": {
                "component_id": "test-component",
                "worker_name": "test-worker"
            }
        }
    });
    
    let response = client.call(request).await.unwrap();
    
    assert!(response["result"]["content"][0]["text"].is_string());
    // Add more assertions
}
```

### Step 4: Document

Update `MCP_SERVER.md`:

```markdown
### 3. create_worker

Creates a new worker instance for a component.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "create_worker",
    "arguments": {
      "component_id": "component-id",
      "worker_name": "my-worker"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [{
      "type": "text",
      "text": "{\"worker_id\":\"...\",\"status\":\"running\"}"
    }]
  }
}
```
```

## Testing Best Practices

### Unit Tests

Test individual tool methods:

```rust
#[tokio::test]
async fn test_tool_success() {
    let ctx = create_test_context();
    let server = McpServerImpl::new(Arc::new(ctx));
    
    let result = server.my_tool(MyRequest { param: "test" }).await;
    
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_tool_error_handling() {
    let ctx = create_test_context_with_error();
    let server = McpServerImpl::new(Arc::new(ctx));
    
    let result = server.my_tool(MyRequest { param: "bad" }).await;
    
    assert!(result.is_err());
}
```

### Integration Tests

Test the full MCP protocol:

```rust
#[tokio::test]
async fn test_end_to_end() {
    // Start server
    let server = start_test_server().await;
    
    // Create client
    let client = create_mcp_client("http://localhost:3000/mcp");
    
    // Initialize
    let init = client.initialize().await.unwrap();
    assert_eq!(init.protocol_version, "2024-11-05");
    
    // List tools
    let tools = client.list_tools().await.unwrap();
    assert!(tools.len() > 0);
    
    // Call tool
    let result = client.call_tool("my_tool", json!({"param": "test"})).await.unwrap();
    assert!(result.is_success());
}
```

## Performance Optimization

### Async Operations

Always use async operations:

```rust
// Good
let result = self.ctx.handler().async_operation().await?;

// Bad - blocks the thread
let result = self.ctx.handler().sync_operation()?;
```

### Connection Pooling

The HTTP server automatically pools connections. For database or API clients:

```rust
// Use Arc for shared state
pub struct McpServerImpl {
    pub ctx: Arc<Context>,
    pub client_pool: Arc<Pool>,
}
```

### Caching

Cache expensive operations:

```rust
use tokio::sync::RwLock;

pub struct McpServerImpl {
    pub ctx: Arc<Context>,
    pub cache: Arc<RwLock<HashMap<String, CachedData>>>,
}
```

## Security Considerations

### Input Validation

Always validate inputs:

```rust
#[tool(name = "my_tool", description = "...")]
async fn my_tool(&self, request: MyRequest) -> Result<CallToolResult, ErrorData> {
    // Validate
    if request.param.is_empty() {
        return Err(ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "param cannot be empty".to_string(),
            None,
        ));
    }
    
    // Process
    // ...
}
```

### Error Messages

Don't leak sensitive information in errors:

```rust
// Good
Err(ErrorData::new(
    ErrorCode::INTERNAL_ERROR,
    "Failed to process request".to_string(),
    None,
))

// Bad - leaks internal details
Err(ErrorData::new(
    ErrorCode::INTERNAL_ERROR,
    format!("Database query failed: {}", db_error),
    None,
))
```

### Rate Limiting

Consider adding rate limiting for production:

```rust
use tower::limit::RateLimitLayer;

let app = Router::new()
    .nest_service("/mcp", mcp_service)
    .layer(RateLimitLayer::new(100, Duration::from_secs(1)));
```

## Debugging Common Issues

### Tool not found

**Symptom**: `method_not_found` error when calling tool

**Cause**: 
- Tool name mismatch
- `#[tool_router]` not applied
- Method not public

**Fix**:
- Check tool name in `#[tool(name = "...")]`
- Ensure `#[tool_router]` is on the impl block
- Make method `pub async fn`

### JSON deserialization error

**Symptom**: Error parsing request or response

**Cause**:
- Missing `JsonSchema` derive
- Type mismatch
- Invalid JSON

**Fix**:
- Add `#[derive(JsonSchema, Deserialize, Serialize)]`
- Check type definitions match actual data
- Validate JSON with online tool

### Server won't start

**Symptom**: Address already in use

**Cause**: Port is occupied

**Fix**:
```bash
# Find process using port
netstat -ano | findstr :3000

# Use different port
golem-cli mcp-server start --port 3001
```

## CI/CD Integration

### GitHub Actions

```yaml
name: MCP Server Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run MCP tests
        run: cargo test --package golem-cli --test mcp_server
```

### Pre-commit Hooks

```bash
# .git/hooks/pre-commit
#!/bin/bash
cargo test --package golem-cli --test mcp_server
cargo clippy --package golem-cli -- -D warnings
cargo fmt --package golem-cli -- --check
```

## Resources

- [MCP Specification](https://spec.modelcontextprotocol.io/)
- [rust-mcp-sdk GitHub](https://github.com/rust-mcp-stack/rust-mcp-sdk)
- [Axum Documentation](https://docs.rs/axum)
- [Tokio Tutorial](https://tokio.rs/tokio/tutorial)

## Getting Help

- Check existing issues on GitHub
- Ask in Discord/Slack channel
- Review MCP specification
- Look at similar tool implementations
