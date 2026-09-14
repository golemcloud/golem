---
name: modifying-test-components
description: Builds or modifies selected test WASM components in test-components/. Use for fixture source changes, missing artifacts, or SDK changes requiring targeted downstream rebuilds.
---

# Modifying Test Components

Test WASMs are normally generated, gitignored artifacts. Build only what the selected tests need; `test-components/build-components.sh` is authoritative for normal Rust, TypeScript, benchmark membership and CI chunking.

## Find the Dependency and Build Path

1. Read the component's `AGENTS.md` when present and inspect its `golem.yaml`, `Cargo.toml`, or `package.json`.
2. Locate consumers with exact artifact/component-name searches in `integration-tests/`, `golem-worker-executor/`, and `golem-test-framework/`.
3. Locate SDK dependencies recursively without relying on a one-directory glob:
   `rg -n -g 'Cargo.toml' -g 'package.json' -g 'golem.yaml' 'golem-rust|golem-ts-sdk' test-components/`.
   Do not classify by language from directory names alone.
4. Check membership in the arrays at the top of `test-components/build-components.sh`. Use its group/chunk commands for listed applications or the component-specific instructions for an unlisted fixture.

For listed applications, ensure `golem-cli` exists (the script honors `GOLEM_CLI` and otherwise resolves the Cargo target directory). Build the package with `cargo build -p golem-cli --bin golem-cli` when needed—not `golem`. Do not hardcode `target/`; set `GOLEM_CLI` explicitly when using a redirected target directory.

All mutating `golem build` commands require `--yes`. Normal release Rust builds are orchestrated by:

```shell
cd test-components
./build-components.sh rust       # or rust-N / ts / ts-N / benchmarks
```

The script builds release Rust artifacts, runs each copy command, and handles TS presets. For one component, follow its current manifest/AGENTS command and verify the copied top-level `test-components/*.wasm`, not merely an intermediate `golem-temp` file.

## Keep Automatic Migrations

Before rebuilding, note the existing status of the selected component's source directory. A build
with the latest locally built Golem binary may migrate tracked source files, including manifests,
embedded skills, and other current-format metadata. Inspect changes newly produced by the build to
confirm they are migration output, then keep and include all of them in the changeset. They are part
of the rebuild even when they are unrelated to the source or SDK change that prompted it. Do not
revert, discard, or omit these migrations to narrow the diff; test components intentionally evolve
with Golem.

## SDK Prerequisites

- Rust SDK change: rebuild only components whose manifests resolve to the changed local SDK.
- TypeScript SDK change: from `sdks/ts`, install dependencies, run `pnpm run build`, then `pnpm run build-agent-template` before affected TS components. Current tooling also needs the repository's configured Node/pnpm, WASI SDK, `wasm-rquickjs-cli`, Rust `wasm32-wasip2`, and other prerequisites documented by `sdks/ts/AGENTS.md`/CI.
- `cargo make build-sdk-ts` skips when both `packages/golem-ts-sdk/dist` and `packages/golem-ts-sdk/wasm/agent_guest.wasm` exist; that is an existence cache, not freshness validation. Clean/rebuild explicitly after TS SDK source changes.

## Tracked Concurrent Fixtures

Two intentional exceptions are tracked WASMs and are excluded from `build-components.sh`:

- `test-components/concurrent-delivery-order/concurrent_delivery_order.wasm` — `cargo make build-concurrent-delivery-order-component`
- `test-components/concurrent-runtime-events/concurrent_runtime_events.wasm` — `cargo make build-concurrent-runtime-events-component`

Use only their dedicated tasks; they build minimal component-model-async fixtures and run `wasm-tools validate --features all`. Commit changed fixture WASMs.

## Validate

Confirm every expected destination exists and is non-empty; run `wasm-tools validate --features all <artifact>` when diagnosing or changing component construction. Then run the smallest consuming test. Tests that invoke Cargo, Golem, npm, or compilers as subprocesses belong specifically in the CLI integration test suite; unit, worker-executor, and non-CLI integration tests must not spawn them. Do not add legacy build fallbacks or compatibility paths; that remains prohibited until the repository-wide backward-compatibility policy is revised.
