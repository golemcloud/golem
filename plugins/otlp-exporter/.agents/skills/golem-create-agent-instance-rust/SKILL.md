---
name: golem-create-agent-instance-rust
description: "Creating a new Rust Golem agent instance with golem agent new. Use when asked to create, instantiate, or pre-create an agent without invoking a method."
---

# Creating a Golem Agent Instance with `golem agent new`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent new <AGENT_ID> [OPTIONS]
```

This **creates a new agent instance** without invoking any method on it. The agent is initialized with its constructor parameters and starts in an idle state, ready to receive invocations. If the agent's component has not been deployed yet and the CLI is run from an application directory, it will automatically build and deploy the component first.

Unlike `golem agent invoke`, which implicitly creates the agent on first call, `golem agent new` explicitly pre-creates the agent — useful when you need to set environment variables, configuration, or WASI config at creation time.

## Agent ID Format

The agent ID identifies the agent type and its constructor parameters:

```
AgentTypeName(param1, param2, ...)
```

The agent ID can optionally be prefixed with environment or application paths:

| Format | Description |
|--------|-------------|
| `AgentTypeName(params)` | Standalone agent name |
| `env/AgentTypeName(params)` | Environment-specific |
| `app/env/AgentTypeName(params)` | Application and environment-specific |
| `account/app/env/AgentTypeName(params)` | Account, application, and environment-specific |

For agents with no constructor parameters, use empty parentheses: `AgentTypeName()`.

## Examples

### Create an agent with no constructor parameters

```shell
golem agent new 'MyAgent()'
```

### Create an agent with constructor parameters

```shell
golem agent new 'ChatRoom("general")'
```

### Create an agent with environment variables

```shell
golem agent new 'MyAgent("user-123")' --env API_KEY=sk-abc123 --env LOG_LEVEL=debug
```

### Create an agent with configuration

```shell
golem agent new 'MyAgent()' --config max_retries=5 --config timeout_seconds=30
```

### Create an agent with WASI config

```shell
golem agent new 'MyAgent()' --wasi-config MY_WASI_VAR=some-value
```

### Create an agent in a specific environment

```shell
golem agent new 'staging/MyAgent("user-123")'
```

### Combine environment variables and configuration

```shell
golem agent new 'OrderProcessor("us-east")' \
  --env DATABASE_URL=postgres://... \
  --config batch_size=100 \
  --config retry_policy.max_attempts=3
```

## Available Options

| Option | Description |
|--------|-------------|
| `-e, --env <ENV=VAL>` | Environment variables visible to the agent (can be repeated) |
| `-c, --config <PATH=VALUE>` | Configuration entries for the agent, using dot-separated paths (can be repeated). Only configuration declared by the agent can be provided. If not provided, the default from the manifest (`agents.*.config`) is used. |
| `-w, --wasi-config <VAR=VAL>` | WASI config entries visible to the agent (can be repeated). This is for compatibility with third-party libraries that depend on `wasi:config`; prefer typed configuration (`-c`) for your own agent config. |

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, `golem agent new` will automatically build and deploy the component before creating the agent.

## Value Syntax

The agent ID parameters use **Rust syntax**:

- Field names use `snake_case`
- Options: `Some(value)` / `None`
- Records: `MyRecord { field_one: 1, field_two: "hello" }`
- Enums/Variants: `MyEnum::VariantName` or `MyEnum::VariantName(value)`
- Tuples: `(1, "hello")`
- Results: `Ok(value)` / `Err(value)`

```shell
golem agent new 'UserManager(MyConfig { region: "us-east", max_connections: Some(10) })'
```

## When to Use `golem agent new` vs `golem agent invoke`

- Use `golem agent new` when you need to **pre-create** an agent with specific environment variables, configuration, or WASI config before any invocation.
- Use `golem agent invoke` when you want to call a method — the agent is created automatically on first invocation if it doesn't exist.
- An agent created with `golem agent new` can be invoked later with `golem agent invoke` using the same agent ID.
