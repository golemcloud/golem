---
name: golem-add-agent-go
description: "Adding a new Go agent to a Golem component. Use when the user asks to create, add, or define a new agent type, split an agent into definition/implementation, or add agent methods in a Go Golem project."
---

# Adding a New Agent to a Go Golem Component

## Overview

An **agent** is a durable, stateful unit of computation in Golem. In Go each agent is split across two packages:

- a state-free **definition** (`package <name>`) — the agent's identity, its method descriptors, and the input/output types. Other agents import this package to call it.
- an **implementation** (`package impl`, in a nested `impl/` subpackage) — the private state and the method handlers. Importing this package registers the agent with the SDK.

This split lets agents call one another without a Go import cycle, and keeps state private to the implementation.

## Steps

1. **Create the definition** — add `agents/<name>/<name>.go` with `package <name>`.
2. **Declare identity + types** — an `ID` struct (its fields are the constructor parameters), plus any input/output structs.
3. **Define the agent and its methods** — `golem.DefineAgent` + one `golem.DefineMethod` descriptor per method.
4. **Create the implementation** — add `agents/<name>/impl/impl.go` with `package impl`: a private `state`, `golem.Implement`, and one `golem.Handle` per method.
5. **Wire it in** — blank-import the impl package in `main.go`.
6. **Build** — run `golem build`.

## Definition (`agents/counter/counter.go`)

```go
// Package counter is the DEFINITION of the counter agent.
package counter

import "github.com/golemcloud/golem/sdks/go/golem"

// ID holds the constructor parameters, and is the type-level identity used by
// cross-agent calls. Two agents with the same ID are the same agent.
type ID struct{ Name string }

// AddIn is a method's parameter list: one WIT parameter per exported field.
type AddIn struct{ By int64 }

// Agent is the state-free definition. Mode defaults to Durable.
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "CounterAgent",
	Description: "A simple durable counter",
})

// Method descriptors are package-level vars: the same value drives the published
// schema, the implementation binding, and calls from other agents.
var (
	Increment = golem.DefineMethod[ID, golem.Unit, int64]("increment", golem.Desc("Increase the count by one"))
	Add       = golem.DefineMethod[ID, AddIn, int64]("add", golem.Desc("Add to the count"))
	Value     = golem.DefineMethod[ID, golem.Unit, int64]("value", golem.Desc("Return the current value"))
)
```

`golem.Unit` is the empty parameter/result placeholder (a method with no input or no output).

## Implementation (`agents/counter/impl/impl.go`)

```go
// Package impl is the IMPLEMENTATION of the counter agent.
package impl

import (
	"myapp/agents/counter"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// state is the agent's private, durable state — invisible to callers.
type state struct{ value int64 }

func (s *state) current() int64 { return s.value }

// Implement binds the constructor and returns a handle; Handle registers each
// method on it.
var agent = golem.Implement(counter.Agent, func(counter.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, counter.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	})
	golem.Handle(agent, counter.Add, func(ctx *golem.Context[state], in counter.AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	})
	// A handler can also be an ordinary Go method, bound with a method expression.
	golem.Handle(agent, counter.Value, golem.Bind0((*state).current))
}
```

The handler receives `*golem.Context[state]`; `ctx.State` is your `*state`. `golem.Bind0` adapts a no-input `(*state)` method into a handler; `golem.Bind` adapts a one-input method.

## Wiring: `main.go`

`main.go` (`package main`) blank-imports each agent's **impl** package so its registration runs on import, plus the SDK runtime:

```go
package main

import (
	_ "myapp/agents/counter/impl"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}
```

Adding another agent = another `agents/<name>/` folder + one more blank import here.

## Custom Types

Parameter and result types are Go structs; each exported field becomes one WIT field, named by lower-camel-casing the Go name (`AmountCents` → `amountCents`).

- Use sized integers (`int64`, `uint64`, …). Bare `int`/`uint` are **rejected** — their width is platform-dependent.
- A `*T` field means `option<T>`. A nil slice/map is an **empty** list/map, never "absent" — use `*[]T` if you must distinguish.
- Variants/enums are declared with `golem.DefineVariant` / `golem.DefineEnum`.

## Returning Failures

Handlers return only their output value. Signal failure two ways:

- **Uncaught errors** — a `panic` (e.g. from `golem.Must(...)` on an unexpected error) is treated as a crash: the invocation is retried per the agent's retry policy, and if retries are exhausted the agent becomes failed. The caller does **not** see it as a normal result.
- **Domain errors** the caller should observe as a value — model them in the method's output type with `golem.Result[Ok, Err]`:

```go
// definition
var Record = golem.DefineMethod[ID, RecordIn, golem.Result[int64, string]]("record")

// handler
golem.Handle(agent, Record, func(ctx *golem.Context[state], in RecordIn) golem.Result[int64, string] {
	if in.Amount < 0 {
		return golem.Err[int64, string]("cannot record a negative amount")
	}
	ctx.State.total += in.Amount
	return golem.Ok[int64, string](ctx.State.total)
})
```

Returning `golem.Err(...)` completes the invocation successfully — the caller receives the error as a value (and can unwrap with `.MustOk()`). Panicking instead triggers a retry and can fail the whole agent.

## Key Constraints

- Method parameters/results are passed by value; wrap the parameter list in a struct (one field per WIT parameter).
- `ID` must be a struct; its fields are the agent identity. Agents are created implicitly on first invocation. An `ID` with no fields (`type ID struct{}`) is a **singleton** — exactly one instance of the agent.
- Invocations run **sequentially in a single thread** — no concurrency within one agent, no locks needed.
- Method descriptors must be **package-level vars** in the definition package (the same value is used by the schema, the binding, and callers).
- State lives only in the impl package (unexported) — callers never see it.
- Target is **WASM only**: no raw sockets; outgoing HTTP goes through the SDK's WASI transport (see `golem-make-http-request-go`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | This agent needs to call another agent (RPC) |
| `golem-add-http-endpoint-go` | Expose this agent's methods over HTTP |
| `golem-multi-instance-agent-go` | Address many instances of an agent by ID |
| `golem-custom-snapshot-go` | Customize how the agent's state is snapshotted |
