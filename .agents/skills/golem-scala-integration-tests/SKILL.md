---
name: golem-scala-integration-tests
description: "Runs and debugs the Scala SDK's self-hosted Golem integration suite. Use for generated test agents, REPL samples, HTTP routes, host provisioning, or GolemExamplesIntegrationSpec."
---

# Golem Scala Integration Tests

The suite is `sdks/scala/integration-tests/src/test/scala/golem/integration/GolemExamplesIntegrationSpec.scala`; its application is `sdks/scala/test-agents/`.

## Repository test-policy conflict

This existing sbt suite directly launches the `golem` executable. That is a
known source-level conflict with the repository rule that subprocess-based
tests belong in the CLI integration test suite. Do not add coverage that
extends this pattern: place new subprocess coverage in CLI integration tests,
and keep non-CLI Scala tests process-free. The commands below document how to
verify the existing suite until that coverage is migrated.

## Prerequisites

- Build the repository's `golem` binary and put its directory on `PATH`. The suite executes `golem`, not `golem-cli` and not a fixed binary under the home directory.
- Build the TypeScript SDK packages and set `GOLEM_TS_PACKAGES_PATH` to their packages directory.
- Keep local port 9881 free; the suite owns the server lifecycle. HTTP fixtures also use localhost:9006.
- Generate the Scala guest runtimes. The test manifest consumes `.generated/agent_guest.wasm`; current CI runs the generator first.

For a complete local Scala publication from repository root, use `cargo make build-sdk-scala`. For the exact CI setup, follow the `scala-sdk-integration-tests` job in `.github/workflows/ci.yaml`.

## Run

Use non-client sbt so `build.sbt` can forward `GOLEM_TS_PACKAGES_PATH` into the forked test JVM:

```bash
cd sdks/scala
GOLEM_TS_PACKAGES_PATH="$PWD/../ts/packages" \
  sbt -batch "++3.8.2; integrationTests/test"
```

Focused ZIO tests use the suite's current test names/tags, for example:

```bash
GOLEM_TS_PACKAGES_PATH=<packages> \
  sbt -batch '++3.8.2; integrationTests/testOnly -- -t snapshot-counter'
```

If client mode is unavoidable, explicitly pass `-Dgolem.tsPackagesPath=...` in `integrationTests / Test / javaOptions`; ordinary environment changes are not reliably propagated by a running sbt client.

## Lifecycle

`GolemServer.layer` currently:

1. checks `golem --version`, the TS package path, and port 9881;
2. locates `test-agents/golem.yaml` from supported repository/checkout layouts;
3. safely removes stale `test-agents/golem-temp` content, including symlinks;
4. starts `golem -vvv server run --clean --disable-app-manifest-discovery` and writes `sdks/scala/target/scala-integration/golem-server.log`;
5. invokes local manifest-aware `golem deploy` with one retry;
6. provisions retry policies and secret values used by fixtures;
7. runs all tests sequentially and kills the server process tree on release.

Commands use `--yes --local --app-manifest-path <test-agents/golem.yaml>`. The suite forwards `GOLEM_TS_PACKAGES_PATH` and, for build/server commands, `RUST_BACKTRACE=1`.

Do not start a second server, pre-deploy manually, or kill unrelated system processes as routine setup.

## Current application contract

`test-agents/golem.yaml` is a manifest v1.6 application named `scala-examples`, with component ID `scala:examples`. Its build command selects sbt project `testAgents`, writes `.golem/scala.js`, injects that module into `.generated/agent_guest.wasm`, and preinitializes the resulting component. REPL samples are TypeScript files under `test-agents/samples/`; HTTP tests exercise manifest deployments on port 9006.

Avoid copying a full sample or agent inventory into this skill. The spec's `samples` table and manifest-coverage test are authoritative, and some tests are intentionally ignored unless their external dependency is enabled.

## Debugging

- Read `target/scala-integration/golem-server.log` first when startup or deploy fails.
- The test process prints each `golem` command's output; inspect the first failing build/deploy/provision command.
- If the CLI reports an up-to-date build after relevant SDK changes, remove only the affected generated build state (typically `test-agents/.golem`) and regenerate/redeploy through the suite.
- Let the suite's symlink-aware cleanup remove `golem-temp`; do not replace it with unsafe recursive deletion in test code.
- Verify `GOLEM_TS_PACKAGES_PATH` points at built packages when REPL imports fail.

## Maintaining existing coverage

Do not add new subprocess-based coverage to this suite. Put new generated-application, REPL, and
CLI lifecycle coverage in the CLI integration test suite. When a contract change requires an
existing Scala sample or assertion to change, keep `test-agents/src/main/scala/`,
`test-agents/samples/`, and `test-agents/golem.yaml` aligned with the affected existing test; the
manifest-coverage test detects unregistered sample scripts.

Whenever an existing test changes, run that focused test. Use the full existing suite only for
shared lifecycle, manifest, generated-runtime, or cross-sample changes.
