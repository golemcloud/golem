---
name: golem-invoke-agent-rust
description: "Invoking a Rust Golem agent method from the CLI. Use when asked to call, invoke, or run a method on a deployed agent using golem agent invoke."
---

# Invoking a Golem Agent with `golem agent invoke`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

This invokes a method on a deployed agent and **waits for the result**. The agent is automatically created on first invocation if it does not exist yet. Standard output, error, and log streams from the agent are streamed live to the terminal by default.

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

### Invoke a method with no parameters

```shell
golem agent invoke 'MyAgent()' get_status
```

### Invoke a method with parameters

```shell
golem agent invoke 'MyAgent("user-123")' process_order '"order-456"' 42
```

### Invoke with an agent that has constructor parameters

```shell
golem agent invoke 'ChatRoom("general")' send_message '"Hello, world!"'
```

### Invoke in a specific environment

```shell
golem agent invoke 'staging/MyAgent("user-123")' get_status
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | Only trigger the invocation without waiting for the result (fire-and-forget) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (requires `--trigger`; ISO 8601 format) |

## Idempotency

Every invocation uses an idempotency key. If not provided, one is generated automatically. The same idempotency key guarantees that the invocation is executed at most once, even if the CLI call is retried.

```shell
golem agent invoke -i my-unique-key 'MyAgent()' do_work
```

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, `golem agent invoke` will automatically build and deploy the component before invoking.

## Value Syntax

The agent ID parameters and method arguments use **Rust syntax**:

- Field names use `snake_case`
- Options: `Some(value)` / `None`
- Records: `MyRecord { field_one: 1, field_two: "hello" }`
- Enums/Variants: `MyEnum::VariantName` or `MyEnum::VariantName(value)`
- Tuples: `(1, "hello")`
- Results: `Ok(value)` / `Err(value)`

```shell
golem agent invoke 'MyAgent("user-123")' update_profile 'MyProfile { display_name: "Alice", age: Some(30) }'
```
