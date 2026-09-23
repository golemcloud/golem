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

MoonBit does **not** currently support putting an agent definition in a shared package and importing its generated client from two separate components. `#derive.agent` describes the concrete implementation type, and `golem_sdk_tools agents` emits the registration, dispatch, and client code together in the component package.

Use an internal guest bridge instead. Given a provider component `example:weather` containing `WeatherAgent`, declare the caller's dependency in `golem.yaml`:

```yaml
components:
  example:weather:
    dir: weather
    templates: moonbit
  example:caller:
    dir: caller
    templates: moonbit
    dependencies:
      agents:
        - example:weather/WeatherAgent
```

`golem build` generates `golem-temp/bridge-sdk/moonbit/internal/weather-agent-guest-client`. Add it as a local module dependency in the application's `moon.mod.json`:

```json
{
  "name": "example/weather-app",
  "preferred-target": "wasm",
  "deps": {
    "golemcloud/golem_sdk": "0.5.1",
    "weather-agent-guest-client": {
      "path": "golem-temp/bridge-sdk/moonbit/internal/weather-agent-guest-client"
    }
  }
}
```

Import its client package from the caller's `moon.pkg`:

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

This client is generated from the built provider's discovered schema before the caller is compiled. It uses the guest RPC host API directly, not the external REST bridge transport. Do not import the provider component's executable MoonBit package into the caller.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `trigger_` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-moonbit` skill.
