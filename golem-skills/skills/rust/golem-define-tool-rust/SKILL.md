---
name: golem-define-tool-rust
description: "Defines and implements a typed Golem tool in Rust. Use when creating a tool provider, command schema, or callable tool component."
---

# Define a Golem tool in Rust

Declare the public command surface with `#[tool_definition]`, then implement it with
`#[tool_implementation]`. The macro generates metadata, guest exports, and a typed client.

```rust
use golem_rust::{tool_definition, tool_implementation};

#[tool_definition(version = "1.0.0")]
pub trait Echo {
    async fn echo(&self, value: String) -> String;
}

struct EchoImpl;

#[tool_implementation]
impl Echo for EchoImpl {
    async fn echo(&self, value: String) -> String {
        value
    }
}
```

Use `IntoSchema` and `FromSchema` for custom inputs, outputs, and error payload types. For a
declared command error in `Result<T, E>`, derive `golem_rust::ToolError` on `E` and annotate its
variants with `#[tool_error(...)]`. Keep the definition and implementation in provider source;
never edit macro-generated bindings. A tool component can provide tools without defining an agent.
