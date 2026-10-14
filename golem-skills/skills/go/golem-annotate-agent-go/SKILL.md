---
name: golem-annotate-agent-go
description: "Configuring a Go agent's definition — name, description, mode, HTTP mount, and method descriptions. Use when the user asks to set an agent's name/description, make it durable or ephemeral, mount it under an HTTP path, or add descriptions to agent methods for metadata/LLM discovery in a Go Golem project."
---

# Configuring a Go Agent's Definition

## Overview

An agent's **definition** carries all of its declarative metadata. In Go there are no attribute macros — the metadata is plain values you pass to `golem.DefineAgent`:

- the `golem.Spec` struct — `Name`, `Description`, `Mode`, `HTTP` (mount), and `Snapshot`;
- per-method options passed to `Agent.Method` — `golem.Description(...)` for a method description and `golem.HTTP(...)` for HTTP routes.

This metadata is surfaced in the published agent-type schema (used by the platform, by other agents, and by metadata/LLM tool discovery). It has no effect on the handler logic itself.

## Steps

1. **Set identity + mode** on the `Spec`: `Name`, `Description`, and (optionally) `Mode`.
2. **Describe each method** with the `golem.Description("...")` option on `Agent.Method`.
3. **Mount over HTTP** (optional) by setting `Spec.HTTP` and adding `golem.HTTP(...)` routes to methods.
4. **Build** — run `golem build`.

## Spec fields (`agents/inventory/inventory.go`)

```go
// Package inventory is the DEFINITION of the inventory agent.
package inventory

import "github.com/golemcloud/golem/sdks/go/golem"

// ID holds the constructor parameters and is the agent's type-level identity.
type ID struct{ WarehouseID string }

type RestockIn struct {
	SKU      string
	Quantity uint32
}

type PickIn struct {
	SKU      string
	Quantity uint32
}

// Agent is the state-free definition. All declarative metadata lives on Spec.
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "InventoryAgent",
	Description: "Tracks per-warehouse stock levels",
	Mode:        golem.Durable, // default; state persists and replays deterministically
})
```

`golem.Spec` has these fields:

| Field | Type | Meaning |
|-------|------|---------|
| `Name` | `string` | Wire-level agent type name, as seen by the platform and other agents. **Required.** |
| `Description` | `string` | Free-text description surfaced in the agent-type metadata. |
| `PromptHint` | `string` | Tells AI callers when to create this agent; surfaced with its constructor. |
| `Mode` | `golem.Mode` | `golem.Durable` (default) or `golem.Ephemeral`. |
| `HTTP` | `*golem.Mount` | When set, mounts the agent's methods under an HTTP path prefix (see below). |
| `Snapshot` | `golem.SnapshotPolicy` | Snapshot cadence; zero value disables snapshotting. |
| `Dependencies` | `[]golem.AgentDependency` | Agents of this component that this one depends on, published with its metadata. |

### Mode

- **`golem.Durable`** — the default (`Mode` zero value). State persists across invocations and replays deterministically on recovery.
- **`golem.Ephemeral`** — no state is persisted across invocations.

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "StatelessScorer",
	Mode: golem.Ephemeral,
})
```

## Describing methods

Attach `golem.Description("...")` to a method to set the description carried in the schema. Setting it more than once for the same method is a **definition error**, not a silent overwrite.

```go
type CheckStockIn struct{ SKU string }

var (
	CheckStock = Agent.Method[CheckStockIn, uint32]("check-stock", golem.Description("Returns the number of units in stock for the given SKU; 0 if unknown."))

	Restock = Agent.Method[RestockIn, uint32]("restock", golem.Description("Increases the stock count for the given SKU by the amount; returns the new total."))

	// Model an observable failure in the result type, and describe it.
	Pick = Agent.Method[PickIn, golem.Result[uint32, string]]("pick", golem.Description("Decreases stock for the given SKU; returns err if there is insufficient stock."))
)
```

Method options compose — pass several in one `Agent.Method` call (e.g. `golem.Description(...)` together with `golem.PromptHint(...)`, `golem.HTTP(...)` or `golem.ReadOnly()`).

## Mounting over HTTP

Set `Spec.HTTP` to a `*golem.Mount` and add `golem.HTTP(...)` routes per method. Every `ID` field must appear as a `{var}` in the mount path so the platform can identify the instance.

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "InventoryAgent",
	Description: "Tracks per-warehouse stock levels",
	HTTP: &golem.Mount{
		Path: "/warehouses/{warehouseId}", // {warehouseId} binds ID.WarehouseID
		CORS: []string{"*"},
	},
})

// {sku} binds CheckStockIn.SKU: a parameter is a field of the input struct.
var CheckStock = Agent.Method[CheckStockIn, uint32]("check-stock", golem.Description("Returns units in stock for the given SKU."), golem.HTTP(golem.GET("/stock/{sku}")))
```

`golem.Mount` fields: `Path` (required), `Auth`, `CORS`, `PhantomAgent`, `PhantomID` (select an existing phantom by `golem.PhantomIDPath(name)` or `golem.PhantomIDQuery(name)`, optionally `.Optional()`), `WebhookSuffix`, `ExposeFiles`, `FileResponseHeaders`. Verb constructors are `golem.GET/POST/PUT/DELETE/PATCH/...` and `golem.Custom("VERB", "/path")`. For full route-binding rules (path/query/header/body, per-route auth and CORS) load `golem-add-http-endpoint-go`.

## Key Constraints

- `Spec.Name` is **required** and is the wire-level type name — keep it stable, other agents address this agent by it.
- `Mode` defaults to `golem.Durable` (the zero value); set `golem.Ephemeral` only for stateless agents.
- `golem.Description(...)` and `golem.HTTP(...)` are **definition-side** metadata on `Agent.Method`; they never change handler behavior.
- Setting `Desc` more than once on one method is a definition error reported at discovery.
- `Desc` describes what a method does; `golem.PromptHint(...)` (and `Spec.PromptHint` for the agent) tells AI callers when to use it, like Rust's `#[description]`/`#[prompt]` pair. Each is set at most once per method.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Create the agent's definition + implementation from scratch |
| `golem-add-http-endpoint-go` | Full HTTP mount/route binding rules (path/query/header/body, auth, CORS) |
| `golem-mark-read-only-go` | Mark a method read-only and configure result caching |
| `golem-configure-durability-go` | Tune durability/retry behavior for a durable agent |
