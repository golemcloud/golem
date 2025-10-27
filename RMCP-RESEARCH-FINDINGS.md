# RMCP Library Research Findings

**Research Date**: 2025-10-27
**Library**: rmcp (Rust Model Context Protocol SDK)
**Version**: 0.8.3 (latest)
**Official Repo**: https://github.com/modelcontextprotocol/rust-sdk

## Summary

RMCP is the official Rust SDK for Model Context Protocol, providing transport-agnostic server and client implementations. For Golem CLI's requirements (HTTP JSON-RPC, no stdio), we'll use **rmcp-actix-web** transport.

## Key Dependencies

```toml
[dependencies]
# Core MCP functionality
rmcp = { version = "0.8", features = ["server"] }

# HTTP/SSE transport (actix-web based)
rmcp-actix-web = "0.8"

# Already in Golem CLI
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
# For testing
reqwest = "0.11"  # MCP client testing
mockito = "1.0"   # HTTP mocking
tempfile = "3.8"  # Test fixtures
```

## Architecture Overview

```
┌─────────────────────────────────────────────────┐
│         Golem CLI (--serve flag)                │
├─────────────────────────────────────────────────┤
│  MCP Server Layer (rmcp ServerHandler)          │
│  ┌───────────┬──────────────┬─────────────┐    │
│  │  Tools    │  Resources   │ Prompts     │    │
│  │  (CLI     │  (Manifests) │ (Optional)  │    │
│  │  Commands)│              │             │    │
│  └───────────┴──────────────┴─────────────┘    │
├─────────────────────────────────────────────────┤
│  Transport Layer (rmcp-actix-web)               │
│  ┌───────────────────────────────────────┐     │
│  │  StreamableHttpService                 │     │
│  │  - SSE endpoint (GET /sse)            │     │
│  │  - JSON-RPC endpoint (POST /message)  │     │
│  │  - Session management                  │     │
│  └───────────────────────────────────────┘     │
├─────────────────────────────────────────────────┤
│  HTTP Server (actix-web)                        │
│  Listening on localhost:<port>                  │
└─────────────────────────────────────────────────┘
```

## Implementation Patterns

### 1. Server Handler (Core MCP Logic)

```rust
use rmcp::prelude::*;

pub struct GolemMcpServer {
    // Golem CLI context
    context: Arc<Context>,
    // Track authenticated client
    client_id: Arc<tokio::sync::Mutex<Option<String>>>,
}

#[tool_handler]
impl ServerHandler for GolemMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()      // CLI commands as tools
                .enable_resources()  // Manifest files
                .build(),
            server_info: Implementation {
                name: "golem-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "Golem CLI MCP Server. Exposes CLI commands as tools and manifest files as resources."
                .to_string()
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(self.get_info())
    }
}
```

### 2. Tool Registration (Commands as Tools)

```rust
#[tool_router]
impl GolemMcpServer {
    /// List all components
    #[tool(description = "List all Golem components in the current project")]
    async fn component_list(
        &self,
        Parameters(params): Parameters<ComponentListParams>,
    ) -> Result<CallToolResult, McpError> {
        // Execute underlying CLI command
        let result = execute_cli_command(
            &self.context,
            vec!["component", "list"],
            params,
        ).await?;

        Ok(CallToolResult::success(vec![
            Content::text(result.stdout)
        ]))
    }

    /// Add a new component
    #[tool(description = "Add a new component to the Golem project")]
    async fn component_add(
        &self,
        Parameters(params): Parameters<ComponentAddParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate input (security!)
        validate_component_name(&params.name)?;

        let result = execute_cli_command(
            &self.context,
            vec!["component", "add", &params.name],
            params,
        ).await?;

        Ok(CallToolResult::success(vec![
            Content::text(result.stdout)
        ]))
    }

    // ... more tools for each CLI command
}
```

### 3. Resource Exposure (Manifests)

```rust
#[tool_handler]
impl ServerHandler for GolemMcpServer {
    async fn list_resources(
        &self,
        _request: ListResourcesRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = discover_manifests(&self.context).await?;

        Ok(ListResourcesResult {
            resources: resources.into_iter().map(|path| {
                Resource {
                    uri: format!("file://{}", path.display()),
                    name: path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    description: Some("Golem manifest file".to_string()),
                    mime_type: Some("application/yaml".to_string()),
                }
            }).collect(),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        // Security: validate path doesn't escape project
        let path = validate_resource_path(&request.uri)?;

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| McpError::internal_error(
                format!("Failed to read resource: {}", e),
                None
            ))?;

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::Text {
                uri: request.uri,
                mime_type: Some("application/yaml".to_string()),
                text: content,
            }],
        })
    }
}
```

### 4. HTTP Transport Setup (actix-web)

```rust
use rmcp_actix_web::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use actix_web::{App, HttpServer, web};
use std::{sync::Arc, time::Duration};

pub async fn serve_mcp_server(
    context: Arc<Context>,
    port: u16,
) -> anyhow::Result<()> {
    // Create MCP service
    let mcp_service = Arc::new(GolemMcpServer {
        context: context.clone(),
        client_id: Arc::new(tokio::sync::Mutex::new(None)),
    });

    // Setup HTTP transport with SSE
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || {
            Ok(mcp_service.clone())
        }))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    // Start actix-web server
    HttpServer::new(move || {
        App::new()
            .service(
                web::scope("/mcp")
                    .service(http_service.clone().scope())
            )
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await?;

    Ok(())
}
```

### 5. CLI Integration

```rust
// In command.rs - add to GolemCliGlobalFlags
#[derive(Debug, Clone, Default, Args)]
pub struct GolemCliGlobalFlags {
    // ... existing flags ...

    /// Start MCP server on specified port
    #[arg(long, global = true, display_order = 112)]
    pub serve: Option<u16>,
}

// In main.rs
async fn main() -> ExitCode {
    let args = GolemCliCommand::parse();

    // Check if MCP server mode
    if let Some(port) = args.global_flags.serve {
        // Start MCP server instead of normal CLI
        return match mcp_server::serve(context, port).await {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("MCP Server error: {}", e);
                ExitCode::FAILURE
            }
        };
    }

    // Normal CLI execution
    // ...
}
```

## Security Considerations

### Input Validation
```rust
fn validate_component_name(name: &str) -> Result<(), McpError> {
    // Prevent path traversal
    if name.contains("..") || name.contains("/") || name.contains("\\") {
        return Err(McpError::invalid_params(
            "Invalid component name: path traversal detected",
            None
        ));
    }

    // Alphanumeric + hyphen/underscore only
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(McpError::invalid_params(
            "Invalid component name: use alphanumeric, hyphen, or underscore only",
            None
        ));
    }

    Ok(())
}

fn validate_resource_path(uri: &str) -> Result<PathBuf, McpError> {
    // Must be file:// URI
    let path = uri.strip_prefix("file://")
        .ok_or_else(|| McpError::invalid_params(
            "Resource URI must use file:// scheme",
            None
        ))?;

    let path = PathBuf::from(path);

    // Must be absolute or within project
    let canonical = path.canonicalize()
        .map_err(|e| McpError::internal_error(
            format!("Invalid path: {}", e),
            None
        ))?;

    // Verify it's under project root
    let project_root = std::env::current_dir()
        .map_err(|e| McpError::internal_error(
            format!("Cannot determine project root: {}", e),
            None
        ))?;

    if !canonical.starts_with(&project_root) {
        return Err(McpError::invalid_params(
            "Resource path outside project directory",
            None
        ));
    }

    Ok(canonical)
}
```

### Sensitive Command Filtering
```rust
fn is_command_safe_to_expose(command: &str) -> bool {
    // Don't expose commands that deal with secrets
    const UNSAFE_COMMANDS: &[&str] = &[
        "profile",  // Contains auth tokens
        // Add more as needed
    ];

    !UNSAFE_COMMANDS.contains(&command)
}
```

## Progress Notifications (Incremental Output)

```rust
use rmcp::protocol::NotificationParams;

async fn execute_with_progress(
    &self,
    command: Vec<&str>,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResult, McpError> {
    // Capture stdout/stderr streams
    let mut cmd = tokio::process::Command::new("golem-cli")
        .args(&command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| McpError::internal_error(
            format!("Failed to spawn command: {}", e),
            None
        ))?;

    // Stream output as notifications
    if let Some(mut stdout) = cmd.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        while reader.read_line(&mut line).await? > 0 {
            // Send progress notification
            context.send_notification(
                "notifications/progress",
                serde_json::json!({
                    "progress": {
                        "message": line.trim(),
                    }
                })
            ).await?;

            line.clear();
        }
    }

    let output = cmd.wait_with_output().await?;

    Ok(CallToolResult::success(vec![
        Content::text(String::from_utf8_lossy(&output.stdout).to_string())
    ]))
}
```

## Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_info() {
        let server = GolemMcpServer::new(test_context());
        let info = server.get_info();

        assert_eq!(info.server_info.name, "golem-cli");
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
    }

    #[tokio::test]
    async fn test_input_validation() {
        assert!(validate_component_name("valid-name").is_ok());
        assert!(validate_component_name("../etc/passwd").is_err());
        assert!(validate_component_name("name;rm -rf /").is_err());
    }
}
```

### Integration Tests
```rust
#[tokio::test]
async fn test_full_mcp_workflow() {
    // Start test server
    let server = spawn_test_server().await;

    // Create MCP client
    let client = create_test_client(server.url()).await;

    // Initialize
    let init_response = client.initialize().await?;
    assert!(init_response.capabilities.tools.is_some());

    // List tools
    let tools = client.list_tools().await?;
    assert!(tools.iter().any(|t| t.name == "component_list"));

    // Execute tool
    let result = client.call_tool("component_list", json!({})).await?;
    assert!(result.is_success());

    // List resources
    let resources = client.list_resources().await?;
    assert!(!resources.is_empty());

    // Read resource
    let content = client.read_resource(&resources[0].uri).await?;
    assert!(!content.is_empty());
}
```

## File Structure

```
cli/golem-cli/src/
├── mcp_server/
│   ├── mod.rs              # Main server setup
│   ├── server.rs           # ServerHandler implementation
│   ├── tools/
│   │   ├── mod.rs
│   │   ├── component.rs    # Component-related tools
│   │   ├── worker.rs       # Worker-related tools
│   │   ├── app.rs          # App-related tools
│   │   └── generator.rs    # Auto-generate from Clap
│   ├── resources/
│   │   ├── mod.rs
│   │   ├── discovery.rs    # Manifest discovery
│   │   └── validation.rs   # Path validation
│   ├── notifications.rs    # Progress notifications
│   └── security.rs         # Input validation
└── tests/
    └── mcp_server/
        ├── initialization_tests.rs
        ├── tools_tests.rs
        ├── resources_tests.rs
        └── integration_tests.rs
```

## Next Steps

1. ✅ Research complete
2. Start Phase 1 RED: Write initialization tests
3. Implement minimal server to pass tests
4. Continue TDD cycle through all phases

## References

- RMCP Docs: https://docs.rs/rmcp/
- RMCP Actix Web: https://docs.rs/rmcp-actix-web/
- MCP Protocol: https://modelcontextprotocol.io/
- Example: https://www.shuttle.dev/blog/2025/08/13/sse-mcp-server-with-oauth-in-rust
