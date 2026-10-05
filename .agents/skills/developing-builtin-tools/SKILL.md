---
name: developing-builtin-tools
description: Creates or modifies component-backed Golem built-in tools, including descriptors, Wizer builds, registry provisioning, and sidecar tests.
---

# Developing Built-in Tools

Built-in tool sources live under `builtin-tools/`; generated WASMs are ignored and published through `golemcloud/golem-builtins`. `BuiltinToolDescriptor` owns exported tool metadata and references a logical artifact ID. `builtin-artifacts.lock.json` owns that component artifact's independent version and SHA-256; the registry derives its canonical release URL. Server configuration contains only cache settings and explicit source overrides.

## Workflow

- Build through the component's Golem manifest and focused root `cargo make` task. Do not invoke the language compiler as a substitute for the component build.
- Keep JavaScript/TypeScript CLIs bundled into their components and use the `optimized` preset so Wizer pre-initializes them.
- Validate descriptor names and versions against extracted component metadata before provisioning.
- Treat component artifact versions and exported tool versions independently: bump the artifact for every byte change, and bump each tool coordinate whose implementation should move to the new component revision.
- Never commit the generated WASM or add `include_bytes!`. Load `publishing-builtin-artifacts` to release changed bytes and update the release lock.
- Preserve idempotent component reuse: tools backed by the same component use the same artifact ID, while published tool release coordinates remain immutable.

Verify the focused component build and smoke tests, registry provisioner tests, downloader/cache tests, and the relevant worker-executor sidecar test. Tests consume generated files or the shared prepopulated artifact cache rather than tracked WASMs.
