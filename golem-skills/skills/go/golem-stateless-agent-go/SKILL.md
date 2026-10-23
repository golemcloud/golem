---
name: golem-stateless-agent-go
description: "Creating ephemeral (stateless) agents in a Go Golem project. Use when the user wants a stateless agent, a fresh instance per invocation, no shared state between calls, or a request-handler style agent in a Go Golem project."
---

# Creating Ephemeral (Stateless) Agents (Go)

## Overview

An **ephemeral agent** gets a **fresh instance for every invocation**. Unlike the default durable agents, an ephemeral agent:

- **Has no shared state** — each invocation starts from a clean init function; values set in one call are gone by the next.
- **Has no method recovery replay** — an oplog is still recorded lazily (inspectable via `golem agent oplog`). If the executor restarts while preparing an invocation, Golem can replay the recorded initialization so that the method can still start. Once the method starts, a failure is terminal; Golem does not replay the method.
- **Discards its memory** after each invocation completes.
- **Requires phantom addressing** — constructor parameters (`ID`) initialize each invocation, but do not name a reusable durable instance. Create a new `Agent.NewPhantom(id)` client for each independent invocation; an ephemeral invocation's resulting agent ID cannot be passed to `Agent.Bind`.

This fits **pure request handlers**, **stateless transformers**, **adapters**, and **serverless-style functions** where each call is independent.

## How to make an agent ephemeral

Set `Mode: golem.Ephemeral` on the `Spec`. `Mode` defaults to `golem.Durable`.

```go
// Package handler is the DEFINITION of a stateless request-handler agent.
package handler

import "github.com/golemcloud/golem/sdks/go/golem"

// No constructor inputs; each ephemeral invocation still gets fresh state.
type ID struct{}

type HandleIn struct{ Input string }

var Agent = golem.DefineAgent[ID](golem.Spec{
    Name:        "RequestHandlerAgent",
    Description: "Stateless request handler — fresh instance per invocation",
    Mode:        golem.Ephemeral, // <- makes every invocation start clean
})

var Handle = Agent.Method[HandleIn, string]("handle", golem.Description("Process one request and return the result"))
```

### Implementation

The implementation is written exactly like a durable agent's — the `state` type just holds nothing meaningful across calls (or is empty), because it is recreated each invocation:

```go
package impl

import (
    "strings"

    "myapp/agents/handler"

    "github.com/golemcloud/golem/sdks/go/golem"
)

// state is empty: an ephemeral agent keeps nothing between invocations.
type state struct{}

var agent = handler.Agent.Implement(func(handler.ID) *state { return &state{} })

func init() {
    agent.Handle(handler.Handle, func(_ *golem.Context[state], in handler.HandleIn) string {
        return "processed: " + strings.ToUpper(in.Input)
    })
}
```

## What "fresh instance per invocation" means

`Mode` controls state lifetime; client addressing must also use `NewPhantom` for an ephemeral target.

`NewPhantom` chooses its UUID once per client, not once per call. Do not reuse that client for independent invocations: a terminal ephemeral identity cannot be invoked again. For the handler defined above:

```go
for _, input := range inputs {
    c := handler.Agent.NewPhantom(handler.ID{})
    result := handler.Handle.MustCall(c, handler.HandleIn{Input: input})
    _ = result
}
```

```go
// DURABLE (default) — state accumulates across calls.
var Durable = golem.DefineAgent[ID](golem.Spec{Name: "DurableCounter"})
// increment() three times → 1, 2, 3

// EPHEMERAL — state resets every call.
var Ephemeral = golem.DefineAgent[ID](golem.Spec{Name: "EphemeralCounter", Mode: golem.Ephemeral})
// increment() with three fresh NewPhantom clients → 1, 1, 1
```

Each invocation of an ephemeral agent:
1. Runs the init function (`func(ID) *state`) to build a fresh state.
2. Executes the method handler.
3. Discards the instance entirely.

## Combining with HTTP endpoints

Ephemeral agents are a natural fit for HTTP request handlers — declare an HTTP mount alongside `Mode: golem.Ephemeral`. Every mount `{var}` must be an `ID` field, so a singleton `ID struct{}` mounts at a path with no variables:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
    Name: "ConverterAgent",
    Mode: golem.Ephemeral,
    HTTP: &golem.Mount{Path: "/api/convert"},
})

var (
    ToUpper = Agent.Method[ConvertIn, string]("toUpper", golem.HTTP(golem.POST("/to-upper")))
    ToLower = Agent.Method[ConvertIn, string]("toLower", golem.HTTP(golem.POST("/to-lower")))
)
```

## When to use ephemeral agents

| Use case | Why ephemeral |
|----------|---------------|
| Stateless HTTP API (REST adapter, proxy) | No state to persist between requests |
| Data transformation / format conversion | Pure function — input in, output out |
| Validation service | Each validation is independent |
| Webhook receiver that forwards events | No need to remember previous events |
| Stateless computation (math, encoding) | No side effects worth persisting |

## When NOT to use ephemeral agents

- **Counters, accumulators, shopping carts** — need state across calls → use durable (default).
- **Workflow orchestrators, sagas** — need the oplog for recovery → use durable (default).
- **Agents where at-least-once semantics on external effects matter** → use durable (default).
- **Any agent where one call's result depends on a previous call** → use durable (default).

## Key Constraints

- Ephemeral mode is set at the **agent type level** (`Spec.Mode`) — all instances of the type are ephemeral.
- Constructor parameters initialize each fresh invocation; they do not create reusable durable identities. Use `NewPhantom(id)`, not plain `Get(id)` or `Bind(agentID)`, for an ephemeral target.
- Ephemeral agents can still call other agents (RPC), make HTTP requests, and use the rest of the SDK — they do not retain state across independent invocations or recover methods by replay.
- The oplog is recorded lazily (inspectable via `golem agent oplog`). If the executor restarts while preparing an invocation, Golem can replay the recorded initialization so that the method can still start. Once the method starts, a failure is terminal; Golem does not replay the method.
- Snapshot-based recovery does not apply to ephemeral agents; leave `Spec.Snapshot` at its default.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent and its methods (durable baseline) |
| `golem-add-http-endpoint-go` | Expose the stateless agent over HTTP |
| `golem-call-another-agent-go` | An ephemeral agent calling other agents |
| `golem-configure-durability-go` | Durability controls for the durable alternative |
