# golem-rust

This repository contains Rust crates that help writing [Golem](https://golem.cloud) programs.

## golem-rust

The `golem-rust` crate contains Rust wrappers for Golem's runtime API, including
the [transaction API](https://learn.golem.cloud/docs/transaction-api).

The `golem-rust` crate uses the shared `golem-rust-macro` and
`golem-tool-metadata` authoring crates from the repository's root workspace.

Semantic retry policies can be used for both host-managed operations and arbitrary user code. See
the [`golem-rust` retry example](golem-rust/README.md#retrying-user-code-with-semantic-policies) for
how to resolve a named policy through the host and execute it locally with `RetrySchedule`.

## Agent implementations

Traits annotated with `#[agent_definition]` must be implemented with
`#[agent_implementation]`. A plain `impl AgentTrait for Type` now fails during
`cargo check` with a missing hidden item named
`agent_implementation_annotation`, which points to the forgotten annotation.
The post-build `discover-agent-types` check remains the fallback for detecting
agent definitions that have no implementation anywhere.

## Runtime tool reflection

Rust exposes four client approaches: Normal RPC through the ordinary client for
a shared source definition; caller-defined static method-only or full clients;
discovered clients driven by an immutable metadata snapshot; and fully dynamic
schema-native clients. Method-only
agent clients bind existing durable IDs. Full agent clients own creation and
declare durable or ephemeral lifecycle. `ToolClientDefinition` is the
caller-defined static tool approach and can be named or bound to a target name.

With `export_golem_agentic` enabled, `golem_rust::agentic::reflection::get_tool_type`
discovers a tool visible to the caller. Walk `ToolType::root` or `ToolType::node`
to inspect callable and namespace-only nodes; namespace nodes are visible but cannot
be invoked. Choose a callable command with `ToolType::command`; command aliases are
accepted and `ToolCommand::path` returns the canonical path. The command exposes its
ordered arguments, input schema, and declared output schema. This metadata/callable
split is intentional. Tool reflection reports metadata and local-input failures
through `ToolReflectionError`, which keeps them distinct from nested transport and
remote tool failures.

```rust,ignore
use golem_rust::agentic::reflection::get_tool_type;

let command = get_tool_type("weather")?.command(&["forecast"])?;
let result = command
    .invoke_json(&serde_json::json!({ "city": "Budapest" }))
    .await?;
```

`invoke_value` accepts a schema-native value. Both forms validate inputs before opening
RPC and validate declared outputs when the call completes. `start_value` returns a pending
invocation with separate stdout, result, collection, and cancellation capabilities. Use
it for a command with required stdout; `collect` drains stdout while awaiting the result.
`trigger_value` is available for commands without required caller-readable stdout.

`DynamicToolClient` accepts a caller-packed `TypedSchemaValue` and command path when no
descriptor is available. It offers awaited, pending, and trigger calls, but has no
deployed schema for local input or output validation. Reflected and dynamic calls return
recoverable errors, including `MalformedRemoteOutput` for an invalid declared result.

Schema definition and command-tree restrictions are checked while definitions are
built. Constructors, config, canonical JSON, effective command constraints, defaults,
and required streams are checked before creation or invocation. Result cardinality,
resolved schema graphs, values, and declared custom-error payloads are checked when the
call completes; authorization and deployed availability remain host decisions.

Use Rust `Option<T>` for Normal RPC or caller-owned optional values. In reflected
`SchemaValue` records, use `SchemaValue::Option { inner: None }` for absence and
`Some(Box::new(value))` for presence. Optional scalar tool positionals and options use
the same carrier, tails use an empty list, and flags use their effective value.
Canonical JSON uses base-10 strings for `s64`, `u64`, duration nanoseconds, and
quantity mantissas; smaller integers remain JSON numbers.
