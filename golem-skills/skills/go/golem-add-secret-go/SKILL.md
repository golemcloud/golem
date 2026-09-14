---
name: golem-add-secret-go
description: "Adding typed secrets to a Go Golem agent with golem.Secret[T]. Use when the user needs API keys, passwords, or tokens that must not be checked into source control in a Go Golem project."
---

# Adding Secrets to a Go Agent

## Overview

Secrets are sensitive configuration values (API keys, passwords, tokens) read at runtime through `golem.Secret[T]`. A secret uses the **same config mechanism as regular typed config** — it is just a field whose type is `golem.Secret[T]` inside the agent's config struct (see `golem-add-config-go`). The config value carries an opaque secret handle; the plaintext is revealed only when your code calls `.Get()`.

Secrets are **not** stored in `golem.yaml` (which is source-controlled) except through `secretDefaults` for local development. In real environments they are managed per-environment via the CLI.

## Steps

1. **Add a `golem.Secret[T]` field** to the agent's config struct.
2. **Define / implement** the agent as a configured agent (`DefineConfiguredAgent` + `ImplementConfigured`).
3. **Reveal** the value at runtime with `.Get()`.
4. **Provide values** via `secretDefaults` (local) or the CLI (per environment).

## Declaring a Secret Field

A secret is any field of type `golem.Secret[T]`, at any depth of the config struct:

```go
// Package client is the DEFINITION of an API-client agent.
package client

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type Config struct {
	Endpoint string
	APIKey   golem.Secret[string] // a secret leaf
	DB       DBConfig
}

type DBConfig struct {
	Host     string
	Password golem.Secret[string] // secret at path db.password
}

var Agent = golem.DefineConfiguredAgent[ID, Config](golem.Spec{
	Name:        "ClientAgent",
	Description: "Calls an external API using a secret key",
})

var Connect = golem.DefineMethod[ID, golem.Unit, string]("connect",
	golem.Desc("Describe the connection using secret values"))
```

## Reading a Secret at Runtime

Read the config, then call `.Get()` on the secret field to reveal the current plaintext. Because `.Get()` re-reads the host on every call, a *rotated* secret is observed rather than a stale snapshot:

```go
// Package impl is the IMPLEMENTATION of the API-client agent.
package impl

import (
	"fmt"

	"myapp/agents/client"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.ImplementConfigured(client.Agent,
	func(_ *golem.InitContext[client.ID, state, client.Config]) *state { return &state{} })

func init() {
	golem.Handle(agent, client.Connect, func(ctx *golem.Context[state], _ golem.Unit) string {
		cfg := golem.Config(client.Agent, ctx)

		apiKey := cfg.APIKey.Get()      // reveals the current plaintext
		dbPass := cfg.DB.Password.Get() // secret at any depth

		return fmt.Sprintf("connecting to %s (key len=%d, db host=%s, pw len=%d)",
			cfg.Endpoint, len(apiKey), cfg.DB.Host, len(dbPass))
	})
}
```

A `golem.Secret[T]` cannot be constructed from a plaintext and cannot be a method parameter or return value — it is config-only, always obtained from the agent's config. Its `String()`/`GoString()` render as `golem.Secret(redacted)`, so it stays out of logs formatted with `%v`/`%s`.

## Secret Defaults in `golem.yaml` (local only)

For local development, set defaults under `secretDefaults.<environment>`. Keys are **camelCase** and follow the same path flattening as config; nested structs are nested maps. These are **not** used in production:

```yaml
secretDefaults:
  local:
    apiKey: "dev-key-123"
    db:
      password: "dev-password"
```

## Managing Secrets via CLI (per environment)

Outside local defaults, secrets are environment-scoped — each deployment environment has its own values, set through the platform CLI (paths use the same camelCase as the config keys):

```shell
golem secret create apiKey --secret-type String --secret-value "sk-abc123"
golem secret create db.password --secret-type String --secret-value "s3cret"
golem secret list
golem secret update-value apiKey --secret-value "new-value"
golem secret delete apiKey
```

## Key Constraints

- A secret is a **config field** of type `golem.Secret[T]` — the agent must be a configured agent (`DefineConfiguredAgent` / `ImplementConfigured`); see `golem-add-config-go`.
- Only **`.Get()`** reveals the plaintext, and it re-reads the host each call, so a rotated value is observed without restarting the agent. Each reveal pins the resolved revision for deterministic retry/replay.
- Secret values are **never** written to `golem.yaml` except via `secretDefaults` (local dev). Production values come from the CLI, per environment.
- A missing required secret fails agent creation/deployment. Read a secret from within an invocation only — `.Get()` calls the host, and a read failure panics (surfaces as an agent error).
- Over RPC, secret fields are always platform-provisioned; `golem.WithConfig` overrides only **local** config, never secrets.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-config-go` | The non-secret typed-config mechanism secrets build on |
| `golem-add-agent-go` | Define the base agent the config attaches to |
| `golem-call-another-agent-go` | Understand why secrets are not overridable over RPC |
