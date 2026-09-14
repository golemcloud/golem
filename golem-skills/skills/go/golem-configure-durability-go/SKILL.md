---
name: golem-configure-durability-go
description: "Choosing between durable and ephemeral agents in a Go Golem project, and adding periodic snapshots. Use when the user asks about agent durability modes, making an agent stateless, configuring agent persistence, or reducing oplog replay cost in a Go Golem project."
---

# Configuring Agent Durability (Go)

## Overview

Durability is a property of the **agent definition** — it lives on the `golem.Spec`
you pass to `golem.DefineAgent`. There is no attribute or macro; you set two
struct fields:

- `Mode` — `golem.Durable` (the default) or `golem.Ephemeral`.
- `Snapshot` — a `golem.SnapshotPolicy`, defaulting to `golem.SnapshotDisabled`.

Nothing in the implementation package changes: handlers are written the same way
regardless of mode.

## Steps

1. **Decide the mode** — durable (default) unless the agent is genuinely stateless.
2. **Set `Mode`** on the `golem.Spec` in the definition package (omit it for durable).
3. **Add a snapshot policy** if the oplog would otherwise grow unboundedly.
4. **Build** — run `golem build`.

## Durable Agents (Default)

By default every Golem agent is **durable** — `Mode` defaults to `golem.Durable`,
so you never write it explicitly:

- State persists across invocations, failures, and restarts.
- Every side effect is recorded in an **oplog** (operation log).
- On failure, the agent is transparently recovered by replaying the oplog.
- No special code needed — durability is automatic.

You **cannot opt out of oplog writes for a durable agent** — the oplog is how
durability works. If oplog volume or replay cost is the concern (long-running
agents, heartbeats, polling, recurring tasks), do not try to skip persistence;
add **periodic snapshots** instead (below).

```go
// Package counter is the DEFINITION of a standard durable agent.
package counter

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

// Mode is omitted, so it defaults to golem.Durable.
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "CounterAgent",
	Description: "A simple durable counter",
})

var (
	Increment = golem.DefineMethod[ID, golem.Unit, int64]("increment", golem.Desc("Increase the count by one"))
	Value     = golem.DefineMethod[ID, golem.Unit, int64]("value", golem.Desc("Return the current value"))
)
```

The implementation is ordinary — the private `state` is rebuilt by oplog replay
on recovery:

```go
package impl

import (
	"myapp/agents/counter"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ count int64 }

var agent = golem.Implement(counter.Agent, func(counter.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, counter.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.count++
		return ctx.State.count
	})
	golem.Handle(agent, counter.Value, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return ctx.State.count
	})
}
```

## Durable with Periodic Snapshots

Same durability guarantees as the default, but recovery starts from the **latest
snapshot** instead of replaying the whole oplog. Use this whenever the oplog grows
unboundedly — long-running agents, high-frequency state changes, heartbeats,
polling loops, recurring tasks.

Set the `Snapshot` field on the `golem.Spec`:

```go
import "time"

// Snapshot every 10 invocations.
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:     "CounterAgent",
	Snapshot: golem.SnapshotEveryN(10),
})

// Or at most once per interval.
var HeartbeatAgent = golem.DefineAgent[ID](golem.Spec{
	Name:     "HeartbeatAgent",
	Snapshot: golem.SnapshotPeriodic(30 * time.Second),
})

// Or let the platform pick the cadence.
var PipelineAgent = golem.DefineAgent[ID](golem.Spec{
	Name:     "PipelineAgent",
	Snapshot: golem.SnapshotDefault,
})
```

The snapshot policy constructors are:

| Constructor | Meaning |
|---|---|
| `golem.SnapshotDisabled` | The zero value — the platform never snapshots. |
| `golem.SnapshotDefault` | Snapshot at the platform's default cadence. |
| `golem.SnapshotEveryN(n uint16)` | Snapshot every `n` invocations. |
| `golem.SnapshotPeriodic(d time.Duration)` | Snapshot on a fixed time interval. |

### What gets snapshotted

Go reflection **cannot see unexported fields**, so the idiomatic private state
(e.g. an unexported `count`) is only captured if the state type implements the
`golem.Snapshotter` interface:

```go
type Snapshotter interface {
	Save() ([]byte, error)
	Load([]byte) error
}
```

Without a `Snapshotter`, the snapshot is the JSON of the state's **exported**
fields only. If your durable state is unexported (the common case), implement
`Save`/`Load` on the `*state` before relying on snapshots.

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is
not needed:

- State is discarded after each invocation completes.
- No durable state across invocations — lower overhead.
- Useful for pure functions, request handlers, or adapters.

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "StatelessHandler",
	Mode: golem.Ephemeral,
})
```

## When to Choose Which

| Use Case | Mode |
|----------|------|
| Counter, shopping cart, workflow orchestrator | **Durable** (default) |
| Stateless request processor, transformer | **Ephemeral** |
| Long-running saga or multi-step pipeline | **Durable** (default) |
| Pure computation, no side effects worth persisting | **Ephemeral** |
| Agent that calls external APIs with at-least-once semantics | **Durable** (default) |
| Long-running agent with heartbeats, polling, or recurring tasks | **Durable + periodic snapshots** |
| Any durable agent whose oplog grows so large that replay is slow | **Durable + periodic snapshots** |

When in doubt, use the default (durable). Ephemeral is an optimization for agents
that genuinely don't need persistence; add periodic snapshots whenever recovery
time matters.

## Key Constraints

- Mode and snapshot policy are set on the **definition** (`golem.Spec`), not the
  implementation — the handler code is identical either way.
- `Mode` defaults to `golem.Durable`; only set it to switch to `golem.Ephemeral`.
- `Snapshot` defaults to `golem.SnapshotDisabled`; snapshots do not change
  durability guarantees, only recovery time.
- **Read-only methods require a durable agent** — marking a method on an
  `Ephemeral` agent read-only is a definition error (see `golem-mark-read-only-go`).
- Unexported private state is not captured by the default reflective snapshot;
  implement `golem.Snapshotter` if you snapshot such an agent.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent and its methods in the first place |
| `golem-mark-read-only-go` | Add cacheable, side-effect-free query methods (durable only) |
| `golem-atomic-block-go` | Fine-grained persistence / idempotency / atomicity controls |
| `golem-recurring-task-go` | Long-running agents that heartbeat or schedule future work |
