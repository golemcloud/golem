---
name: golem-multi-instance-agent-go
description: "Addressing multiple instances of a Go agent by identity. Use when the user wants many instances of an agent (one per user/tenant/session), a composite key/identity, or ephemeral/phantom instances in a Go Golem project."
---

# Multiple Instances of a Go Agent

## Overview

An agent *type* has many *instances*, each identified by its **`ID`** (the constructor parameters). `Agent.Get(id)` returns a client for the durable instance with that id — **two `Get`s with the same id address the same agent**, created implicitly on first invocation. For one-off/ephemeral work use `Agent.NewPhantom(id)`, which allocates a fresh, non-persistent instance.

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

var Touch = golem.DefineMethod[ID, golem.Unit, int64]("touch", golem.Desc("Record activity, return hit count"))
```

## Addressing a specific instance

```go
// Durable instance for this (user, device) — same id always maps to the same agent.
c := session.Agent.Get(session.ID{User: "alice", Device: "laptop"})
hits := session.Touch.Call(c, golem.Unit{})
```

Iterate ids to address many instances:

```go
for _, u := range users {
    c := session.Agent.Get(session.ID{User: u, Device: "web"})
    session.Touch.Trigger(c, golem.Unit{}) // fan out
}
```

## Ephemeral (phantom) instances

`NewPhantom(id)` allocates a **fresh** instance with no durable identity — the right choice for throwaway/per-request work, and the only way to get a client for an ephemeral agent:

```go
c := session.Agent.NewPhantom(session.ID{User: "alice", Device: "kiosk"})
session.Touch.Call(c, golem.Unit{})

// To reach the SAME phantom again later, capture and reuse its id:
if pid, ok := c.PhantomID().Get(); ok {      // PhantomID() is Option[UUID]
	c2 := session.Agent.Get(session.ID{User: "alice", Device: "kiosk"},
		golem.WithPhantomID(pid))             // same phantom instance
	_ = c2
}
```

## Key Constraints

- The `ID` fields *are* the identity: same values → same agent; different values → different agent. There is no separate "create" step.
- Instances are **durable by default**; use `NewPhantom` for ephemeral ones. A phantom's id lives only on its client — capture `Client.PhantomID()` if you need to re-address it.
- Over HTTP, each `ID` field must appear as a `{var}` in the mount path so a URL selects the instance (see `golem-add-http-endpoint-go`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent type and its `ID` |
| `golem-call-another-agent-go` | Call a specific instance from another agent |
| `golem-add-http-endpoint-go` | Select an instance from a request URL |
