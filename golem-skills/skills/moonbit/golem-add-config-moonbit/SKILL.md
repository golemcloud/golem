---
name: golem-add-config-moonbit
description: "Adding typed configuration to a MoonBit Golem agent. Use when the user asks to add configuration, settings, or typed config parameters to a MoonBit agent."
---

# Adding Typed Configuration to an Agent (MoonBit)

The MoonBit SDK has full support for **code-first typed configuration** via `#derive.config`, `@config.Config[T]`, and `@config.Secret[T]`, equivalent to the Rust SDK's `ConfigSchema` / `#[agent_config]`.

## 1. Define Config Types with `#derive.config`

Annotate record structs with `#derive.config`. This auto-generates `ConfigField` trait implementations (via `golem_sdk_tools`) for schema collection and typed loading. Config types can be nested but must not be generic.

```moonbit
#derive.config
pub(all) struct DatabaseConfig {
  host : String
  port : UInt
  timeout : UInt64
}

#derive.config
pub(all) struct AppConfig {
  app_name : String
  debug : Bool
  database : DatabaseConfig  // nested config type
}
```

### Supported field types

All primitive types that implement `ConfigField`: `String`, `Bool`, `Int`, `UInt`, `Int64`, `UInt64`, `Float`, `Double`, `Byte`, `Char`, `Bytes`, plus `T?` (optional), `Array[T]`, `Result[T, E]`, and nested `#derive.config` structs.

## 2. Inject Config into the Agent Constructor

Add a `@config.Config[T]` parameter to the agent's `new` function. The platform loads and validates all config values at agent construction time:

```moonbit
#derive.agent
pub(all) struct MyAgent {
  config : @config.Config[AppConfig]
}

fn MyAgent::new(config : @config.Config[AppConfig]) -> MyAgent {
  { config }
}
```

Access config values through `self.config.value`:

```moonbit
pub fn MyAgent::do_work(self : Self) -> Unit {
  let cfg = self.config.value
  if cfg.debug {
    @log.debug("Connected to \{cfg.database.host}:\{cfg.database.port}")
  }
}
```

## 3. Add Secrets with `@config.Secret[T]`

Wrap sensitive fields in `@config.Secret[T]`. Secrets are stored per-environment and fetched dynamically (not snapshot-cached like regular config):

```moonbit
#derive.config
pub(all) struct DatabaseConfig {
  host : String
  port : UInt
  password : @config.Secret[String]  // secret field
}
```

Access the current secret value with `get!()`:

```moonbit
pub fn MyAgent::connect(self : Self) -> Unit {
  let db = self.config.value.database
  let password = db.password.get!()
  // use password...
}
```

## 4. Provide Config Values

### In `golem.yaml` (application manifest)

Config values are typed — they match the schema defined in code:

```yaml
agents:
  MyAgent:
    config:
      app_name: "my-app"
      debug: true
      database:
        host: "localhost"
        port: 5432
        timeout: 30000
```

### Secrets via `secretDefaults` in the manifest

```yaml
secretDefaults:
  local:
    database:
      password: "{{ DB_PASSWORD }}"
```

### Secrets via CLI

```shell
golem agent-secret create database.password --secret-type string --secret-value "pwd"
golem agent-secret update-value database.password --secret-value "new-pwd"
```

## 5. How It Works Under the Hood

1. `#derive.config` is processed by `golem_sdk_tools` (see `config_types.mbt` and `config_emit.mbt`).
2. For each annotated struct, the tool generates `ConfigField` trait implementations with:
   - `collect_entries(path)` — declares the config schema to the platform (field paths and WIT types)
   - `load(path)` — loads typed values from the host at runtime
3. `@config.load_config()` is called during agent construction, which recursively loads all fields.
4. `Secret[T]` fields store only the key path and expected type; the actual value is fetched on each `get!()` call via `@host.get_config_value`.
5. The platform validates that all required config and secrets are provided at deploy time.

## Key Constraints

- `#derive.config` does not support generic (parameterized) structs
- Config types cannot have cycles (validated at build time)
- `#derive.config` types cannot be nested inside `Option`, `Array`, `Result`, or `Tuple` containers — only direct nesting of one config struct inside another is allowed
- `Secret` cannot wrap a `#derive.config` type — only primitive/leaf types
- Config values (non-secret) are loaded once at construction; secrets are fetched dynamically on each `get!()` call
