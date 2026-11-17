---
name: golem-call-another-agent-moonbit
description: "Calling another agent and awaiting the result in a MoonBit Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (MoonBit)

## Overview

The `#derive.agent` code generation tool auto-generates a `<AgentName>Client` struct for each agent, enabling agent-to-agent communication via RPC. An awaited call blocks the calling agent until the target agent returns a result.

## Getting a Client (Scoped)

Use `<AgentName>Client::scoped_upsert(...)` with the target agent's constructor parameters and an async
callback. Call it from an `async fn`; MoonBit has no `await` keyword. The client is automatically
dropped when the callback returns:

```moonbit
CounterClient::scoped_upsert("my-counter", async fn(counter) {
  counter.increment()
  counter.increment()
  let value = counter.get_value()
  value
})
```

This is the **recommended** pattern — it ensures the client resource is cleaned up automatically.

Note: agents with no constructor parameters omit the parameter from `scoped_upsert` / `upsert`.

## Getting a Client (Manual)

Use `<AgentName>Client::upsert(...)` for manual lifecycle management. You **must** call `client.drop()` when done:

```moonbit
let counter = CounterClient::upsert("my-counter")
counter.increment()
let value = counter.get_value()
counter.drop()  // must call drop when done
```

Both address the **keyed** agent of the constructor parameters. The host creates the agent when it is missing — on the first invocation, or on `agent_id()` — and reuses it otherwise.

A durable agent's client also has:

- `<AgentName>Client::create(params..., keyed?, config?, env?)` (async): creates a new agent and returns after its initialization. Without `keyed=true` the new agent has no key and a new agent ID; with it, it is the keyed agent, and an existing one raises `RpcError::Conflict`.
- `<AgentName>Client::get_by_id(agent_id)`: addresses the existing agent with an `@schema.AgentId`. It never creates the agent; an unknown ID raises `RpcError::NotFound`.
- `client.agent_id()` (async): the `@schema.AgentId` of the client's agent.
- `scoped_create(...)` and `scoped_get_by_id(...)`, the scoped forms of `create` and `get_by_id`.

An ephemeral agent's client has only `upsert` and `scoped_upsert`; every invocation runs on a new one-shot instance.

## Awaited Call

Call a method and block until the result returns:

```moonbit
CounterClient::scoped_upsert("my-counter", async fn(counter) {
  counter.increment()
  let count = counter.get_value()
  count
})
```

The calling agent **blocks** until the target agent processes the request and returns. This is the standard RPC pattern.

## Passing Complex Types

Agent methods accept custom types defined in your agent code:

```moonbit
TaskManagerClient::scoped_upsert(async fn(tm) {
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

## Agents Without a Key

To create multiple distinct instances with the same constructor parameters, use `create` without `keyed`: each call makes a new agent with a new agent ID. See the `golem-multi-instance-agent-moonbit` skill.

## Cross-Component RPC

MoonBit does **not** currently support putting an agent definition in a shared package and importing its generated client from two separate components. `#derive.agent` describes the concrete implementation type, and `golem_sdk_tools agents` emits the registration, dispatch, and client code together in the component package.

Use an internal guest bridge instead. The provider must build before the bridge exists. This
example uses a TypeScript provider `example:weather` containing `WeatherAgent` and a MoonBit caller:

```yaml
components:
  example:weather:
    dir: weather
    templates: ts
  example:caller:
    dir: caller
    templates: moonbit
    dependencies:
      agents:
        - example:weather/WeatherAgent
```

`golem build` generates `golem-temp/bridge-sdk/moonbit/internal/weather-agent-guest-client`. The
generated module contains `moon.mod` and `moon.work`; do not edit these generated files.

Add the generated module directory to the application's `moon.work`, preserving any existing
local SDK member:

```moonbit
members = [
  ".",
  "golem-temp/bridge-sdk/moonbit/internal/weather-agent-guest-client",
]
```

Add `"weather-agent-guest-client@0.0.1"` to the imports in the application's `moon.mod`. The
workspace resolves that module locally. The stock Golem MoonBit template uses workspace artifact
paths based on the application name, so the caller needs no custom build or embed commands.
Component namespaces do not need to match the application name.

For a MoonBit provider, use an independently buildable module and workspace. A provider in the
caller's workspace cannot resolve a bridge member that has not been generated yet.

Import the generated client package from the caller's `moon.pkg`:

```moonbit
import {
  "weather-agent-guest-client/client" @weather,
  // ...the caller's Golem SDK imports
}
```

Then call the provider through the generated typed client:

```moonbit
pub async fn ExampleAgent::weather_in_london(self : Self) -> String {
  let _ = self
  @weather.WeatherAgentClient::scoped(
    "London",
    async fn(remote) { remote.current_weather() },
  )
}
```

`scoped` addresses the keyed agent through `upsert` and drops the client afterwards. The generated client also has `upsert`, and for a durable provider `create` (async, with `keyed?`, one labelled optional per local config value, and `env?`), `get_by_id(agent_id)` and `agent_id()`.

This client is generated from the built provider's discovered schema before the caller is compiled. It uses the guest RPC host API directly, not the external REST bridge transport. Do not import the provider component's executable MoonBit package into the caller.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `trigger_` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-moonbit` skill.
