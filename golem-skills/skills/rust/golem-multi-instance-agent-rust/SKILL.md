---
name: golem-multi-instance-agent-rust
description: "Creating multiple agent instances with the same constructor parameters in Rust. Use when the user needs multiple distinct agents sharing constructor values, agents without a key, the generated client's create/get_by_id/agent_id methods, the phantom_agent HTTP option, or multi-instance agents in Rust."
---

# Multiple Agents With the Same Constructor Parameters in Rust

Every agent has an **agent ID**: a UUID that Golem issues. An agent can also have a **key**, the
canonical `AgentType(params...)` text of its constructor parameters. Only one **keyed** agent exists
for each key, so calling `upsert` with the same parameters always reaches the same agent. To have
many independent agents with identical parameters, create agents **without a key**: each one gets
a new agent ID, and only that ID reaches it.

The persistent identity and reconnect behavior below applies to durable agents only. An ephemeral
agent type has only `upsert`: its client is a logical proxy, and each invocation runs on a new
one-shot instance. A completed execution is terminal, not a reusable stateful instance.

## Addressing Agents From the CLI

```shell
# The keyed agent of the parameters (created on first invocation when missing)
golem agent invoke 'Counter("shared")' increment

# A new agent without a key; prints its agent ID
golem agent create 'Counter("shared")'

# An agent by its agent ID
golem agent invoke 'Counter[<AGENT_ID>]' increment
golem agent invoke '<AGENT_ID>' increment
```

## Creating and Addressing Agents (RPC)

The `#[agent_definition]` macro generates a `<AgentName>Client` with these constructor methods for
a durable agent type:

| Method | Description |
|--------|-------------|
| `upsert(params...)` | Connect to the **keyed** agent of the parameters, and create it when it is missing |
| `create(params..., options)` | `async`: create an agent and return after its initialization. Without `keyed`, the new agent has no key and a new agent ID |
| `get_by_id(&agent_id)` | Connect to the existing agent with this ID. Never creates the agent |
| `agent_id()` | `async`: the agent ID of the agent the client connects to |

`create` takes a `golem_rust::CreateOptions`:

- `keyed: true` creates the keyed agent of the parameters instead, and fails with
  `GolemReflectError::Conflict` when it already exists.
- `config` overrides the agent's configuration (see below).
- `env` adds environment variables to the ones the manifest defines.

`get_by_id` fails with `GolemReflectError::RemoteNotFound` when no agent of this type has the ID.

### Example

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait Counter {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get_value(&self) -> u32;
}

// --- In another agent, using the generated CounterClient: ---

// Keyed: always the same agent for the same name
let counter = CounterClient::upsert("shared".to_string());

// New agents without a key: each call creates a brand new independent instance
let first = CounterClient::create("shared".to_string(), Default::default()).await?;
let second = CounterClient::create("shared".to_string(), Default::default()).await?;
// first and second are different agents, both with name="shared"

// Retrieve the agent ID to reconnect later
let id: golem_rust::AgentId = first.agent_id().await?;

// Reconnect to the existing agent by its ID
let same_as_first = CounterClient::get_by_id(&id)?;
```

`create`, `get_by_id` and `agent_id` return `Result<_, golem_rust::GolemReflectError>`, so call
them from a method that can propagate or handle that error.

### Configuration and Environment Overrides

If the constructor injects `#[agent_config] config: Config<MyAgentConfig>`, the `config` field of
`CreateOptions` is the generated `MyAgentConfigRpc` override record (a tuple of records when the
constructor injects more than one config parameter), not flattened config fields. Without injected
config, `config` is `()`. `upsert` takes no overrides.

```rust
let agent = MyAgentClient::create(
    name,
    golem_rust::CreateOptions {
        keyed: true,
        config: MyAgentConfigRpc { foo: Some(42), ..Default::default() },
        env: vec![("MODE".to_string(), "test".to_string())],
    },
)
.await?;
```

These are definition-derived guest clients; external bridge client configuration signatures differ.

## Querying the Agent ID From Inside an Agent

An agent can read its own agent ID with `golem_rust::agentic::get_agent_id()`, or through the
`BaseAgent` trait:

```rust
use golem_rust::agentic::BaseAgent;

fn some_method(&self) -> golem_rust::AgentId {
    self.get_agent_id()
}
```

The `GOLEM_AGENT_KEY` environment variable holds the key of a keyed agent; an agent without a key
has no `GOLEM_AGENT_KEY`.

## HTTP-Mounted Agents Without a Key

When an agent is mounted as an HTTP endpoint, you can set `phantom_agent = true` to make every incoming HTTP request create a **new agent without a key** automatically:

```rust
use golem_rust::{agent_definition, endpoint};

#[agent_definition(mount = "/api", phantom_agent = true)]
pub trait RequestHandler {
    fn new() -> Self;
    #[endpoint(post = "/handle")]
    fn handle(&self, input: String) -> String;
}
```

Each HTTP request will be handled by a fresh agent with its own agent ID, even though all instances share the same (empty) constructor parameters.

## Key Points

- Agents without a key of durable types are **fully durable** — they persist just like keyed durable agents.
- An agent ID is a UUID that Golem issues; it cannot be computed from the constructor parameters.
- `create` without `keyed` always creates a new agent; reconnect to it later with `get_by_id`.
- A keyed agent and an agent without a key with the same constructor parameters are **different agents** — they do not share state.
