---
name: golem-call-tool-rust
description: "Calls a typed Golem tool from Rust. Use when invoking a tool provider and handling declared or protocol errors."
---

# Call a Golem tool from Rust

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
