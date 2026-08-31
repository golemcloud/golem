# Golem TypeScript SDK

## Overview

This directory contains the TypeScript SDK for building Golem components. It's a pnpm monorepo with multiple packages.

## Prerequisites

- Node.js
- pnpm (managed via packageManager field)
- wasm-rquickjs-cli: install the version in `WASM_RQUICKJS_VERSION` from
  `.github/workflows/ci.yaml` with `cargo install --locked wasm-rquickjs-cli@<VERSION>`

## Building

```shell
npx pnpm install         # Install dependencies
npx pnpm run build       # Build all packages
```

## Testing

```shell
npx pnpm run test                           # Run all tests
cd packages/golem-ts-sdk && pnpm run test   # Run tests for specific package
```

## Code Style

```shell
npx pnpm run lint          # Run ESLint
npx pnpm run format        # Format code with Prettier
npx pnpm run format:check  # Check formatting
```

For an isolated package change, lint that package and check only changed files:

```shell
npx pnpm --filter <affected-package> run lint
npx pnpm exec prettier --check <changed-paths>
```

Use root `lint`, `format:check`, and build/test commands for cross-package changes. Apply `npx pnpm run format` only when formatting fixes are needed, then inspect the diff.

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

The agent template embeds the existing `packages/golem-ts-sdk/dist/index.mjs`. Rebuild the SDK bundle and then the template when `wasm-rquickjs-cli`, WIT dependencies, wrapper/toolchain inputs, or any source/dependency in the Rollup graph rooted at `packages/golem-ts-sdk/src/index.ts` changes.

The Preview 3 wrapper still requires the `wasm32-wasip2` Rust target because Rust does not yet
provide a dedicated `wasm32-wasip3` target:

```shell
rustup target add wasm32-wasip2
```

Rebuild the bundle and template in this order:

```shell
npx pnpm --filter @golemcloud/golem-ts-sdk run build
npx pnpm run build-agent-template
```

The first command refreshes `dist/index.mjs`; the second embeds it in `agent_guest.wasm`. Running either command alone can leave runtime artifacts stale. Type-only, test-only, documentation, bridge, or REPL changes that cannot affect the bundle or WIT do not require a template rebuild.

**Testing local wasm-rquickjs changes:** If modifying wasm-rquickjs locally (in a separate
checkout), install it from the local path:

```shell
cd /path/to/wasm-rquickjs
cargo install --path .
```

Then rerun the bundle-and-template sequence above so the wrapper uses both the updated tool and a fresh SDK bundle.

## Integration with Main Repository

This SDK is part of the main Golem repository but is **not built by `cargo make build`**. When changes affect generated applications or platform integration, run targeted CLI integration tests that exercise the changed behavior:

```shell
# From repository root
cargo make build-cli-test-bins-non-ci
(cd sdks/ts && npx pnpm run build && npx pnpm run build-agent-template)
# Build the specific test components required by <affected-filter>.
cargo-test-r run --package golem-cli --test integration <affected-filter> -- --report-time --nocapture
```

These prerequisites provide fresh local CLI binaries and SDK/template artifacts; selected CLI tests may additionally require targeted test-component builds. Do not rely on `cargo make build-sdk-ts` alone after source edits: it skips whenever its output paths already exist. Equivalent narrower prerequisite commands are acceptable when they produce the same artifacts. Use `cargo make cli-integration-tests` for broad template, bridge, REPL, or generated-application changes whose tests cannot be isolated, after refreshing changed SDK artifacts. For faster broad local runs with dev-release binaries, use `cargo make cli-integration-tests-dev-release`. SDK-only implementation, unit-test, or documentation changes do not require root workspace checks.

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
