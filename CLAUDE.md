# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Golem is a distributed cloud platform for running WebAssembly components. This is a **Rust monorepo** with 25+ workspace members including services, CLI tools, SDKs, and integration tests.

**Key Documentation:**
- Developer guide: [CONTRIBUTING.md](CONTRIBUTING.md)
- REST API changes: Auto-generated OpenAPI specs (must regenerate after API changes)
- MCP Server: [MCP-SERVER-IMPLEMENTATION.md](MCP-SERVER-IMPLEMENTATION.md)

## Build System

**Primary build tool**: `cargo-make` (not standard cargo commands)

### Essential Commands

```bash
# Development workflow (recommended)
cargo make dev-flow              # Fix format/clippy + build (no tests)
cargo make                       # Same as dev-flow (default task)

# Building
cargo make build                 # Debug build (all workspace members)
cargo make build-release         # Release build
cargo build -p <crate-name>      # Build single crate during development

# Code quality
cargo make fix                   # Auto-fix rustfmt + clippy issues
cargo make check                 # Check without applying fixes

# Testing
cargo make unit-tests            # Unit tests only
cargo make worker-executor-tests # Worker executor tests (requires redis)
cargo make integration-tests     # Integration tests (requires docker + redis)
cargo make cli-integration-tests # CLI tests
cargo make test                  # ALL tests (unit + worker + integration)

# Run specific test
cargo test -p golem-worker-executor api::promise -- --report-time

# API/Config management
cargo make generate-openapi      # Regenerate after REST API changes (REQUIRED)
cargo make generate-configs      # Regenerate after config struct changes (REQUIRED)
cargo make check-openapi         # Verify OpenAPI is up-to-date (CI check)
cargo make check-configs         # Verify configs are up-to-date (CI check)

# WIT dependencies
cargo make wit                   # Fetch WIT dependencies from wit/deps.toml
cargo make check-wit             # Verify WIT deps are up-to-date

# Local development
cargo make run                   # Run all services locally (requires lnav, nginx, redis)
```

### Memory Constraints

If cargo runs out of memory during builds, create `~/.cargo/config.toml`:

```toml
[build]
jobs = 4  # Limit parallel compilation jobs
```

## Architecture

### Service Structure

**Core Services** (microservices architecture):
- `golem-worker-executor` - Executes WASM components in isolated environments
- `golem-worker-service` - Worker lifecycle management and API
- `golem-component-service` - Component registry and versioning
- `golem-component-compilation-service` - WASM compilation pipeline
- `golem-shard-manager` - Distributed system coordination
- `golem-debugging-service` - Debug worker instances
- `cloud-service` - Cloud platform orchestration

**CLI Tools**:
- `cli/golem-cli` - Main CLI with MCP server (`golem-cli --serve 8080`)
- `cli/golem` - Single-binary distribution
- `cli/golem-templates` - Project templates and scaffolding

**Libraries**:
- `golem-common` - Shared types and utilities
- `golem-api-grpc` - gRPC protocol definitions
- `golem-client` - Generated OpenAPI client (auto-generated, DO NOT edit manually)
- `golem-service-base` - Common service infrastructure
- `golem-wasm` - WASM runtime abstractions
- `golem-rib` - Golem's expression language (RIB - Rust In Bytecode)

**SDKs**:
- `sdks/rust/golem-rust` - Rust SDK for building Golem components
- `sdks/ts` - TypeScript SDK

### Key Patterns

1. **OpenAPI-First REST APIs**: All REST APIs use `poem-openapi` crate. Specs are generated from Rust code, not written manually.

2. **WIT Dependencies**: WebAssembly Interface Types managed in `wit/deps.toml`, copied to workspace members via `cargo make wit`.

3. **Configuration**: Service configs generated from Rust structs using `figment` crate.

4. **Testing**: Uses `test-r` library. Tests organized into groups for parallel CI execution.

5. **Wasmtime Fork**: Uses custom Wasmtime fork (`golem-wasmtime-v33.0.0` branch) with Golem-specific patches.

## MCP Server Integration

The CLI includes an MCP (Model Context Protocol) server:

```bash
golem-cli --serve 8080  # Start MCP server on port 8080
```

**Capabilities**:
- **96 tools**: All CLI commands exposed as MCP tools (16 sensitive commands filtered)
- **Resources**: Discovers `golem.yaml` manifests in current/parent/child directories
- **Transport**: HTTP JSON-RPC with Server-Sent Events (SSE)
- **Endpoint**: `http://localhost:8080/mcp`
- **Implementation**: `cli/golem-cli/src/mcp_server/`

See [MCP-SERVER-IMPLEMENTATION.md](MCP-SERVER-IMPLEMENTATION.md) for details.

## Development Workflow

### Before Opening PR

1. **Fix code quality issues**:
   ```bash
   cargo make fix
   ```

2. **If you modified REST API** (any `poem-openapi` changes):
   ```bash
   cargo make generate-openapi
   git add openapi/
   ```

3. **If you modified service configs** (changed config structs):
   ```bash
   cargo make generate-configs
   git add */config/
   ```

4. **Verify CI checks will pass**:
   ```bash
   cargo make check           # rustfmt + clippy
   cargo make check-openapi   # OpenAPI up-to-date
   cargo make check-configs   # Configs up-to-date
   cargo make check-wit       # WIT deps up-to-date
   ```

### Test Organization

**Worker Executor Tests** are split into groups:
- `cargo make worker-executor-tests-group1` - Group 1/2
- `cargo make worker-executor-tests-group2` - Group 2/2
- `cargo make worker-executor-tests-misc` - Untagged + RDBMS tests

**Integration Tests** are split into groups:
- `cargo make integration-tests-group1` - Main integration tests
- `cargo make integration-tests-group2` - Service-specific tests
- `cargo make integration-tests-group3` - Sharding tests

**CLI Tests** are split into groups:
- `cargo make cli-integration-tests-group1` - Untagged + Group 1
- `cargo make cli-integration-tests-group2` - Group 2
- `cargo make cli-integration-tests-group3` - Group 3

## Common Pitfalls

1. **Don't manually edit generated files**:
   - `golem-client/src/` - Generated from OpenAPI spec
   - `openapi/*.yaml` - Generated from Rust code
   - Service config files in `*/config/` - Generated from Rust structs

2. **Always regenerate after changes**:
   - Modified REST API → `cargo make generate-openapi`
   - Modified config structs → `cargo make generate-configs`
   - Modified WIT deps → `cargo make wit`

3. **Use cargo-make, not plain cargo** for most tasks:
   - ❌ `cargo test` (won't run all test groups)
   - ✅ `cargo make test` (runs all test groups in sequence)

4. **Debugger setup**: Use `--nocapture` flag when debugging tests to prevent child process spawning.

5. **Prerequisites**: Ensure `protoc` (version 28+), `redis`, and `docker` are installed.

## Rust Toolchain

**Required**:
- Rust stable (latest)
- `wasm32-wasip1` target
- `cargo-make` installed globally
- `protoc` v28+ (Protocol Buffers compiler)

**Installation**:
```bash
rustup update stable
rustup default stable
rustup target add wasm32-wasip1
cargo install --force cargo-make
```

## Project-Specific Patterns

### Service Configuration
All services use `figment` for config management with three sources (priority order):
1. Environment variables (highest)
2. Config files (TOML)
3. Code defaults (lowest)

### Error Handling
- Services use `anyhow::Result` for error propagation
- Public APIs use typed error enums with `thiserror`

### Async Runtime
- All services use `tokio` runtime
- No `async-std` or other runtimes

### Logging
- `tracing` crate for structured logging
- File logging configured via `GOLEM__TRACING__FILE_DIR`
- Use `lnav` for merged log viewing during local development
