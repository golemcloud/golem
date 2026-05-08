---
name: golem-call-from-external-rust
description: "Calling Golem agents from external Rust applications using generated bridge SDKs. Use when the user wants to invoke agents from outside the Golem platform, from a Rust CLI, server, or any native Rust application."
---

# Calling Agents from External Rust Applications

## Overview

Golem can generate typed Rust client libraries (bridge SDKs) for calling agents from any external Rust application — a CLI tool, a web server, a background job, etc. The generated client communicates with the Golem server's REST API and provides a fully typed interface matching the agent's methods.

## Step 1: Enable Bridge Generation

Add a `bridge` section to `golem.yaml`:

```yaml
bridge:
  rust:
    agents: "*"                    # Generate for all agents
    # Or list specific agents:
    # agents:
    #   - MyAgent
    #   - my-app:billing
    outputDir: ./bridge-sdk/rust   # Optional custom output directory
    additionalDerives:             # Optional: extra derives for generated Rust types
      - pattern: ".*"              # Regex matched against generated type names
        derives: [PartialEq]
      - pattern: "^.*Id$"
        derives: [Eq, Hash]
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

Use `additionalDerives` when the external Rust application needs generated bridge types to implement extra traits, for example `PartialEq` in tests or `Eq`/`Hash` for map/set keys. Patterns must be valid regular expressions, and derives must be syntactically valid Rust derive paths whose macros are available to the generated crate at compile time. Invalid derive rules fail `golem build` during bridge validation, before stale generated output is reused. Do not add `Debug`, `Clone`, `serde::Serialize`, or `serde::Deserialize` here; those are already generated in the standard derives / `serde` feature path.

## Step 2: Generate the Bridge SDK

The recommended approach is to declare the bridge in `golem.yaml` (as shown above) and let `golem build` produce the SDK as part of the normal build:

```shell
golem build
```

This produces a Rust crate per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/rust/` by default). Re-running `golem build` after agent changes keeps the generated client in sync automatically.

Avoid invoking `golem generate-bridge` manually — it exists as a low-level escape hatch, but the manifest-driven flow above is the supported way to keep bridges configured, reproducible, and up to date.

## Step 3: Use the Generated Client

Add the generated crate as a path dependency in your external Rust project's `Cargo.toml`:

```toml
[dependencies]
my-agent-client = { path = "../path/to/bridge-sdk/rust/my-agent-client" }
```

Then use the generated client:

```rust
use my_agent_client::{configure, MyAgent};
use golem_client::bridge::GolemServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the Golem server connection
    configure(GolemServer::Local, "my-app", "local");

    // Get or create an agent instance
    let agent = MyAgent::get("my-instance".to_string()).await?;

    // Call methods — fully typed parameters and return values
    let result = agent.do_something("input".to_string()).await?;
    println!("Result: {:?}", result);

    Ok(())
}
```

## Server Configuration

The `GolemServer` enum supports three modes:

```rust
// Local development server (http://localhost:9881)
GolemServer::Local

// Golem Cloud
GolemServer::Cloud { token: "your-api-token".to_string() }

// Custom deployment
GolemServer::Custom {
    url: reqwest::Url::parse("https://my-golem.example.com")?,
    token: "your-token".to_string(),
}
```

## Phantom Agents

To create multiple agent instances with the same constructor parameters, use phantom agents:

```rust
let agent = MyAgent::get_phantom(uuid::Uuid::new_v4(), "shared-name".to_string()).await?;
```

Or generate a random phantom ID automatically:

```rust
let agent = MyAgent::new_phantom("shared-name".to_string()).await?;
```

## Agent Configuration

If the agent has local configuration fields, use the `_with_config` variants:

```rust
let agent = MyAgent::get_with_config(
    "my-instance".to_string(),
    Some(my_config_value),    // config parameter (Option)
).await?;
```

## Generated Crate Dependencies

The generated crate has feature-gated dependencies:

- `default = ["client"]` preserves the normal fully functional client SDK.
- `client` enables the HTTP client stack and depends on `serde` and `golem-types`.
- `serde` enables serde derives on generated types.
- `golem-types` enables lightweight Golem helper types used by multimodal and unstructured text/binary schemas without pulling in the HTTP client stack.

For normal external clients, use the default features. For type-only use cases, depend on the generated crate with `default-features = false` and enable only what you need:

```toml
[dependencies]
my-agent-client = { path = "../path/to/bridge-sdk/rust/my-agent-client", default-features = false, features = ["serde"] }
```

When `client` is disabled, the crate provides generated types only; runtime client APIs such as `configure(...)`, `MyAgent::get(...)`, and method invocation helpers are not available.

If the generated types include multimodal or unstructured text/binary fields, also enable `golem-types`:

```toml
my-agent-client = { path = "../path/to/bridge-sdk/rust/my-agent-client", default-features = false, features = ["serde", "golem-types"] }
```

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types match the agent definition
- All custom types (records, variants, enums, flags) are generated as corresponding Rust types
- The client uses async/await with `reqwest` for HTTP communication
- Each agent type gets its own crate with a `Cargo.toml` and `src/lib.rs`
- Changing `additionalDerives` forces the next `golem build` to regenerate the Rust bridge SDK
