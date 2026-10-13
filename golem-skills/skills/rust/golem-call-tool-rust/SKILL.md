---
name: golem-call-tool-rust
description: "Calls a typed Golem tool from Rust. Use when invoking generated built-in or MCP imported tool clients, a tool provider, or handling declared and protocol errors."
---

# Call a Golem tool from Rust

## Call an existing tool through a generated client

Load `golem-generating-tool-clients` for built-in releases and MCP imports. Add the
tool to the Rust component's `dependencies.tools` and configure its runtime binding
separately. `golem build` automatically generates the client without an explicit
bridge selection. Add a local Cargo dependency on the generated crate, relative
to the consumer manifest:

```toml
[dependencies]
bash-tool-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/bash-tool-guest-client" }
```

Inside an async agent method, call the built-in `bash` release `0.2.1`:

```rust
use bash_tool_guest_client::BashClient;

match BashClient::new().run("printf 'hello\\n'".into(), "".into(), 600).await {
    Ok(result) => println!("{}", result.stdout),
    Err(error) => eprintln!("tool invocation failed: {error}"),
}
```

The script's `exit_code` is part of the result; check it when a nonzero exit should
fail the agent method. A command with output attachments instead returns a
`ToolInvocation`; use its collection helper to drain both channels concurrently
and check the result and output outcomes separately. Use the generated signature
for MCP structured results rather than assuming the call returns a string.

## Call from a definition owned by the application

`#[tool_definition]` generates a `<Tool>Client`. Its async methods return
`Result<T, ToolError<E>>`, where `E` is the tool's declared error type:

```rust
use golem_rust::agentic::ToolError;

let client = EchoClient::default();
match client.echo("hello".to_string()).await {
    Ok(value) => println!("{value}"),
    Err(ToolError::Tool(error)) => eprintln!("declared tool error: {error:?}"),
    Err(error) => eprintln!("tool invocation failed: {error}"),
}
```

The default client targets the definition's tool name. Use the generated named-target constructor
only when deployment assigns another tool registration name. Do not treat protocol errors as the
tool's declared domain error. Drop or finish any stream handles according to the generated method
signature.
