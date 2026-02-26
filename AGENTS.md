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

**Note:** The SDKs in `sdks/` are not part of the main build flow. Load the `sdk-development` skill when working on SDKs.

## Testing

Tests use [test-r](https://test-r.vigoo.dev). **Important:** Each test file must import `test_r::test` or tests will not run:

```rust
use test_r::test;

#[test]
fn my_test() {
    // ...
}
```

**Do not run `cargo make test`** - it runs all tests and takes a very long time. Instead, choose the appropriate test command:

| Change Type | Test Command |
|-------------|--------------|
| Core logic, utilities | `cargo make unit-tests` |
| Worker executor functionality | `cargo make worker-executor-tests` |
| Service integration | `cargo make integration-tests` |
| CLI changes | `cargo make cli-tests` |

**Whenever tests are modified, always run the affected tests to verify they still pass before considering the task complete.**

For specific tests during development:
```shell
cargo test -p <crate> <test_module> -- --report-time
```

## Test Components

Worker executor tests and integration tests use pre-compiled WASM files from the `test-components/` directory. These are checked into the repository and **rebuilding them is not automated**. Do not attempt to rebuild test components - use the existing compiled WASM files, EXCEPT if the test component itself has an AGENTS.md file with instructions of how to do so.

Load the `modifying-test-components` skill when rebuilding is needed.

## Running Locally

```shell
cargo build -p golem       # Build the golem binary
./target/debug/golem       # Run locally
```

## Skills

Load these skills for guided workflows on complex tasks:

| Skill | When to Use |
|-------|-------------|
| `modifying-http-endpoints` | Adding or modifying REST API endpoints (covers OpenAPI regeneration, golem-client rebuild, type mappings) |
| `adding-dependencies` | Adding or updating crate dependencies (covers workspace dependency management, versioning, features) |
| `debugging-hanging-tests` | Diagnosing worker executor or integration tests that hang indefinitely |
| `modifying-test-components` | Building or modifying test WASM components, or rebuilding after SDK changes |
| `modifying-wit-interfaces` | Adding or modifying WIT interfaces and synchronizing across sub-projects |
| `modifying-service-configs` | Changing service configuration structs, defaults, or adding new config fields |
| `sdk-development` | Working on the Rust or TypeScript SDKs in `sdks/` |
| `pre-pr-checklist` | Final checks before submitting a pull request |

## Before Submitting a PR

**Always run before creating a pull request:**
```shell
cargo make fix
```

This runs `rustfmt` and `clippy` with automatic fixes. Load `pre-pr-checklist` skill for the full workflow.

## Code Style

- Follow existing code conventions in the file you're editing
- Do not add unnecessary comments
- Use existing libraries and utilities from the codebase
- Security: Never expose or log secrets/keys

## Dependency Management

All crate dependencies must have their versions specified in the root workspace `Cargo.toml` under `[workspace.dependencies]`. Workspace members must reference them using `x = { workspace = true }` in their own `Cargo.toml` rather than specifying versions directly.

## Debugging Tests

Use `--nocapture` when debugging tests to allow debugger attachment:
```shell
cargo test -p <crate> <test> -- --nocapture
```

**Always save test output to a file** when running worker executor tests, integration tests, or CLI tests. These tests are slow and produce potentially thousands of lines of logs. Never pipe output directly to `grep`, `head`, `tail`, etc. — if you need to examine different parts of the output, you would have to re-run the entire slow test. Instead:
```shell
cargo test -p <crate> <test> -- --nocapture > tmp/test_output.txt 2>&1
# Then search/inspect the saved file as needed
grep -n "pattern" tmp/test_output.txt
```

**Handling hanging tests:** Load the `debugging-hanging-tests` skill for a step-by-step workflow.

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
