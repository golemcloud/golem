---
name: golem-call-another-agent-moonbit
description: "Calling another agent and awaiting the result in a MoonBit Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (MoonBit)

## Overview

The `#derive.agent` code generation tool auto-generates a `<AgentName>Client` struct for each agent, enabling agent-to-agent communication via RPC. An awaited call blocks the calling agent until the target agent returns a result.

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

Note: agents with no constructor parameters omit the parameter from `scoped` / `get`.

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

## Passing Complex Types

Agent methods accept custom types defined in your agent code:

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

## Phantom Agents

To create multiple distinct instances with the same constructor parameters, use phantom agents. See the `golem-multi-instance-agent-moonbit` skill.

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `trigger_` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-moonbit` skill.
