---
name: modifying-test-components
description: "Building or modifying test WASM components in test-components/. Use when a test component needs to be rebuilt, a new test component is needed, or SDK changes require downstream test component rebuilds."
---

# Modifying Test Components

Worker executor tests, integration tests, and CLI integration tests use built WASM artifacts from `test-components/`. These `.wasm` files are no longer checked into the repository; compiling the components needed by the selected tests is a separate step before running those tests.

## Key Rules

1. **Do not rebuild more test components than necessary.** Prefer rebuilding only the components required by the selected tests.
2. **Only rebuild if the test component has its own `AGENTS.md`** with build instructions. If it doesn't, the component cannot be rebuilt by you.
3. **SDK changes require rebuilding dependent test components.** If you modify `sdks/rust/` or `sdks/ts/`, you must rebuild any test components that use the changed SDK.

## When to Rebuild

| Change | Action Required |
|--------|----------------|
| Modifying a test component's source code | Rebuild that component (if it has an AGENTS.md) |
| Modifying `sdks/rust/` (golem-rust) | Rebuild Rust test components that depend on it |
| Modifying `sdks/ts/` (golem-ts-sdk) | Rebuild agent template WASM first, then TS test components |
| Modifying WIT interfaces | Run `cargo make wit`, then rebuild affected components |
| Running worker executor / integration / CLI integration tests that depend on missing test-component WASMs | Rebuild the specific components those tests use before running the tests |
| No source changes and required WASMs already exist | Do not rebuild |

## Rebuilding a Test Component

### Step 1: Check for build instructions

```shell
cat test-components/<component-name>/AGENTS.md
```

If no `AGENTS.md` exists, **stop** — you cannot rebuild this component.

### Step 2: Follow the component's AGENTS.md

Each component's `AGENTS.md` contains specific build instructions. Follow them exactly.

## TS SDK Change Rebuild Chain

When modifying the TypeScript SDK, you must follow this exact rebuild order:

### 1. Build the TS SDK packages

```shell
cd sdks/ts
npx pnpm install
npx pnpm run build
```

### 2. Rebuild the agent template WASM

This step is **required** before rebuilding any TS test components. The agent template embeds the SDK runtime.

```shell
cd sdks/ts
npx pnpm run build-agent-template
```

**Requires `cargo-component` v0.21.1** — see `sdks/ts/AGENTS.md` for installation.

### 3. Rebuild affected TS test components

Follow each component's `AGENTS.md` for specific instructions.

## Rust SDK Change Rebuild Chain

When modifying the Rust SDK:

### 1. Build the Rust SDK

```shell
cargo build -p golem-rust
cargo build -p golem-rust-macro
```

### 2. Rebuild affected Rust test components

Follow each component's `AGENTS.md` for specific instructions. Rust test components typically use `cargo component build` with a local path dependency on `golem-rust`.

## Finding Test Components

Test components live in `test-components/`. To find which ones have build instructions:

```shell
ls test-components/*/AGENTS.md
```

To find which test components depend on a specific SDK, check their `Cargo.toml` (Rust) or `package.json` (TS) for SDK references.

## Checklist

1. Confirmed the component has an `AGENTS.md` with build instructions
2. If SDK was changed: rebuilt SDK first
3. If TS SDK was changed: rebuilt agent template WASM before components
4. Followed the component's specific `AGENTS.md` build instructions
5. Confirmed the expected `.wasm` artifact now exists under `test-components/`
6. Ran the relevant tests after the rebuild (`cargo make worker-executor-tests`, `cargo make integration-tests`, or `cargo make cli-tests`)
