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
    external:
      agents: "*"                    # Generate for all agents
      # Or list specific agents:
      # agents:
      #   - MyAgent
      #   - my-app:billing
      outputDir: ./bridge-sdk/rust   # Optional custom output directory
      additionalDerives:
        - '^Order.*=PartialEq,Eq'
        - 'Response$=my_derives::ApiType'
      additionalDependencies:
        anyhow: "1"                 # Version shorthand
        my_derives:
          path: ./crates/my-derives # Relative to this golem.yaml
          package: my-derive-macros # Optional Cargo package rename
          features: [api]
          defaultFeatures: false
        remote_derive:
          git: https://github.com/example/derive-macros
          rev: 0123456789abcdef      # Exactly one of branch/tag/rev
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

## Step 2: Generate the Bridge SDK

The recommended approach is to declare the bridge in `golem.yaml` (as shown above) and let `golem build` produce the SDK as part of the normal build:

```shell
golem build --yes
```

This produces a Rust crate per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/rust/` by default). Re-running `golem build` after agent changes keeps the generated client in sync automatically.

Avoid invoking `golem generate-bridge` manually — it exists as a low-level escape hatch, but the manifest-driven flow above is the supported way to keep bridges configured, reproducible, and up to date.

The equivalent low-level CLI options are repeatable and use Cargo TOML syntax for dependencies:

```shell
golem generate-bridge --language rust \
  --derive-rule '^Order.*=PartialEq,Eq' \
  --derive-rule 'Response$=my_derives::ApiType' \
  --rust-dependency 'anyhow = "1"' \
  --rust-dependency 'my_derives = { package = "my-derive-macros", path = "./crates/my-derives", features = ["api"], default-features = false }'
```

CLI dependency paths are relative to the invocation directory. Manifest dependency paths are relative to the directory containing the manifest declaration. These settings apply only to generated Rust agent bridges, not tool bridges.

## Step 3: Use the Generated Client

Add the generated crate as a path dependency in your external Rust project's `Cargo.toml`:

```toml
[dependencies]
my-agent-client = { path = "../path/to/bridge-sdk/rust/my-agent-client" }
```

This default enables the complete client, including configuration, REST invocation, scheduling,
and streaming. To use only generated portable model types, without Golem or HTTP client
dependencies, disable default features:

```toml
[dependencies]
my-agent-client = {
  path = "../path/to/bridge-sdk/rust/my-agent-client",
  default-features = false,
}
```

Enable the independent `serde` feature when those model types need serialization:

```toml
[dependencies]
my-agent-client = {
  path = "../path/to/bridge-sdk/rust/my-agent-client",
  default-features = false,
  features = ["serde"],
}
```

Type-only mode includes generated records, variants, enums, flags, multimodal types, bare
`AgentBinary`, and unstructured text/binary helpers whose schemas do not transitively contain a
stream. Streams are runtime capabilities rather than portable data, so stream-bearing named and
multimodal types are available only with the `client` feature. Do not expect placeholders for
those types in a no-default build.

Then use the generated client:

```rust
use my_agent_client::{GolemServer, MyAgent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the Golem server connection
    my_agent_client::configure(GolemServer::Local, "my-app", "local");

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
    url: "https://my-golem.example.com".parse()?,
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

## Generated Crate Features and Dependencies

Generated external crates use `default = ["client"]`. The `client` feature owns the Golem client,
HTTP, runtime, scheduling, and streaming dependencies. The separate `serde` feature adds serde
derives to portable generated models and helpers; `client` does not imply `serde`.

With default features disabled, the generated crate does not activate `golem-client`,
`golem-common`, `reqwest`, or `reqwest-middleware`, making the portable subset suitable for targets
such as `wasm32-unknown-unknown`.

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types match the agent definition
- All custom types (records, variants, enums, flags) are generated as corresponding Rust types
- The client uses async/await with `reqwest` for HTTP communication
- Each agent type gets its own crate with a `Cargo.toml` and `src/lib.rs`
- Use `default-features = false` only for the portable, stream-free model subset; agent client
  structs and stream-bearing types require the default `client` feature
