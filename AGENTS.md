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

**Note:** The SDKs in `sdks/` are not part of the main build flow. Load `sdk-development` when working on the Rust or TypeScript SDKs, and `golem-scala-development` when working on the Scala SDK.

## Testing

Tests use [test-r](https://test-r.vigoo.dev). **Important:** Each test file must import `test_r::test` or tests will not run.

Worker executor tests, integration tests, and CLI integration tests may depend on built test components from `test-components/`. These `.wasm` artifacts are not checked into the repository anymore, so build the specific components needed by the selected tests before running them. Use the `modifying-test-components` skill for targeted rebuilds, or `rebuild-all-test-components` when a full rebuild is needed.

**Do not run `cargo make test`** — it runs all tests and takes a very long time. Instead, choose the appropriate test command:

| Change Type | Test Command |
|-------------|--------------|
| Core logic, utilities | `cargo make unit-tests` |
| Worker executor functionality | `cargo make worker-executor-tests` |
| Service integration | `cargo make integration-tests` |
| CLI changes | `cargo make cli-integration-tests` |

For specific tests: `cargo test -p <crate> -- <test_name> --report-time`

For CLI integration test reruns or isolation, use `cargo make cli-integration-tests-group1` through `cargo make cli-integration-tests-group6`.

**Whenever tests are modified, always run the affected tests to verify they still pass before considering the task complete.**

Load the `testing` skill for detailed guidance on test filtering, debugging failures, test components, and timeouts.

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
| `testing` | Running and debugging tests (covers test filtering, debugging failures, test components, timeouts) |
| `debugging-hanging-tests` | Diagnosing worker executor or integration tests that hang indefinitely |
| `modifying-test-components` | Building or modifying test WASM components, or rebuilding after SDK changes |
| `modifying-wit-interfaces` | Adding or modifying WIT interfaces and synchronizing across sub-projects |
| `modifying-service-configs` | Changing service configuration structs, defaults, or adding new config fields |
| `sdk-development` | Working on the Rust or TypeScript SDKs in `sdks/` |
| `golem-scala-development` | Compile, publish, and test the Golem Scala SDK in `sdks/scala/` |
| `golem-scala-integration-tests` | Running and debugging Scala SDK integration tests |
| `golem-scala-base-image` | WIT folder structure and regenerating `agent_guest.wasm` for the Scala SDK |
| `golem-scala-code-generation` | Writing Scala code generators for the Scala SDK |
| `investigating-executor-performance` | Investigating worker-executor performance with OTLP tracing and Jaeger |
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
- `sdks/` - Language-specific SDKs (Rust, TypeScript, Scala) - **not part of main build flow, see SDK-specific AGENTS.md**
- `golem-skills/` - Skill definitions and skill testing harness
- `integration-tests/` - Integration test suite
- `test-components/` - Test WASM components
