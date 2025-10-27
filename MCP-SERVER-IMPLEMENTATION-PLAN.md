# Golem CLI MCP Server Implementation Plan

**Bounty**: $3,500 (PARTIALLY CLAIMED - 2 successful PRs already merged)
**Issue**: https://github.com/golemcloud/golem/issues/1926
**Started**: 2025-10-27
**Status**: Issue still OPEN despite partial completions

## ⚠️ CRITICAL DISCOVERY
Two PRs have already been successfully merged and rewarded:
- PR #290 by @webbdays - Used `rmcp` Rust library
- PR #319/#322 by @fjkiani - SSE wiring and tests

**STOP**: Need to verify if bounty is still available or if the work is complete.
The golem-cli repository was merged into main golem repository on August 15, 2025.

## Overview
Implement an MCP (Model Context Protocol) Server into the Golem CLI tool, enabling AI agents like Claude Code to perform all Golem CLI operations through a standardized interface.

## Requirements

### Core Functionality
1. **Server Mode Flag**: Add `--serve` command flag that enables MCP Server mode on a specified port
2. **HTTP JSON-RPC Endpoint**: Support HTTP JSON-RPC endpoint on `localhost:<port>` (no stdio support)
3. **Incremental Output**: Support for long-running commands via logging/notifications
4. **Manifest Hierarchy**: Expose hierarchy of manifest files and referenceable files as MCP resources
5. **Command Tools**: Expose different CLI commands as different MCP tools
6. **End-to-End Tests**: Comprehensive testing between MCP Server and Client

### Technical Approach
- Leverage existing Clap metadata to reduce maintenance burden
- Create custom MCP tools for optimal agent experience
- Ensure all Golem CLI operations are accessible via MCP interface

## Architecture Analysis

### Golem CLI Structure
```
golem/
├── cli/
│   └── golem-cli/
│       ├── Cargo.toml (main dependencies, features, build config)
│       ├── src/
│       │   ├── main.rs (entry point, tokio runtime)
│       │   ├── command.rs (Clap command definitions, ~2000 lines)
│       │   ├── command_handler/ (command implementations)
│       │   ├── lib.rs (library exports)
│       │   ├── config.rs (profile and config management)
│       │   ├── context.rs (execution context)
│       │   └── ...
│       └── tests/ (integration tests)
```

### Existing Commands (from command.rs)
The CLI uses Clap with the following subcommand structure:
- `app` - Application management
- `component` - Component management
- `agent` (worker) - Worker/agent management
- `api` - API definitions
- `cloud` - Cloud operations
- `profile` - Profile management
- `plugin` - Plugin management
- `server` (conditional) - Server commands (behind `server-commands` feature flag)

### Key Files
- `cli/golem-cli/Cargo.toml`: Dependencies include MCP-relevant crates we'll need to add
- `cli/golem-cli/src/command.rs`: ~2000 lines of Clap command definitions
- `cli/golem-cli/src/main.rs`: Entry point using tokio runtime
- `cli/golem-cli/src/command_handler/`: Individual command implementations

## Implementation Plan

### Phase 1: MCP Server Foundation
**Goal**: Set up basic MCP server infrastructure

1. **Add MCP Dependencies** to `Cargo.toml`:
   ```toml
   # MCP Server support
   jsonrpc-core = "18.0"
   jsonrpc-http-server = "18.0"
   mcp-protocol = { version = "0.1", features = ["server"] }
   ```

2. **Create MCP Server Module** (`src/mcp_server/mod.rs`):
   - HTTP JSON-RPC server setup
   - Request/response handling
   - Connection management
   - Error handling

3. **Add `--serve` Flag** to `GolemCliGlobalFlags` in `command.rs`:
   ```rust
   /// Start MCP server on specified port
   #[arg(long, global = true, display_order = 112)]
   pub serve: Option<u16>,
   ```

### Phase 2: Command-to-Tool Mapping
**Goal**: Expose Clap commands as MCP tools

1. **Create Tool Generator** (`src/mcp_server/tools.rs`):
   - Parse Clap `Command` metadata
   - Generate MCP tool definitions
   - Map Clap arguments to MCP tool parameters
   - Handle subcommands and nested structures

2. **Tool Naming Convention**:
   - Top-level: `golem_<command>` (e.g., `golem_component`)
   - Nested: `golem_<command>_<subcommand>` (e.g., `golem_component_add`)

3. **Parameter Mapping**:
   - Clap required args → MCP required parameters
   - Clap optional args → MCP optional parameters
   - Preserve help text and documentation

### Phase 3: Resource Exposure
**Goal**: Expose manifest files as MCP resources

1. **Manifest Discovery** (`src/mcp_server/resources.rs`):
   - Scan current directory for `golem.yaml`
   - Traverse parent directories
   - Traverse child directories
   - Identify component manifests

2. **Resource Definitions**:
   ```json
   {
     "uri": "file:///path/to/golem.yaml",
     "name": "Application Manifest",
     "description": "Root application manifest",
     "mimeType": "application/yaml"
   }
   ```

3. **File Exposure**:
   - All manifest files (golem.yaml, component.yaml, etc.)
   - Referenced files (WASM components, WIT files, etc.)
   - Build outputs

### Phase 4: Incremental Output
**Goal**: Support long-running commands with progress updates

1. **Logging/Notification System** (`src/mcp_server/notifications.rs`):
   - Capture CLI output streams (stdout/stderr)
   - Send incremental updates via MCP notifications
   - Progress tracking for long operations

2. **Integration Points**:
   - Hook into existing logging system (`src/log.rs`)
   - Capture command execution progress
   - Send JSON-RPC notifications to clients

### Phase 5: Testing
**Goal**: Comprehensive E2E testing

1. **MCP Client Test Harness** (`tests/mcp_integration.rs`):
   - Create test MCP client
   - Test all exposed tools
   - Verify resource discovery
   - Test incremental output
   - Error handling scenarios

2. **Test Coverage**:
   - All major command families
   - Resource discovery and retrieval
   - Long-running operations
   - Error cases
   - Concurrent requests

## Development Workflow

### Using Claude Flow SPARC Methodology

```bash
cd /Users/michaeloboyle/Documents/github/golem

# Initialize swarm for coordinated development
npx claude-flow@alpha swarm init --topology hierarchical --max-agents 8

# Spawn specialized agents
npx claude-flow@alpha agent spawn --type researcher "Rust MCP implementation patterns"
npx claude-flow@alpha agent spawn --type architect "MCP server architecture design"
npx claude-flow@alpha agent spawn --type coder "Implement MCP server infrastructure"
npx claude-flow@alpha agent spawn --type tester "E2E MCP testing strategy"
```

### Checkpoints and Git Commits

Following swarm state persistence requirements:
```bash
# After each major milestone
git add . && git commit -m "Swarm: [Component] [Action] [Result]"
```

Example commit messages:
- `Swarm: MCP Server - Add dependencies and module structure`
- `Swarm: Tool Generator - Implement Clap-to-MCP mapping`
- `Swarm: Resources - Add manifest discovery system`
- `Swarm: Notifications - Implement incremental output support`
- `Swarm: Tests - Add E2E MCP client tests`

## Technical Considerations

### Rust Crates Needed
1. **jsonrpc-core**: JSON-RPC 2.0 protocol implementation
2. **jsonrpc-http-server**: HTTP server for JSON-RPC
3. **serde_json**: JSON serialization (already in project)
4. **tokio**: Async runtime (already in project)

### MCP Protocol Implementation
- Use standard MCP protocol v1.0
- Follow JSON-RPC 2.0 specification
- Implement required MCP methods:
  - `initialize`
  - `tools/list`
  - `tools/call`
  - `resources/list`
  - `resources/read`
  - `notifications/progress`

### Clap Metadata Extraction
The existing Clap command structure provides:
- Command names and descriptions
- Argument names, types, and help text
- Required vs optional arguments
- Default values
- Value validators

This metadata can be programmatically extracted and transformed into MCP tool schemas.

## Success Criteria

### Must Have
- [x] Server starts with `--serve <port>` flag
- [ ] All CLI commands exposed as MCP tools
- [ ] Manifest files exposed as MCP resources
- [ ] HTTP JSON-RPC endpoint working on localhost
- [ ] Incremental output for long-running commands
- [ ] E2E tests passing
- [ ] Documentation updated

### Should Have
- [ ] Efficient tool discovery and caching
- [ ] Clear error messages
- [ ] Performance benchmarks
- [ ] Integration examples

### Nice to Have
- [ ] Hot reload of manifest changes
- [ ] WebSocket support for real-time updates
- [ ] Tool usage analytics

## Testing Strategy

### Local Testing Before PR
1. **Build and run locally**:
   ```bash
   cargo build --release
   ./target/release/golem-cli --serve 8080
   ```

2. **Test with MCP client**:
   ```bash
   # Create test client script
   curl -X POST http://localhost:8080 \
     -H "Content-Type: application/json" \
     -d '{"jsonrpc": "2.0", "method": "tools/list", "id": 1}'
   ```

3. **Benchmark performance**:
   - Measure tool listing time
   - Test resource discovery performance
   - Verify incremental output latency

### Integration Testing
- Test with Claude Code MCP client
- Verify all commands work via MCP
- Test concurrent requests
- Validate error handling

## Red Flags to Watch For

Based on bounty work protocol:
- Performance < 5% improvement (if optimizing existing code)
- Tests fail after changes
- Maintainers express concerns about approach
- High variance in benchmarks

## Documentation Needs

1. **User Guide**: How to start and use MCP server
2. **Developer Guide**: How the implementation works
3. **API Reference**: All available tools and resources
4. **Examples**: Integration with Claude Code and other MCP clients

## Timeline

### Week 1: Foundation and Architecture
- Set up MCP server infrastructure
- Implement basic tool exposure
- Resource discovery system

### Week 2: Implementation and Testing
- Complete tool-command mapping
- Incremental output support
- E2E tests

### Week 3: Polish and Documentation
- Performance optimization
- Documentation
- PR preparation

## Notes

- Keep development artifacts (.claude/, .swarm/) out of the PR
- Use .gitignore to exclude tracking files from the start
- Follow Golem's contribution guidelines
- Test thoroughly before submission
- Be prepared to iterate based on maintainer feedback

## References

- MCP Protocol Spec: https://modelcontextprotocol.io/
- Golem Docs: https://learn.golem.cloud
- Clap Documentation: https://docs.rs/clap/
- JSON-RPC 2.0 Spec: https://www.jsonrpc.org/specification
