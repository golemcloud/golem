# zio-golem mill plugin

This directory contains a Mill plugin that mirrors the sbt `GolemPlugin` behavior for Golem Scala.js agent projects.

## Features

| Feature | Description |
|---|---|
| `golemBasePackage` | Base package for `@agentImplementation` auto-registration |
| `golemAgentGuestWasmFile` | Smart detection of where to write `agent_guest.wasm` (searches for `golem.yaml`) |
| `golemPrepare` | Ensures `agent_guest.wasm` exists in `.generated/` |
| `golemBuildComponent` | Builds the Scala.js bundle for golem-cli (`mill <module>.golemBuildComponent ...`) |
| `scalaJSModuleInitializers` | Auto-configured for the generated `RegisterAgents` entrypoint |
| Source generation | Scans for `@agentImplementation` classes and generates registration code |

## Usage

```scala
import $ivy.`dev.zio::zio-golem-mill:<VERSION>`
import golem.mill.GolemAutoRegister

object myApp extends GolemAutoRegister {
  def scalaJSVersion   = "1.20.0"
  def scalaVersion     = "3.3.7"
  def golemBasePackage = T(Some("myapp"))
}
```

## Status

- See `golem/docs/supported-versions.md` for the intended supported Mill versions.
