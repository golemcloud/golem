---
name: modifying-wit-interfaces
description: "Adding or modifying WIT (WebAssembly Interface Types) interfaces. Use when changing .wit files, updating WIT dependencies, or working with component interfaces."
---

# Modifying WIT Interfaces

Golem uses WIT (WebAssembly Interface Types) to define component interfaces. WIT files are centrally managed and synchronized across multiple sub-projects.

## Directory Structure

### Central WIT directory

The root `wit/` directory is the source of truth:

```
wit/
├── host.wit           # Core Golem host interface
├── deps.toml          # WIT dependency declarations
├── deps.lock          # Locked dependency versions
└── deps/              # Fetched WIT dependencies
    ├── io/
    ├── clocks/
    ├── golem-1.x/
    ├── golem-core/
    ├── golem-agent/
    └── logging/
```

### Synchronized copies

WIT files are copied to these sub-projects by `cargo make wit`:

| Target | WIT deps copied |
|--------|----------------|
| `golem-wasm/wit/deps/` | io, clocks, golem-1.x, golem-core |
| `golem-common/wit/deps/` | io, clocks, golem-1.x, golem-core, golem-agent |
| `cli/golem-cli/wit/deps/` | clocks, io, golem-1.x, golem-core, golem-agent, logging |
| `sdks/rust/golem-rust/wit/deps/` | All deps + golem-ai |
| `sdks/ts/wit/deps/` | All deps + golem-ai |
| `test-components/oplog-processor/wit/deps/` | All deps |

**Never manually edit** files in any `wit/deps/` directory. They are overwritten by `cargo make wit`.

## Modifying an Existing WIT Interface

### Step 1: Edit the WIT file

Edit the relevant `.wit` file in the root `wit/` directory (e.g., `wit/host.wit` or a file under `wit/deps/`).

If editing a dependency managed by `deps.toml`, you may need to update the dependency version in `wit/deps.toml` first.

### Step 2: Synchronize WIT across sub-projects

```shell
cargo make wit
```

This removes all `wit/deps/` directories in sub-projects and re-copies the correct subset from the root.

### Step 3: Verify synchronization

```shell
cargo make check-wit
```

This runs `cargo make wit` and then checks `git diff` to ensure the committed WIT files match what the sync produces. If this fails in CI, you forgot to run `cargo make wit`.

### Step 4: Build and verify

```shell
cargo make build
```

WIT changes affect generated bindings in multiple crates. A full build ensures all bindings are regenerated correctly.

## Adding a New WIT Dependency

### Step 1: Add to deps.toml

Edit `wit/deps.toml` to add the new dependency.

### Step 2: Fetch and sync

```shell
cargo make wit
```

### Step 3: Update sync tasks if needed

If the new dependency needs to be available in specific sub-projects, edit `Makefile.toml` to add the copy step in the appropriate `wit-*` task (e.g., `wit-golem-wasm`, `wit-golem-common`, `wit-golem-cli`, `wit-sdks`, `wit-test-components`).

## Downstream Impact

WIT changes can have wide-reaching effects:

| What changed | What needs rebuilding |
|---|---|
| Core interfaces (`golem-1.x`, `golem-core`) | Everything: services, SDKs, test components |
| Agent interfaces (`golem-agent`) | golem-common, CLI, SDKs, agent test components |
| SDK-only interfaces (`golem-ai`) | SDKs only |
| Host interface (`host.wit`) | Worker executor, services |

### SDK rebuild chain

If WIT changes affect SDK interfaces:

1. **Rust SDK**: Rebuild `golem-rust` (bindings are generated via `wit_bindgen::generate!`)
2. **TS SDK**: Rebuild packages (`npx pnpm run build` in `sdks/ts/`), then rebuild agent template WASM (`npx pnpm run build-agent-template`)
3. **Test components**: Rebuild any test components that use the changed interfaces (see their `AGENTS.md`)

## Checklist

1. WIT file edited in root `wit/` directory (not in a `wit/deps/` copy)
2. `cargo make wit` run to synchronize
3. `cargo make check-wit` passes
4. `Makefile.toml` sync tasks updated if a new dependency was added
5. `cargo make build` succeeds
6. SDKs rebuilt if SDK interfaces changed
7. Test components rebuilt if their interfaces changed
8. `cargo make fix` run before PR
