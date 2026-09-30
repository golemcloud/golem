---
name: developing-builtin-tools
description: Creates or modifies component-backed Golem built-in tools, including descriptors, Wizer builds, registry provisioning, and sidecar tests.
---

# Developing Built-in Tools

Built-in tool sources live under `builtin-tools/`; generated WASMs are ignored and published through `golemcloud/golem-builtins`. `BuiltinToolDescriptor` owns exported tool metadata and references a logical artifact ID. `builtin-artifacts.json` owns that artifact's release URL and SHA-256.

## Workflow

- Build through the component's Golem manifest and focused root `cargo make` task. Do not invoke the language compiler as a substitute for the component build.
- Keep JavaScript/TypeScript CLIs bundled into their components and use the `optimized` preset so Wizer pre-initializes them.
- Validate descriptor names and versions against extracted component metadata before provisioning.
- Never commit the generated WASM or add `include_bytes!`. Load `publishing-builtin-artifacts` to release changed bytes and update the pinned manifest.
- Preserve idempotent component reuse: tools backed by the same component use the same artifact ID, while published tool release coordinates remain immutable.

Verify the focused component build and smoke tests, registry provisioner tests, downloader/cache tests, and the relevant worker-executor sidecar test. Tests consume generated files or the shared prepopulated artifact cache rather than tracked WASMs.
