# zio-golem mill plugin

This directory contains a Mill plugin that mirrors the sbt `GolemPlugin` behavior for Golem Scala.js agent projects.

## Features

| Feature | Description |
|---|---|
| `golemBasePackage` | Base package for `@agentImplementation` auto-registration |
| `golemAgentGuestWasmFile` | Smart detection of where to write `agent_guest.wasm` (searches for `golem.yaml`) |
| `golemPrepare` | Ensures `agent_guest.wasm` exists in `.generated/` |
| `golemBuildComponent` | Builds the `fullLinkJS` bundle for golem-cli (`mill <module>.golemBuildComponent --component <name> --outPath <file>`); fails when the linker emits more than one module |
| `scalaJSModuleInitializers` | Auto-configured for the generated `RegisterAgents` entrypoint |
| Source generation | Scans for `@agentImplementation` classes and generates registration code |

## Usage

```scala
import $ivy.`dev.zio::zio-golem-mill:<VERSION>`
import golem.mill.GolemAutoRegister

object myApp extends GolemAutoRegister {
  def scalaVersion     = "3.3.7"
  def golemBasePackage = Task(Some("myapp"))
}
```

## Status

- See `golem/docs/supported-versions.md` for the intended supported Mill versions.

## Static config

The plugin shares config discovery and creation-override generation with sbt.
See [Static config](../docs/config.md) for declaration, access and default rules.
After `sbt golemPublishLocal`, the `staticConfigFixture` module builds the real
SDK consumer shared with the sbt fixture, rather than the lightweight API stubs.
