# Golem Rust SDK

## Overview

This directory contains the Rust SDK for building Golem components:
- `golem-rust` - Runtime API wrappers including the transaction API, durability, agentic framework, and value type conversions
- `golem-rust-macro` - Procedural macros for `ValueAndType` derivation and agent definition

## Building

```shell
cargo build              # Build all crates
cargo build -p golem-rust        # Build runtime crate
cargo build -p golem-rust-macro  # Build macro crate
```

## Testing

Tests use [test-r](https://test-r.vigoo.dev).

**Important:** Each test file must enable test-r:
- In `lib.rs`: `#[cfg(test)] test_r::enable!();`
- In test files: `test_r::enable!();` at the top, then `use test_r::test;`

```shell
cargo test               # Run all tests
cargo test -p golem-rust # Test specific crate
cargo test -p golem-rust -- --nocapture  # Debug with output
```

Some tests require specific features:
```shell
cargo test -p golem-rust --features export_golem_agentic  # Agent tests
```

## Crate Structure

### golem-rust

| Module | Purpose |
|--------|---------|
| `bindings` | WIT-generated bindings via `wit_bindgen::generate!` |
| `transaction` | Transaction API for durable execution |
| `durability` | Durability helpers (requires `durability` feature) |
| `value_and_type` | Type conversion traits (`IntoValue`, `FromValueAndType`, `Schema`) |
| `agentic` | Agent framework (requires `export_golem_agentic` feature) |
| `json` | JSON serialization helpers (requires `json` feature) |

### golem-rust-macro

| Macro | Purpose |
|-------|---------|
| `#[derive(IntoValue)]` | Derive `IntoValue` trait for structs/enums |
| `#[derive(FromValueAndType)]` | Derive `FromValueAndType` trait |
| `#[derive(Schema)]` | Derive schema generation |
| `#[agent_definition]` | Define an agent trait with metadata |
| `#[agent_implementation]` | Implement an agent trait |
| `#[derive(AllowedLanguages)]` | Define allowed languages for unstructured text |
| `#[derive(AllowedMimeTypes)]` | Define allowed MIME types for binary data |

## Feature Flags

| Feature | Description |
|---------|-------------|
| `default` | Enables `durability`, `json`, `macro` |
| `durability` | Durability helpers |
| `json` | JSON serialization via serde |
| `macro` | Re-exports `golem-rust-macro` |
| `export_golem_agentic` | Full agentic framework support |
| `export_load_snapshot` | Snapshot loading support |
| `export_save_snapshot` | Snapshot saving support |
| `golem_ai` | AI/LLM integration bindings |
| `chrono` | Chrono type conversions |
| `uuid` | UUID type conversions (always enabled) |
| `url` | URL type conversions |
| `bytes` | Bytes type conversions |
| `bigdecimal` | BigDecimal type conversions |
| `rust_decimal` | Decimal type conversions |

## Code Style

- Follow Rust idioms and existing code conventions
- Use `cargo fmt` and `cargo clippy` before committing
- Do not add unnecessary comments
- Use existing patterns from neighboring code

## WIT Dependencies

WIT files are synced from the parent repository. **Do not manually edit** files in `golem-rust/wit/deps/`.

To update WIT dependencies, run from the **repository root**:
```shell
cargo make wit
```

## Adding New Type Conversions

When adding `IntoValue`/`FromValueAndType` support for new types:

1. Add the feature flag to `Cargo.toml` if it requires an external crate
2. Create a new module in `src/value_and_type/` (e.g., `my_type.rs`)
3. Implement `IntoValue`, `FromValueAndType`, and optionally `Schema` traits
4. Add conditional compilation: `#[cfg(feature = "my_type")]`
5. Re-export in `src/value_and_type/mod.rs`
6. Add roundtrip tests in `src/value_and_type/tests.rs` using `roundtrip_test!` macros

## Agent Definition Pattern

```rust
use golem_rust::{agent_definition, agent_implementation, agentic::BaseAgent};

#[agent_definition]
trait MyAgent: BaseAgent {
    fn new(init: String) -> Self;
    fn do_something(&self, input: String) -> String;
}

struct MyAgentImpl { /* fields */ }

#[agent_implementation]
impl MyAgent for MyAgentImpl {
    fn new(init: String) -> Self { /* ... */ }
    fn do_something(&self, input: String) -> String { /* ... */ }
}
```

## Integration with Main Repository

This SDK is part of the main Golem repository but is **not built by `cargo make build`**. When changes affect core functionality, test with the full Golem test suite:

```shell
# From repository root
cargo make worker-executor-tests  # Tests that use SDK features
```

## Testing Local SDK Changes

When using compiled `golem` or `golem-cli` binaries, newly generated Golem applications use **fixed SDK versions from crates.io** by default.

To test local SDK changes, set the `GOLEM_RUST_PATH` environment variable:

```shell
export GOLEM_RUST_PATH=/path/to/golem/sdks/rust/golem-rust
golem-cli app new my-test-app      # Will use local SDK
```

This is useful for:
- Running CLI integration tests with local SDK modifications
- Manually creating test applications to verify SDK changes
- Debugging SDK issues in real component scenarios
