---
name: sdk-development
description: "Working on the Rust, TypeScript, MoonBit, or Go SDKs in sdks/. Use when modifying SDK code, adding SDK features, releasing an SDK, or testing SDK changes with the main Golem platform."
---

# SDK Development

The SDKs in `sdks/` are **not part of the main build flow** (`cargo make build` does not build them). Each SDK has its own build system and conventions.

## Rust SDK (`sdks/rust/`)

### Crates

- `golem-rust` — Runtime API wrappers (transactions, durability, agentic framework, value conversions)
- `golem-rust-macro` — Procedural macros (`#[derive(IntoValue)]`, `#[agent_definition]`, etc.)

### Building

```shell
cd sdks/rust
cargo build -p golem-rust
cargo build -p golem-rust-macro
```

### Testing

Tests use `test-r`. Each test file must have `test_r::enable!();` at the top.

```shell
cargo test -p golem-rust
cargo test -p golem-rust --features export_golem_agentic  # Agent tests
```

### Testing with the main platform

```shell
# From repository root
cargo make worker-executor-tests
```

### Testing with golem-cli

Set `GOLEM_RUST_PATH` to use local SDK in generated applications:

```shell
export GOLEM_RUST_PATH=/path/to/golem/sdks/rust/golem-rust
golem-cli app new my-test-app
```

### Code style

```shell
cargo fmt
cargo clippy
```

## TypeScript SDK (`sdks/ts/`)

### Prerequisites

- Node.js
- pnpm (managed via `packageManager` field)
- `wasm-rquickjs-cli`: `cargo install wasm-rquickjs-cli --version <VERSION>` (check `WASM_RQUICKJS_VERSION` in `.github/workflows/ci.yaml`)
- `cargo-component` v0.21.1 (exact version required for agent template builds)

### Packages

Build order matters: `golem-ts-types-core` → `golem-ts-typegen` → `golem-ts-sdk`

### Building

```shell
cd sdks/ts
npx pnpm install
npx pnpm run build
```

### Testing

```shell
npx pnpm run test
cd packages/golem-ts-sdk && pnpm run test  # Specific package
```

### Agent template WASM

The agent template WASM embeds the SDK runtime. You **must** rebuild it when:

- `wasm-rquickjs-cli` is updated
- WIT dependencies change
- SDK runtime code changes (`baseAgent.ts`, `index.ts`, `resolvedAgent.ts`)

```shell
cargo install cargo-component --version 0.21.1
npx pnpm run build-agent-template
```

Running `pnpm run build` alone is **not sufficient** — it only updates the JS bundle, not the pre-compiled WASM that TS components use.

### Testing with the main platform

```shell
# From repository root
cargo make cli-integration-tests
```

### Testing with golem-cli

```shell
export GOLEM_TS_PACKAGES_PATH=/path/to/golem/sdks/ts/packages
npx pnpm install && npx pnpm run build  # Build first!
golem-cli app new my-test-app
```

### Code style

```shell
npx pnpm run lint
npx pnpm run format
```

## MoonBit SDK (`sdks/moonbit/`)

See `sdks/moonbit/AGENTS.md` for full details. The MoonBit SDK has its own build system (`moon`) and code generation tools (`golem_sdk_tools`).

### Building

```shell
cd sdks/moonbit/golem_sdk
moon check --target wasm          # Type-check
moon build --target wasm          # Build
```

### Testing

```shell
cd sdks/moonbit/golem_sdk
moon test                         # Run SDK tests
cd sdks/moonbit/golem_sdk_tools
moon test                         # Run code generation tool tests
```

### Regenerating WIT bindings

```shell
cd sdks/moonbit/golem_sdk
wit-bindgen moonbit ./wit --derive-show --derive-eq --derive-error --project-name golemcloud/golem_sdk --ignore-stub
moon fmt
```

### Code style

```shell
moon fmt
moon info    # Regenerate .mbti interface files
```

## Go SDK (`sdks/go/golem/`)

Module path **`github.com/golemcloud/golem/sdks/go/golem`**, package `golem`.

The directory is `sdks/go/golem`, not `sdks/go`, on purpose: Go binds an import to the *package
clause*, but tooling and readers expect it to match the **last path element**. Naming the directory
`golem` makes them agree, so agents import the SDK with no alias:

```go
import "github.com/golemcloud/golem/sdks/go/golem"   // binds `golem`
```

A hyphenated name (`golem-go-sdk`) could not do this — hyphens are not valid Go identifiers, so it
would force an alias back. It also leaves `sdks/go/` free for siblings.

Built with
`componentize-go`, which is pinned per-project through Go's `tool` directive — never installed
globally.

### Building and testing

```shell
cd sdks/go/golem
go test ./...                                       # native tests; fast, no wasm needed
go vet .                                            # host vet, hand-written package
GOOS=wasip1 GOARCH=wasm go build ./...              # compile everything for the real target
GOOS=wasip1 GOARCH=wasm go vet -unsafeptr=false -composites=false .
```

Vet is scoped to `.`, not `./...`: the generated `internal/wit` bindings legitimately trip vet's
`unsafe.Pointer` / unkeyed-field checks, so the whole tree is *compiled* (the `go build` above), not
vetted, for the wasm target. CI runs exactly this set — plus a "bindings are committed" check — in the
`build-golem-go` job (`.github/workflows/ci.yaml`).

Native tests cover everything that does not reach a host import. `empty.s` lets a generated package
*compile* for the host, but the linker still needs a definition for any `//go:wasmimport` symbol that
host-arch code actually **references** — so RPC calls, `Future`, and `ClientFor` can only run under
wasm. Exercise those by creating an app with `golem app new … go` in a playground and building it;
the playground's `.golem-sdk-overrides` points the generated `go.mod` at this checkout.

### Regenerating WIT bindings

```shell
cargo make generate-sdk-go-bindings
```

Generated code lands in `internal/wit/` and is **wiped on every run**. The hand-written export slots
in `internal/exports/` survive because `--export-pkg-name` points the generated glue at them. The task
also runs `dev-tools/go-bindgen-fixup`, which works around two upstream wit-bindgen `crates/go` bugs
(tag-constant collisions, and a missing `empty.s` for the exports package).

### ⚠️ Releasing: Go has no package registry

Every other SDK publishes to a registry (crates.io / npm / maven / mooncakes) from a workflow
triggered by a `golem-<lang>-v*` tag. **Go has none — the git tag *is* the release**, read directly
from this repo by `proxy.golang.org`. There is no publish workflow to run.

Because the module lives in a subdirectory, Go **requires** the tag to be prefixed with that
subdirectory. This is a Go rule, not a choice:

```
sdks/go/golem/v0.1.0   ✅ the only form Go recognises
golem-go-v0.1.0        ❌ invisible to Go — do not use
```

Notes:

- This deliberately breaks the `golem-<lang>-v*` convention the other SDKs follow. It cannot be
  avoided; see <https://go.dev/ref/mod> ("module subdirectory ... also serves as a prefix for
  semantic version tags").
- It does **not** collide with anything. There is no root `go.mod`, so the repo's `v1.5.x` release
  tags are invisible to Go, and no existing tag has the `sdks/go/golem/` shape.
- Consumers using the default `GOPROXY` download only the `sdks/go/golem` subtree (~7 MB), not the
  whole repository. Only `GOPROXY=direct` clones the full repo, once per module cache. Repo size is
  not a concern: the mirror serves far larger monorepos of exactly this shape (aws-sdk-go-v2 is
  ~1.2 GB with subdirectory-tagged modules); the only hard limit is 500 MiB on the *module zip*.
- The mirror stores a module permanently only if it can **detect a license**, and Go has no metadata
  field for it — `sdks/go/golem/LICENSE` is the only mechanism, so it must stay in the module subtree.
  It carries the Golem Source License, matching the TS SDK packages.

### Local SDK overrides

`GOLEM_GO_PATH` (or `GOLEM_PATH`, which derives it) makes the CLI emit a `replace` directive into a
generated app's `go.mod`, pointing at the local checkout. This is how the playground tests SDK changes
without publishing — and, until the first tag exists, the **only** way a generated Go app can resolve
the SDK.

## Downstream Rebuild Requirements

SDK changes can require rebuilding test components. This is the most common source of errors.

### Rust SDK change → test components

1. Build `golem-rust` / `golem-rust-macro`
2. Find Rust test components depending on the SDK: check `test-components/*/Cargo.toml` for `golem-rust` references
3. Rebuild each affected component following its `AGENTS.md`

### TS SDK change → test components

1. Build TS SDK packages (`npx pnpm run build` in `sdks/ts/`)
2. Rebuild agent template WASM (`npx pnpm run build-agent-template` in `sdks/ts/`)
3. Find TS test components depending on the SDK
4. Rebuild each affected component following its `AGENTS.md`

**The agent template rebuild step is critical and easily forgotten.**

## WIT Dependencies

Every SDK has WIT files synced from the root `wit/` directory. **Never manually edit** `wit/deps/` in
any SDK — `cargo make wit` mirrors them, and `cargo make diff-wit` guards against drift.

```shell
# From repository root
cargo make wit
```

## Checklist

1. SDK code modified
2. SDK builds successfully
3. SDK tests pass
4. Agent template rebuilt (if TS SDK runtime code changed)
5. Go SDK: a generated app still builds (the only cover for host-call paths)
6. Dependent test components rebuilt (if any)
7. Platform tests pass (`cargo make worker-executor-tests` for Rust SDK, `cargo make cli-integration-tests` for TS SDK)
8. Code formatted and linted
