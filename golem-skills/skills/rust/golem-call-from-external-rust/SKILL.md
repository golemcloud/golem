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
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

## Step 2: Generate the Bridge SDK

Run:

```shell
golem build
```

Bridge generation happens automatically as part of the build. Alternatively, generate bridges without a full build:

```shell
golem generate-bridge
golem generate-bridge --language rust
golem generate-bridge --agent-type-name MyAgent
```

This produces a Rust crate per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/rust/` by default).

## Step 3: Use the Generated Client

Add the generated crate as a path dependency in your external Rust project's `Cargo.toml`:

```toml
[dependencies]
my-agent-client = { path = "../path/to/bridge-sdk/rust/my-agent-client" }
```

Then use the generated client:

```rust
use my_agent_client::{MyAgent, global_config};
use golem_client::bridge::{Configuration, GolemServer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the Golem server connection
    global_config(Configuration {
        app_name: "my-app".to_string(),
        env_name: "local".to_string(),
        server: GolemServer::Local,
    });

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

The generated crate depends on `golem-client`, `golem-common`, `golem-wasm`, `reqwest`, `serde_json`, `uuid`, and `chrono`. These are resolved from crates.io or from the Golem repository depending on the SDK version.

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types match the agent definition
- All custom types (records, variants, enums, flags) are generated as corresponding Rust types
- The client uses async/await with `reqwest` for HTTP communication
- Each agent type gets its own crate with a `Cargo.toml` and `src/lib.rs`
