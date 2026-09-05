---
name: golem-agent-reflection-rust
description: "Discovering and calling Golem agents through runtime reflection in Rust. Use when agent types or methods are selected dynamically, schemas must be inspected at runtime, caller-owned method contracts are needed, or schema-native values are invoked directly."
---

# Calling Agents with Runtime Reflection (Rust)

Use generated clients when the target definition is available at compile time.
The reflection API provides three other authority models: caller-owned Rust
codecs, runtime-reflected schemas, and direct schema-free `SchemaValue`
invocation.

## Discover Agent Types

Discover the agent types registered in the current environment:

```rust
use golem_rust::{get_agent_type, get_all_agent_types};

let available = get_all_agent_types()?;
let counter_type = get_agent_type("CounterAgent")?;

println!("{} {:?}", counter_type.name(), counter_type.mode());
for method in counter_type.methods() {
    println!("{}", method.name());
}
```

Agent type names are environment-unique. `AgentType` exposes the current
implementation component, lifecycle mode, constructor schema, and method
schemas. Discovery is authoritative for reflected calls; it does not modify a
caller-owned contract or retry a call after schema changes. Rust lookup helpers
are strict: a missing type is returned as `GolemReflectError::AgentTypeNotFound`.

## Inspect and Pack Schemas

Constructor and method schemas are `SchemaRef` values. They validate
`SchemaValue`, pack canonical JSON, unpack results, and render JSON Schema:

```rust
use serde_json::json;

let method = counter_type.method("add")?;
let input = method.input().pack_json(&json!({ "by": 5 }))?;
let json_schema = method.input().to_json_schema(true);
```

Use `invoke_json` for automatic JSON packing and unpacking. Use explicit
`pack_json`, `invoke_value`, and `unpack_json` when the boundary needs access to
schema-native values.

## Invoke Through Reflected Schemas

Create a durable client from the reflected type and invoke a method selected by
name:

```rust
use serde_json::json;

let counter = counter_type.client().get_json(&json!({ "name": "main" }))?;
let result = counter
    .method("add")?
    .invoke_json(&json!({ "by": 5 }))
    .await?;

println!("{:?}", result.value);
println!("{}", result.metadata.agent_id.agent_id);
```

Reflected methods also expose `trigger_value`, `pending_value`, and
`schedule_value`. Pending invocations are futures and can be cancelled before
completion. Scheduled invocations return a cancellation token. Live stream
inputs and outputs are supported by awaited `invoke_value` calls. They are not
supported by trigger or scheduled calls; a pending call cannot accept a live
stream input.

## Define a Caller-Owned Typed Contract

`#[agent_client]` creates a partial, lifecycle-free typed interface without
performing discovery. The trait's Rust `IntoSchema` and `FromSchema` codecs are
the schema authority:

```rust
use golem_rust::{AgentId, agent_client};

#[agent_client(type_name = "CounterAgent")]
trait CounterContract {
    fn add(&self, by: u32) -> u32;
    fn reset(&self);
}

fn bind(agent_id: &AgentId) -> Result<CounterContractClient, golem_rust::GolemReflectError> {
    CounterContractClient::for_agent_id(agent_id)
}
```

Binding resolves the type's current implementation metadata by its
environment-unique name, but the trait's codecs remain the schema authority.
It fails when that type is not registered. Generated named methods are awaited
calls. The client also generates
`trigger_<method>`, `pending_<method>`, and `schedule_<method>`. Binding checks
the agent type name but does not discover the remote schema or lifecycle.

## Construct and Bind Agent IDs

When reflected metadata is available, let the reflected constructor schema
validate the identity:

```rust
use serde_json::json;

let agent_id = counter_type.agent_id_json(&json!({ "name": "main" }), None)?;
let reflected = agent_id.reflected_client(&counter_type)?;
let typed = CounterContractClient::for_agent_id(&agent_id)?;
```

`get_agent_type_for(&agent_id)` resolves the currently registered type for an
existing identity. It does not create the agent.

For schema-free infrastructure, construct an ID from a component ID, type name,
manually packed constructor record, and optional phantom UUID:

```rust
use golem_rust::{AgentId, AgentIdExt, SchemaValue};

let agent_id = AgentId::from_value(
    component_id,
    "CounterAgent",
    SchemaValue::Record {
        fields: vec![SchemaValue::String("main".to_string())],
    },
    None,
)?;
```

This path performs no local schema or lifecycle verification. Constructor
record fields must be in declaration order; the runtime authoritatively accepts
or rejects the attempted identity or invocation.

## Invoke Direct Schema Values

Use `dynamic_client` only when the caller intentionally owns manually packed
Golem values and arbitrary method names:

```rust
use golem_rust::SchemaValue;

let result = agent_id
    .dynamic_client()?
    .method("add")
    .invoke_value(SchemaValue::Record {
        fields: vec![SchemaValue::U32(5)],
    })
    .await?;
```

Dynamic methods also support `trigger_value`, `pending_value`, and
`schedule_value`. They never use caller-authored or reflected schemas.

## Phantom and Ephemeral Lifecycles

Reflected durable factories support normal lookup, known phantom UUIDs, and
new phantom UUIDs. A new durable phantom returns its client, reusable agent ID,
and phantom UUID together.

For raw ephemeral invocation, construct a one-shot address directly:

```rust
use golem_rust::{DynamicAgentClient, SchemaValue};

let request = DynamicAgentClient::ephemeral(
    component_id,
    "RequestAgent",
    SchemaValue::Record {
        fields: vec![SchemaValue::String("summarize".to_string())],
    },
)?;

assert!(request.agent_id().is_none());
let result = request
    .method("run")
    .invoke_value(SchemaValue::Record {
        fields: vec![SchemaValue::String("hello".to_string())],
    })
    .await?;
println!("{}", result.metadata.agent_id.agent_id);
```

No reusable pre-invocation identity is guaranteed for an ephemeral call. The
final identity comes from invocation metadata and cannot be resumed or invoked
again.

## Choose an Invocation Path

| Situation | Use |
|---|---|
| Generated definition available | Generated `<Agent>Client` |
| Partial typed contract owned by the caller | `#[agent_client]` |
| Type or method selected at runtime with JSON | `get_agent_type` plus `invoke_json` |
| Explicit runtime-schema packing | `SchemaRef::pack_json`, `invoke_value`, `unpack_json` |
| Schema-free infrastructure with Golem values | `AgentId::from_value` and `dynamic_client` |
