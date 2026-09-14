---
name: golem-parallel-workers-go
description: "Fan out work to multiple parallel agents and collect results in a Go Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning worker agents for parallel work, or aggregating results from multiple agents in a Go Golem project."
---

# Parallel Workers — Fan-Out / Fan-In (Go)

## Overview

A single Golem agent processes invocations **sequentially** — it cannot run work in parallel by itself. To execute work concurrently, distribute it across **multiple agent instances** and have them run at the same time. The Go SDK gives you two building blocks:

1. **`CallAsync` + `Future.Get`** — start several cross-agent calls at once, then await each. This is the SDK's only source of concurrency: `Call` (synchronous `invoke-and-await`) blocks the whole component, whereas `Future.Get` is async, so a goroutine blocked in it yields to the component-model event loop and lets the other in-flight calls proceed.
2. **`Trigger` + promises** — fire-and-forget each worker, hand each a promise ID, then await all the promises. Best for long-running work where you don't want a call in flight the whole time.

> **Platform contract:** a single target instance handles one invocation at a time. Concurrency comes from fanning out to **different** target instances (distinct `ID`s), not from calling the same instance repeatedly.

> **No `fork`:** the Rust SDK's `fork()` has no equivalent in the current Go SDK. Achieve parallelism by addressing multiple worker instances by `ID` as shown below.

## Approach 1: async fan-out with `CallAsync` / `Future.Get`

Start every worker call with `CallAsync`, collect the `*golem.Future[Out]` values in a slice, then `Get()` each. All calls are in flight while you await.

### Worker definition (`agents/worker/worker.go`)

```go
package worker

import "github.com/golemcloud/golem/sdks/go/golem"

// ID identifies a worker instance — distinct IDs run in parallel.
type ID struct{ Index int64 }

type ProcessIn struct{ Item string }

var Agent = golem.DefineAgent[ID](golem.Spec{
    Name:        "WorkerAgent",
    Description: "Processes one item",
})

var Process = golem.DefineMethod[ID, ProcessIn, string]("process",
    golem.Desc("Process a single item and return the result"))
```

### Coordinator fan-out (`agents/coordinator/impl/impl.go`)

```go
package impl

import (
    "myapp/agents/coordinator"
    "myapp/agents/worker"

    "github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(coordinator.Agent, func(coordinator.ID) *state { return &state{} })

func init() {
    golem.Handle(agent, coordinator.FanOut, func(_ *golem.Context[state], in coordinator.FanOutIn) []string {
        // 1. Start every call — each returns immediately with a future.
        futures := make([]*golem.Future[string], len(in.Items))
        for i, item := range in.Items {
            // A distinct worker instance per item → they run concurrently.
            c := worker.Agent.Get(worker.ID{Index: int64(i)})
            futures[i] = worker.Process.CallAsync(c, worker.ProcessIn{Item: item})
        }

        // 2. Await each. All calls were already in flight, so this fans in.
        results := make([]string, len(futures))
        for i, f := range futures {
            results[i] = f.Get() // Get() blocks (and yields) for this one result
        }
        return results
    })
}
```

`Future.Get` is **fail-loud**: an infra failure (a `golem.RemoteCallError`) traps and surfaces as an agent-error. Model *expected* per-item outcomes as a `golem.Result` in the worker method's output instead, then unwrap or inspect each result:

```go
// worker.Process returns golem.Result[string, string]
res := futures[i].Get()          // golem.Result[string, string]
if res.IsErr() {
    failures = append(failures, res.Err())
} else {
    successes = append(successes, res.Ok())
}
```

## Approach 2: fire-and-forget with promise collection

For long-running work, `Trigger` each worker (fire-and-forget, no call held open) and pass each a promise ID; the worker completes its promise when done, and the coordinator awaits them all.

### Coordinator

```go
golem.Handle(agent, coordinator.DispatchAndCollect, func(ctx *golem.Context[state], in coordinator.RegionsIn) []string {
    // One promise per worker.
    promises := make([]*golem.Promise[string], len(in.Regions))
    for i := range in.Regions {
        promises[i] = golem.NewPromise[string]()
    }

    // Fire-and-forget: hand each worker its region and its promise id.
    for i, region := range in.Regions {
        c := worker.Agent.Get(worker.ID{Region: region})
        worker.RunReport.Trigger(c, worker.RunReportIn{Promise: promises[i].ID()})
    }

    // Await all promises (each durably suspends until its worker completes it).
    results := make([]string, len(promises))
    for i, p := range promises {
        results[i] = p.Await()
    }
    return results
})
```

### Worker completes its promise

```go
golem.Handle(agent, worker.RunReport, func(ctx *golem.Context[state], in worker.RunReportIn) golem.Unit {
    report := "Report for " + ctx.State.region + ": OK"
    golem.CompletePromise(in.Promise, report)
    return golem.Unit{}
})
```

`Trigger` returns a `golem.InvocationID` and does not wait; a failure after the invocation is accepted is not reported at the trigger site — the worker signals completion (or failure) through its promise.

## When to use which

| Criteria | `CallAsync` / `Future` | `Trigger` + promises |
|----------|------------------------|----------------------|
| Short calls, want results inline | ✅ Best fit | Works but heavier |
| Long-running work, don't hold a call open | ⚠️ Call stays in flight | ✅ Best fit |
| Need the invocation identity for other work | `Future.ID` | `Trigger` returns `InvocationID` |
| Per-item error as a value | Return `golem.Result` in the method | Encode outcome in the promise payload |

## Key Constraints

- **No threads within an agent.** Parallelism comes from distributing across distinct worker instances (distinct `ID`s); fanning out to the *same* instance serializes.
- Don't hold a `*golem.Future` across invocations — start it and `Get()` it within the same handler. `Get` consumes the future (a second `Get` panics).
- `Future.Get` and `Promise.Await` are fail-loud; model expected outcomes as a `golem.Result` in the method output, not as a trap.
- Avoid a **synchronous** cycle: two agents each blocking in `Call` on the other deadlocks. Break cycles with `Trigger` / promises.
- Worker instances persist after the coordinator finishes (they are durable by default); give them stable IDs you can address again, or make them ephemeral (see `golem-stateless-agent-go`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | The synchronous / async call mechanics (`Call`, `CallAsync`) |
| `golem-fire-and-forget-go` | `Trigger` details for fire-and-forget dispatch |
| `golem-wait-for-external-input-go` | Promises used to collect worker results |
| `golem-multi-instance-agent-go` | Address many worker instances by `ID` (and phantoms) |
