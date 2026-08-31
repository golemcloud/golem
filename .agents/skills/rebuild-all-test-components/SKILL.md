---
name: rebuild-all-test-components
description: Rebuilds the complete normal test-component artifact set. Use only when explicitly asked for a full rebuild or when many worker/integration test WASMs are missing.
---

# Rebuild All Test Components

Run from the repository root:

```shell
cargo make build-test-components
```

This is the authoritative full rebuild. It builds repository prerequisites including **golem-cli** and the TypeScript SDK, then runs `test-components/build-components.sh rebuild` across its Rust, TypeScript, and benchmark arrays. Those arrays and `list-groups` output define the scope used by CI; do not maintain a second component list in this skill.

Do not build the `golem` package as a substitute for `golem-cli`. The component script honors `GOLEM_CLI` and resolves the Cargo target directory; never hardcode `target/...`. Every mutating `golem build` invocation must include `--yes`.

## Keep Automatic Migrations

Record the existing status under `test-components/` before the rebuild. The latest locally built
Golem binary may migrate component source directories while rebuilding them, updating manifests,
embedded skills, or other current-format metadata. Inspect newly produced source changes to confirm
they came from the rebuild, then keep and include every such migration in the changeset, even when
it is unrelated to the original reason for rebuilding. Do not revert or omit migrations to reduce
the diff; a full rebuild also advances test components to the current Golem format.

## Scope Exception

The full rebuild intentionally excludes two tracked minimal component-model-async fixtures:

- `cargo make build-concurrent-delivery-order-component`
- `cargo make build-concurrent-runtime-events-component`

Run those dedicated tasks only when their sources or committed WASMs must change. They validate their outputs and the resulting WASMs remain tracked; ordinary generated `test-components/*.wasm` files remain gitignored.

## TypeScript Caveat and Prerequisites

`build-test-components` depends on `build-sdk-ts`, which skips work if both the SDK `dist` directory and `wasm/agent_guest.wasm` already exist. This does not prove freshness. After TS SDK changes, clean and rebuild the SDK and agent template before the full rebuild.

Required tools follow current CI and scoped SDK instructions: Rust targets, cargo-make, Node/pnpm, WASI SDK, and the configured `wasm-rquickjs-cli` version. Read `sdks/ts/AGENTS.md` and `.github/workflows/ci.yaml` rather than copying versions into this skill.

## Focused Alternatives

Use `./build-components.sh rust`, `ts`, `benchmarks`, or a listed chunk (`rust-N`, `ts-N`) when a full rebuild is unnecessary. `./build-components.sh list-groups` is the CI matrix source. Use the `modifying-test-components` skill for one component.

## Validation

After completion:

1. Check each selected application's copy destination exists and is non-empty; do not accept only intermediate `golem-temp` artifacts.
2. Compare produced top-level WASM names/count against the selected arrays and inspect build failures rather than repeatedly rerunning everything.
3. Use `wasm-tools validate --features all` for suspicious or structurally changed artifacts.
4. Run the smallest relevant worker/integration/CLI test group.

Generated artifacts are prerequisites, not unit-test work: unit tests must never spawn Cargo, Golem, npm, or compilers. Do not add backwards-compatible build paths for removed component layouts.
