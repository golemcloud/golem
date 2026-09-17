# golem-rust

This repository contains Rust crates that help writing [Golem](https://golem.cloud) programs.

## golem-rust

The `golem-rust` crate contains Rust wrappers for Golem's runtime API, including
the [transaction API](https://learn.golem.cloud/docs/transaction-api).

The `golem-rust` crate uses the shared `golem-rust-macro` and
`golem-tool-metadata` authoring crates from the repository's root workspace.

## Agent implementations

Traits annotated with `#[agent_definition]` must be implemented with
`#[agent_implementation]`. A plain `impl AgentTrait for Type` now fails during
`cargo check` with a missing hidden item named
`agent_implementation_annotation`, which points to the forgotten annotation.
The post-build `discover-agent-types` check remains the fallback for detecting
agent definitions that have no implementation anywhere.

## Runtime tool reflection

With `export_golem_agentic` enabled, `golem_rust::agentic::reflection::get_tool_type`
discovers a tool visible to the caller. Choose a command with `ToolType::command`;
command aliases are accepted and `ToolCommand::path` returns the canonical path.
The command exposes its ordered arguments, input schema, and declared output schema.

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
