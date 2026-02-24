# Golem TypeScript SDK

## Overview

This directory contains the TypeScript SDK for building Golem components. It's a pnpm monorepo with multiple packages.

## Prerequisites

- Node.js
- pnpm (managed via packageManager field)
- wasm-rquickjs-cli: `cargo install wasm-rquickjs-cli --version <VERSION>` (look up `WASM_RQUICKJS_VERSION` in `.github/workflows/ci.yaml`)

## Building

```shell
npx pnpm install         # Install dependencies
npx pnpm run build       # Build all packages
```

Build order is important: `golem-ts-types-core` → `golem-ts-typegen` → `golem-ts-sdk`

## Testing

```shell
npx pnpm run test                           # Run all tests
cd packages/golem-ts-sdk && pnpm run test   # Run tests for specific package
```

When making changes to `golem-ts-typegen` or `golem-ts-types-core`, rebuild before testing `golem-ts-sdk`:

```shell
pnpm install && pnpm run build
```

## Code Style

```shell
npx pnpm run lint          # Run ESLint
npx pnpm run format        # Format code with Prettier
npx pnpm run format:check  # Check formatting
```

**Run before committing:**

```shell
npx pnpm run lint
npx pnpm run format
```

## Cleaning

```shell
npx pnpm clean   # Remove all build artifacts and node_modules
```

## WIT Dependencies

WIT files are synced from the parent repository. Do not manually edit files in `wit/deps/`.

To update WIT dependencies, run from the **repository root**:

```shell
cargo make wit
```

## Agent Template WASM

When `wasm-rquickjs-cli` is updated or WIT dependencies change, the agent template WASM must be rebuilt.

**Requires cargo-component v0.21.1** (exact version required):

```shell
cargo install cargo-component --version 0.21.1
cargo-component --version  # Verify: must be 0.21.1
```

Rebuild the template:

```shell
npx pnpm run build-agent-template
```

**Important:** You must also run `build-agent-template` whenever you modify SDK runtime code (e.g., `baseAgent.ts`, `index.ts`, `resolvedAgent.ts`). Running `pnpm run build` alone only updates the JS bundle, but TS components use a pre-compiled `agent_guest.wasm` that embeds the SDK. Without rebuilding the template, TS components will bundle stale SDK code.

**Testing local wasm-rquickjs changes:** If modifying wasm-rquickjs locally (in a separate checkout), install it from the local path:

```shell
cd /path/to/wasm-rquickjs
cargo install --path .
```

Then `pnpm run build-agent-template` will use the updated version.

## Integration with Main Repository

This SDK is part of the main Golem repository but is **not built by `cargo make build`**. When changes affect core functionality, test with the full Golem test suite:

```shell
# From repository root
cargo make cli-tests  # Tests that use SDK features
```

## Testing Local SDK Changes

When using compiled `golem` or `golem-cli` binaries, newly generated Golem applications use **fixed SDK versions from npm** by default.

To test local SDK changes, set the `GOLEM_TS_PACKAGES_PATH` environment variable:

```shell
export GOLEM_TS_PACKAGES_PATH=/path/to/golem/sdks/ts/packages
golem-cli app new my-test-app      # Will use local SDK
```

This is useful for:

- Running CLI integration tests with local SDK modifications
- Manually creating test applications to verify SDK changes
- Debugging SDK issues in real component scenarios

**Important:** Make sure to build the SDK packages before testing:

```shell
npx pnpm install && npx pnpm run build
```

**Troubleshooting:** If you get "permission denied" errors when building applications created with `golem-cli app new`, delete the application's `node_modules` directory and rebuild:

```shell
cd /path/to/your-golem-app
rm -rf node_modules
# Then rebuild the application
```
