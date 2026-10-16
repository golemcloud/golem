---
name: developing-builtin-tools
description: Creates or modifies component-backed Golem built-in tools, including descriptors, Wizer builds, registry provisioning, and sidecar tests.
---

# Developing Built-in Tools

Built-in tool sources live under `builtin-tools/`; generated WASMs are ignored. `BuiltinToolDescriptor` owns exported tool metadata and references a logical artifact ID. `builtin-artifacts.json` owns source inputs, output paths, and build tasks. Development uses local WASMs; tagged releases publish immutable assets through `golemcloud/golem-builtins` and generate `builtin-artifacts.release.json`. Server configuration contains only cache settings and explicit source overrides.

## Workflow

- Build through the component's Golem manifest and focused root `cargo make` task. Do not invoke the language compiler as a substitute for the component build.
- Keep JavaScript/TypeScript CLIs bundled into their components and use the `optimized` preset so Wizer pre-initializes them.
- Validate descriptor names and versions against extracted component metadata before provisioning.
- Use `cargo make ensure-builtin-artifacts` for incremental preparation. No publication or version bump is required for development WASM overrides.
- During release preparation, bump each exported tool coordinate whose implementation should move to the new component revision. Artifact versions use the Golem release version.
- Never commit the generated WASM or add `include_bytes!`. Load `publishing-builtin-artifacts` for tagged release publication.
- Preserve idempotent component reuse: tools backed by the same component use the same artifact ID, while published tool release coordinates remain immutable.

Verify the focused component build and smoke tests, registry provisioner tests, downloader/cache tests, and the relevant worker-executor sidecar test. Tests consume locally built files rather than tracked WASMs or published development pins.
