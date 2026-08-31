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

Make contract changes directly in the current package and update every in-tree host, SDK, test
component, generated binding, and synchronized copy. Do not add old-signature aliases, duplicate
legacy interfaces/packages, fallback handling, or adapters for older guests or hosts.

### Synchronized copies (generated — do not hand-edit)

`cargo make wit` mirrors the configured root dependencies into the `wit/deps/`
directories below. **Never manually edit one of these generated copies** — your changes will be
overwritten. Edit the root and re-sync. The explicitly hand-synchronized `golem-schema` copies in
the following section are the exception.

| Target | WIT deps copied |
|--------|----------------|
| `golem-common/wit/deps/` | clocks, golem-1.x, golem-core-v2, golem-agent, golem-secrets, golem-tool |
| `cli/golem-cli/wit/deps/` | clocks, golem-1.x, golem-core-v2, golem-agent, logging |
| `sdks/rust/golem-rust/wit/deps/` | **all** root deps |
| `sdks/ts/wit/deps/` | **all** root deps |
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
| `golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit` | `golem:core/types@2.0.0` | `wit/deps/golem-core-v2/golem-core-v2.wit` |
| `golem-schema/wit/deps/golem-tool/common.wit` | `golem:tool/common@0.1.0` | `wit/deps/golem-tool/common.wit` |

`golem-schema` generates the shared core and tool transport types and host bindings from these
copies via its `golem-schema.wit` world, so both must match their root source exactly. After
`cargo make wit`, copy each changed file into `golem-schema` and verify it with `diff -q`.

Note also that `golem-quota` is copied only to the SDKs (via the `wit-sdks`
glob), not to `golem-common` or `cli/golem-cli`.

## Modifying an Existing WIT Interface

### Step 1: Edit the WIT file

Edit the relevant `.wit` file in the root `wit/` directory (e.g., `wit/host.wit` or a file under `wit/deps/<package>/`). This is the source of truth. Do not edit generated sub-project copies; update the explicitly hand-synchronized `golem-schema` copies after changing the root.

### Step 2: Synchronize WIT across sub-projects

```shell
cargo make wit
cp wit/deps/golem-core-v2/golem-core-v2.wit \
  golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit
cp wit/deps/golem-tool/common.wit \
  golem-schema/wit/deps/golem-tool/common.wit
diff -q wit/deps/golem-core-v2/golem-core-v2.wit \
  golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit
diff -q wit/deps/golem-tool/common.wit \
  golem-schema/wit/deps/golem-tool/common.wit
```

`cargo make wit` mirrors the correct subset of the root `wit/deps/` into each sub-project,
idempotently (it rewrites only files whose bytes changed, so unchanged files keep their mtime —
avoiding needless rebuilds). Run only the `cp`/`diff` pairs for a hand-synchronized package that
changed; they are shown together so neither `golem-schema` exception is missed.

### Step 3: Review synchronization

```shell
cargo make wit
git status --short -- \
  golem-common/wit/deps golem-schema/wit/deps cli/golem-cli/wit/deps \
  sdks/rust/golem-rust/wit/deps sdks/ts/wit/deps \
  sdks/scala/wit/deps sdks/moonbit/golem_sdk/wit/deps
```

Review every changed synchronized copy against the root source change. `cargo make check-wit` re-runs synchronization and then requires these paths to be clean in Git, so it is a clean-checkout/CI drift check: it is expected to fail in a normal PR worktree containing intentional, uncommitted or staged synchronized changes. CI runs it after checkout to ensure committed copies are current.

### Step 4: Build and verify the affected scope

Use the downstream impact table below to choose builds and tests. For example, a package not copied into either root consumer requires affected SDK checks, not a root workspace build; a `host.wit` change requires worker executor and affected service checks; core interfaces generally require broad verification.

Use `cargo make build` when the WIT change affects most of the root workspace or when the consumer set is unclear. If the change affects SDK-facing types, run the relevant SDK build/tests independently because `cargo make build` does not build the SDKs.

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
```

Review the synchronized-copy diff locally. Run `cargo make check-wit` only from a clean checkout containing the committed change, or leave that clean-checkout drift check to CI.

## Downstream Impact

WIT changes can have wide-reaching effects:

| What changed | What needs rebuilding |
|---|---|
| Core interfaces (`golem-1.x`, `golem-core-v2`) | `golem-schema`, services, SDKs, and test components |
| Agent interfaces (`golem-agent`) | golem-common, CLI, SDKs, agent test components |
| `clocks` | golem-common, CLI, and SDKs that import the changed definitions |
| `golem-secrets` | golem-common and SDKs that import the changed definitions |
| `golem-tool` | `golem-schema`, golem-common, and SDKs that import the changed definitions |
| `logging` | CLI and SDKs that import the changed definitions |
| Other package under `wit/deps/` | All four synchronized SDK inputs; rebuild SDK bindings/components that import it |
| Host interface (`host.wit`) | Worker executor, services |

### SDK rebuild chain

If WIT changes affect SDK interfaces:

1. **Rust SDK**: Rebuild `golem-rust` (bindings are generated via `wit_bindgen::generate!`)
2. **TS SDK**: Rebuild packages (`npx pnpm run build` in `sdks/ts/`), then rebuild agent template WASM (`npx pnpm run build-agent-template`)
3. **Scala SDK**: Regenerate `agent_guest.wasm`, adjust Scala SDK types or codecs if the WIT shape changed, and run the relevant Scala test suites
4. **MoonBit SDK**: In `sdks/moonbit/golem_sdk/`, run `moon run script bindgen` (the pinned generator and required post-processing), then `moon fmt` and `moon check --target wasm`
5. **Test components**: Rebuild any test components that use the changed interfaces (see their `AGENTS.md`)

## Checklist

1. WIT file edited in root `wit/`; generated copies were not hand-edited
2. `cargo make wit` run to synchronize
3. Changed `golem-schema` core/tool copies synchronized by hand and byte-compared with the root
4. Synchronized-copy diff reviewed; `cargo make check-wit` left to clean-checkout/CI validation unless verifying a committed checkout
5. `Makefile.toml` sync tasks updated if a new dependency was added
6. Root crates affected according to the impact table check/build successfully
7. SDKs rebuilt if SDK interfaces changed
8. Relevant SDK tests run when their WIT inputs changed
9. Test components rebuilt if their interfaces changed
10. Full root workspace build run only for broad or unclear root-workspace impact
11. Formatting and linting follow the scope-based `pre-pr-checklist`
