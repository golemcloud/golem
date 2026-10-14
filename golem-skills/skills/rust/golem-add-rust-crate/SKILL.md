---
name: golem-add-rust-crate
description: "Add a new Rust crate dependency to a Rust Golem project. Use when the user asks to add a library, crate, or dependency."
---

# Add a Rust Crate Dependency

## Important constraints

- The compilation target is `wasm32-wasip2` — only crates that support this target will work.
- Crates requiring unsupported OS facilities may fail to compile or fail at runtime.
- A generic `wasm32-wasi` compatibility claim is insufficient; verify the exact features used
  against `wasm32-wasip2` and exercise them in Golem.
- Pure Rust is not by itself proof of compatibility. Platform-specific C libraries, threading,
  sockets, and memory-mapping APIs require particular scrutiny.

## Steps

1. **Add the dependency to `Cargo.toml`**

   In the component's `Cargo.toml` (not a workspace `Cargo.toml`), add the crate under `[dependencies]`:

   ```toml
   [dependencies]
   my-crate = "1.0"
   ```

   If the project has a Cargo workspace with `[workspace.dependencies]`, add the version there and reference it with `my-crate = { workspace = true }` in the component crate.

2. **Build to verify**

   ```shell
   golem build --yes
   ```

   Do NOT run `cargo build` directly — always use `golem build`.

3. **If the build fails**

   - Check the error for unsupported target features or missing native dependencies.
   - Try enabling a `wasm` or `wasi` feature flag if the crate provides one.
   - Look for an alternative crate that supports WASM.

## Already available crates

These crates are already in the project's `Cargo.toml` — do NOT add them again:

- `golem-rust` — Golem agent framework, durability, transactions
- `wstd` — WASI standard library (HTTP client, async I/O)
- `log` — logging
- `serde` / `serde_json` — serialization

## HTTP and networking

Prefer `wstd::http` for outgoing HTTP requests. Compile support for an API does not guarantee that
the runtime grants the required network capability, so test the actual operation in Golem.

## AI / LLM features

To add AI capabilities, load the `golem-add-llm-rust` skill. The published `golem-ai` release and the current WASIp2-compatible source do not currently use the same API, so do not guess a crate version or copy an older example.
