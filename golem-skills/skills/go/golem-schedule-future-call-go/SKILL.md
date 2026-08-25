---
name: golem-schedule-future-call-go
description: "Scheduling a one-off future agent call from Go agent code, and canceling it. Use when the user wants a delayed invocation, a timed call for later, or to cancel a pending scheduled call in a Go Golem project."
---

# Scheduling a Future Agent Call from Go Code

## Overview

`Method.Schedule(client, at, in)` enqueues a **one-off** invocation of a method to run at a specific future time. It returns a `*golem.ScheduledInvocation`, whose `Cancel()` prevents the invocation if it has not started yet.

Like `Trigger`, `Schedule` does not wait for a result — so it is safe to target **another** agent or the **same** instance (a synchronous self-`Call` would deadlock). The scheduled time is recorded durably and survives restarts.

For a task that **reschedules itself** on an interval (a periodic / cron-like loop), see `golem-recurring-task-go`; this skill covers scheduling a single future call and canceling it.

## Steps

1. **Get a client** for the target instance: `counter.Agent.Get(counter.ID{Name: "my-counter"})`.
2. **Schedule the method** at a `time.Time`: `counter.Increment.Schedule(c, when, in)`.
3. **Keep the returned `*golem.ScheduledInvocation`** if you may need to cancel it.
4. **Cancel** with `inv.Cancel()` before the scheduled time to prevent it.

## Scheduling a future call on another agent

```go
package impl

import (
	"time"

	"myapp/agents/counter"
	"myapp/agents/scheduler"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ pending *golem.ScheduledInvocation }

var agent = golem.Implement(scheduler.Agent, func(scheduler.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, scheduler.Arm, func(ctx *golem.Context[state], _ golem.Unit) golem.Unit {
		c := counter.Agent.Get(counter.ID{Name: "my-counter"})
		// Run counter.increment 60 seconds from now.
		ctx.State.pending = counter.Increment.Schedule(c, time.Now().Add(60*time.Second), golem.Unit{})
		return golem.Unit{}
	})
}
```

`Schedule` returns a `*golem.ScheduledInvocation`; its `ID` field is the invocation's `InvocationID`.

### Scheduling a method that takes arguments

Pass the method's input value as the last argument, exactly as with `Call` / `Trigger`:

```go
c := counter.Agent.Get(counter.ID{Name: "my-counter"})
counter.Add.Schedule(c, time.Now().Add(24*time.Hour), counter.AddIn{By: 5})
```

## Canceling a scheduled call

Keep the `*golem.ScheduledInvocation` (e.g. in state) and call `Cancel()`:

```go
func init() {
	golem.Handle(agent, scheduler.Cancel, func(ctx *golem.Context[state], _ golem.Unit) golem.Unit {
		if ctx.State.pending != nil {
			ctx.State.pending.Cancel() // no-op if it has already started
			ctx.State.pending = nil
		}
		return golem.Unit{}
	})
}
```

`Cancel()` is idempotent: it does nothing if already canceled, and cannot un-run an invocation that has already started.

## Scheduling a future call to itself

To message its own instance an agent needs a client for its own `ID`. `Context` exposes only the id string, so capture the typed `ID` in state at construction (see `golem-recurring-task-go` for the full pattern):

```go
self := counter.Agent.Get(ctx.State.self) // ctx.State.self captured in the constructor
counter.Increment.Schedule(self, time.Now().Add(time.Minute), golem.Unit{})
```

## Schedule vs Trigger

- `Schedule(c, at, in) *ScheduledInvocation` — run once at a future `time.Time`; cancelable via `Cancel()`.
- `Trigger(c, in) InvocationID` — enqueue immediately, no delay, not cancelable. Use it to advance to the next step now.
- Never self-`Call` — a synchronous call into the same instance deadlocks; self-messaging must use `Schedule` or `Trigger`.

## Key Constraints

- `Schedule` / `Trigger` are called on the **method descriptor** (`counter.Increment.Schedule(c, ...)`), and the client comes from the callee's **definition** (`counter.Agent.Get(id)`).
- Compute the time with `time.Now().Add(d)` (a `time.Time`); it is recorded durably.
- To cancel later, you must retain the `*golem.ScheduledInvocation` — don't discard it if cancellation may be needed. Hold it in agent state, not across a bare `var` initializer.
- For a self-scheduling recurring loop, don't hand-roll it here — use `golem-recurring-task-go`.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-recurring-task-go` | A self-rescheduling / periodic (cron-like) task |
| `golem-fire-and-forget-go` | Enqueue an immediate invocation (`Trigger`) |
| `golem-call-another-agent-go` | Await a result from another agent (`Call` / `CallAsync`) |
| `golem-schedule-agent-go` | Schedule a future invocation from the CLI |
