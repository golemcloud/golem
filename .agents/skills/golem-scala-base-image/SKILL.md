---
name: golem-scala-base-image
description: "Explains the Golem Scala SDK WIT folder structure and how to regenerate the agent_guest.wasm base image. Use when working with WIT definitions, upgrading Golem versions, or regenerating the guest runtime WASM."
---

# Golem Scala Base Image (agent_guest.wasm)

The base image `agent_guest.wasm` is a QuickJS-based WASM component that serves as the guest runtime for Scala.js agents on Golem. It must be regenerated whenever WIT definitions change.

## WIT Folder Structure

```
sdks/scala/wit/
├── main.wit      # Hand-maintained world definition (golem:agent-guest)
├── dts/          # Generated TypeScript d.ts (source of truth for JS exports)
└── deps/         # Synced from wit/deps/ by `cargo make wit` (committed)
    ├── golem-core/
    ├── golem-agent/
    ├── golem-1.x/
    ├── golem-rdbms/
    ├── golem-durability/
    ├── blobstore/
    ├── cli/
    ├── clocks/
    ├── config/
    ├── ...
    └── sockets/
```

- **`main.wit`** defines the `golem:agent-guest` world — the set of imports/exports the agent component uses. This file is checked in and maintained manually.
- **`deps/`** is synced from the repo root's `wit/deps/` by running `cargo make wit` from the repository root. The contents are committed (same approach as the Rust and TypeScript SDKs).

## When to Regenerate

The base image **must be regenerated** whenever:

1. **`wit/main.wit` changes** — adding/removing imports or exports
2. **WIT dependencies update** — run `cargo make wit` from the repo root first, then regenerate
3. **`wasm-rquickjs` updates** — a new version of the wrapper generator may produce different output

The generated `agent_guest.wasm` is checked in at two locations (embedded in the sbt and mill plugins):
- `sdks/scala/sbt/src/main/resources/golem/wasm/agent_guest.wasm`
- `sdks/scala/mill/resources/golem/wasm/agent_guest.wasm`

## Prerequisites

### 1. Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-wasip1
```

### 2. cargo-component

```bash
cargo install cargo-component
```

### 3. wasm-rquickjs (pinned to 0.1.0)

The script enforces a specific version of `wasm-rquickjs` and will refuse to run if the installed version does not match. The required version is defined by `REQUIRED_WASM_RQUICKJS_VERSION` in `generate-agent-guest-wasm.sh`.

```bash
cargo install wasm-rquickjs-cli@0.1.0
```

## How to Regenerate

First sync WIT dependencies, then run the generate script:

```bash
# From the repository root
cargo make wit

# Then from sdks/scala/
cd sdks/scala
./scripts/generate-agent-guest-wasm.sh
```

The script performs these steps:

1. Stages a clean WIT package in `.generated/agent-wit-root/` (copies `main.wit` + `deps/`)
2. Generates TypeScript d.ts definitions via `wasm-rquickjs generate-dts`
3. Runs `wasm-rquickjs generate-wrapper-crate` to produce a Rust crate from the WIT
4. Builds with `cargo component build --release` targeting `wasm32-wasip1`
5. Installs the resulting `agent_guest.wasm` into both plugin resource directories
6. Copies d.ts files to `sdks/scala/wit/dts/`

## Updating WIT Dependencies

WIT dependencies are managed the same way as the Rust and TypeScript SDKs — via `cargo make wit` from the repository root:

```bash
# From the repository root
cargo make wit
```

This copies all WIT packages from `wit/deps/` into `sdks/scala/wit/deps/`. The results are committed to the repository.

## How It Fits Together

At build time, the sbt/mill `GolemPlugin` extracts the embedded `agent_guest.wasm` from plugin resources and writes it to the user project's `.generated/agent_guest.wasm`. Then `golem-cli` uses this base runtime to compose the final component: it injects the user's Scala.js bundle into the QuickJS runtime and wraps it as a proper Golem agent component.

The Scala SDK does **not** parse WIT to generate Scala bindings. Instead, Scala macros + ZIO Schema produce `AgentMetadata` at compile time, and `WitTypeBuilder` maps schema types to WIT-compatible JS representations at runtime. The WIT definitions only flow through the WASM guest runtime.
