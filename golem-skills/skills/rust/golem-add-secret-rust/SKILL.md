---
name: golem-add-secret-rust
description: "Adding secrets to Rust Golem agents. Use when the user needs to store sensitive configuration such as API keys, passwords, or tokens that should not be checked into source control."
---

# Adding Secrets to a Rust Agent

## Overview

Secrets are sensitive configuration values (API keys, passwords, tokens) stored per-environment and accessed at runtime via `Secret<T>` from `golem_rust::agentic::Secret`. Unlike regular config fields, the config value carries an opaque secret handle and plaintext is revealed only when agent code calls `.get()`. Secrets are **not** stored in `golem.yaml` (which is checked into source control). They are managed via the CLI or through `secretDefaults` for local development.

## Declaring Secrets in a Config Struct

Use `#[config_schema(secret)]` on fields of type `Secret<T>`:

```rust
use golem_rust::ConfigSchema;
use golem_rust::agentic::Secret;

#[derive(ConfigSchema)]
pub struct MyAgentConfig {
    pub name: String,
    #[config_schema(secret)]
    pub api_key: Secret<String>,
    #[config_schema(nested)]
    pub db: DbConfig,
}

#[derive(ConfigSchema)]
pub struct DbConfig {
    pub host: String,
    pub port: i32,
    #[config_schema(secret)]
    pub password: Secret<String>,
}
```

## Wiring Secrets Into The Agent Constructor

Secrets use the same typed config mechanism as regular agent config. Receive the
config via `Config<T>` in the constructor and annotate that parameter with
`#[agent_config]`:

```rust
use golem_rust::{ConfigSchema, agent_definition, agent_implementation};
use golem_rust::agentic::{Config, Secret};

#[derive(ConfigSchema)]
pub struct MyAgentConfig {
    #[config_schema(secret)]
    pub api_key: Secret<String>,
}

#[agent_definition]
pub trait MyAgent {
    fn new(name: String, #[agent_config] config: Config<MyAgentConfig>) -> Self;
    async fn connect(&self) -> Result<String, String>;
}

struct MyAgentImpl {
    config: Config<MyAgentConfig>,
}

#[agent_implementation]
impl MyAgent for MyAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<MyAgentConfig>) -> Self {
        Self { config }
    }

    async fn connect(&self) -> Result<String, String> {
        use golem_rust::http::{self, Request};

        let config = self.config.get().map_err(|_| "Configuration unavailable".to_string())?;
        let api_key = config.api_key.get().map_err(|_| "Credential unavailable".to_string())?;
        let request = Request::get("https://api.example.com/status")
            .header("Authorization", format!("Bearer {api_key}"))
            .body(Vec::new())
            .map_err(|_| "Invalid request".to_string())?;
        let response = http::send(request).await.map_err(|_| "Request failed".to_string())?;
        let status = response.status();
        // Consume the body, but do not return provider data that might contain secrets.
        let _ = http::response_bytes(response).await.map_err(|_| "Body read failed".to_string())?;
        Ok(format!("Service returned HTTP {status}"))
    }
}
```

- The `#[agent_config]` annotation is required on the `Config<T>` constructor parameter.
- Do not construct `Config<T>` with `Config::new()` in user code. That bypasses the
  constructor-based metadata path used to embed config and secret declarations into the component.

## Reading Secrets at Runtime

Call `.get()` on a `Secret<T>` to explicitly reveal the value. Both config loading and secret
reveal return `Result`; map failures to safe domain errors as above. Secrets are lazily loaded, so
a fresh `.get()` call can observe updated values after the update is visible. For the nested
schema from the first section, acquire a database password for a client without returning it:

```rust
fn database_password(&self) -> Result<String, String> {
    let config = self.config.get().map_err(|_| "Configuration unavailable".to_string())?;
    config.db.password.get().map_err(|_| "Database credential unavailable".to_string())
}
```

Keep this an internal helper, not an agent trait method. Pass its result to the database client;
never expose it in an RPC result, HTTP response, or log.

## Managing Secrets via CLI

Secrets are environment-scoped — each deployment environment has its own set of secret values.

```shell
# Create secrets in the current environment
golem secret create apiKey --type String --value "sk-abc123"
golem secret create db.password --type String --value "s3cret"

# Without a value option the value is prompted for (hidden input); or pipe it in
printenv DB_PASSWORD | golem secret create db.password --type String --value-stdin

# Create, or update the value if the secret already exists
golem secret create apiKey --type String --value "sk-abc123" --update-existing

# List all secrets
golem secret list

# Update a secret value
golem secret update apiKey --value "new-value"

# Remove a secret value
golem secret update apiKey --unset

# Delete a secret (asks for confirmation; --yes skips it)
golem secret delete apiKey
```

> **Note:** For `update` and `delete`, you can also use `--id <uuid>` instead of the positional path.

## Secret Defaults in golem.yaml

For local development convenience, set defaults under `secretDefaults`. Use these only for non-sensitive development values. The CLI applies defaults by environment name; it does not prohibit defaults in production. Never put production credentials in this checked-in manifest:

```yaml
secretDefaults:
  local:
    apiKey: "dev-key-123"
    db:
      password: "dev-password"
```

## Key Points

- Secrets use the same `Config<T>` constructor injection mechanism as regular typed config
- If the agent also needs non-secret typed config guidance, use `golem-add-config-rust` alongside this skill
- Secret paths use **camelCase** — Rust `snake_case` fields are converted automatically (e.g., `api_key` → `apiKey`)
- The `--type` argument accepts Rust type names: `String`, `i32`, `bool`, `Vec<String>`, `Option<i32>` (if parsing as Rust fails, the other supported languages' type syntax is tried)
- Secrets are stored **per-environment**, not per-agent-instance
- A declaration with a missing required secret can deploy, but agent creation or start fails — use `Option<Secret<T>>` for optional secrets
- Secrets are revealed on `.get()`, and each reveal pins the resolved secret revision for deterministic retries and replay
- Fresh `.get()` calls can observe runtime updates without restarting the agent
- If you also need non-secret config fields, follow the same `#[agent_config]` constructor pattern
