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

    // Address the keyed agent of the constructor parameters
    let agent = MyAgent::upsert("my-instance".to_string())?;

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

### Reconnect window of streaming calls

A streaming call reconnects with a growing delay (50 ms up to 3 s) while the server accepts neither
its start nor a resume; HTTP 429 and 5xx on the upgrade and retryable rejections are retried too.
`configure` allows 5 minutes without progress; choose another window, or no limit, with
`configure_with_reconnect`:

```rust
use std::time::Duration;
use my_agent_client::ReconnectPolicy;

my_agent_client::configure_with_reconnect(
    GolemServer::Local,
    "my-app",
    "local",
    ReconnectPolicy::Within(Duration::from_secs(30)), // or ReconnectPolicy::Unlimited
);
```

Past the window the call fails with `ClientError::Unreachable { endpoint, window, last_cause }`.
Dropping the call's future, for example with `tokio::time::timeout`, stops it earlier.

## Addressing Agents

`upsert` does not contact the server: the server creates the keyed agent of the constructor
parameters on the first invocation when it is missing. The client of a durable agent type also has:

- `create(params..., options)` — `async`: creates an agent and returns after its initialization.
  Without `keyed`, each call creates a new agent without a key and with a new agent ID, so you can
  have many agents with the same constructor parameters. With `keyed: true` it creates the keyed
  agent and fails with `ClientError::Conflict` when it exists.
- `get_by_id(agent_id)` — `async`: addresses the existing agent with this agent ID. It never
  creates the agent; an unknown ID fails with `ClientError::NotFound`.
- `agent_id()` — `async`: the agent ID (a UUID that Golem issues) of the agent the client addresses.

```rust
use my_agent_client::CreateOptions;

let agent = MyAgent::create("shared-name".to_string(), CreateOptions::default()).await?;
let id = agent.agent_id().await?;
let same_agent = MyAgent::get_by_id(id).await?;
```

An ephemeral agent type has only `upsert`; each invocation runs on a new one-shot instance.

## Agent Configuration

If the agent has local configuration fields, pass overrides and environment variables to `create`.
`config` is the `Option` of the value of one configuration declaration (`None` keeps the value),
or a tuple of these in declaration order; `upsert` takes no overrides:

```rust
let agent = MyAgent::create(
    "my-instance".to_string(),
    CreateOptions {
        keyed: true,
        config: Some(my_config_value),
        env: std::collections::HashMap::from([("MODE".to_string(), "test".to_string())]),
    },
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
