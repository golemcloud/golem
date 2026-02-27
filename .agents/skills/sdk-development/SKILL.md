---
name: sdk-development
description: "Working on the Rust or TypeScript SDKs in sdks/. Use when modifying SDK code, adding SDK features, or testing SDK changes with the main Golem platform."
---

# SDK Development

The SDKs in `sdks/` are **not part of the main build flow** (`cargo make build` does not build them). Each SDK has its own build system and conventions.

## Rust SDK (`sdks/rust/`)

### Crates

- `golem-rust` — Runtime API wrappers (transactions, durability, agentic framework, value conversions)
- `golem-rust-macro` — Procedural macros (`#[derive(IntoValue)]`, `#[agent_definition]`, etc.)

### Building

```shell
cd sdks/rust
cargo build -p golem-rust
cargo build -p golem-rust-macro
```

### Testing

Tests use `test-r`. Each test file must have `test_r::enable!();` at the top.

```shell
cargo test -p golem-rust
cargo test -p golem-rust --features export_golem_agentic  # Agent tests
```

### Testing with the main platform

```shell
# From repository root
cargo make worker-executor-tests
```

### Testing with golem-cli

Set `GOLEM_RUST_PATH` to use local SDK in generated applications:

```shell
export GOLEM_RUST_PATH=/path/to/golem/sdks/rust/golem-rust
golem-cli app new my-test-app
```

### Code style

```shell
cargo fmt
cargo clippy
```

## TypeScript SDK (`sdks/ts/`)

### Prerequisites

- Node.js
- pnpm (managed via `packageManager` field)
- `wasm-rquickjs-cli`: `cargo install wasm-rquickjs-cli --version <VERSION>` (check `WASM_RQUICKJS_VERSION` in `.github/workflows/ci.yaml`)
- `cargo-component` v0.21.1 (exact version required for agent template builds)

### Packages

Build order matters: `golem-ts-types-core` → `golem-ts-typegen` → `golem-ts-sdk`

### Building

```shell
cd sdks/ts
npx pnpm install
npx pnpm run build
```

### Testing

```shell
npx pnpm run test
cd packages/golem-ts-sdk && pnpm run test  # Specific package
```

### Agent template WASM

The agent template WASM embeds the SDK runtime. You **must** rebuild it when:

- `wasm-rquickjs-cli` is updated
- WIT dependencies change
- SDK runtime code changes (`baseAgent.ts`, `index.ts`, `resolvedAgent.ts`)

```shell
cargo install cargo-component --version 0.21.1
npx pnpm run build-agent-template
```

Running `pnpm run build` alone is **not sufficient** — it only updates the JS bundle, not the pre-compiled WASM that TS components use.

### Testing with the main platform

```shell
# From repository root
cargo make cli-tests
```

### Testing with golem-cli

```shell
export GOLEM_TS_PACKAGES_PATH=/path/to/golem/sdks/ts/packages
npx pnpm install && npx pnpm run build  # Build first!
golem-cli app new my-test-app
```

### Code style

```shell
npx pnpm run lint
npx pnpm run format
```

## Downstream Rebuild Requirements

SDK changes can require rebuilding test components. This is the most common source of errors.

### Rust SDK change → test components

1. Build `golem-rust` / `golem-rust-macro`
2. Find Rust test components depending on the SDK: check `test-components/*/Cargo.toml` for `golem-rust` references
3. Rebuild each affected component following its `AGENTS.md`

### TS SDK change → test components

1. Build TS SDK packages (`npx pnpm run build` in `sdks/ts/`)
2. Rebuild agent template WASM (`npx pnpm run build-agent-template` in `sdks/ts/`)
3. Find TS test components depending on the SDK
4. Rebuild each affected component following its `AGENTS.md`

**The agent template rebuild step is critical and easily forgotten.**

## WIT Dependencies

Both SDKs have WIT files synced from the root `wit/` directory. **Never manually edit** `wit/deps/` in either SDK.

```shell
# From repository root
cargo make wit
```

## Checklist

1. SDK code modified
2. SDK builds successfully
3. SDK tests pass
4. Agent template rebuilt (if TS SDK runtime code changed)
5. Dependent test components rebuilt (if any)
6. Platform tests pass (`cargo make worker-executor-tests` for Rust SDK, `cargo make cli-tests` for TS SDK)
7. Code formatted and linted
