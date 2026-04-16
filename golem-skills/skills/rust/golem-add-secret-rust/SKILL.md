---
name: golem-add-secret-rust
description: "Adding secrets to Rust Golem agents. Use when the user needs to store sensitive configuration such as API keys, passwords, or tokens that should not be checked into source control."
---

# Adding Secrets to a Rust Agent

## Overview

Secrets are sensitive configuration values (API keys, passwords, tokens) stored per-environment and accessed at runtime via `Secret<T>` from `golem_rust::agentic::Secret`. Unlike regular config fields, secrets are **not** stored in `golem.yaml` (which is checked into source control). They are managed via the CLI or through `secretDefaults` for local development.

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

## Reading Secrets at Runtime

Call `.get()` on a `Secret<T>` to retrieve the value. Secrets are lazily loaded, so updated values are picked up on the next `.get()` call:

```rust
fn connect(&self) -> String {
    let config = self.config.get();
    let api_key = config.api_key.get();
    let db_password = config.db.password.get();
    format!("Connecting to {}:{} with key {}", config.db.host, config.db.port, api_key)
}
```

## Managing Secrets via CLI

Secrets are environment-scoped — each deployment environment has its own set of secret values.

```shell
# Create secrets in the current environment
golem agent-secret create apiKey --secret-type '{"type":"Str"}' --secret-value '"sk-abc123"'
golem agent-secret create db.password --secret-type '{"type":"Str"}' --secret-value '"s3cret"'

# List all secrets
golem agent-secret list

# Update a secret value
golem agent-secret update-value apiKey --secret-value '"new-value"'

# Delete a secret
golem agent-secret delete apiKey
```

> **Note:** For `update-value` and `delete`, you can also use `--id <uuid>` instead of the positional path.

## Secret Defaults in golem.yaml

For local development convenience, set defaults under `secretDefaults`. These are **not** used in production environments:

```yaml
secretDefaults:
  local:
    - path: [apiKey]
      value: "dev-key-123"
    - path: [db, password]
      value: "dev-password"
```

## Key Points

- Secret paths use **camelCase** — Rust `snake_case` fields are converted automatically (e.g., `api_key` → `apiKey`)
- The `--secret-type` argument takes a JSON-encoded analysed type: `'{"type":"Str"}'` for `String`, `'{"type":"S32"}'` for `i32`
- Secrets are stored **per-environment**, not per-agent-instance
- Missing required secrets cause agent creation/deployment to fail — use `Option<Secret<T>>` for optional secrets
- Secrets are lazily loaded on `.get()`, allowing runtime updates without restarting the agent
