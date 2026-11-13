---
name: golem-build
description: "Building a Golem application. Use when asked to build a Golem project, compile components to WASM, or troubleshoot build errors."
---

# Building a Golem Application with `golem build`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem build --yes
```

Run this from the application root directory (where the root `golem.yaml` is located). It builds all components defined in the project. **Always pass `--yes`** to avoid interactive prompts.

## What `golem build` Does

The build is a multi-step pipeline:

1. **Check** — verifies that required build tools are installed (e.g., `cargo` for Rust, `npm`/`node` for TypeScript).
2. **Build** — executes the build commands defined in `golem.yaml` for each component to produce the final artifact at `componentWasm`. These commands are language-specific:
   - **Rust**: runs `cargo build --target wasm32-wasip2` (or with `--release` for the release preset).
   - **TypeScript**: runs `rollup` to bundle the component (type checking happens in-process during bundling), then injects the bundle into a prebuilt QuickJS WASM and optionally preinitializes it.
   - **Effect**: runs `tsc` and `rollup`, then injects the bundle into the QuickJS WASM shipped by `@golemcloud/effect-golem`. Effect agents are self-describing, so this pipeline does not run `golem-typegen`.
   - **Scala**: runs Scala.js compilation, JavaScript linking, QuickJS WASM injection, agent wrapper generation, and WASM composition.
3. **Generate Bridge** — automatically generates guest clients selected by component `dependencies.agents` and `dependencies.tools`, and any explicitly selected bridge targets. Local providers build before their consumers; dependency clients are generated before consumer compilation. Prefer tool dependencies over explicit bridge selection for clients used by a component. Configure runtime tool bindings separately: a dependency does not add a binding, and a binding alone does not generate a client.

### Up-to-date Checks

`golem build` tracks file hashes of sources and targets. If nothing changed since the last build, steps are skipped automatically. Use `--force-build` to bypass this.

## Build Output

`componentWasm` is the sole final artifact used for metadata extraction and upload. Golem uses its bytes unchanged, without an output-copy stage. Paths are relative to the effective component directory.

The built-in templates configure `componentWasm` as follows:

- **Rust (debug preset)**: `target/wasm32-wasip2/debug/<component_name_snake_case>.wasm`
- **Rust (release preset)**: `target/wasm32-wasip2/release/<component_name_snake_case>.wasm`
- **TypeScript, Effect, and Scala (optimized preset)**: `golem-temp/agents/<component_name_snake_case>.preinitialized.wasm`
- **TypeScript, Effect, and Scala (quick preset)**: `golem-temp/agents/<component_name_snake_case>.dynamic.wasm`

Rust paths use the resolved Cargo target directory (`cargoTarget`), which defaults to `target`. The component name in snake_case is derived from the component name in `golem.yaml`. For example, a Rust component named `my-app:rust-main` produces `target/wasm32-wasip2/debug/my_app_rust_main.wasm` with the debug preset and default target directory.

## Available Options

| Option | Description |
|--------|-------------|
| `[COMPONENT_NAME]...` | Build only specific components (by default, all components are built) |
| `-s, --step <STEP>` | Run specific build step(s): `check`, `build`, `gen-bridge` |
| `--skip-check` | Skip build-time requirement checks |
| `--force-build` | Skip up-to-date checks, rebuild everything |
| `-P, --preset <PRESET>` | Select a component preset (e.g., `release`) |
| `-Y, --yes` | Non-interactive mode — **always use this flag** |

## Build Configuration in `golem.yaml`

Build commands are defined per component template and preset in `golem.yaml`:

```yaml
componentTemplates:
  rust:
    presets:
      debug:
        default: true
        build:
        - command: cargo build --target wasm32-wasip2
        componentWasm: "target/wasm32-wasip2/debug/<name>.wasm"
```

The `build` section is a list of steps. Each step can be:
- `command:` — a shell command to execute
- `injectToPrebuiltQuickjs:` — injects a JS bundle into a QuickJS WASM (TypeScript/Effect/Scala only)
- `preinitializeJs:` — preinitializes the JS runtime in the WASM (TypeScript/Effect/Scala only)

Each step can specify `sources` and `targets` for incremental builds, `dir` for the working directory, and `env` for environment variables.

## Cleaning Build Artifacts

```shell
golem clean
```

This removes `golem-temp/` and any other directories listed in the `clean` section of each preset.

## Common Build Errors

- **Missing `wasm32-wasip2` target** (Rust): run `rustup target add wasm32-wasip2`
- **Missing npm packages** (TypeScript/Effect): `golem build` automatically runs `npm install` if `package.json` is present but `node_modules` is missing
- **Type errors** (TypeScript/Effect): fix the errors in `.ts` source files reported by the language's build pipeline

For a deeper breakdown of build pipeline stages, dependency auto-fix behavior, and manifest tracing, load `golem-troubleshoot-build`.

## Related Skills

- Load `golem-generating-tool-clients` to generate and import typed clients for existing built-in tools or MCP imports
- Load `golem-troubleshoot-build` when a build fails or when diagnosing manifest/configuration issues
