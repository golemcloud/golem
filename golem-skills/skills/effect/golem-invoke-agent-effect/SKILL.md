---
name: golem-invoke-agent-effect
description: "Invoking an Effect-based Golem agent method from the CLI. Use when asked to call, invoke, or run a method on a deployed Effect agent using golem agent invoke."
---

# Invoking an Effect Golem Agent with `golem agent invoke`

Both `golem` and `golem-cli` can be used — all commands below work with either binary. Effect
agents use the ordinary Golem CLI; the guest-side typed `Agent.client` API is for calls from
another agent, not shell invocation.

## Usage

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

This invokes a method on a deployed agent and **waits for the result**. The agent is automatically
created on first invocation if it does not exist yet. Standard output, error, and log streams from
the agent are streamed live to the terminal by default.

For an Effect agent definition:

- The agent type name is exactly the `name` passed to `defineAgent`.
- Constructor arguments follow the declaration order in `constructorParams`.
- The function name is exactly the key declared in `methods`, including its TypeScript casing.
- Method arguments follow the declaration order in that method's `params`.

Do not translate names to kebab-case or snake_case. For example, a `defineAgent` named `Counter`
with a `methods` key named `incrementBy` is invoked as:

```shell
golem agent invoke 'Counter("c1")' incrementBy 5
```

## Output

Effect components report TypeScript as their source language, so text output renders return values
using TypeScript syntax. Multiple return values are rendered as a TypeScript tuple, for example
`[1, "ok"]`. Methods returning `void` or no value print `void` in text mode.

For machine-readable output, use `--format json` or `--format yaml`. A single return value includes
`result` plus `result_json`; multiple return values include `result` plus `results_json`; methods
returning `void` or no value omit result fields.

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
golem agent invoke 'MyAgent()' getStatus
```

### Invoke a method with parameters

```shell
golem agent invoke 'MyAgent("user-123")' processOrder '"order-456"' 42
```

### Invoke an Effect agent with constructor parameters

```shell
golem agent invoke 'ChatRoom("general")' sendMessage '"Hello, world!"'
```

### Invoke in a specific environment

```shell
golem agent invoke 'staging/MyAgent("user-123")' getStatus
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | Only trigger the invocation without waiting for the result (fire-and-forget) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (requires `--trigger`; ISO 8601 format) |

## Idempotency

Every invocation uses an idempotency key. If not provided, one is generated automatically. The
same idempotency key guarantees that the invocation is executed at most once, even if the CLI call
is retried.

```shell
golem agent invoke -i my-unique-key 'MyAgent()' doWork
```

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application
directory, `golem agent invoke` will automatically build and deploy the component before invoking.

## Value Syntax

Effect agent IDs, method arguments, and text results use **TypeScript syntax**:

- Field names use `camelCase`
- Options: raw value or `undefined`
- Records: `{ fieldOne: 1, fieldTwo: "hello" }`
- Enums: `"variant-name"`
- Variants: `{ tag: "variant-name", value: 42 }`
- Tuples: `[1, "hello"]`
- Results: `{ ok: value }` / `{ error: value }`

Quote structured values as one shell argument, and quote string literals twice: once as a shell
argument and once as a TypeScript string literal.

```shell
golem agent invoke 'MyAgent("user-123")' updateProfile '{ displayName: "Alice", age: 30 }'
```
