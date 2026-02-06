# Golem Development Guide

## Overview

Golem is a distributed computing platform built in Rust. It uses `cargo-make` for build orchestration.

## Prerequisites

- **Rust**: Latest stable toolchain via rustup, with `wasm32-wasip1` target
- **cargo-make**: Latest version (`cargo install --force cargo-make`)
- **cargo-test-r**: Latest version (`cargo install -f --locked --git https://github.com/vigoo/test-r --branch cargo-test-r cargo-test-r`)
- **redis**: Required for tests
- **docker**: Required for integration tests

## Building

```shell
cargo make build          # Full debug build
cargo make build-release  # Full release build
cargo build -p <crate>    # Build specific crate
```

Always run `cargo make build` before starting work to ensure all dependencies are compiled.

**Note:** The SDKs in `sdks/` are not part of the main build flow. When working on SDKs, follow the specific instructions in `sdks/rust/AGENTS.md` or `sdks/ts/AGENTS.md`.

## Testing

Tests use [test-r](https://test-r.vigoo.dev). **Important:** Each test file must import `test_r::test` or tests will not run:

```rust
use test_r::test;

#[test]
fn my_test() {
    // ...
}
```

Choose the appropriate test command based on what you're changing:

**Do not run `cargo make test`** - it runs all tests and takes a very long time. Instead, choose the appropriate test command:

| Change Type | Test Command |
|-------------|--------------|
| Core logic, utilities | `cargo make unit-tests` |
| Worker executor functionality | `cargo make worker-executor-tests` |
| Service integration | `cargo make integration-tests` |
| CLI changes | `cargo make cli-tests` |
| API changes (HTTP) | `cargo make api-tests-http` |
| API changes (gRPC) | `cargo make api-tests-grpc` |

For specific tests during development:
```shell
cargo test -p <crate> <test_module> -- --report-time
```

Worker executor tests are grouped for parallel CI execution:
```shell
cargo make worker-executor-tests-group1  # Run specific group
```

## Test Components

Worker executor tests and integration tests use pre-compiled WASM files from the `test-components/` directory. These are checked into the repository and **rebuilding them is not automated**. Do not attempt to rebuild test components - use the existing compiled WASM files.

## Running Locally

Build and run the all-in-one `golem` binary from `cli/golem`:

```shell
cargo build -p golem       # Build the golem binary
./target/debug/golem       # Run locally
```

Or build everything together with `cargo make build` and run the same binary.

## Code Generation

When modifying REST API endpoints:
```shell
cargo make generate-openapi   # Regenerate OpenAPI specs
```

When modifying service configuration types:
```shell
cargo make generate-configs   # Regenerate config files
```

## Before Submitting a PR

**Always run before creating a pull request:**
```shell
cargo make fix
```

This runs `rustfmt` and `clippy` with automatic fixes. Address any remaining warnings or errors.

## Code Style

- Follow existing code conventions in the file you're editing
- Do not add unnecessary comments
- Use existing libraries and utilities from the codebase
- Security: Never expose or log secrets/keys

## WIT Dependencies

When working with WIT interfaces:
```shell
cargo make wit        # Fetch WIT dependencies
cargo make check-wit  # Verify WIT dependencies are up-to-date
```

## Debugging Tests

Use `--nocapture` when debugging tests to allow debugger attachment:
```shell
cargo test -p <crate> <test> -- --nocapture
```

## Project Structure

- `golem-worker-executor/` - Worker execution engine
- `golem-worker-service/` - Worker management service
- `golem-component-compilation-service/` - Component compiler
- `golem-shard-manager/` - Distributed shard management
- `golem-registry-service/` - Component registry
- `golem-common/` - Shared types and utilities
- `golem-wasm/` - WASM utilities
- `golem-rib/` - Rib language implementation
- `cli/` - CLI tools (golem-cli, golem)
- `sdks/` - Language-specific SDKs (Rust, TypeScript) - **not part of main build flow, see SDK-specific AGENTS.md**
- `integration-tests/` - Integration test suite
- `test-components/` - Test WASM components
