---
name: golem-add-config-rust
description: "Adding typed configuration to a Rust Golem agent. Use when the user asks about agent config, Config<T>, ConfigSchema, #[agent_config], or passing configuration values to agents."
---

# Adding Typed Configuration to an Agent (Rust)

Golem agents can receive typed configuration via `Config<T>` from `golem_rust::agentic::Config`. Configuration values are validated against a schema derived from your Rust struct.

## 1. Define a Config Struct

Derive `ConfigSchema` on a struct whose fields become config keys:

```rust
use golem_rust::ConfigSchema;

#[derive(ConfigSchema)]
pub struct MyAgentConfig {
    pub foo: i32,
    pub bar: String,
    #[config_schema(nested)]
    pub nested: NestedConfig,
}

#[derive(ConfigSchema)]
pub struct NestedConfig {
    pub a: bool,
    pub b: Vec<i32>,
}
```

- All fields must implement the `ConfigSchema` trait.
- Nested structs require the `#[config_schema(nested)]` annotation.
- `Option<T>` fields are optional and default to `None` if not provided.

## 2. Add `Config<T>` to the Agent Constructor

Annotate the config parameter with `#[agent_config]`:

```rust
use golem_rust::{agent_definition, agent_implementation};
use golem_rust::agentic::Config;

#[agent_definition]
pub trait MyAgent {
    fn new(name: String, #[agent_config] config: Config<MyAgentConfig>) -> Self;
    fn get_foo(&self) -> i32;
}

struct MyAgentImpl {
    config: Config<MyAgentConfig>,
}

#[agent_implementation]
impl MyAgent for MyAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<MyAgentConfig>) -> Self {
        Self { config }
    }

    fn get_foo(&self) -> i32 {
        self.config.get().foo
    }
}
```

- The `#[agent_config]` annotation is **required** on the `Config<T>` parameter.
- Do not call `Config::new()` yourself in user code. `Config<T>` metadata is discovered from the `#[agent_config]` constructor parameter, and manual construction bypasses that registration path.
- Config is loaded lazily when `.get()` is called.

## 3. Set Config in `golem.yaml`

Provide default values under `agents.<Name>.config` (or `components.<Name>.config`):

```yaml
agents:
  MyAgent:
    config:
      foo: 42
      bar: "hello"
      nested:
        a: true
        b: [1, 2, 3]
```

Config values in `golem.yaml` use **camelCase** keys — Rust snake_case fields are converted automatically.

## 4. Pass Config via CLI

Override or supply config when creating an agent instance:

```shell
golem agent new my-ns:my-component/my-agent-1 \
  --config foo=42 \
  --config bar=hello \
  --config nested.a=true \
  --config nested.b="[1, 2, 3]"
```

Dot-separated keys address nested struct fields.

## 5. RPC Config Overrides

When calling an agent via RPC, use the generated `*ConfigRpc` type and `get_with_config` to override config at call time:

```rust
let agent = MyAgentConfigRpc::get_with_config(name, config_overrides);
```

## Config Cascade

Config values follow the same cascade as environment variables:

**componentTemplates → components → agents → presets**

- Values in `golem.yaml` act as defaults.
- Values passed via `golem agent new --config` or RPC `get_with_config` override those defaults.
- If the config includes `Secret<T>` fields, also use `golem-add-secret-rust` for secret-specific declaration and CLI guidance.
