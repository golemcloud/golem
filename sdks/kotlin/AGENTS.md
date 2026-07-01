# Golem Kotlin SDK

## Overview

This directory contains the Kotlin SDK for building Golem agents with Kotlin/JS. It reaches
Wasm the same way the Scala SDK does — Kotlin/JS (IR) → rollup → inject into a prebuilt QuickJS
guest — and mirrors `sdks/scala/` structurally:

- `sdk/` — agent runtime, `@Agent`/`@Endpoint`/`@Prompt`/`@Description` annotations, `BaseAgent`,
  DataValue helpers, host bindings (Kotlin Multiplatform, JS target). The published library.
- `ksp/` — KSP processor: reads `@Agent` classes and generates the registration glue + a WIT
  metadata file (the sbt/Mill-macro analogue). Published.
- `gradle-plugin/` — the `cloud.golem.wasm-component` Gradle plugin (the sbt/Mill-plugin
  analogue): bundles the agent JS and ships the prebuilt `agent_guest.wasm` as a resource.
- `wit/` — WIT definitions (synced via `cargo make wit` from the repo root).
- `scripts/generate-agent-guest-wasm.sh` — regenerates `.generated/agent_guest.wasm`.
- `kotlin-template.yaml` — a standalone `golem build` component template that `example/` pulls in
  via `includes:`. The `golem new --template kotlin` flow does NOT use this file; its project
  template lives embedded in golem-cli at `cli/golem-cli/templates/kotlin/` (the `common-on-demand`
  manifest there defines the equivalent `componentTemplates: kotlin` build steps).
- `example/` — standalone counter example, equivalent to what `golem new --template kotlin` scaffolds.
- `test-agents/`, `integration-tests/` — test agents + integration suite.

## Packaging model (load-bearing)

The agent project depends on `cloud.golem:golem-kotlin-sdk` as a NORMAL Kotlin/JS library; the
SDK is **bundled into the agent's own JS bundle** (it is not baked into the wasm). The
`agent_guest.wasm` is generic — QuickJS with only a `user=@slot` — so it is SDK-version-
independent and only regenerated when the Golem WIT surface changes. The injected agent bundle
IS the export module the host calls `guest` on. This mirrors Scala (`scala.js` bundles the SDK).

## Prerequisites

- JDK 17+. The SDK modules here build with the Gradle wrapper (8.11). Projects scaffolded by
  `golem new --template kotlin` instead require a **system `gradle`** (8.11+) on `PATH` — no
  wrapper is shipped in the project template (a binary jar can't pass through `golem new`'s
  text-based file generation), mirroring how the Scala template relies on a system `sbt`.
- Node.js + `rollup` (global) for bundling
- For regenerating the guest wasm: `wasm-rquickjs` (crate `wasm-rquickjs-cli`), Rust +
  `wasm32-wasip2`, `cargo-component`, `wasm-tools`

## Building / publishing (local)

This SDK is **not built by `cargo make build`** — it has its own Gradle build. For local dev /
CI, publish the artifacts to mavenLocal at `0.0.0-SNAPSHOT` (the Scala `golemPublishLocal`
analogue) so the example/scaffold resolves them:

```shell
(cd sdk && ./gradlew publishToMavenLocal)
(cd ksp && ./gradlew publishToMavenLocal)
(cd gradle-plugin && ./gradlew publishToMavenLocal)
```

## Testing

```shell
(cd sdk && ./gradlew jsTest)     # runtime unit tests
(cd ksp && ./gradlew test)       # KSP processor tests (kctfork)
```

## Regenerating the guest runtime

When the Golem WIT surface changes:

```shell
cargo make wit                              # from repo root, sync wit/deps
cd sdks/kotlin && ./scripts/generate-agent-guest-wasm.sh
(cd gradle-plugin && ./gradlew publishToMavenLocal)   # republish with the new embedded wasm
```

## End-to-end

Using the bundled `example/` (its manifest deploys on domain `localhost`):

```shell
golem server run --data-dir "$(mktemp -d)"
cd example && golem build && golem deploy --yes
golem agent invoke 'CounterAgent("c1")' increment      # -> 1, 2, 3
curl -s -X POST http://localhost:9006/counters/c1/increment -H 'Host: localhost'
```

`golem new --template kotlin --component-name example:counter --yes app` scaffolds an equivalent
project from the embedded template (it deploys on domain `app.localhost:9006`, so use
`-H 'Host: app.localhost:9006'` for its HTTP routes). Building it requires a system `gradle`.

## Versions

- Kotlin 2.4.0 (JS IR; `js { outputModuleName.set(...) }` — the 2.4.0 DSL)
- KSP2 `symbol-processing-api` / Gradle plugin `2.3.9`
- Maven coordinates: group `cloud.golem`, `golem-kotlin-sdk` / `golem-kotlin-ksp`, plugin id
  `cloud.golem.wasm-component`

## Code style

Follow `.editorconfig` / ktlint conventions; format before committing.
