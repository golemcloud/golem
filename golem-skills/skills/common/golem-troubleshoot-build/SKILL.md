---
name: golem-troubleshoot-build
description: "Troubleshooting Golem build failures. Use when a build fails, produces unexpected errors, or when diagnosing dependency, tool, or manifest configuration issues."
---

# Troubleshooting Golem Build Failures

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Build Pipeline Overview

`golem build` executes a multi-step pipeline. Understanding which step failed is the first step to diagnosing problems:

1. **Check** — verifies build tools are installed and dependencies are correct
2. **Build** — runs language-specific compilation commands
3. **Add Metadata** — embeds component name and version into the output WASM
4. **Generate Bridge** — generates bridge SDK code for inter-component communication

Run a single step in isolation with `--step`:

```shell
golem build --step check --yes      # Run only the check step
golem build --step build --yes      # Run only the build step
```

## Step 1: Build Tool Requirement Checks

The **check** step verifies that required system tools are installed and meet minimum version requirements. The checks are language-specific:

### Rust Projects
| Tool | Check | Install Hint |
|------|-------|--------------|
| `rustup` | `rustup --version` (minimum version enforced) | https://www.rust-lang.org/tools/install |
| `rustc` | `rustc --version` (minimum version enforced) | `rustup install stable && rustup default stable` |
| `cargo` | `cargo version` (minimum version enforced) | Installed with Rust toolchain |
| `wasm32-wasip2` target | `rustup target list --installed` | `rustup target add wasm32-wasip2` |

### TypeScript Projects
| Tool | Check | Install Hint |
|------|-------|--------------|
| `node` | `node --version` (minimum version enforced) | https://nodejs.org/ |
| `npm` | `npm --version` (minimum version enforced) | Installed with Node.js |

### MoonBit Projects
| Tool | Check | Install Hint |
|------|-------|--------------|
| `moon` | `moon version` (minimum version enforced) | https://docs.moonbitlang.com |

If a tool is missing or below the minimum version, the check step fails with an error message and an install hint. To skip these checks (e.g., if you know tools are installed but detection fails), use `--skip-check`.

## Step 2: Dependency Validation and Auto-Fix

After tool checks, `golem build` validates project dependencies and can auto-fix them:

### Rust Dependency Checks
The CLI inspects every Rust component's `Cargo.toml` (and workspace `Cargo.toml` if present) for:
- **`golem-rust`** — must be present with a semantically compatible version (or matching path for local development)
- **`wstd`** — checked if present, version compatibility verified
- **`log`** — checked if present, ensures the `kv` feature is enabled
- **`serde`** — checked if present, ensures the `derive` feature is enabled
- **`serde_json`** — checked if present, version compatibility verified

If dependencies are outdated or missing required features, the CLI shows a diff and prompts for confirmation before applying fixes.

### TypeScript Dependency Checks
The CLI inspects the root `package.json` for:
- **`@golemcloud/golem-ts-sdk`** — must be present with a compatible version
- **`@golemcloud/golem-ts-typegen`** (devDependency) — must be present
- **Rollup plugins** (`@rollup/plugin-alias`, `@rollup/plugin-node-resolve`, `@rollup/plugin-typescript`, `@rollup/plugin-commonjs`, `@rollup/plugin-json`) — version compatibility checked
- **`typescript`**, **`rollup`**, **`tslib`**, **`@types/node`** — version compatibility checked

The CLI also validates `tsconfig.json` settings:
- `compilerOptions.moduleResolution` must be `"bundler"`
- `compilerOptions.experimentalDecorators` must be `true`
- `compilerOptions.emitDecoratorMetadata` must be `true`

### AGENTS.md and Skill Files
The CLI checks that the project's `AGENTS.md` and `.agents/skills/` directory contain up-to-date content matching the current CLI version's templates. If they are stale, the CLI updates them automatically during the build.

### Viewing Planned Fixes Without Applying
Run the check step alone to see what the CLI wants to fix without building:

```shell
golem build --step check --yes
```

## Common Build Errors and Solutions

### "X is not available" / Tool Not Found
The CLI cannot find a required tool. Check that it is installed and on your `PATH`:
```shell
which cargo     # Rust
which node      # TypeScript
which moon      # MoonBit
```

### "X version could not be detected"
The CLI found the tool but could not parse its version. This usually means a non-standard version string. Try updating the tool.

### Missing Rust Target
```
rust target wasm32-wasip2 is not installed
```
Fix: `rustup target add wasm32-wasip2`

### Dependency Version Mismatch
The CLI shows a diff of the changes it wants to make. Review the diff — it will update version numbers or add missing features. Pass `--yes` to auto-accept.

### TypeScript `tsconfig.json` Issues
If `moduleResolution` is not set to `"bundler"` or decorator settings are missing, the build will fail during type checking. The CLI auto-fixes these during the check step.

### Up-to-Date Check Confusion
`golem build` tracks file hashes to skip unchanged steps. If files were modified outside the build (e.g., by `cargo build`), the check may be stale. Force a full rebuild:
```shell
golem build --force-build --yes
```

Or clean and rebuild:
```shell
golem clean
golem build --yes
```

## Diagnosing Manifest Configuration Issues with `manifest-trace`

When environment variables, config, plugins, or files are not what you expect at runtime, use `manifest-trace` to see exactly how the manifest configuration is resolved:

```shell
golem component manifest-trace
```

This command outputs a detailed trace for each component showing:

- **Applied Layers** — the ordered list of configuration layers that contributed to the final values (e.g., component template, component definition, agent template, agent common, environment presets, custom presets)
- **Property Values and Origins** — for each property (`config`, `env`, `wasiConfig`, `plugins`, `files`, `build`, `clean`, `componentWasm`, `outputWasm`), the final resolved value plus a trace of which layer introduced or modified it
- **Merge Operations** — whether values were merged (showing inserted/updated entries) or entirely replaced, and from which layer

### Example Usage

```shell
# Trace all components
golem component manifest-trace

# Trace a specific component
golem component manifest-trace my-app:main
```

### What to Look For

1. **Missing environment variables**: Check the `env` property trace to see if the variable was defined in a layer that was overridden or replaced by a later layer
2. **Wrong config values**: Check the `config` property trace — a later layer may have replaced the entire config object instead of merging individual fields
3. **Plugin not applied**: Check the `plugins` property trace to see if the plugin was defined in a layer that is not in the `appliedLayers` list (e.g., an environment that is not active)
4. **Unexpected merge behavior**: Check `envMergeMode`, `pluginsMergeMode`, or `filesMergeMode` — these control whether later layers merge with or replace earlier values
5. **Preset not active**: Verify that the expected preset appears in `appliedLayers` — if it is missing, it may not be set as the default or selected via the environment

### Combining with Environment Selection

To see how the trace changes under a specific environment:

```shell
golem component manifest-trace -e staging
```

This activates the `staging` environment's presets, letting you verify that environment-specific overrides are applied correctly.
