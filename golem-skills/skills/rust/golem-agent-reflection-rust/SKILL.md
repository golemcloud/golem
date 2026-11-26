---
name: golem-agent-reflection-rust
description: "Composing caller-defined static, discovered, and fully dynamic Golem clients in Rust. Use when schemas are caller-owned or discovered at runtime, or an existing agent must be reached by its AgentId."
---

# Calling Agents with Runtime Reflection (Rust)

Choose the narrowest of four client approaches:

| Client approach | Authority | Use it when |
|---|---|---|
| Normal RPC | A source definition shared through Rust's ordinary RPC mechanism | Producer and caller share a compile-time definition |
| Caller-defined static clients | Caller-owned Rust types and codecs | The caller owns a stable typed subset without discovery |
| Discovered clients | The environment's immutable metadata snapshot | Types or methods are selected at runtime and need validation or JSON projection |
| Fully dynamic clients | Manually packed `SchemaValue` / `TypedSchemaValue` | Infrastructure deliberately retains no schema authority |

An agent ID (`golem_rust::AgentId`) is a UUID that Golem issues; it cannot be
computed from the constructor parameters. A method-only agent client connects to
an existing durable agent by its `AgentId` and never creates or discovers one. A
full client declares the type, constructor, lifecycle, and optional config, so it
owns `upsert` (the keyed agent of the constructor parameters), `create`, and
`get_by_id`; an ephemeral full client has only `upsert`. Dynamic agent clients
connect to an existing durable agent by its `AgentId`. Tool namespaces are
discoverable but only nodes with command bodies are callable.

## Validation and Optional Values

Validation happens at three different boundaries:

| Boundary | What Rust validates | What remains authoritative remotely |
|---|---|---|
| Definition/build | Schema well-formedness, duplicate caller-owned tool paths, command-tree restrictions | Whether an optimistic caller-defined target is deployed |
| Pack/create/start | JSON shape, exact integer range, constructor/config shape, required stdin/stdout/stderr, defaults, `Present`, nested `ValueIs`, and command constraints | Authorization, registered lifecycle, effective config and secrets |
| Completion | Result cardinality, resolved graph equivalence, declared custom-error payloads, and value decoding | Execution failures and undeclared remote errors |

Normal RPC and caller-defined Rust values use `Option<T>`. Reflected canonical JSON
treats an omitted optional record field or an explicit `null` as absent; re-encoding
may include that field with `null`, and reflection JSON Schema omits it from
`required`. When using
`SchemaValue` directly, use `SchemaValue::Option { inner: None }` or
`SchemaValue::Option { inner: Some(Box::new(value)) }`. Optional tool scalar
positionals and options use that option carrier. Repeatable/tail inputs use an
empty list for absence, flags use their effective boolean/count value, and an
explicit default is represented by the default value. Do not replace an absent
optional carrier with a zero or empty string. In tool command JSON, an omitted
scalar option, positional with a declared default, or flag takes its declared
default, and the command input JSON Schema does not require it.

Canonical JSON represents `s64`, `u64`, duration nanoseconds, and quantity
mantissas as canonical base-10 strings. Smaller integers remain JSON numbers.
Packing rejects leading `+`, non-canonical leading zeroes, and out-of-range
values before transport; projected JSON Schema uses the same string patterns
and exposes the exact range as metadata.

Capabilities, futures, and streams cannot be packed or unpacked as reflected
JSON. Their reflection JSON Schema projection is unsatisfiable; use
schema-native value APIs for those leaves.

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
caller-owned definition or retry a call after schema changes. Rust lookup helpers
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

let counter = counter_type.client().upsert_json(&json!({ "name": "main" }))?;
let result = counter
    .method("add")?
    .invoke_json(&json!({ "by": 5 }))
    .await?;

println!("{:?}", result.value);
println!("{}", result.metadata.agent_id.uuid);
```

The reflected factory has `upsert_json` / `upsert_value` (the keyed agent of the
constructor, created when missing), the async `create_json` / `create_value`, and
`get_by_id(&agent_id)` for an existing durable agent. Ephemeral types support only
`upsert_*`. A reflected client's async `agent_id()` returns its `AgentId`.

Creation-time overrides go into the `golem_rust::CreateOptions` of `create_json`
(canonical JSON entries `(path, value)`) or `create_value` (schema-native
`AgentConfigValue` entries). With `keyed: true`, `create` creates the keyed agent
and fails with `GolemReflectError::Conflict` when it already exists; otherwise the
new agent has no key and a new agent ID. `env` adds environment variables.
Override entries with undeclared paths, secret fields, or invalid values are
rejected before constructing RPC. Component defaults may satisfy required local
fields; the host validates effective config at creation and provisions secrets.

```rust
let created = counter_type
    .client()
    .create_json(
        &json!({ "name": "main" }),
        golem_rust::CreateOptions {
            keyed: false,
            config: vec![(vec!["model".to_string()], json!("fast"))],
            env: Vec::new(),
        },
    )
    .await?;
let agent_id = created.agent_id().await?;
```

Reflected methods also expose the async `trigger_value(input)` and
`schedule_value(at, input)`. Both return a `golem_rust::Invocation<Option<SchemaValue>>`
handle once the target has durably accepted the invocation; its async `status`,
`await_result` and `cancel` observe or withdraw it, and `method.invocation(&id)`
rebuilds the handle from its `OwnedInvocationId` without a remote call. Only the
caller that started the invocation, or a caller with management permission, may
use the handle; dropping it only detaches. Live stream inputs and outputs are
supported by awaited `invoke_value` calls. Trigger and schedule reject methods
with streams and never lend a scope card.

## Define a Caller-Owned Typed Client

`#[agent_client]` has two forms. A trait containing only instance
methods is a method-only client: `get_by_id` connects it to any existing durable
agent by its `AgentId`, whatever its type. The trait's Rust `IntoSchema` and
`FromSchema` codecs are the method-schema authority:

```rust
use golem_rust::{AgentId, agent_client};

#[agent_client]
trait CounterApi {
    fn add(&self, by: u32) -> u32;
    fn reset(&self);
}

fn connect(agent_id: &AgentId) -> Result<CounterApiClient, golem_rust::GolemReflectError> {
    CounterApiClient::get_by_id(agent_id)
}
```

Method-only clients do not declare a name, constructor, mode, or config and
do not discover the remote lifecycle. They never create an agent. Statically
named methods are awaited calls returning
`Result<InvocationOutput<R>, GolemReflectError>`. The client also generates the
async `trigger_<method>(args...)` and `schedule_<method>(at, args...)`, which
return a `golem_rust::Invocation<R>` handle, and an async `agent_id()`. Unlike
`#[agent_definition]` clients, the scheduled time is the first argument.

A fully defined client declares exactly one static constructor returning `Self`
and must also declare `type_name`. Either half without the other is a compile
error. Constructor parameters use the same encoding and `#[agent_config]`
rules as `#[agent_definition]`:

```rust
#[agent_client(type_name = "CounterAgent")]
trait CounterApi {
    fn new(
        name: String,
        #[agent_config] config: golem_rust::agentic::Config<CounterConfig>,
    ) -> Self;
    fn add(&self, by: u32) -> u32;
}

let keyed = CounterApiClient::upsert("main".to_string())?;
let fresh = CounterApiClient::create(
    "main".to_string(),
    golem_rust::CreateOptions {
        keyed: false,
        config: CounterConfigRpc { model: Some("fast".to_string()) },
        env: Vec::new(),
    },
)
.await?;
let existing = CounterApiClient::get_by_id(&agent_id)?;
let id = fresh.agent_id().await?;
```

A fully defined client encodes the constructor through its locally declared
constructor shape before creating RPC transport, so `upsert` and `create` return
`Result`. `get_by_id` connects only to an agent of the declared type name; an
unknown ID or an agent of another type is `GolemReflectError::RemoteNotFound`.
`create` without `keyed` creates a new agent without a key; with `keyed: true` it
fails with `GolemReflectError::Conflict` when the keyed agent exists. A fully
defined ephemeral client uses `mode = "ephemeral"` and has only `upsert`.

The programmatic equivalent starts by selecting the method-only form:

```rust
let method_only = MethodOnlyAgentClientDefinition::builder()
    .method_only()
    .method::<AddInput, u32>("add")?
    .build();
let full = MethodOnlyAgentClientDefinition::builder()
    .durable::<CounterId>("CounterAgent")
    .config::<CounterConfig>()
    .method::<AddInput, u32>("add")?
    .build();

let constructor = CounterId { name: "main".to_string() };
let keyed = full.upsert(&constructor)?;
let configured = full
    .create(
        &constructor,
        golem_rust::CreateOptions {
            keyed: true,
            config: CounterConfigRpc { model: Some("fast".to_string()) },
            env: Vec::new(),
        },
    )
    .await?;
let existing = full.get_by_id(&agent_id)?;
let any_type = method_only.get_by_id(&agent_id)?;

let ephemeral = MethodOnlyAgentClientDefinition::builder()
    .ephemeral::<CounterId>("CounterRequest")
    .method::<AddInput, u32>("add")?
    .build();
let logical = ephemeral.upsert(&constructor)?;
```

`upsert` and `create` validate the constructor value against `CounterId`
before transport creation. Only fully defined durable clients expose `create`
and `get_by_id`; the method-only definition has only `get_by_id`. The config of
`create` is `C::RpcType`, the typed override carrier generated by
`ConfigSchema`, after `.config::<C>()`, and `()` without it. Every method on a
fully defined caller-authored `#[agent_client]`
reports invocation failures through `Result`; ordinary RPC clients
keep their established call signatures. Malformed declared remote outputs,
including unexpected unit values on awaited calls or awaited handles, report
`GolemReflectError::MalformedRemoteOutput`.

## Look Up and Connect by Agent ID

An `AgentId` comes from Golem: from a client's `agent_id()`, from invocation
metadata, or from agent metadata. To find the agent ID of an existing keyed
agent, let the reflected constructor schema validate the constructor and ask the
host. The lookup never creates the agent and returns `None` when no keyed agent
of these constructor parameters exists:

```rust
use serde_json::json;

if let Some(agent_id) = counter_type.lookup_agent_id_json(&json!({ "name": "main" }))? {
    let reflected = counter_type.client().get_by_id(&agent_id)?;
    let typed = CounterApiClient::get_by_id(&agent_id)?;
}
```

`get_agent_type_for(&agent_id)` resolves the currently registered type from the
type tag of an agent ID. It does not create the agent.

## Invoke Direct Schema Values

Use `DynamicAgentClient` only when the caller intentionally owns manually packed
Golem values and arbitrary method names. `DynamicAgentClient::from_agent_id`
connects to the existing agent with the ID, whatever its type, and never creates
it. Constructor and input record fields must be in declaration order; the
runtime authoritatively accepts or rejects the invocation:

```rust
use golem_rust::SchemaValue;

let result = golem_rust::DynamicAgentClient::from_agent_id(&agent_id)?
    .method("add")
    .invoke_value(SchemaValue::Record {
        fields: vec![SchemaValue::U32(5)],
    })
    .await?;
```

Dynamic methods also support `trigger_value` and `schedule_value`, which return
an invocation handle; `DynamicAgentMethod::invocation(&id)` rebuilds one from its
ID. They never use caller-authored or reflected schemas.

## Discovery to a Fully Dynamic Agent

Keep the discovered schemas beside the dynamic client and apply their policy
explicitly. Moving a packed value into `DynamicAgentClient` does not make that
client validate it:

```rust
use golem_rust::{AgentMode, DynamicAgentClient, GolemReflectError, get_agent_type};
use serde_json::{Value, json};

async fn search_dynamically() -> Result<Value, GolemReflectError> {
    let agent_type = get_agent_type("SearchAgent")?;
    if agent_type.mode() != AgentMode::Durable {
        return Err(GolemReflectError::InvalidType(
            "SearchAgent must be durable".to_string(),
        ));
    }
    let method = agent_type.method("search")?;

    let input = method
        .input()
        .pack_json(&json!({ "query": "golem", "cursor": null }))?;

    let agent_id = agent_type
        .client()
        .upsert_json(&json!({ "tenant": "docs" }))?
        .agent_id()
        .await?;
    let result = match DynamicAgentClient::from_agent_id(&agent_id)?
        .method(method.name())
        .invoke_value(input)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            eprintln!("dynamic search failed: {error}");
            return Err(error);
        }
    };

    let output = method.output().ok_or_else(|| {
        GolemReflectError::MalformedRemoteOutput(
            "search unexpectedly declares unit output".to_string(),
        )
    })?;
    let value = result.value.ok_or_else(|| {
        GolemReflectError::MalformedRemoteOutput(
            "search returned unit instead of a value".to_string(),
        )
    })?;
    output.validate_value(&value)?;
    output.unpack_json(&value)
}
```

This awaited example owns no stream or invocation handle. If packed values contain
owned streams, transfer each input once and consume or close every returned
stream. Dropping an invocation handle only detaches; call `cancel` first when a
not-yet-started invocation is no longer wanted.

## Discover and Call Tools

Tool discovery returns one immutable registration snapshot. Walk `root()` or
`node()` to show every namespace and its documentation; check `is_callable()`
before converting a node to a `ToolCommand`:

Rust deliberately keeps metadata-only `ToolNode` separate from callable
`ToolCommand`. Tool reflection uses `ToolReflectionError` so metadata and local
input failures remain distinct from the nested transport and remote `ToolError`;
agent schema helpers continue to use `GolemReflectError`.

```rust
use golem_rust::{SchemaValue, get_tool_type};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let tool = get_tool_type("search")?;
    for child in tool.root().children() {
        println!("{} callable={}", child.name(), child.is_callable());
    }

    let command = tool.node(&["documents", "find"])
        .ok_or("missing command")?
        .command()?;
    let result = command.invoke_value(SchemaValue::Record {
        fields: vec![
            SchemaValue::String("golem".to_string()),
            SchemaValue::Option { inner: None },
        ],
    }).await?;
    println!("{result:?}");
    Ok(())
}
```

`invoke_value` is the simple awaited path. For independently consumed stdout or stderr,
call `start_value`; it consumes the optional stdin carrier and returns a
`ToolInvocation` containing both output readers plus cancellation/result handles.
Move each declared output to a concurrent consumer or call `collect()`, which drives stdout,
stderr, and structured completion together. `collect()` reports the structured result
error first when multiple channels fail. Call `cancel()` when abandoning work and
`close()` on an output when abandoning only that attachment. A declared custom
error is returned as `ToolError::Tool(ReflectedToolCustomError)` with its typed
payload; structural host failures and undeclared custom errors remain distinct.

Use `ToolClientDefinition` for a caller-owned typed subset without discovery:

```rust
use golem_rust::{FromSchema, IntoSchema, ToolClientDefinition};

#[derive(IntoSchema)]
struct SearchInput {
    query: String,
    limit: Option<u32>,
}

#[derive(IntoSchema, FromSchema)]
struct SearchOutput {
    hits: Vec<String>,
}

async fn search() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let definition = ToolClientDefinition::unnamed()
        .command::<SearchInput, SearchOutput, _, _>(["search"])?
        .build()?;
    let client = definition.client_for("document-search")?;
    let command = client.command::<SearchInput, SearchOutput>(&["search"])?;
    let output = command.invoke(&SearchInput {
        query: "golem".to_string(),
        limit: None,
    }).await?;
    Ok(output.hits)
}
```

`client()` uses the definition's declared name; `client_for()` supplies a name
for an unnamed reusable definition. Both are fallible. They validate the local
typed schema before transport creation and invocation but intentionally do not
discover the remote tool. Use `DynamicToolClient` only for already packed
`TypedSchemaValue` inputs and explicit command paths.

## Durable and Ephemeral Lifecycles

Reflected durable factories support `upsert_*` for the keyed agent, `create_*`
for a new agent (keyed or without a key), and `get_by_id` for an existing agent.

Ephemeral agent types have only `upsert_*`. The client is a logical proxy, and
each invocation runs on a new one-shot instance:

```rust
use golem_rust::{AgentMode, SchemaValue, get_agent_type};
use serde_json::json;

let request_type = get_agent_type("RequestAgent")?;
if request_type.mode() != AgentMode::Ephemeral {
    return Err("RequestAgent must be ephemeral".into());
}
let request = request_type
    .client()
    .upsert_json(&json!({ "task": "summarize" }))?;

let result = request
    .method("run")?
    .invoke_value(SchemaValue::Record {
        fields: vec![SchemaValue::String("hello".to_string())],
    })
    .await?;
println!("{}", result.metadata.agent_id.uuid);
```

No reusable identity exists for an ephemeral call. The agent ID of the one-shot
instance comes from invocation metadata and cannot be resumed or invoked again.

## Choose an Invocation Path

| Situation | Use |
|---|---|
| Shared source definition available | Normal RPC through `<Agent>Client` |
| Caller-owned typed method subset | method-only or full `#[agent_client]` |
| Type or method selected at runtime with JSON | `get_agent_type` plus `invoke_json` |
| Explicit runtime-schema packing | `SchemaRef::pack_json`, `invoke_value`, `unpack_json` |
| Fully dynamic infrastructure with packed Golem values | `DynamicAgentClient::from_agent_id` and `invoke_value` |
