---
name: golem-multi-instance-agent-go
description: "Addressing multiple instances of a Go agent by identity. Use when the user wants many instances of an agent (one per user/tenant/session), a composite key/identity, or ephemeral/phantom instances in a Go Golem project."
---

# Multiple Instances of a Go Agent

## Overview

An agent *type* has many *instances*, each identified by its **`ID`** (the constructor parameters) and an optional phantom UUID. `Agent.Get(id)` returns a client for the durable instance with that id — **two `Get`s with the same id address the same agent**, created implicitly on first invocation. `Agent.NewPhantom(id)` adds a fresh UUID to the identity; it does not change `Spec.Mode`, which defaults to durable.

## Identity: single or composite

The `ID` struct's fields are the identity — use several fields for a composite key:

```go
package session

import "github.com/golemcloud/golem/sdks/go/golem"

// One session instance per (user, device).
type ID struct {
	User   string
	Device string
}

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "SessionAgent", Description: "Per-user, per-device session"})

var Touch = Agent.Method[golem.Unit, int64]("touch", golem.Description("Record activity, return hit count"))
```

## Addressing a specific instance

```go
// Durable instance for this (user, device) — same id always maps to the same agent.
c := session.Agent.Get(session.ID{User: "alice", Device: "laptop"})
hits := session.Touch.MustCall(c, golem.Unit{})
```

Iterate ids to address many instances:

```go
for _, u := range users {
    c := session.Agent.Get(session.ID{User: u, Device: "web"})
    session.Touch.MustTrigger(c, golem.Unit{}) // fan out
}
```

## Phantom identities and ephemeral mode

`NewPhantom(id)` returns a client with a **fresh phantom UUID fixed for that client**. The session definition above is durable, so its phantom instance retains state and can be addressed again. Ephemeral types instead set `Mode: golem.Ephemeral`; they require phantom addressing. Create a **new client with `NewPhantom` for each independent ephemeral invocation**. Reusing a client reuses its UUID; a terminal ephemeral identity cannot be invoked again or bound as a reusable durable identity.

```go
c := session.Agent.NewPhantom(session.ID{User: "alice", Device: "kiosk"})
session.Touch.MustCall(c, golem.Unit{})

// To reach the SAME phantom again later, capture and reuse its id:
if pid, ok := c.PhantomID().Get(); ok {      // PhantomID() is Option[UUID]
	c2 := session.Agent.Get(session.ID{User: "alice", Device: "kiosk"},
		golem.WithPhantomID(pid))             // same phantom instance
	_ = c2
}
```

## Key Constraints

- The `ID` fields plus the optional phantom UUID define identity: the same full identity addresses the same durable agent. There is no separate "create" step.
- Instances are **durable by default**, including phantoms. Set `Spec.Mode` separately for ephemeral execution. Capture `Client.PhantomID()` to re-address a durable phantom with `WithPhantomID`; do not treat an ephemeral invocation's ID as a reusable durable identity.
- For an ephemeral type, call `NewPhantom(id)` again before each independent invocation, not once outside a loop of calls.
- Over HTTP, each `ID` field must appear as a `{var}` in the mount path so a URL selects the instance (see `golem-add-http-endpoint-go`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent type and its `ID` |
| `golem-call-another-agent-go` | Call a specific instance from another agent |
| `golem-add-http-endpoint-go` | Select an instance from a request URL |
