---
name: sdk-development
description: "Working on the Rust, TypeScript, or MoonBit SDKs in sdks/. Use when modifying SDK code, adding SDK features, or testing SDK changes with the main Golem platform."
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

Run only worker executor tests that exercise the changed SDK behavior. Use the full suite for broad runtime, durability, value-conversion, or agent framework changes whose consumers cannot be isolated.

### Testing with golem-cli

Set `GOLEM_RUST_PATH` to use local SDK in generated applications:

```shell
export GOLEM_RUST_PATH=/path/to/golem/sdks/rust/golem-rust
golem-cli app new my-test-app
```

### Code style

```shell
cargo fmt -p <affected-sdk-crate> -- --check
cargo clippy -p <affected-sdk-crate> --all-targets -- -Dwarnings
```

## TypeScript SDK (`sdks/ts/`)

### Prerequisites

- Node.js
- pnpm (managed via `packageManager` field)
- `wasm-rquickjs-cli`: `cargo install wasm-rquickjs-cli --version <VERSION>` (check `WASM_RQUICKJS_VERSION` in `.github/workflows/ci.yaml`)
- `cargo-component` v0.21.1 (exact version required for agent template builds)

### Packages

Build order matters: `golem-ts-sdk` → `golem-ts-bridge` → `golem-ts-repl`.

### Building

```shell
cd sdks/ts
npx pnpm install
npx pnpm run build
```

### Testing

```shell
npx pnpm --filter <affected-package> run test
npx pnpm run test  # All packages, for cross-package changes
```

### Agent template WASM

The agent template WASM embeds the existing `packages/golem-ts-sdk/dist/index.mjs`. Build that bundle before rebuilding the template whenever a change can affect the emitted runtime. Triggers include:

- `wasm-rquickjs-cli` is updated
- WIT dependencies change
- any source or dependency in the Rollup graph rooted at `packages/golem-ts-sdk/src/index.ts` changes
- wrapper generation or agent-template toolchain inputs change

The filenames above are intentionally described by dependency graph rather than a fixed list: runtime modules can be added or reorganized.

```shell
cargo install cargo-component --version 0.21.1
npx pnpm --filter @golemcloud/golem-ts-sdk run build
npx pnpm run build-agent-template
```

The package build refreshes `dist/index.mjs`; `build-agent-template` then embeds it in the pre-compiled WASM. Running either command alone is not sufficient after a runtime change.

### Testing with the main platform

```shell
# From repository root
cargo make build-cli-test-bins-non-ci
(cd sdks/ts && npx pnpm run build && npx pnpm run build-agent-template)
# Build the specific test components required by <affected-filter>.
cargo-test-r run --package golem-cli --test integration <affected-filter> -- --report-time --nocapture
```

Prefer targeted CLI integration filters that generate or exercise the affected SDK feature. They require fresh CLI binaries, SDK/template artifacts, and any selected test components. Use the full CLI suite only for broad template, bridge, REPL, or generated-application changes; after TS source changes, refresh the SDK/template first because `build-sdk-ts` skips when output files already exist.

### Testing with golem-cli

```shell
export GOLEM_TS_PACKAGES_PATH=/path/to/golem/sdks/ts/packages
npx pnpm install && npx pnpm run build  # Build first!
golem-cli app new my-test-app
```

### Code style

```shell
npx pnpm --filter <affected-package> run lint
npx pnpm exec prettier --check <changed-paths>
```

## MoonBit SDK (`sdks/moonbit/`)

See `sdks/moonbit/AGENTS.md` for full details. The MoonBit SDK has its own build system (`moon`) and code generation tools (`golem_sdk_tools`).

### Building

```shell
cd sdks/moonbit/golem_sdk
moon check --target wasm          # Type-check
moon build --target wasm          # Build
```

### Testing

```shell
cd sdks/moonbit/golem_sdk
moon test <affected-package-or-file>
cd sdks/moonbit/golem_sdk_tools
moon test <affected-package-or-file>
```

### Regenerating WIT bindings

```shell
cd sdks/moonbit/golem_sdk
moon run script bindgen  # Enforces the pinned Golem wit-bindgen and required post-processing
moon fmt
```

### Code style

```shell
moon fmt
moon info    # Regenerate .mbti files when public interfaces changed
```

## Downstream Rebuild Requirements

SDK changes can require rebuilding test components. This is the most common source of errors.

### Rust SDK change → test components

1. Build `golem-rust` / `golem-rust-macro`
2. Find Rust test components depending on the SDK: check `test-components/*/Cargo.toml` for `golem-rust` references
3. Rebuild each affected component following its `AGENTS.md`

### TS runtime/WIT change → test components

1. Build the affected TS SDK package and its required package dependencies (`npx pnpm run build` in `sdks/ts/` is the broad option)
2. Rebuild agent template WASM (`npx pnpm run build-agent-template` in `sdks/ts/`)
3. Find TS test components depending on the SDK
4. Rebuild each affected component following its `AGENTS.md`

Type-only, test-only, documentation, bridge, or REPL changes that cannot affect `golem-ts-sdk/dist/index.mjs` or WIT do not require an agent-template rebuild.

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
4. Agent template rebuilt from a fresh bundle (if TS runtime bundle or WIT inputs changed)
5. Dependent test components rebuilt (if any)
6. Platform tests that exercise changed SDK/platform integration pass
7. Affected SDK code is formatted and linted with its native tools
8. Full SDK/platform suites run only for broad or unclear impact
