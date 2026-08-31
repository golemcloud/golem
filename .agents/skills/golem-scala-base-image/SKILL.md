---
name: golem-scala-base-image
description: "Explains the Golem Scala SDK WIT worlds and regenerates its three QuickJS guest runtime WASMs. Use when WIT dependencies, guest roles, wasm-rquickjs, or embedded Scala runtime artifacts change."
---

# Golem Scala Guest Runtimes

Use the checked-in script and WIT as the source of truth. Do not reproduce the wrapper-generation steps manually.

## Current contract

`sdks/scala/wit/main.wit` defines three Preview 3 worlds:

| World | Embedded artifact | Role |
|---|---|---|
| `golem:agent-guest/agent-guest` | `agent_guest.wasm` | ordinary agent and tool guest |
| `golem:agent-guest/tool-middleware-guest` | `tool_middleware_guest.wasm` | pure tool middleware |
| `golem:agent-guest/agent-tool-middleware-guest` | `agent_tool_middleware_guest.wasm` | combined agent/tool/middleware guest |

The ordinary world currently includes `golem:agent/agent-guest@2.0.0`, exports `golem:tool/guest@0.1.0`, and uses the v2 `schema-graph`, `schema-value-tree`, and `typed-schema-value` model through the synced `golem-core-v2` dependency. Read `wit/main.wit` rather than maintaining a second exhaustive import list here; notably, the host surface still includes versioned Golem APIs such as `golem:api@1.5.0` and `golem:durability@1.6.0`.

`wit/deps/` is a generated mirror of the root WIT dependencies. Never hand-edit it. Sync it from the repository root:

```bash
cargo make wit
```

## Generated and tracked files

The script installs all three WASMs into both plugin resource directories:

```text
sdks/scala/sbt/src/main/resources/golem/wasm/
sdks/scala/mill/resources/golem/wasm/
```

The WASMs are build artifacts and are intentionally ignored by Git. The ordinary role's generated declarations in `sdks/scala/wit/dts/` are tracked and are checked for drift in CI. The role-specific intermediate declarations and wrapper crates under `sdks/scala/.generated/` are untracked.

## Prerequisites and regeneration

The current pipeline uses:

- Rust stable with `wasm32-wasip2`
- Preview 3 (`wasm-rquickjs --target wasi-p3`)
- `wasm-rquickjs-cli` **0.4.2**, matching the root CI/workflow setting
- ordinary `cargo build --target wasm32-wasip2 --release` on generated wrapper crates; it does not use `cargo-component`

```bash
rustup target add wasm32-wasip2
cargo install --locked wasm-rquickjs-cli@0.4.2

# Repository root
cargo make wit

# SDK root
cd sdks/scala
./scripts/generate-agent-guest-wasm.sh
```

The generator stages each world, generates d.ts and a wrapper crate with the `user=@slot` injection point, applies the repository's required Preview 3 `wit-bindgen` override, builds it, and copies the output to both plugins. Scala.js bundles SDK and user code into the injected module; there is no separately embedded SDK JavaScript module.

## Verification

Generation runs the ordinary export-contract check. Also verify the role matrix when role worlds or packaging changes:

```bash
cd sdks/scala
./scripts/test-agent-guest-export-contract.sh
./scripts/test-agent-guest-role-contracts.sh
git diff -- wit/dts
git status --short -- wit/dts sbt/src/main/resources/golem/wasm mill/resources/golem/wasm
```

The role-contract script requires `wasm-tools` and confirms that sbt and Mill package identical bytes. CI's generated-file action validates tracked d.ts drift and rejects accidentally tracked WASMs.

Regenerate after changing `wit/main.wit`, synced WIT dependencies, the role matrix, the wrapper script, or the pinned `wasm-rquickjs` toolchain.
