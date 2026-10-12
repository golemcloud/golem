---
name: golem-recurring-task-go
description: "Recurring / scheduled work in a Go agent. Use when the user wants a periodic task, a cron-like loop, a self-rescheduling agent, or to schedule a future invocation (of itself or another agent) in a Go Golem project."
---

# Recurring & Scheduled Tasks in Go

## Overview

A recurring task is a method that **reschedules itself**: it does its work, then schedules the next run. Scheduling uses `Method.Schedule(client, at, in)`, which arranges an invocation at a future time and returns a cancelable `*golem.ScheduledInvocation` (and an error if the host refuses it; `MustSchedule` panics instead).

To message itself an agent needs a client for its **own** instance. `Agent.Bind(ctx.AgentID())` returns one: `ctx.AgentID()` is the running instance's id, read from the host on each call, so it is also right after a snapshot restore or a fork.

## Steps

1. **Capture the agent's own `ID` in state** in the `Implement` constructor.
2. **Add a tick method** that does the work and then `Schedule`s itself for the next run.
3. **Start the loop** — a `Start` method (or an external trigger) schedules the first tick.

## Example

Definition (`agents/poller/poller.go`):

```go
package poller

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "PollerAgent", Description: "Periodic poller"})

var (
	Start = Agent.Method[golem.Unit, golem.Unit]("start", golem.Description("Begin the recurring loop"))
	Tick  = Agent.Method[golem.Unit, golem.Unit]("tick", golem.Description("One iteration; reschedules itself"))
)
```

Implementation (`agents/poller/impl/impl.go`):

```go
package impl

import (
	"time"

	"myapp/agents/poller"

	"github.com/golemcloud/golem/sdks/go/golem"
)

const interval = time.Minute

type state struct{}

var agent = poller.Agent.Implement(func(poller.ID) *state { return &state{} })

func init() {
	agent.Handle(poller.Start, func(ctx *golem.Context[state], _ golem.Unit) golem.Unit {
		self := golem.Must(poller.Agent.Bind(ctx.AgentID()))
		poller.Tick.MustSchedule(self, time.Now().Add(interval), golem.Unit{})
		return golem.Unit{}
	})

	agent.Handle(poller.Tick, func(ctx *golem.Context[state], _ golem.Unit) golem.Unit {
		// ... do the periodic work ...

		// schedule the next iteration
		self := golem.Must(poller.Agent.Bind(ctx.AgentID()))
		poller.Tick.MustSchedule(self, time.Now().Add(interval), golem.Unit{})
		return golem.Unit{}
	})
}
```

Kick off the loop by invoking `start` once (from the CLI, an HTTP endpoint, or another agent).

## Scheduling vs immediate self-messaging

- `Schedule(client, at, in) (*ScheduledInvocation, error)` — run at a future `time.Time`. Keep the handle to cancel it: `inv.Cancel()`. `MustSchedule` panics instead of returning the error.
- `Trigger(client, in)` / `MustTrigger(client, in)` — enqueue immediately, no delay (use it to advance a state machine to the next step now).
- **Never self-`Call`** — a synchronous call into the same instance deadlocks (one instance processes one invocation at a time). Self-messaging must use `Trigger` or `Schedule`.

## Key Constraints

- Build the self client from `ctx.AgentID()`, not from an `ID` kept in state: a snapshot restore does not run the constructor, so state set only there is empty after a restore.
- The reschedule must live in the tick handler itself (a package-level descriptor referencing its own handler in an `init()` is fine; there is no var-initialization cycle because the handler is registered in `init`, not in a `var` initializer).
- Compute the next time with `time.Now().Add(interval)`; the scheduled time is recorded durably.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-fire-and-forget-go` | Enqueue an immediate invocation (`Trigger`) |
| `golem-call-another-agent-go` | Await a result from another agent |
| `golem-add-agent-go` | Define the agent |
