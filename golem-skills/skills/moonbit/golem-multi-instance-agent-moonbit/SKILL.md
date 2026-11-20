---
name: golem-multi-instance-agent-moonbit
description: "Addressing multiple instances of a MoonBit agent by key or agent ID, including agents without a key that share constructor parameters. Use when the user needs multiple independent instances sharing the same constructor parameters, or asks about upsert/create/get_by_id in MoonBit."
---

# Multiple Agent Instances in MoonBit

Every agent has an **agent ID**: an `@schema.AgentId`, an opaque UUID that Golem issues when it creates the agent. An agent can also have a **key**: its agent type and constructor parameters. A keyed agent is unique per key — calling `upsert` with the same parameters always returns the same agent. An agent created **without a key** gets a new agent ID, so you can have many independent instances with identical constructor parameters. Such an agent is reachable only by its agent ID.

## Addresses

On the CLI, a key addresses the keyed agent of the constructor parameters:

```
AgentType(param1, param2)
```

An agent ID addresses one agent, keyed or not:

```
AgentType[a09f61a8-677a-40ea-9ebe-437a0df51749]
a09f61a8-677a-40ea-9ebe-437a0df51749
```

The agent ID does not hold the agent type or the constructor parameters, so it cannot be computed offline: ask a client with `agent_id()`, or read it from agent metadata.

## Creating and Addressing Agents (RPC)

For durable agents, `#derive.agent` generates a `<AgentName>Client` with three constructor methods:

| Method | Description |
|--------|-------------|
| `AgentClient::upsert(params...)` | A client of the **keyed** agent of the parameters; the host creates it when it is missing |
| `AgentClient::create(params..., keyed?, config?, env?)` (async) | Creates a **new** agent and returns after its initialization. Without `keyed=true` it has no key and a new agent ID; with it, it is the keyed agent, and an existing one raises `RpcError::Conflict` |
| `AgentClient::get_by_id(agent_id)` | A client of the **existing** agent with the ID. Never creates the agent; an unknown ID, or the ID of an agent of another type, raises `RpcError::NotFound` |

`client.agent_id()` (async) returns the `@schema.AgentId` of the client's agent. On a client of `upsert` the host creates the agent when it is missing.

### Example

```moonbit
#derive.agent
struct Counter {
  name : String
  mut value : UInt
}

fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

pub fn Counter::increment(self : Counter) -> Unit {
  self.value += 1
}

pub fn Counter::get_value(self : Counter) -> UInt {
  self.value
}

// --- In another agent's async method, using the generated CounterClient: ---

// Keyed: always the same agent for the same name
let counter = CounterClient::upsert("shared")

// New agents without a key: different agents, both with name="shared"
let created1 = CounterClient::create("shared")
let created2 = CounterClient::create("shared")

// Retrieve the agent ID to reconnect later
let id : @schema.AgentId = created1.agent_id()

// Reconnect to an existing agent by its ID
let same_as_created1 = CounterClient::get_by_id(id)

// Persist the agent ID as a string and parse it back later
let saved : String = id.to_string()
let restored = CounterClient::get_by_id(@schema.AgentId::parse(saved))
```

### Scoped Variants

The generated client also includes async scoped versions that automatically call `drop()` when the closure completes. Run these fragments inside an async function and propagate async to callers:

```moonbit
// Scoped keyed agent
CounterClient::scoped_upsert("shared", async fn(client) {
  client.increment()
  client.get_value()
})

// Scoped new agent without a key
CounterClient::scoped_create("shared", async fn(client) {
  client.increment()
  client.get_value()
})

// Scoped existing agent (reconnect)
CounterClient::scoped_get_by_id(agent_id, async fn(client) {
  client.get_value()
})
```

### Config and Environment

If the constructor has a `@config.Config[ConfigType]` parameter with overridable non-secret leaves, the native SDK generator adds a `config? : ConfigTypeOverride` labelled argument to `create` and `scoped_create`, not flattened config fields. Configs without such leaves have no override record or `config?` argument. Construct the generated `ConfigTypeOverride` record with optional leaf values and nested override records, or use its `empty()` constructor. `env?` adds environment variables to the ones the manifest defines. `upsert` takes no overrides: an agent it creates gets its provisioned config.

```moonbit
let client = CounterClient::create("shared", keyed=true, env=[("MODE", "test")])
```

A constructor parameter named `keyed`, `config` or `env` is renamed with a trailing `_` in `create`. External CLI bridge clients use a separate generated API with flattened config options; inspect the selected client's signatures rather than mixing the two families.

## HTTP-Mounted Phantom Agents

When an agent is mounted as an HTTP endpoint, you can set `mount_phantom` to `true` to make every incoming HTTP request create a **new phantom instance** automatically:

```moonbit
#derive.agent
#derive.mount("/api")
#derive.mount_phantom(true)
struct RequestHandler {
  status : String
}

fn RequestHandler::new() -> RequestHandler {
  { status: "ready" }
}

#derive.endpoint(get="/status")
pub fn RequestHandler::handle(self : Self) -> String {
  self.status
}
```

Each HTTP request will be handled by a fresh agent instance with its own agent ID, even though all instances share the same (empty) constructor parameters.

## Key Points

- These recipes describe durable agents. Ephemeral clients have only `upsert` and `scoped_upsert`: each invocation runs on a new one-shot instance, whose agent ID the invocation metadata returns, and it does not reconnect to persistent mutable state.
- The agent ID is a standard UUID that Golem issues; `@schema.AgentId::parse` reads its text form.
- `create` without `keyed=true` makes a new agent on every call. Keep its agent ID if you need to reach it again.
- `upsert` is idempotent: calling it with the same parameters always returns the same keyed agent.
- A keyed agent and an agent without a key with the same constructor parameters are **different agents** — they do not share state.
- Always call `client.drop()` when done, or use the `scoped_*` variants which handle this automatically.
