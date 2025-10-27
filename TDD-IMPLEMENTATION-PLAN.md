# Golem MCP Server - TDD Implementation Plan

**Session ID**: session-1761591077751-bl8g0w3yi
**Swarm ID**: swarm-1761591077750-i4ty50is2
**Strategy**: Test-Driven Development (Red-Green-Refactor)
**Started**: 2025-10-27

## TDD Methodology

### Red-Green-Refactor Cycle
1. **RED**: Write failing test that defines desired behavior
2. **GREEN**: Write minimal code to make test pass
3. **REFACTOR**: Improve code quality while keeping tests green

### Test Levels
1. **Unit Tests**: Individual functions and components
2. **Integration Tests**: Component interactions
3. **E2E Tests**: Full MCP server functionality
4. **Security Tests**: Input validation and edge cases

## Phase 1: MCP Server Foundation (TDD)

### Test Suite 1.1: Server Initialization
```rust
// tests/mcp_server/initialization_tests.rs

#[test]
fn test_server_creates_with_valid_port() {
    // RED: Define expected behavior
}

#[test]
fn test_server_rejects_invalid_port() {
    // RED: Port validation
}

#[test]
fn test_server_starts_http_endpoint() {
    // RED: HTTP server startup
}

#[test]
fn test_server_handles_graceful_shutdown() {
    // RED: Clean shutdown
}
```

**Implementation Steps**:
1. Write failing tests
2. Create `cli/golem-cli/src/mcp_server/mod.rs`
3. Implement minimal `McpServer` struct
4. Add `--serve` flag to CLI
5. Make tests pass
6. Refactor for clarity

### Test Suite 1.2: JSON-RPC Protocol
```rust
// tests/mcp_server/jsonrpc_tests.rs

#[test]
fn test_handles_jsonrpc_initialize_request() {
    // RED: MCP initialize method
}

#[test]
fn test_rejects_invalid_jsonrpc_format() {
    // RED: Error handling
}

#[test]
fn test_returns_valid_jsonrpc_response() {
    // RED: Response format
}

#[test]
fn test_handles_concurrent_requests() {
    // RED: Async handling
}
```

## Phase 2: Tool Exposure (TDD)

### Test Suite 2.1: Tool Discovery
```rust
// tests/mcp_server/tool_discovery_tests.rs

#[test]
fn test_lists_all_available_tools() {
    // RED: tools/list endpoint
}

#[test]
fn test_tool_has_valid_json_schema() {
    // RED: Schema generation
}

#[test]
fn test_tool_includes_clap_metadata() {
    // RED: Clap integration
}

#[test]
fn test_filters_security_sensitive_commands() {
    // RED: Security filtering
}
```

**Implementation Steps**:
1. Write test for listing tools
2. Create `mcp_server/tools.rs`
3. Implement tool metadata extraction from Clap
4. Add JSON Schema generation
5. Tests pass
6. Refactor for maintainability

### Test Suite 2.2: Tool Execution
```rust
// tests/mcp_server/tool_execution_tests.rs

#[test]
fn test_executes_component_list_command() {
    // RED: Basic command execution
}

#[test]
fn test_validates_required_parameters() {
    // RED: Parameter validation
}

#[test]
fn test_returns_command_output() {
    // RED: Output capture
}

#[test]
fn test_handles_command_errors() {
    // RED: Error propagation
}

#[test]
fn test_sanitizes_user_input() {
    // RED: Security validation
}
```

## Phase 3: Resource Exposure (TDD)

### Test Suite 3.1: Resource Discovery
```rust
// tests/mcp_server/resource_discovery_tests.rs

#[test]
fn test_discovers_golem_yaml_in_current_dir() {
    // RED: Current directory scan
}

#[test]
fn test_discovers_manifests_in_parent_dirs() {
    // RED: Ancestor traversal
}

#[test]
fn test_discovers_manifests_in_child_dirs() {
    // RED: Child traversal
}

#[test]
fn test_includes_component_manifests() {
    // RED: Component discovery
}

#[test]
fn test_respects_gitignore_patterns() {
    // RED: File filtering
}
```

### Test Suite 3.2: Resource Reading
```rust
// tests/mcp_server/resource_reading_tests.rs

#[test]
fn test_reads_manifest_file_content() {
    // RED: File reading
}

#[test]
fn test_returns_correct_mime_type() {
    // RED: MIME type detection
}

#[test]
fn test_handles_missing_resource() {
    // RED: Error handling
}

#[test]
fn test_follows_symlinks_safely() {
    // RED: Security consideration
}
```

## Phase 4: Incremental Output (TDD)

### Test Suite 4.1: Progress Notifications
```rust
// tests/mcp_server/notifications_tests.rs

#[test]
fn test_sends_progress_notification() {
    // RED: Notification system
}

#[test]
fn test_captures_stdout_during_execution() {
    // RED: Output capture
}

#[test]
fn test_captures_stderr_separately() {
    // RED: Error stream capture
}

#[test]
fn test_notifies_completion() {
    // RED: Completion events
}

#[test]
fn test_handles_long_running_commands() {
    // RED: Async progress
}
```

## Phase 5: Integration & E2E Tests

### Test Suite 5.1: End-to-End Scenarios
```rust
// tests/integration/e2e_tests.rs

#[test]
fn test_full_mcp_client_workflow() {
    // 1. Connect to server
    // 2. Initialize MCP session
    // 3. List tools
    // 4. Execute tool
    // 5. Read resource
    // 6. Disconnect
}

#[test]
fn test_claude_code_integration() {
    // Simulate Claude Code MCP client
}

#[test]
fn test_multiple_concurrent_clients() {
    // Stress test
}
```

### Test Suite 5.2: Security Tests
```rust
// tests/security/validation_tests.rs

#[test]
fn test_rejects_path_traversal_attempts() {
    // ../../../etc/passwd
}

#[test]
fn test_rejects_command_injection() {
    // ; rm -rf /
}

#[test]
fn test_rate_limiting() {
    // DDoS protection
}

#[test]
fn test_sanitizes_all_user_inputs() {
    // Input validation
}
```

## Development Workflow

### Hive Mind Worker Assignments

**Queen (Strategic Coordinator)**:
- Overall TDD strategy
- Test coverage monitoring
- Integration coordination

**Researcher Worker**:
- Study `rmcp` library API
- Research MCP protocol spec
- Investigate Rust testing patterns
- Document findings

**Coder Worker**:
- Write implementation code
- Follow RED-GREEN-REFACTOR
- Maintain test coverage
- Implement security measures

**Analyst Worker**:
- Review test coverage
- Identify edge cases
- Performance analysis
- Security audit

**Tester Worker**:
- Write comprehensive tests
- Design E2E scenarios
- Create test fixtures
- Validate all paths

### Git Commit Strategy

```bash
# After each RED phase
git commit -m "Swarm: TDD RED - Add tests for [feature]"

# After each GREEN phase
git commit -m "Swarm: TDD GREEN - Implement [feature] to pass tests"

# After each REFACTOR phase
git commit -m "Swarm: TDD REFACTOR - Improve [component] code quality"
```

## Test Coverage Goals

### Minimum Requirements
- **Unit Test Coverage**: 80%+
- **Integration Test Coverage**: 70%+
- **E2E Test Coverage**: 90%+ of user scenarios
- **Security Test Coverage**: 100% of input paths

### Quality Metrics
- All tests must pass before commit
- No flaky tests allowed
- Fast test execution (<5 seconds for unit tests)
- Clear test names and documentation

## Dependencies to Add

```toml
[dependencies]
rmcp = "0.1"  # Official MCP Rust SDK

[dev-dependencies]
mockito = "1.0"  # HTTP mocking
tempfile = "3.8"  # Temporary test files
proptest = "1.4"  # Property-based testing
criterion = "0.5"  # Benchmarking
```

## Testing Tools

1. **cargo test**: Standard Rust testing
2. **cargo tarpaulin**: Code coverage
3. **cargo audit**: Security vulnerabilities
4. **cargo clippy**: Linting
5. **cargo bench**: Performance benchmarks

## Success Criteria (TDD)

Before considering phase complete:
- [ ] All tests written (RED phase)
- [ ] All tests passing (GREEN phase)
- [ ] Code refactored (REFACTOR phase)
- [ ] Coverage meets minimum %
- [ ] No clippy warnings
- [ ] No security vulnerabilities
- [ ] Documentation complete
- [ ] Performance benchmarks pass

## Monitoring Progress

```bash
# Check hive mind status
npx claude-flow@alpha hive-mind status

# View test results
cargo test --all

# Check coverage
cargo tarpaulin --out Html

# Run security audit
cargo audit

# Run benchmarks
cargo bench
```

## Timeline (TDD Approach)

### Week 1: Foundation
- **Day 1-2**: Phase 1 Tests + Implementation (Server basics)
- **Day 3-4**: Phase 2.1 Tests + Implementation (Tool discovery)
- **Day 5**: Phase 2.2 Tests + Implementation (Tool execution)

### Week 2: Features
- **Day 6-7**: Phase 3 Tests + Implementation (Resources)
- **Day 8-9**: Phase 4 Tests + Implementation (Notifications)
- **Day 10**: Integration testing

### Week 3: Polish
- **Day 11-12**: E2E tests + Security tests
- **Day 13**: Performance optimization
- **Day 14**: Documentation + Demo video
- **Day 15**: PR preparation

## Red Flags to Watch

- Tests passing by accident (false positives)
- Skipping RED phase (writing code before tests)
- Low test coverage (<70%)
- Flaky tests
- Slow test execution
- Security tests not comprehensive

## Notes

- **Commit after each cycle**: RED → commit, GREEN → commit, REFACTOR → commit
- **Keep tests independent**: Each test should run in isolation
- **Use test fixtures**: Create reusable test data
- **Mock external dependencies**: Don't rely on network/filesystem
- **Document test intent**: Clear test names and comments

## References

- TDD Kent Beck: "Test-Driven Development By Example"
- Rust Testing: https://doc.rust-lang.org/book/ch11-00-testing.html
- rmcp Library: (to be researched)
- MCP Protocol: https://modelcontextprotocol.io/
