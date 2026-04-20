---
name: golem-call-another-agent-moonbit
description: "Calling another MoonBit Golem agent and awaiting the result (RPC). Use when the user asks to invoke, call, or communicate with another agent from within agent code."
---

# Calling Another Agent (MoonBit)

## Overview

The `agents` code generation tool auto-generates a `<AgentName>Client` struct for each agent, enabling agent-to-agent communication via RPC. Each method on the client has three variants:

- `client.method(args)` — awaited call (blocks until result)
- `client.trigger_method(args)` — fire-and-forget (returns immediately)
- `client.schedule_method(scheduled_at, args)` — scheduled invocation at a future time

## Getting a Client (Scoped)

Use `<AgentName>Client::scoped(...)` with the target agent's constructor parameters and a callback. The client is automatically dropped when the callback returns:

```moonbit
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.increment()
  counter.increment()
  let value = counter.get_value()
  value
})
```

This is the **recommended** pattern — it ensures the client resource is cleaned up automatically.

## Getting a Client (Manual)

Use `<AgentName>Client::get(...)` for manual lifecycle management. You **must** call `client.drop()` when done:

```moonbit
let counter = CounterClient::get("my-counter")
counter.increment()
let value = counter.get_value()
counter.drop()  // must call drop when done
```

This does **not** create the agent — the agent is created implicitly on its first invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and block until the result returns:

```moonbit
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.increment()
  let count = counter.get_value()
  count
})
```

The calling agent **blocks** until the target agent processes the request and returns. This is the standard RPC pattern.

## Fire-and-Forget

Use `trigger_` prefixed methods to invoke without waiting for the result:

```moonbit
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.trigger_increment()
})
```

The call returns immediately. Use this to break RPC cycles or start background work.

## Scheduled Call

Use `schedule_` prefixed methods to invoke at a future time:

```moonbit
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.schedule_increment(scheduled_at)
})
```

## Passing Complex Types

Agent methods accept custom types defined in your WIT interfaces:

```moonbit
TaskManagerClient::scoped(fn(tm) raise @common.AgentError {
  let count = tm.add_task({
    title: "Build RPC support",
    priority: High,
    description: Some("Implement agent-to-agent communication"),
  })
  let high_tasks = tm.get_by_priority(High)
  let _ = high_tasks
  count
})
```

Note: agents with no constructor parameters omit the parameter from `scoped` / `get`.

## Phantom Agents

To create multiple distinct instances with the same constructor parameters, use phantom agents:

```moonbit
let phantom = CounterClient::new_phantom("my-counter")
let id = phantom.phantom_id()
// Later, reconnect to the same phantom:
let same = CounterClient::get_phantom("my-counter", id.unwrap())
```

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `trigger_` (fire-and-forget) to break cycles:

```moonbit
// WRONG: Agent A calls B, Agent B calls A — deadlock!
// Agent A:
BClient::scoped("b", fn(b) raise @common.AgentError { b.do_work() })
// Agent B:
AClient::scoped("a", fn(a) raise @common.AgentError { a.do_work() })

// CORRECT: Break the cycle with trigger_
// Agent B:
AClient::scoped("a", fn(a) raise @common.AgentError { a.trigger_do_work() })
```
