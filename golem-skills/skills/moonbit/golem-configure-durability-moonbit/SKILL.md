---
name: golem-configure-durability-moonbit
description: "Choosing between durable and ephemeral agents in a MoonBit Golem project. Use when the user asks to change durability mode, switch between durable and ephemeral, or configure persistence behavior."
---

# Configuring Agent Durability (MoonBit)

## Durable Agents (Default)

By default, all Golem agents are **durable**:

- State persists across invocations, failures, and restarts
- Every side effect is recorded in an **oplog** (operation log)
- On failure, the agent is transparently recovered by replaying the oplog
- No special code needed — durability is automatic

A standard durable agent:

```moonbit
#derive.agent
struct CounterAgent {
  name: String
  mut count: UInt
}

fn CounterAgent::new(name: String) -> CounterAgent {
  { name, count: 0 }
}

pub fn CounterAgent::increment(self: Self) -> UInt {
  self.count = self.count + 1
  self.count
}

pub fn CounterAgent::get_count(self: Self) -> UInt {
  self.count
}
```

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is not needed:

- State is discarded after each invocation completes
- No oplog replay — lower overhead (an oplog is still recorded lazily for debugging via `golem agent oplog`, but never replayed)
- Each invocation calls `new()` to create a fresh instance, executes the method, then discards the instance
- Useful for pure functions, request handlers, or adapters

```moonbit
#derive.agent("ephemeral")
struct StatelessHandler {
}

fn StatelessHandler::new() -> StatelessHandler {
  {  }
}

pub fn StatelessHandler::handle(self: Self, input: String) -> String {
  "processed: " + input
}
```

## When to Choose Which

| Use Case | Mode |
|----------|------|
| Counter, shopping cart, workflow orchestrator | **Durable** (default) |
| Stateless request processor, transformer | **Ephemeral** |
| Long-running saga or multi-step pipeline | **Durable** (default) |
| Pure computation, no side effects worth persisting | **Ephemeral** |
| Agent that calls external APIs with at-least-once semantics | **Durable** (default) |

When in doubt, use the default (durable). Ephemeral mode is an optimization for agents that genuinely don't need persistence.

## Switching Between Modes

To make a durable agent ephemeral, change:

```moonbit
#derive.agent
struct MyAgent {
```

to:

```moonbit
#derive.agent("ephemeral")
struct MyAgent {
```

To make an ephemeral agent durable, change `#derive.agent("ephemeral")` back to `#derive.agent`.

After changing the annotation, rebuild with `golem build` to regenerate derived files. **Never edit generated files** — `golem_reexports.mbt`, `golem_agents.mbt`, and `golem_derive.mbt` are auto-generated.
