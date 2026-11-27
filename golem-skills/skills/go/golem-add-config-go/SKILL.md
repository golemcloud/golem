---
name: golem-add-config-go
description: "Adding typed configuration to a Go Golem agent with DefineConfiguredAgent and ctx.Config. Use when the user asks about agent config, configuration values, config structs, or passing settings to a Go agent."
---

# Adding Typed Configuration to a Go Agent

## Overview

A Go agent's configuration is a single **config struct** attached to the agent's type. Each exported field becomes one whole config value unless it has a `golemConfig:"nested"` tag. The host resolves static values from deployment config, creation overrides, and code defaults. Each `ctx.Config(...)` call reads static fields from the host's selected owner revision; the SDK does not cache them. Secret fields are lazy handles and read the host only when revealed.

Two things change versus a plain agent:

- define with `golem.DefineConfiguredAgent[ID, Cfg]` instead of `golem.DefineAgent[ID]`,
- implement with `Agent.ImplementConfigured` (its constructor receives an `*golem.InitContext[ID, state, Cfg]`),

then read the config with `ctx.Config(Agent)` inside a method, or `ctx.Config()` inside the constructor.

## Steps

1. **Declare the config struct** — one exported field per config key.
2. **Attach it** with `golem.DefineConfiguredAgent[ID, Cfg]` in the definition.
3. **Implement** with `Agent.ImplementConfigured`; read config from the `InitContext` in the constructor.
4. **Read at method time** with `ctx.Config(Agent)`.
5. **Provide values** under `agents.<Name>.config` in `golem.yaml`.

## Definition (`agents/shop/shop.go`)

```go
// Package shop is the DEFINITION of the shop agent.
package shop

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

// Config is the agent's whole config surface. Each exported field is one config
// value; a struct field is one record, not a group of config declarations.
type Config struct {
	Greeting string
	Tax      TaxConfig         // one record value at path tax
	Timezone *string           // optional (read back as nil when unset)
	Features []string          // list
	Limits   map[string]int64  // map
}

type TaxConfig struct{ RatePct int64 }

var Agent = golem.DefineConfiguredAgent[ID, Config](golem.Spec{
	Name:        "ShopAgent",
	Description: "A shop whose behaviour is driven by config",
})

var (
	Greet    = Agent.Method[golem.Unit, string]("greet", golem.Description("Greeting, from config"))
	Settings = Agent.Method[golem.Unit, []string]("settings", golem.Description("Return the configured features"))
)
```

Field-name conventions match plain agent types: use sized integers (`int64`, not bare `int`), and a `*T` field is `option<T>`. Field names are lowerCamelCase for the wire, with each initialism as one word (`RatePct` → `ratePct`, `APIKey` → `apiKey`, `AccountID` → `accountId`).

The `Tax TaxConfig` field above declares one record-valued config key, `tax`. To declare its fields as separate config keys, mark the group explicitly:

```go
type Config struct {
	Tax TaxConfig `golemConfig:"nested"` // declares tax.ratePct
}
```

A nested group must be a struct. Put defaults and restrictions on its fields, not on the group itself. Both forms use nested YAML maps, but only the tagged form declares `tax.ratePct` as an independent config key.

## Code Defaults

Use `golemDefault` to declare a literal canonical JSON default for a static field:

```go
type Config struct {
	Retries  uint32  `golem:"max=5" golemDefault:"3"`
	Greeting string  `golemDefault:"\"Welcome\""`
	Timezone *string `golemDefault:"null"`
}
```

The SDK validates defaults against each field's schema and exports them in metadata. The host supplies a code default only when neither deployment config nor a creation override supplies the value. An explicit `null`, `false`, zero, or empty value is not missing. Omitting the tag differs from declaring `golemDefault:"null"`. Defaults are literals, not expressions or SDK-side fallbacks, and cannot supply secrets or other host-managed handles.

## Implementation (`agents/shop/impl/impl.go`)

```go
// Package impl is the IMPLEMENTATION of the shop agent.
package impl

import (
	"myapp/agents/shop"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ greeting string }

// ImplementConfigured binds the constructor; its InitContext exposes Config().
var agent = shop.Agent.ImplementConfigured(func(ctx *golem.InitContext[shop.ID, state, shop.Config]) *state {
	// Constructor-time config read: seed state once from config.
	return &state{greeting: ctx.Config().Greeting}
})

func init() {
	agent.Handle(shop.Greet, func(ctx *golem.Context[state], _ golem.Unit) string {
		return ctx.State.greeting
	})

	agent.Handle(shop.Settings, func(ctx *golem.Context[state], _ golem.Unit) []string {
		cfg := ctx.Config(shop.Agent) // method-time config read
		return cfg.Features
	})
}
```

`ctx.Config(shop.Agent)` returns the whole `shop.Config` value; `ctx` is the running method's `*golem.Context[state]` (requiring it means config is only readable from inside a running method). The constructor reads the same config via `ctx.Config()` on its `InitContext`.

Each call reads the static fields again from the host's selected owner revision and builds lazy secret handles. Saving a returned value in application state, as the constructor does with `greeting`, preserves that saved value; later config reads do not update it automatically.

A config field takes the same `golem` restriction tag as a method parameter (see `golem-add-agent-go`), which publishes it in the config's declared type:

```go
type Config struct {
	Retries uint32 `golem:"max=5"`
}
```

## Providing Values in `golem.yaml`

Set defaults under `agents.<Name>.config`. Keys are **camelCase** — they match the Go field names in lowerCamelCase, with each initialism as one word (`AccountID` → `accountId`); nested structs are nested maps, and a `map[string]V` is a list of `[key, value]` pairs:

```yaml
agents:
  ShopAgent:
    config:
      greeting: "Welcome to ACME"
      tax:
        ratePct: 20
      features: ["fast-checkout", "gift-wrap"]
      limits: [["maxItems", 100]]
      # timezone is omitted → the optional *string field reads back as nil
```

## Overriding Config Over RPC

When one agent calls another, the caller can override the callee's **local** (non-secret) config when it creates the callee, with the callee definition's `WithConfig` option to `Create`, which checks the config type at compile time. `golem.Keyed()` creates the keyed agent of the `ID`, the one `Upsert` reaches, and fails with a `*golem.RemoteCallError` of kind `golem.RemoteConflict` when it already exists; without it, `Create` makes a new agent without a key:

```go
client, err := shop.Agent.Create(
	shop.ID{Name: "acme"},
	golem.Keyed(),
	shop.Agent.WithConfig(shop.Config{Greeting: "Hi from the caller"}),
)
```

`Upsert` takes no config overrides: an agent that it creates gets its provisioned config. Only declared local keys can be overridden; secret fields are always platform-provisioned and are skipped. `WithConfig` replaces every local value: it sends every local field of the struct, zero values included. To set only some, or to call an agent in another component, use the generated guest client's `With<Agent>Config` option (see `golem-call-another-agent-go`). It sends only the fields you set, through `golem.WithConfigEntries(golem.ConfigEntryOf(path, value)…)`.

## Key Constraints

- Config is **agent-attached and instance-bound** — read only from within that agent's own constructor or methods, never from free-floating code.
- Reading an **undeclared** key, or an unset **required** key, traps. Declare a field as a pointer (`*T`) to read an absent value back as `nil` instead of trapping.
- Each config read fetches static fields from the host's selected owner revision. The SDK does not cache them across reads; secret handles defer host reads until `.Get()` or `.MustGet()` (see `golem-add-secret-go`).
- Use `golem.DefineAgent[ID]` (no config) for agents that need none; `golem.NoConfig` is the empty config type and never needs naming.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the base agent this config attaches to |
| `golem-add-secret-go` | Add `golem.Secret[T]` fields for sensitive values |
| `golem-call-another-agent-go` | Override a callee's config with the definition's `WithConfig` over RPC |
