---
name: golem-fire-and-forget-go
description: "Fire-and-forget agent invocations in Go with Trigger. Use when the user wants to enqueue work without awaiting a result, send a notification to another agent, fan out to many agents, or kick off background work in a Go Golem project."
---

# Fire-and-Forget Invocations in Go (`Trigger`)

## Overview

`Method.Trigger(client, in)` enqueues an invocation **without waiting for its result**. It returns once the target has durably accepted the invocation, with a `*golem.Invocation[Out]` handle, or an error if the invocation was not accepted (`MustTrigger` panics instead). Use it for notifications, fan-out, or handing work to a background/worker agent. Unlike a synchronous `Call`, `Trigger` waits only for the target to accept the invocation, not for it to run — so it is also safe for an agent to message **itself** (a synchronous self-`Call` would deadlock, since one instance handles one invocation at a time).

## Steps

1. **Import the target agent's definition package.**
2. **Get a client** with `Agent.Upsert(id)`.
3. **Trigger the method**: `Target.Method.MustTrigger(client, in)` — returns once the target accepted the invocation.

## Example

```go
package impl

import (
	"myapp/agents/coordinator"
	"myapp/agents/worker"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = coordinator.Agent.Implement(func(coordinator.ID) *state { return &state{} })

func init() {
	agent.Handle(coordinator.Start, func(_ *golem.Context[state], in coordinator.StartIn) golem.Unit {
		// Hand each unit of work to a worker without awaiting the outcome.
		for _, w := range in.Jobs {
			c := worker.Agent.Upsert(worker.ID{Name: w.Worker})
			worker.Dispatch.MustTrigger(c, worker.DispatchIn{Job: w.Job})
		}
		return golem.Unit{}
	})
}
```

Fanning out is just a loop of `Trigger` calls — each runs independently on its target agent.

## Trigger vs Call

- `Call(c, in) (Out, error)` / `MustCall(c, in) Out` — blocks, returns the result. Use when you need the value. Never self-`Call`.
- `Trigger(c, in) (*Invocation[Out], error)` / `MustTrigger(c, in) *Invocation[Out]` — returns once the target accepted the invocation, without its result. Use for notifications, fan-out, and self-messaging.
- To have several awaited calls in flight at once, make each `Call` in its own goroutine (see `golem-parallel-workers-go`).

You can ignore the returned handle for pure fire-and-forget: dropping it only detaches, and the invocation keeps running.

## The Invocation Handle

Keep the handle, or its ID, when you need the outcome later:

```go
inv := worker.Dispatch.MustTrigger(c, worker.DispatchIn{Job: job})
id := inv.ID() // golem.InvocationID; String() is <agent-id>.<uuid>

status, err := inv.Status() // golem.InvocationPending, InvocationRunning, InvocationCompleted, ...
out, err := inv.AwaitResult() // (Out, error)

// In a later invocation of this agent, rebuild the handle from the ID (no remote call):
again := worker.Dispatch.Invocation(id)
```

- `AwaitResult` waits without starting or resubmitting the invocation. It fails with a `*golem.RemoteCallError` of kind `golem.RemoteCancelled` when the invocation was cancelled. Several goroutines may await the same handle.
- `Cancel() (bool, error)` returns `true` when it withdrew an invocation that had not started, `false` when it already finished or was already cancelled, and fails with kind `golem.RemoteCannotCancelRunning` while it runs.
- The ID only locates the invocation. Only the agent that triggered it, or a caller with management permission, can read its status, await it or cancel it; others get kind `golem.RemoteDenied`.
- A handle cannot be sent to another agent as a method argument: send its `ID()` instead.
- Generated cross-component clients have the same forms: `Trigger<M>`, `MustTrigger<M>` and `<M>Invocation(id)`.

## Key Constraints

- `Trigger` never carries a scope card, and it panics for a method that takes or returns a stream.
- As with all RPC, `Trigger` is called on the **method descriptor** (`worker.Dispatch.Trigger(client, in)`), and the client comes from the callee's **definition** (`worker.Agent.Upsert(id)`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | You need the callee's result now (`Call`) |
| `golem-recurring-task-go` | Schedule a future/self invocation at a specific time |
| `golem-add-agent-go` | Define the agent being triggered |
