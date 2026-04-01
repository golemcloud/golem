---
name: rebuild-all-test-components
description: "Rebuilds all test WASM components from scratch. Use when explicitly asked to rebuild all test components, or when there are merge conflicts in any test-components/*.wasm files."
---

# Rebuild All Test Components

Clean rebuild of every test WASM component in `test-components/`. This is a heavyweight operation — use it when all components need regeneration, or when worker executor / integration / CLI integration tests need many test-component artifacts and targeted rebuilds would be slower.

## Quick Path (single command)

```shell
cargo make build-test-components
```

This handles everything: builds `golem-cli`, the TS SDK, then cleans and rebuilds all test components.

## Manual Steps (if needed)

### 1. Build golem-cli

The build scripts rely on `golem-cli` from the local build:

```shell
cargo make build
```

### 2. Clean and rebuild the TypeScript SDK (including agent template)

The TS test components depend on the TS SDK and its embedded agent template WASM:

```shell
cd sdks/ts
npx pnpm run clean
npx pnpm install
npx pnpm run build
npx pnpm run build-agent-template
```

**Requires `cargo-component` v0.21.1** — install with:
```shell
cargo install cargo-component --version 0.21.1
```

### 3. Clean all test components

```shell
cd test-components
./build-components.sh clean
```

### 4. Build all test components

```shell
cd test-components
./build-components.sh
```

If any component fails to build, fix the issue and re-run `./build-components.sh`. The script will rebuild all components. Repeat until all components build successfully.

### 5. Verify

After rebuilding, the `.wasm` files in `test-components/` should be present. Note: these files are gitignored and must be built before tests that use them.

## Troubleshooting

- **Missing `wasm-rquickjs-cli`**: Check the required version in `.github/workflows/ci.yaml` (`WASM_RQUICKJS_VERSION`) and install it: `cargo install wasm-rquickjs-cli --version <VERSION>`
- **TS build failures**: Ensure `npx pnpm run build-agent-template` completed successfully before building TS test components
- **Rust build failures**: The script sets `GOLEM_RUST_PATH` automatically to `sdks/rust/golem-rust`; ensure the Rust SDK builds cleanly with `cargo build -p golem-rust`
