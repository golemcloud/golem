---
name: golem-add-rust-crate
description: "Add a new Rust crate dependency to a Rust Golem project. Use when the user asks to add a library, crate, or dependency."
---

# Add a Rust Crate Dependency

## Important constraints

- The compilation target is `wasm32-wasip2` — only crates that support this target will work.
- Crates that use threads, native system calls, `mmap`, networking via `std::net`, or platform-specific C libraries **will not compile**.
- Pure Rust crates and crates that support `wasm32-wasi` generally work.
- If unsure whether a crate compiles for WASM, add it and run `golem build` to find out.

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
   golem build
   ```

   Do NOT run `cargo build` directly — always use `golem build`.

3. **If the build fails**

   - Check the error for unsupported target or missing C dependencies — these crates are incompatible with `wasm32-wasip1`.
   - Try enabling a `wasm` or `wasi` feature flag if the crate provides one.
   - Look for an alternative crate that supports WASM.

## Already available crates

These crates are already in the project's `Cargo.toml` — do NOT add them again:

- `golem-rust` — Golem agent framework, durability, transactions
- `wstd` — WASI standard library (HTTP client, async I/O)
- `log` — logging
- `serde` / `serde_json` — serialization

## HTTP and networking

Use `wstd::http` for HTTP requests. The standard `std::net` module is **not available** on WASM.

## AI / LLM features

To add AI capabilities, add the relevant `golem-ai-*` provider crate (e.g., `golem-ai-llm-openai`) and configure the provider in the component's `golem.yaml` dependencies section.
