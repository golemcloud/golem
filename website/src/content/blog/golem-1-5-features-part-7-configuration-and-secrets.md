---
title: "Golem 1.5 features — Part 7: Configuration and Secrets"
date: "2026-04-18T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-7-configuration-and-secrets"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part7-config-and-secrets/"
---

## Introduction

A series showcasing new Golem 1.5 features (releasing end of April 2026) is underway. This installment focuses on configuration and secrets management. Previous parts cover code-first routes, webhooks, MCP, Node.js compatibility, Scala support, and user-defined snapshotting. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Code-first Configuration

Before version 1.5, the standard way to inject configuration values and secrets was through _environment variables_. The new approach introduces typed configuration that becomes part of an agent's type definition.

### Configuration Implementation

Configuration uses record types that can be nested and are injected into agent constructors:

```typescript
type DbConfig = {
  host: string;
  port: number;
};

type ExampleConfig = {
  debugLogs: boolean;
  alias?: string;
  database: DbConfig;
};

@agent()
class ExampleAgent extends BaseAgent {
  constructor(
    exampleParam: string,
    readonly config: Config<ExampleConfig>
  ) {
    // ...
  }

  useConfig() {
    const config = this.config.value;
    if (config.debugLogs) {
      console.debug("Debug logs enabled");
    }
  }
}
```

```rust
#[derive(ConfigSchema)]
pub struct DbConfig {
    host: String,
    port: u16
}

#[derive(ConfigSchema)]
pub struct ExampleConfig {
    debug_logs: bool,
    alias: Option<String>,
    database: DbConfig
}

#[agent_definition]
pub trait ExampleAgent {
    fn new(name: String, #[agent_config] config: Config<ExampleConfig>) -> Self;
    fn use_config(&self);
}

struct ExampleAgentImpl {
    config: Config<ExampleConfig>
}

#[agent_implementation]
impl ExampleAgent for ExampleAgentImpl {
    fn new(example_param: String, #[agent_config] config: Config<ExampleConfig>) -> Self {
        Self { config }
    }

    fn use_config(&self) {
        let config = self.config.get();
        if config.debug_logs {
            logging::log(logging::Level::Debug, "example", "Debug logs enabled");
        }
    }
}
```

```scala
final case class DbConfig(
  host: String,
  port: Int
)

object DbConfig {
  implicit val schema: Schema[DbConfig] = Schema.derived
}

final case class ExampleConfig(
  debugLogs: Boolean,
  alias: Option[String],
  database: DbConfig
)

object ExampleConfig {
  implicit val schema: Schema[ExampleConfig] = Schema.derived
}


@agentDefinition()
trait ExampleAgent extends BaseAgent with AgentConfig[ExampleConfig] {
  class Id(val exampleParam: String)

  def useConfig(): Future[Unit]
}

@agentImplementation()
final case class ExampleAgentImpl(exampleParam: String, config: Config[ExampleConfig])
  extends ExampleAgent {

  override def useConfig(): Future[Unit] = {
    val config = config.value
    if (config.debugLogs) {
      js.Dynamic.global.console.debug("Debug logs enabled");
    }
  }
}
```

```moonbit
#derive.config
pub(all) struct DbConfig {
  host : String
  port : UInt
}

#derive.config
pub(all) struct ExampleConfig {
  debug_logs : Bool
  alias : String?
  database : DbConfig
}

#derive.agent
pub(all) struct ExampleAgent {
  example_param : String
  config : @config.Config[ExampleConfig]
}

fn ExampleAgent::new(
  example_param : String,
  config : @config.Config[ExampleConfig]
) -> ExampleAgent {
  { example_param, config }
}

pub fn ExampleAgent::use_config(self : Self) -> Unit {
  let config = self.config.value
  if config.debug_logs {
    @log.debug("Debug logs enabled")
  }
}
```

Once configuration requirements are defined in code, agents cannot be deployed without satisfying them. Values are assigned per agent in the application manifest:

```yaml
agents:
  ExampleAgent:
    config:
      debugLogs: true
      alias: "main"
      database:
        host: "localhost"
        port: 5432
```

The manifest's `preset` feature enables reusable configuration bits applicable to multiple agents or all agents within a component.

## Secrets

Secrets represent special configuration that can be updated dynamically (useful for API token rotation). Unlike regular configuration tied to deployments, secrets are accessed via `get()` to retrieve the latest value:

```typescript
type DbConfig = {
  host: string;
  port: number;
  password: Secret<string>;
};
```

```rust
#[derive(ConfigSchema)]
pub struct DbConfig {
    host: String,
    port: u16,
    #[config_schema(secret)]
    password: Secret<String>,
}
```

```scala
final case class DbConfig(
  host: String,
  port: Int,
  password: Secret[String]
)
```

```moonbit
#derive.config
pub(all) struct DbConfig {
  host : String
  port : UInt
  password : @config.Secret[String]
}
```

Secret values are stored per environment, not per deployment. Initial values can be set via the manifest's `secretDefaults` section with environment variable substitution:

```yaml
secretDefaults:
  local:
    - path: [db, password]
      value: "{{ DB_PASSWORD }}"
```

CLI commands manage secrets:

```bash
golem agent-secret create db.password --secret-type string --secret-value "pwd"
golem agent-secret list
golem agent-secret update-value db.password --secret-value "new-pwd"
golem agent-secret delete db.password
```

To access current secret values:

```typescript
const password = config.database.password.get();
```

```rust
let password = config.database.password.get();
```

```scala
val password = config.database.password.get
```

```moonbit
let password = config.database.password.get!()
```
