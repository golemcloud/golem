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
2. **Build** — executes the build commands defined in `golem.yaml` for each component. These commands are language-specific:
   - **Rust**: runs `cargo build --target wasm32-wasip2` (or with `--release` for the release preset).
   - **TypeScript**: runs a multi-stage pipeline — `tsc` for type checking, `golem-typegen` for metadata extraction, `rollup` for bundling, then injects the bundle into a prebuilt QuickJS WASM and optionally preinitializes it.
   - **Scala**: runs Scala.js compilation, JavaScript linking, QuickJS WASM injection, agent wrapper generation, and WASM composition.
3. **Add Metadata** — embeds component name and version into the output WASM binary.
4. **Generate Bridge** — generates bridge SDK code if the project uses inter-component communication.

### Up-to-date Checks

`golem build` tracks file hashes of sources and targets. If nothing changed since the last build, steps are skipped automatically. Use `--force-build` to bypass this.

## Build Output

The final WASM artifact is placed in `golem-temp/agents/` under the application root:

- **Rust (debug preset)**: `golem-temp/agents/<component_name_snake_case>_debug.wasm`
- **Rust (release preset)**: `golem-temp/agents/<component_name_snake_case>_release.wasm`
- **TypeScript**: `golem-temp/agents/<component_name_snake_case>.wasm`
- **Scala**: `golem-temp/agents/<component_name_snake_case>.wasm`

The component name in snake_case is derived from the component name in `golem.yaml`. For example, a component named `my-app:rust-main` produces `my_app_rust_main_debug.wasm`.

## Available Options

| Option | Description |
|--------|-------------|
| `[COMPONENT_NAME]...` | Build only specific components (by default, all components are built) |
| `-s, --step <STEP>` | Run specific build step(s): `check`, `build`, `add-metadata`, `gen-bridge` |
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
        outputWasm: "golem-temp/agents/<name>_debug.wasm"
```

The `build` section is a list of steps. Each step can be:
- `command:` — a shell command to execute
- `injectToPrebuiltQuickjs:` — injects a JS bundle into a QuickJS WASM (TypeScript/Scala only)
- `preinitializeJs:` — preinitializes the JS runtime in the WASM (TypeScript/Scala only)

Each step can specify `sources` and `targets` for incremental builds, `dir` for the working directory, and `env` for environment variables.

## Cleaning Build Artifacts

```shell
golem clean
```

This removes `golem-temp/` and any other directories listed in the `clean` section of each preset.

## Common Build Errors

- **Missing `wasm32-wasip2` target** (Rust): run `rustup target add wasm32-wasip2`
- **Missing npm packages** (TypeScript): `golem build` automatically runs `npm install` if `package.json` is present but `node_modules` is missing
- **Type errors** (TypeScript): fix the errors in `.ts` source files; the `tsc` step runs with `--noEmit false --emitDeclarationOnly`
