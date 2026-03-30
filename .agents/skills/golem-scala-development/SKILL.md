---
name: golem-scala-development
description: "Compile, publish, and test the Golem Scala SDK. Use when working on the sdks/scala/ subtree: building the SDK, publishing locally, compiling/running the example demo, regenerating the agent_guest.wasm, or debugging end-to-end deployment."
---

# Golem Scala SDK Development

The Golem SDK for Scala.js lives under `sdks/scala/` in the Golem repository. It targets the Golem WIT API v1.5.0 and produces WASM components that run on the Golem platform via a QuickJS-based guest runtime.

## Repository Layout

```
sdks/scala/
├── core/           # golem-scala-core (Scala.js facades, agent framework) — JS-only
├── model/          # golem-scala-model (WIT value types, RPC types)
├── macros/         # golem-scala-macros (Scala 3 macros, JVM-only)
├── codegen/        # Shared build-time code generation library
├── sbt/            # golem-scala-sbt (SBT plugin, Scala 2.12)
├── mill/           # Mill plugin
├── wit/            # WIT definitions (main.wit + deps/)
│   ├── main.wit    # Primary WIT — package golem:agent-guest, world agent-guest
│   ├── deps/       # WIT dependencies (copied from golem repo)
│   └── dts/        # Generated TypeScript d.ts (source of truth for JS exports)
├── scripts/        # generate-agent-guest-wasm.sh
├── example/        # Standalone demo project (separate sbt build)
├── test-agents/    # Test agent definitions + implementations for integration tests
├── integration-tests/ # Integration test suite
└── docs/           # Documentation
```

## Scala Versions

- **Scala 3.8.2** — All Golem Scala 3 projects. Prefix sbt commands with `++3.8.2` (without `!` — only golem projects with 3.8.2 in crossScalaVersions are affected).
- **Scala 2.13.18** — Cross-build for Scala 2 users.
- **Scala 2.12.21** — The SBT plugin (`golemScalaSbt`) only. Use `++2.12.21!` (the `!` forces override).

> **Important**: `sbt --client` mode preserves Scala version across invocations. Always specify the version explicitly to avoid version drift.

## SBT Project Names

| Project | Description |
|---------|-------------|
| `golemScalaCoreJS` | Core agent framework, Scala.js facades (JS-only) |
| `golemScalaModelJS` / `golemScalaModelJVM` | WIT value types, RPC types |
| `golemScalaMacros` | Scala 3 macros (JVM only, cross-used at compile time) |
| `golemScalaSbt` | SBT plugin (Scala 2.12) |
| `golemScalaTestAgents` | Test agents for integration tests |

## Running All Tests

Use these sbt aliases (from `sdks/scala/`) to run all golem-scala tests:

| Alias | What it runs |
|-------|-------------|
| `sbt golemTest3` | All unit tests (JVM + JS) + test-agents compile + integration tests — **Scala 3** |
| `sbt golemTest2` | All unit tests (JVM + JS) + test-agents compile — **Scala 2** (integration tests are Scala 3 only) |
| `sbt golemTestAll` | Both of the above (Scala 3 then Scala 2) |

**Always run `golemTestAll` before considering a change complete.**

Integration tests require the TypeScript SDK packages path. The `GOLEM_TS_PACKAGES_PATH` env var is forwarded automatically by `build.sbt`, but `sbt --client` doesn't propagate env vars. Use non-client `sbt` instead:

```bash
cd sdks/scala
GOLEM_TS_PACKAGES_PATH=<TS_PACKAGES_PATH> sbt golemTestAll
```

## Compiling

From `sdks/scala/`:

```bash
# Compile test agents (good smoke test)
sbt "++3.8.2; golemScalaTestAgents/fastLinkJS"

# Compile core
sbt "++3.8.2; golemScalaCoreJS/compile"

# Compile model
sbt "++3.8.2; golemScalaModelJS/compile"
```

Use the sbt logging pattern:
```bash
cd sdks/scala
LOG=".git/agent-logs/sbt-$(date +%s)-$$.log"
mkdir -p "$(dirname "$LOG")"
sbt -Dsbt.color=false "++3.8.2; golemScalaTestAgents/fastLinkJS" >"$LOG" 2>&1
echo "Exit: $? | Log: $LOG"
# Query: tail -50 "$LOG" or grep -i error "$LOG"
```

## Publishing Locally

The `example` project depends on `0.0.0-SNAPSHOT` artifacts. All golem projects have `publish / skip := true` by default, so you must override it.

### Step 1: Publish Dependencies + Golem Libraries (Scala 3.8.2)

```bash
cd sdks/scala
sbt '++3.8.2; set ThisBuild / version := "0.0.0-SNAPSHOT"; set ThisBuild / packageDoc / publishArtifact := false; set every (publish / skip) := false; golemScalaModelJVM/publishLocal; golemScalaModelJS/publishLocal; golemScalaMacros/publishLocal; golemScalaCoreJS/publishLocal'
```

### Step 2: Publish SBT Plugin (Scala 2.12.21)

```bash
cd sdks/scala
sbt '++2.12.21!; set ThisBuild / version := "0.0.0-SNAPSHOT"; set ThisBuild / packageDoc / publishArtifact := false; set every (publish / skip) := false; golemScalaSbt/publishLocal'
```

> **Note**: The `golemPublishLocal` alias exists in `build.sbt` but may need `set every (publish / skip) := false` prepended to work correctly. The explicit commands above are the most reliable approach.

## Building the Example Project

The `example` project at `sdks/scala/example/` is a standalone sbt project (its own `build.sbt`, `project/plugins.sbt`). It depends on the SDK at `0.0.0-SNAPSHOT`.

### Prerequisites
1. Publish the SDK locally (both steps above).

### Clean Build
```bash
cd sdks/scala/example
rm -rf target project/target .bsp .generated .golem
sbt -batch -no-colors -Dsbt.supershell=false compile
```

### Key SBT Tasks
- `sbt golemPrepare` — Generates `.generated/agent_guest.wasm` (extracted from plugin resources) and `.generated/scala-js-template.yaml` (component manifest template).
- `sbt compile` — Compiles the Scala agent code.
- `sbt fastLinkJS` — Links the Scala.js bundle (produces the JS that QuickJS will run).

### Project Structure
- `build.sbt` — Enables `ScalaJSPlugin` + `GolemPlugin`, sets `scalaJSUseMainModuleInitializer := false`, ESModule output.
- `project/plugins.sbt` — Adds `golem-scala-sbt` and `sbt-scalajs`.
- `golem.yaml` — Declares app name, includes `.generated/scala-js-template.yaml`, defines component `scala:demo`.
- `repl-counter.rib` — Rib script for end-to-end testing via `golem-cli repl`.

## End-to-End Testing

### Start the Local Golem Server
```bash
golem-cli server run --clean
```
This starts the all-in-one Golem server on `localhost:9881`.

### Using run.sh
```bash
cd sdks/scala/example
bash run.sh
```

The script does:
1. `sbt golemPrepare` — Generate wasm + manifest template
2. `golem-cli build --yes` — Build the WASM component (links QuickJS runtime + Scala.js bundle)
3. `golem-cli deploy --yes` — Deploy to local Golem server
4. `golem-cli repl scala:demo --script-file repl-counter.rib` — Run the demo

### Manual Steps
```bash
cd sdks/scala/example
sbt golemPrepare
golem-cli build --yes
golem-cli deploy --yes --local
golem-cli repl scala:demo --script-file repl-counter.rib --local
```

## Regenerating agent_guest.wasm

The agent_guest.wasm is the QuickJS-based WASM runtime that wraps the Scala.js bundle. Regenerate it when WIT definitions change.

### Script
```bash
cd sdks/scala
./scripts/generate-agent-guest-wasm.sh
```

### What It Does
1. Stages WIT package from `sdks/scala/wit/` (skipping the `all/` dep directory).
2. Generates TypeScript d.ts definitions via `wasm-rquickjs generate-dts` → saved to `sdks/scala/wit/dts/`.
3. Generates QuickJS wrapper crate via `wasm-rquickjs generate-wrapper-crate`.
4. Builds with `cargo component build --release`.
5. Installs the wasm into `sdks/scala/sbt/src/main/resources/golem/wasm/agent_guest.wasm` and `sdks/scala/mill/resources/golem/wasm/agent_guest.wasm`.
6. Copies d.ts files to `sdks/scala/wit/dts/`.

### Prerequisites
Before running the script, sync WIT dependencies from the repo root:
```bash
cargo make wit
```

### Requirements
- `wasm-rquickjs` v0.1.0 (`cargo install wasm-rquickjs-cli@0.1.0`)
- Rust toolchain + `cargo-component` (`cargo install cargo-component`)

## WIT Management

### Files
- **Primary**: `sdks/scala/wit/main.wit` — The `golem:agent-guest` package definition.
- **Dependencies**: `sdks/scala/wit/deps/` — Copied from `wit/deps/` in the Golem repo root.
- **TypeScript reference**: `sdks/scala/wit/dts/` — Generated d.ts files showing exact JS types expected by the wasm runtime. `exports.d.ts` is the source of truth for what the JS module must export.

### Updating WIT Dependencies
WIT dependencies are managed the same way as the Rust and TypeScript SDKs — via `cargo make wit` from the repository root:
```bash
cargo make wit
```
This copies all WIT packages from `wit/deps/` into `sdks/scala/wit/deps/`. The results are committed to the repository.

### TypeScript SDK Reference
The TypeScript SDK at `sdks/ts/wit/` is the reference for correct WIT definitions when in doubt.

## Common Errors and Solutions

| Error | Cause | Solution |
|-------|-------|----------|
| `Function discover-agent-types not found in interface golem:agent/guest@1.5.0` | Stale `agent_guest.wasm` built from old WIT | Regenerate wasm with `generate-agent-guest-wasm.sh` |
| `Cannot find exported JS function guest.discoverAgentTypes` | Scala.js Guest object doesn't match WIT signature | Update `Guest.scala` to export all 4 functions with correct v1.5.0 signatures (including `principal` param) |
| `YAML deserialization error` in `golem.yaml` about `BuildCommand` | Old GolemPlugin manifest format | Update `GolemPlugin.scala` to use v1.5.0 format (`componentWasm`/`outputWasm`) |
| `Provided exports: (empty)` after deploy | QuickJS fails to evaluate the JS module silently | JS crashes during initialization — check for ESM strict-mode issues, bundle size limits, or import path mismatches |
| `publish / skip` preventing local publish | Default setting in `build.sbt` | Use `set every (publish / skip) := false` in the sbt command |
| Wrong Scala 2.12 version for plugin | Alias or cached sbt version uses wrong 2.12.x | Use the explicit `++2.12.21!` command to force the correct version |
