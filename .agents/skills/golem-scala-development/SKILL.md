---
name: golem-scala-development
description: "Compiles, tests, and publishes the Golem Scala SDK and its sbt/Mill plugins. Use for changes under sdks/scala, including public APIs, generated consumers, local publication, or guest-runtime integration."
---

# Golem Scala SDK Development

Work from `sdks/scala/` unless a command says repository root. The Scala SDK is independent of the root Rust build: `cargo make build` does not build it.

## Authoritative build matrix

- JDK 17+, sbt 1.12.x (`project/build.properties` is authoritative)
- Scala **3.8.2** for SDK modules and the Mill plugin build
- Scala **2.12.21** only for sbt 1.x plugin loading and its shared codegen dependency
- Scala **3.3.7** in the current Mill consumer fixtures; this is fixture configuration, not a general Scala.js limit
- Mill **1.1.8** and Scala.js **1.22.0** in the current Mill build

Scala 2 applications are not supported. `project/plugins.sbt`, `build.sbt`, and `mill/build.mill` are the version sources of truth. Always select the intended Scala version explicitly, especially with an sbt client that may retain session state.

## Current project IDs

Use sbt project IDs, not artifact names:

- `modelJVM`, `modelJS`, `core`, `macros`, `codegen`
- `sbtPlugin`, `testAgents`, `integrationTests`
- fixtures: `emptyAutoRegisterFixture`, `middlewareGuestLinkFixture`

The published sbt plugin is loaded with `addSbtPlugin("dev.zio" % "zio-golem-sbt" % version)` and enabled as `golem.sbt.GolemPlugin`. The Mill plugin is loaded as `dev.zio::zio-golem-mill` and mixed in via `golem.mill.GolemAutoRegister`. Check the example and Mill README for complete consumer setup.

## Compile and test

Start with affected projects, then compile `testAgents` for public API, macro, codegen, or plugin changes:

```bash
sbt "++3.8.2; core/compile"
sbt "++3.8.2; modelJVM/test; modelJS/test"
sbt "++3.8.2; macros/test; codegen/test"
sbt "++2.12.21!; codegen/test; sbtPlugin/test"
sbt "++3.8.2; testAgents/fastLinkJS"
```

`sbt golemTestAll` covers Scala 3 model/core/macros tests and links `testAgents`; it deliberately excludes codegen, Scala 2.12, sbt-plugin, and integration tests. Add those checks explicitly when affected. Use scoped `scalafmtCheckAll`; run `scalafmtAll` only to fix formatting.

## Local publication

The canonical SDK alias is:

```bash
cd sdks/scala
sbt golemPublishLocal
```

For the repository's complete local prerequisite flow (WIT sync, all three guest runtimes, all cross-published modules, and marker caching), use:

```bash
# Repository root
cargo make build-sdk-scala
```

Delete `sdks/scala/target/.local-publish-marker` before rerunning that cargo-make task when inputs changed. For release/CI-sensitive work, copy the explicit publish matrix from current `Makefile.toml` or `.github/workflows/ci.yaml`; do not preserve an older hand-written sequence in documentation.

## Runtime and WIT changes

The Scala.js core exposes the current schema-based host surface. Useful authorities are:

- `HostApi.scala` for typed agent IDs, registry/metadata, oplog, update/fork/revert, promises, and related `golem:api/host@1.5.0` facades;
- `host/DurabilityApi.scala` for schema-typed synchronous and asynchronous custom durable calls;
- `runtime/rpc/` and `RpcCodegen.scala` for invocation and mode-aware client signatures;
- `wit/main.wit` for three Preview 3 guest worlds and current versioned dependencies.

Durable generated clients return plain values/`Unit` plus cancellation tokens where applicable. Ephemeral invocations return metadata-bearing `InvocationResult`, `InvocationReceipt`, `CancelableAsyncInvocation`, and `CancelableInvocationReceipt` values. Do not describe the old wasm-rpc resource API without checking these files.

After WIT or guest-role changes, sync and regenerate as described by `golem-scala-base-image`. The script uses `wasm-rquickjs` 0.4.3, Preview 3, and `wasm32-wasip2`; generated WASMs are ignored while ordinary-role d.ts files are tracked.

## Example and end-to-end work

`example/` is a standalone consumer. Its README, `build.sbt`, `golem.yaml`, and `run.sh` are authoritative. Publish locally before building it. Use the repository binary name `golem`, not the retired `golem-cli`, and do not assume the example reproduces every test-agent or multi-component scenario.

For platform behavior use the dedicated integration-test skill. Do not run heavyweight Scala or integration builds merely to verify documentation edits.
