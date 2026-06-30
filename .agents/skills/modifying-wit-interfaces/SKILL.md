---
name: modifying-wit-interfaces
description: "Adding or modifying WIT (WebAssembly Interface Types) interfaces. Use when changing .wit files, updating WIT dependencies, or working with component interfaces."
---

# Modifying WIT Interfaces

Golem uses WIT (WebAssembly Interface Types) to define component interfaces. WIT files are maintained in-repo and synchronized across multiple sub-projects.

## Directory Structure

### The root `wit/` directory is the source of truth

The root `wit/` directory holds the **hand-edited source of truth** for every
WIT package — both the Golem-owned packages (e.g. `golem:core`, `golem:quota`,
`golem:agent`, `golem:durability`, the `golem-1.x` packages) and the vendored
third-party deps (`wasi:io`, `wasi:clocks`, `wasi:http`, etc.). These files are
**not fetched** from anywhere: there is no `wit/deps.toml`/`wit/deps.lock` and
`cargo make wit` does not download anything — it only **copies** subsets of the
root files into the sub-projects.

```
wit/
├── host.wit           # Core Golem host interface (source of truth)
└── deps/              # Source of truth for ALL WIT packages (hand-edit these)
    ├── golem-1.x/         golem-agent/      golem-core-v2/   golem-durability/
    ├── golem-quota/       golem-rdbms/      golem-websocket/
    ├── io/  clocks/  http/  blobstore/  keyvalue/  config/
    └── filesystem/  random/  sockets/  cli/  logging/
```

To change a Golem WIT interface, edit the relevant file under the **root**
`wit/deps/<package>/` (e.g. `wit/deps/golem-core-v2/golem-core-v2.wit`,
`wit/deps/golem-quota/types.wit`) or `wit/host.wit`.

### Synchronized copies (generated — do not hand-edit)

`cargo make wit` deletes and re-creates the `wit/deps/` directories inside the
sub-projects below by copying from the root. **Never manually edit a sub-project
`wit/deps/` copy** — your changes will be overwritten. Edit the root and re-sync.

| Target | WIT deps copied |
|--------|----------------|
| `golem-common/wit/deps/` | io, clocks, golem-1.x, golem-core-v2, golem-agent |
| `cli/golem-cli/wit/deps/` | clocks, io, golem-1.x, golem-core-v2, golem-agent, logging |
| `sdks/rust/golem-rust/wit/deps/` | **all** root deps + golem-ai (overlaid from `sdks/rust/golem-rust/wit/golem-ai`) |
| `sdks/ts/wit/deps/` | **all** root deps + golem-ai (overlaid from `sdks/ts/wit/golem-ai`) |
| `sdks/scala/wit/deps/` | **all** root deps |
| `sdks/moonbit/golem_sdk/wit/deps/` | **all** root deps |

The exact copy lists live in the `wit-golem-common`, `wit-golem-cli`, and
`wit-sdks` tasks in `Makefile.toml`.

### Copies NOT covered by `cargo make wit` (sync by hand)

Some crates keep their own committed `wit/deps/` copy that the sync tasks above
do **not** touch. If you change a package they embed, copy the root file into
them manually in the same change:

| Hand-synced copy | Embeds | Keep in sync with |
|------------------|--------|-------------------|
| `golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit` | golem-core-v2 only | `wit/deps/golem-core-v2/golem-core-v2.wit` |

`golem-schema` generates the shared `golem:core/types` transport types (guest +
host) from this copy via its `golem-schema.wit` world, so it must match the root
`golem-core-v2` exactly. After `cargo make wit`, run e.g.
`cp wit/deps/golem-core-v2/golem-core-v2.wit golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit`
and verify with `diff -q`.

Note also that `golem-quota` is copied only to the SDKs (via the `wit-sdks`
glob), not to `golem-common` or `cli/golem-cli`.

## Modifying an Existing WIT Interface

### Step 1: Edit the WIT file

Edit the relevant `.wit` file in the root `wit/` directory (e.g., `wit/host.wit` or a file under `wit/deps/<package>/`). This is the source of truth — never edit a sub-project `wit/deps/` copy.

### Step 2: Synchronize WIT across sub-projects

```shell
cargo make wit
```

This mirrors the correct subset of the root `wit/deps/` into each sub-project, idempotently (it rewrites only files whose bytes changed, so unchanged files keep their mtime — avoiding needless rebuilds).

### Step 3: Verify synchronization

```shell
cargo make check-wit
```

This re-runs the sync and then `git status` over every per-crate `wit/deps` copy (golem-common, golem-cli, and all four SDKs) to ensure the committed WIT files match what the sync produces. CI runs this; if it fails, you forgot to run `cargo make wit` (or you hand-edited a generated sub-project copy).

### Step 4: Build and verify

```shell
cargo make build
```

WIT changes affect generated bindings in multiple crates. A full build ensures all bindings are regenerated correctly.
If the change affects SDK-facing types, also run the relevant SDK test suites before considering the work complete.

## Adding a New WIT Package

### Step 1: Add the package directory under the root `wit/deps/`

Create `wit/deps/<package>/` and add its `.wit` file(s). This is the source of
truth — there is no `deps.toml` and nothing is fetched.

### Step 2: Wire it into the sync tasks

Edit `Makefile.toml` so the new package is mirrored where it's needed. The
`wit-sdks` task mirrors **all** root deps to every SDK automatically, but the
`wit-golem-common` and `wit-golem-cli` tasks mirror an explicit subset — add a
`wit/deps/<package> <target>/wit/deps/<package>` source/target pair to the
`dir-mirror` args there if those crates need it.

### Step 3: Sync and verify

```shell
cargo make wit
cargo make check-wit
```

## Downstream Impact

WIT changes can have wide-reaching effects:

| What changed | What needs rebuilding |
|---|---|
| Core interfaces (`golem-1.x`, `golem-core-v2`) | Everything: services, SDKs, test components |
| Agent interfaces (`golem-agent`) | golem-common, CLI, SDKs, agent test components |
| SDK-only interfaces (`golem-ai`) | SDKs only |
| Host interface (`host.wit`) | Worker executor, services |

### SDK rebuild chain

If WIT changes affect SDK interfaces:

1. **Rust SDK**: Rebuild `golem-rust` (bindings are generated via `wit_bindgen::generate!`)
2. **TS SDK**: Rebuild packages (`npx pnpm run build` in `sdks/ts/`), then rebuild agent template WASM (`npx pnpm run build-agent-template`)
3. **Scala SDK**: Regenerate `agent_guest.wasm`, adjust Scala SDK types or codecs if the WIT shape changed, and run the relevant Scala test suites
4. **MoonBit SDK**: Regenerate WIT bindings (`wit-bindgen moonbit` in `sdks/moonbit/golem_sdk/`), then `moon fmt` and `moon check --target wasm`
5. **Test components**: Rebuild any test components that use the changed interfaces (see their `AGENTS.md`)

## Checklist

1. WIT file edited in root `wit/` directory (not in a `wit/deps/` copy)
2. `cargo make wit` run to synchronize
3. `cargo make check-wit` passes
4. `Makefile.toml` sync tasks updated if a new dependency was added
5. `cargo make build` succeeds
6. SDKs rebuilt if SDK interfaces changed
7. Relevant SDK tests run when WIT files change
8. Test components rebuilt if their interfaces changed
9. `cargo make fix` run before PR
