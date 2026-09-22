---
name: sdk-development
description: "Working on the Rust, TypeScript, MoonBit, or Go SDKs in sdks/. Use when modifying SDK code, adding SDK features, releasing an SDK, or testing SDK changes with the main Golem platform."
---

# SDK Development

The SDKs in `sdks/` are **not part of the main build flow** (`cargo make build` does not build them). Each SDK has its own build system and conventions.

## Rust SDK (`sdks/rust/`)

### Crates

- `golem-rust` — Runtime API wrappers (transactions, durability, agentic framework, value
  conversions). Also re-exports the `#[derive(IntoSchema)]` / `#[derive(FromSchema)]` derives, which
  are defined by the root-workspace `golem-schema-derive` crate, not by `golem-rust-macro`.
- `golem-rust-macro` — Procedural macros: `#[agent_definition]`, `#[agent_implementation]`,
  `#[tool_definition]`, `#[golem_operation]`, and the `MultimodalSchema`, `ConfigSchema`,
  `AllowedLanguages`, `AllowedMimeTypes`, and `ToolError` derives. This crate lives in the root
  workspace, alongside `golem-tool-metadata` and `golem-native-tool`.

### Building

```shell
# From the repository root
cargo build --manifest-path sdks/rust/Cargo.toml -p golem-rust
cargo build -p golem-rust-macro
```

### Testing

Tests use `test-r`. Library entry points call `#[cfg(test)] test_r::enable!();`. Integration-test
entry points call `test_r::enable!();` at the top, and `test_r::test` must be in lexical scope for
each `#[test]`.

```shell
cargo test -p golem-rust
cargo test -p golem-rust --features export_golem_agentic  # Agent tests
```

### Testing with the main platform

```shell
# From the repository root, after building the test components needed by <affected-test>:
cargo test -p golem-worker-executor --test integration -- <affected-test> --report-time
```

Use the `modifying-test-components` skill to build the selected test's WASM prerequisite. Run a
worker-executor group or `cargo make worker-executor-tests` only for broad runtime, durability,
value-conversion, or agent framework changes whose consumers cannot be isolated.

### Testing with golem-cli

Set `GOLEM_RUST_PATH` to use local SDK in generated applications:

```shell
export GOLEM_RUST_PATH=/path/to/golem/sdks/rust/golem-rust
golem-cli app new my-test-app
```

### Code style

```shell
cargo fmt -p <affected-sdk-crate> -- --check
cargo clippy -p <affected-sdk-crate> --all-targets -- -Dwarnings
```

## TypeScript SDK (`sdks/ts/`)

### Prerequisites

- Node.js
- pnpm (managed via `packageManager` field)
- `wasm32-wasip2`: `rustup target add wasm32-wasip2` (the Preview 3 wrapper still builds through
  this Rust target)
- `wasm-rquickjs-cli`: `cargo install --locked wasm-rquickjs-cli@<VERSION>` (check
  `WASM_RQUICKJS_VERSION` in `.github/workflows/ci.yaml`)

### Packages

Build order matters: `golem-ts-sdk` → `golem-ts-bridge` → `golem-ts-repl`.

### Building

```shell
cd sdks/ts
npx pnpm install
npx pnpm run build
```

### Testing

```shell
npx pnpm --filter <affected-package-with-a-test-script> run test
npx pnpm run test  # All packages, for cross-package changes
```

Check the affected package's `package.json` before running a package test; for example,
`@golemcloud/golem-ts-bridge` currently has no `test` script.

### Agent template WASM

The agent template WASM embeds the existing `packages/golem-ts-sdk/dist/index.mjs`. Build that bundle before rebuilding the template whenever a change can affect the emitted runtime. Triggers include:

- `wasm-rquickjs-cli` is updated
- WIT dependencies change
- any source or dependency in the Rollup graph rooted at `packages/golem-ts-sdk/src/index.ts` changes
- wrapper generation or agent-template toolchain inputs change

The filenames above are intentionally described by dependency graph rather than a fixed list: runtime modules can be added or reorganized.

```shell
npx pnpm --filter @golemcloud/golem-ts-sdk run build
npx pnpm run build-agent-template
```

The package build refreshes `dist/index.mjs`; `build-agent-template` then embeds it in the pre-compiled WASM. Running either command alone is not sufficient after a runtime change.

### Testing with the main platform

```shell
# From repository root
cargo make build-cli-test-bins-non-ci
(cd sdks/ts && npx pnpm run build && npx pnpm run build-agent-template)
# Build the specific test components required by <affected-filter>.
cargo-test-r run --package golem-cli --test integration <affected-filter> -- --report-time --nocapture
```

Prefer targeted CLI integration filters that generate or exercise the affected SDK feature. They require fresh CLI binaries, SDK/template artifacts, and any selected test components. Use the full CLI suite only for broad template, bridge, REPL, or generated-application changes; after TS source changes, refresh the SDK/template first because `build-sdk-ts` skips when output files already exist.

### Testing with golem-cli

```shell
export GOLEM_TS_PACKAGES_PATH=/path/to/golem/sdks/ts/packages
npx pnpm install && npx pnpm run build  # Build first!
golem-cli app new my-test-app
```

### Code style

```shell
npx pnpm --filter <affected-package> run lint
npx pnpm exec prettier --check <changed-paths>
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
./scripts/run-sdk-tests.sh
# For a narrower package/file check that does not need the JS runner:
moon test --target wasm <affected-package-or-file>
cd sdks/moonbit/golem_sdk_tools
moon test <affected-package-or-file>
```

### Regenerating WIT bindings

```shell
cd sdks/moonbit/golem_sdk
moon run script bindgen  # Enforces the pinned Golem wit-bindgen and required post-processing
moon info
moon fmt
```

### Code style

```shell
moon info    # Regenerate .mbti files when public interfaces changed
moon fmt
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
  It carries the Apache License 2.0 (the current SDK-license direction; a later session aligns the
  other SDKs, which still ship the Golem Source License for now).

### Local SDK overrides

`GOLEM_GO_PATH` (or `GOLEM_PATH`, which derives it) makes the CLI emit a `replace` directive into a
generated app's `go.mod`, pointing at the local checkout. This is how the playground tests SDK changes
without publishing — and, until the first tag exists, the **only** way a generated Go app can resolve
the SDK.

## Downstream Rebuild Requirements

SDK changes can require rebuilding test components. This is the most common source of errors.

### Rust SDK change → test components

1. Build `golem-rust` / `golem-rust-macro`
2. Find Rust test components depending on the SDK: check `test-components/**/Cargo.toml` for `golem-rust` references
3. Rebuild each affected component following its `AGENTS.md`

### TS runtime/WIT change → test components

1. Build the affected TS SDK package and its required package dependencies (`npx pnpm run build` in `sdks/ts/` is the broad option)
2. Rebuild agent template WASM (`npx pnpm run build-agent-template` in `sdks/ts/`)
3. Find TS test components depending on the SDK
4. Rebuild each affected component following its `AGENTS.md`

Type-only, test-only, documentation, bridge, or REPL changes that cannot affect `golem-ts-sdk/dist/index.mjs` or WIT do not require an agent-template rebuild.

## Test Process Ownership

SDK unit tests and worker-executor tests must not invoke external tools or Golem binaries. Put tests
that execute `golem`, `golem-cli`, `cargo`, `rustc`, `npm`, `npx`, `tsc`, `moon`, or another process
in the CLI integration test suite. Build SDKs, generated code, templates, and test components as
prerequisites before running unit or worker-executor tests. Non-CLI integration tests use
`golem-test-framework` for process-backed dependencies and do not spawn additional processes
directly.

## WIT Dependencies

The Rust, TypeScript, and MoonBit SDKs covered by this skill have WIT files synced from the root
`wit/` directory. **Never manually edit** their `wit/deps/` copies.

```shell
# From repository root
cargo make wit
```

## Checklist

1. SDK code modified
2. SDK builds successfully
3. SDK tests pass
4. Agent template rebuilt from a fresh bundle (if TS runtime bundle or WIT inputs changed)
5. Dependent test components rebuilt (if any)
   (Go SDK: a generated app still builds — the only cover for its host-call paths)
6. Platform tests that exercise changed SDK/platform integration pass
7. Affected SDK code is formatted and linted with its native tools
8. Full SDK/platform suites run only for broad or unclear impact
