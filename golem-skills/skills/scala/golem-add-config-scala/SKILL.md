---
name: golem-add-config-scala
description: "Adding typed configuration to Scala Golem agents. Use when the user asks to add config, configuration, settings, or environment-specific parameters to an agent."
---

# Adding Typed Configuration to a Scala Agent

## Overview

Scala agents receive typed configuration via `Config[T]` from `golem.config`. The agent trait declares its config type by mixing in the `AgentConfig[T]` trait. The implementation class receives a `Config[T]` constructor parameter and accesses the resolved value lazily via `.value`.

## Steps

1. **Define config case classes** with `Schema.derived` in companion objects
2. **Mix in `AgentConfig[T]`** on the agent trait
3. **Accept `Config[T]`** in the implementation constructor
4. **Access config** via `config.value`

## Config Types

Define case classes for your configuration. Each needs an `implicit val schema` in its companion object:

```scala
import zio.blocks.schema.Schema

final case class DbConfig(
  host: String,
  port: Int,
)

object DbConfig {
  implicit val schema: Schema[DbConfig] = Schema.derived
}

final case class MyAppConfig(
  appName: String,
  db: DbConfig,
)

object MyAppConfig {
  implicit val schema: Schema[MyAppConfig] = Schema.derived
}
```

## Agent Definition

Mix in `AgentConfig[T]` alongside `BaseAgent`:

```scala
import golem.BaseAgent
import golem.config.AgentConfig
import golem.runtime.annotations.{agentDefinition, description}

@agentDefinition()
trait MyAgent extends BaseAgent with AgentConfig[MyAppConfig] {
  class Id(val value: String)

  def greet(): Future[String]
}
```

## Agent Implementation

Accept `Config[T]` as a constructor parameter and access the value lazily:

```scala
import golem.config.Config
import golem.runtime.annotations.agentImplementation

@agentImplementation()
final class MyAgentImpl(input: String, config: Config[MyAppConfig]) extends MyAgent {
  override def greet(): Future[String] = {
    val cfg = config.value
    Future.successful(s"Hello from ${cfg.appName}! DB at ${cfg.db.host}:${cfg.db.port}")
  }
}
```

## Providing Config Values

**golem.yaml** — set defaults for agents:

```yaml
agents:
  MyAgent:
    config:
      appName: "My Application"
      db:
        host: "localhost"
        port: 5432
```

**CLI** — override at agent creation:

```shell
golem agent new my-ns:my-component/my-agent-1 \
  --config appName="My App" \
  --config db.host=localhost \
  --config db.port=5432
```

**RPC** — override via the generated helper:

```scala
val client = MyAgentClient.getWithConfig(
  "agent-1",
  appName = Some("OverriddenApp"),
  dbHost = Some("new-host"),
  dbPort = Some(9999)
)
```

## Key Constraints

- Config case classes need a companion object with `implicit val schema: Schema[T] = Schema.derived`
- The agent trait mixes in `AgentConfig[T]` to declare its config type
- The implementation class receives `Config[T]` as a constructor parameter
- Config is loaded lazily when `.value` is accessed
- Config values in `golem.yaml` use camelCase keys matching Scala field names
- Config cascade order: `componentTemplates` → `components` → `agents` → `presets`
