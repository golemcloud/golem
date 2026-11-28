---
name: golem-parallel-workers-go
description: "Fan out work to multiple parallel agents and collect results in a Go Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning worker agents for parallel work, or aggregating results from multiple agents in a Go Golem project."
---

# Parallel Workers — Fan-Out / Fan-In (Go)

## Overview

A single Golem agent admits invocations **sequentially**. Within one invocation, goroutines can overlap I/O by interleaving at yield points; this does not promise CPU-thread parallelism. Distribute work across **multiple agent instances** for independent invocation execution. The Go SDK gives you three building blocks:

1. **Goroutines + `Call`** — make each cross-agent call in its own goroutine and wait for all of them. `Call` / `MustCall` await one call at a time **per goroutine**, and a goroutine blocked in `Call` yields to the component-model event loop while the others proceed, so the calls are in flight together.
2. **`Trigger` + handles or promises** — trigger each worker, then collect each result through the returned `*golem.Invocation[Out]` handle (`AwaitResult`) or through a promise the worker completes. Best for long-running work where you don't want a call in flight the whole time.
3. **`golem.ForkSelf`** — copy the running agent, state included, at the current point; the copy continues as a new agent, with a new agent ID and no key.

> **Platform contract:** a single target instance handles one invocation at a time. Independent invocation execution comes from fanning out to **different** target instances (distinct keys or agent IDs), not from calling the same instance repeatedly. This does not prohibit goroutine concurrency within one invocation.

## Approach 1: concurrent fan-out with goroutines and `Call`

Start one goroutine per worker call and wait for them with a `sync.WaitGroup`. All calls are in flight while you wait.

### Worker definition (`agents/worker/worker.go`)

```go
package worker

import "github.com/golemcloud/golem/sdks/go/golem"

// ID identifies a worker instance — distinct IDs run in parallel.
type ID struct{ Index int64 }

type ProcessIn struct{ Item string }

type RunReportIn struct {
    Region  string
    Promise golem.PromiseID
}

var Agent = golem.DefineAgent[ID](golem.Spec{
    Name:        "WorkerAgent",
    Description: "Processes one item",
})

var Process = Agent.Method[ProcessIn, string]("process", golem.Description("Process a single item and return the result"))
var RunReport = Agent.Method[RunReportIn, golem.Unit]("runReport")
```

### Coordinator fan-out (`agents/coordinator/impl/impl.go`)

```go
package impl

import (
    "sync"

    "myapp/agents/coordinator"
    "myapp/agents/worker"

    "github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = coordinator.Agent.Implement(func(coordinator.ID) *state { return &state{} })

func init() {
    agent.Handle(coordinator.FanOut, func(_ *golem.Context[state], in coordinator.FanOutIn) []string {
        results := make([]string, len(in.Items))
        errs := make([]error, len(in.Items))
        var wg sync.WaitGroup
        for i, item := range in.Items {
            wg.Go(func() {
                // A distinct worker instance per item → they run concurrently.
                c := worker.Agent.Upsert(worker.ID{Index: int64(i)})
                results[i], errs[i] = worker.Process.Call(c, worker.ProcessIn{Item: item})
            })
        }
        wg.Wait() // every call was in flight while this waits
        for _, err := range errs {
            golem.Must0(err)
        }
        return results
    })
}
```

`Call` returns `(Out, error)`; the error is a `*golem.RemoteCallError` when the worker failed or could not be reached. `MustCall` panics with it instead, and a panic traps the component, as in Rust — in a goroutine too, so keep the error and decide in the handler, as above. To keep going when one worker fails, collect the errors instead of panicking. Model *expected* per-item outcomes as a `golem.Result` in the worker method's output, then unwrap or inspect each result:

```go
// worker.Process returns golem.Result[string, string]; in each goroutine:
res, err := worker.Process.Call(c, worker.ProcessIn{Item: item})
switch {
case err != nil:
    failures = append(failures, err.Error())
case res.IsErr():
    failures = append(failures, res.Err())
default:
    successes = append(successes, res.Ok())
}
```

## Approach 2: trigger and collect

For long-running work, `Trigger` each worker (no call held open). `Trigger` returns a `*golem.Invocation[Out]` once the worker accepted the invocation; `AwaitResult()` on each handle collects the results:

```go
handles := make([]*golem.Invocation[string], len(in.Items))
for i, item := range in.Items {
    c := worker.Agent.Upsert(worker.ID{Index: int64(i)})
    handles[i] = worker.Process.MustTrigger(c, worker.ProcessIn{Item: item})
}
results := make([]string, len(handles))
for i, h := range handles {
    results[i] = golem.Must(h.AwaitResult())
}
```

To collect in a later invocation of the coordinator, store each `h.ID()` in state and rebuild the handle with `worker.Process.Invocation(id)`. Dropping a handle only detaches; the worker keeps running.

When the worker reports through something other than its return value, pass each a promise ID instead; the worker completes its promise when done, and the coordinator awaits them all.

### Coordinator

```go
agent.Handle(coordinator.DispatchAndCollect, func(ctx *golem.Context[state], in coordinator.RegionsIn) []string {
    // One promise per worker.
    promises := make([]*golem.Promise[string], len(in.Regions))
    for i := range in.Regions {
        promises[i] = golem.NewPromise[string]()
    }

    // Fire-and-forget: hand each worker its region and its promise id.
    for i, region := range in.Regions {
        c := worker.Agent.Upsert(worker.ID{Index: int64(i)})
        worker.RunReport.MustTrigger(c, worker.RunReportIn{Region: region, Promise: promises[i].ID()})
    }

    // Await all promises (each durably suspends until its worker completes it).
    results := make([]string, len(promises))
    for i, p := range promises {
        results[i] = p.MustAwait()
    }
    return results
})
```

### Worker completes its promise

```go
agent.Handle(worker.RunReport, func(_ *golem.Context[state], in worker.RunReportIn) golem.Unit {
    report := "Report for " + in.Region + ": OK"
    golem.CompletePromise(in.Promise, report)
    return golem.Unit{}
})
```

`Trigger` returns the `*golem.Invocation[Out]` handle (and an error if the invocation was not accepted). It does not wait for the worker; a failure after the invocation is accepted is not reported at the trigger site — the worker signals completion (or failure) through its promise, or the handle's `AwaitResult` reports it.

## Approach 3: `golem.ForkSelf`

`golem.ForkSelf` copies the running agent at the current point. Both copies continue from there: `forked` tells them apart, and `forkedID` is the copy's agent ID on both sides. Synchronize them with a promise:

```go
agent.Handle(coordinator.Compute, func(ctx *golem.Context[state], _ golem.Unit) string {
	result := golem.NewPromise[string]()

	if forked, _ := golem.MustForkSelf(); forked {
		// The copy does its share of the work and hands the result over.
		golem.CompletePromise(result.ID(), "forked-result")
		return "forked done" // seen only by the copy's invocation
	}

	// The original waits for the copy.
	return "combined: original + " + result.MustAwait()
})
```

Fork in a loop for N-way fan-out: each iteration's copy takes slice `i` and returns, while the original awaits every promise. `ForkSelf` returns an error (a `*golem.AgentOperationError`) where the host refuses; `MustForkSelf` panics instead.

## When to use which

| Criteria | Goroutines + `Call` | `Trigger` + handles or promises |
|----------|---------------------|---------------------------------|
| Short calls, want results inline | ✅ Best fit | Works but heavier |
| Long-running work, don't hold a call open | ⚠️ Call stays in flight | ✅ Best fit |
| Need the invocation identity for other work | Not returned by a typed `Call` | `Trigger` returns the handle; `ID()` identifies the invocation |
| Per-item error as a value | Return `golem.Result` in the method | Encode outcome in the promise payload |
| Workers need the current state | Pass it as arguments | Pass it as arguments — or use `golem.ForkSelf`, whose copy inherits it |

## Key Constraints

- **Sequential invocation admission is not a synchronization guarantee.** Goroutines in one invocation can interleave at RPC, HTTP, stream and promise waits; protect shared mutable state as needed. Use distinct worker instances for independent invocation execution; fanning out to the *same* durable instance serializes.
- An awaited `Call` cannot be withdrawn by abandoning its goroutine: Go cannot cancel one call in flight. To withdraw work before it starts, `Trigger` it and call the handle's `Cancel()`, which returns `false` when the invocation already finished and fails with kind `golem.RemoteCannotCancelRunning` while it runs.
- Don't keep a `*golem.Invocation` handle across invocations — store its `ID()` and rebuild it with `Method.Invocation(id)`.
- `Call` and `AwaitResult` return a remote failure as an error, and `Promise.Await` returns an error only for a payload that does not decode; their `Must` forms panic. Model expected outcomes as a `golem.Result` in the method output.
- Avoid a **synchronous** cycle: two agents each blocking in `Call` / `MustCall` on the other deadlocks. Break cycles with `Trigger` / promises.
- Worker instances persist after the coordinator finishes (they are durable by default); give them stable keys you can address again, or make them ephemeral (see `golem-stateless-agent-go`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | The call mechanics (`Call`, `Trigger`) |
| `golem-fire-and-forget-go` | `Trigger` details for fire-and-forget dispatch |
| `golem-wait-for-external-input-go` | Promises used to collect worker results |
| `golem-multi-instance-agent-go` | Address many worker instances by key or agent ID |
