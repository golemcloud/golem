---
name: golem-multi-instance-agent-moonbit
description: "Creating multiple agent instances with the same constructor parameters using phantom agents in MoonBit. Use when the user needs multiple independent instances sharing the same identity parameters."
---

# Phantom Agents in MoonBit

Phantom agents allow creating **multiple distinct agent instances** that share the same constructor parameters. Normally, an agent is uniquely identified by its constructor parameter values — calling `get` with the same parameters always returns the same agent. Phantom agents add an extra **phantom ID** (a UUID) to the identity, so you can have many independent instances with identical parameters.

## Agent ID Format

A phantom agent's ID appends the phantom UUID in square brackets:

```
agent-type(param1, param2)[a09f61a8-677a-40ea-9ebe-437a0df51749]
```

A non-phantom agent ID has no bracket suffix:

```
agent-type(param1, param2)
```

## Creating and Addressing Phantom Agents (RPC)

For durable agents, `#derive.agent` generates a `<AgentName>Client` with three constructor methods:

| Method | Description |
|--------|-------------|
| `AgentClient::get(params...)` | Get or create a **non-phantom** agent identified solely by its parameters |
| `AgentClient::new_phantom(params...)` | Create a **new phantom** agent with a freshly generated random UUID |
| `AgentClient::get_phantom(params..., phantom_id)` | Get or create a phantom agent with a **specific** UUID |

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

// --- In another agent, using the generated CounterClient: ---

// Non-phantom: always the same agent for the same name
let counter = CounterClient::get("shared")

// New phantom: creates a brand new independent instance
let phantom1 = CounterClient::new_phantom("shared")
let phantom2 = CounterClient::new_phantom("shared")
// phantom1 and phantom2 are different agents, both with name="shared"

// Retrieve the phantom ID to reconnect later
let id : @schema.Uuid? = phantom1.phantom_id()

// Reconnect to an existing phantom by its UUID
let same_as_phantom1 = CounterClient::get_phantom("shared", id.unwrap())

// Persist the UUID as a string and parse it back later
let saved : String = id.unwrap().to_string()
let restored = CounterClient::get_phantom("shared", @schema.Uuid::parse(saved))
```

### Scoped Variants

The generated client also includes async scoped versions that automatically call `drop()` when the closure completes. Run these fragments inside an async function and propagate async to callers:

```moonbit
// Scoped non-phantom
CounterClient::scoped("shared", async fn(client) {
  client.increment()
  client.get_value()
})

// Scoped new phantom
CounterClient::scoped_new_phantom("shared", async fn(client) {
  client.increment()
  client.get_value()
})

// Scoped get phantom (reconnect)
CounterClient::scoped_get_phantom("shared", phantom_id, async fn(client) {
  client.get_value()
})
```

### WithConfig Variants

If the constructor has a `@config.Config[ConfigType]` parameter with overridable non-secret leaves, the native SDK generator emits methods accepting one `ConfigTypeOverride` record, not flattened config fields. Configs without such leaves have no override record or with-config methods:

- `AgentClient::get_with_config(params..., config_override)`
- `AgentClient::new_phantom_with_config(params..., config_override)`
- `AgentClient::get_phantom_with_config(params..., phantom_id, config_override)`

Construct the generated `ConfigTypeOverride` record with optional leaf values and nested override records, or use its `empty()` constructor, and pass it as the last argument. External CLI bridge clients use a separate generated API with flattened config options; inspect the selected client's signatures rather than mixing the two families.

## Querying the Phantom ID

### From a Client Reference

Use the `phantom_id()` method on any client instance:

```moonbit
let phantom = CounterClient::new_phantom("shared")
let id : @schema.Uuid? = phantom.phantom_id()
// Returns Some(uuid) for phantom agents, None for non-phantom agents
```

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

Each HTTP request will be handled by a fresh agent instance with its own phantom ID, even though all instances share the same (empty) constructor parameters.

## Key Points

- These recipes describe durable agents. Ephemeral clients have no `get`, `scoped`, or `phantom_id` API: use `new_phantom`/`get_phantom` and their scoped variants. An ephemeral client is a logical proxy; each call runs in a fresh execution and does not reconnect to persistent mutable state.
- The phantom ID is a standard UUID.
- `new_phantom` generates the UUID internally.
- `get_phantom` is idempotent: calling it with the same UUID and parameters always returns the same agent.
- Phantom and non-phantom agents with the same constructor parameters are **different agents** — they do not share state.
- Always call `client.drop()` when done, or use the `scoped_*` variants which handle this automatically.
