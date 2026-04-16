---
name: golem-add-secret-scala
description: "Adding secrets to Scala Golem agents. Use when the user asks to add secret values, API keys, passwords, or sensitive configuration to a Scala Golem agent."
---

# Adding Secrets to a Scala Golem Agent

## Overview

**Secrets** are sensitive configuration values (API keys, passwords, tokens) stored per-environment and accessed via `Secret[T]` from `golem.config`. They are declared inside config case classes alongside regular config fields.

## Declaring Secrets

Wrap sensitive fields with `Secret[T]` in your config case class. `Secret[T]` has an implicit `Schema` derivation, so `Schema.derived` works automatically on parent case classes:

```scala
import golem.config.Secret
import zio.blocks.schema.Schema

final case class DbConfig(
  host: String,
  port: Int,
  password: Secret[String],
)

object DbConfig {
  implicit val schema: Schema[DbConfig] = Schema.derived
}

final case class MyAppConfig(
  appName: String,
  apiKey: Secret[String],
  db: DbConfig,
)

object MyAppConfig {
  implicit val schema: Schema[MyAppConfig] = Schema.derived
}
```

## Reading Secrets

`Secret[T]` is lazy — call `.get` to fetch the current value:

```scala
import golem.runtime.annotations.agentImplementation
import golem.config.Config

import scala.concurrent.Future

@agentImplementation()
final class MyAgentImpl(input: String, config: Config[MyAppConfig]) extends MyAgent {
  override def connect(): Future[String] = {
    val cfg = config.value
    val key = cfg.apiKey.get
    val pwd = cfg.db.password.get
    Future.successful(s"Connected to ${cfg.db.host}:${cfg.db.port}")
  }
}
```

## Managing Secrets via CLI

Secret paths use camelCase, matching Scala field names:

```shell
golem agent-secret create apiKey --secret-type String --secret-value "sk-abc123"
golem agent-secret create db.password --secret-type String --secret-value "s3cret"
golem agent-secret list
golem agent-secret update-value apiKey --secret-value "new-value"
golem agent-secret delete apiKey
```

> **Note:** For `update-value` and `delete`, you can also use `--id <uuid>` instead of the positional path.

## Secret Defaults in golem.yaml

Use `secretDefaults` for local development only — manage production secrets via CLI:

```yaml
secretDefaults:
  local:
    - path: [apiKey]
      value: "dev-key-123"
    - path: [db, password]
      value: "dev-password"
```

## Key Constraints

- `Secret[T]` is lazy — call `.get` to retrieve the actual value
- Secret values are stored per-environment, not per-agent-instance
- The `Secret[T]` companion provides an implicit `Schema` so `Schema.derived` works on parent case classes
- Missing required secrets cause agent creation to fail
- Secret paths use camelCase (matching Scala field names)
- The `--secret-type` argument accepts Scala type names: `String`, `Int`, `Boolean`, `List[String]`, `Option[Int]` (JSON-encoded analysed types like `'{"type":"Str"}'` are also supported as a fallback)
- Use `secretDefaults` in `golem.yaml` only for development; manage production secrets via CLI
- If the agent also needs non-secret typed config guidance, use `golem-add-config-scala` alongside this skill
