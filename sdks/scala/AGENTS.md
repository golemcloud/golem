# Golem Scala SDK

## Overview

This directory contains the Scala SDK for building Golem components using Scala.js:
- `model/` - WIT value types, RPC types, annotations (cross JVM+JS)
- `core/` - Agent framework, Scala.js host facades (JS only)
- `macros/` - Scala 3 macros for agent definition/implementation
- `codegen/` - Build-time code generation (shared between sbt/Mill plugins)
- `sbt/` - sbt plugin (`GolemPlugin`)
- `mill/` - Mill plugin
- `wit/` - WIT definitions and TypeScript d.ts references
- `test-agents/` - Test agent definitions for integration tests
- `integration-tests/` - Integration test suite
- `example/` - Standalone example project
- `docs/` - SDK documentation

## Prerequisites

- JDK 17+
- sbt 1.12+

## Building

```shell
sbt compile                    # Compile all projects
sbt "++3.8.2; core/compile"    # Compile core (Scala 3)
```

## Testing

Tests use ZIO Test framework.

```shell
sbt golemTestAll    # Run all tests
```

## Scala Versions

- **Scala 3.8.2** — All SDK modules (model, core, macros)
- **Scala 3.3.7** — Scala.js builds (Scala.js doesn't support 3.7+)
- **Scala 2.12.21** — Internal sbt 1.x plugin implementation only; Scala 2 applications are not supported

## Code Style

```shell
sbt scalafmtAll           # Format all sources
sbt scalafmtCheckAll      # Check formatting
```

Run before committing.

## Integration with Main Repository

This SDK is part of the main Golem repository but is **not built by `cargo make build`**. It has its own sbt build.

## Testing Local SDK Changes

Publish locally for testing:

```shell
sbt golemPublishLocal   # Publishes all artifacts as 0.0.0-SNAPSHOT
```

Then in your Golem application project, the sbt plugin and SDK deps will resolve from your local Ivy cache.

## WIT Dependencies

WIT files are synced from the parent repository. Do not manually edit files in `wit/deps/`.

To update WIT dependencies, run from the **repository root**:

```shell
cargo make wit
```

To regenerate the agent_guest.wasm (after WIT changes):
```shell
./scripts/generate-agent-guest-wasm.sh
```

See the `golem-scala-base-image` skill for details.
