---
name: golem-invoke-agent-scala
description: "Invoking a Scala Golem agent method from the CLI. Use when asked to call, invoke, or run a method on a deployed agent using golem agent invoke."
---

# Invoking a Golem Agent with `golem agent invoke`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

This invokes a method on a deployed agent and **waits for the result**. The agent is automatically created on first invocation if it does not exist yet. Standard output, error, and log streams from the agent are streamed live to the terminal by default.

## Output

Text output renders return values using Scala syntax. Multiple return values are rendered as a Scala tuple, for example `(1, "ok")`. Methods returning `Unit` or no value print `void` in text mode.

For machine-readable output, use `--format json`, `--format yaml`, or `--format toon`. Streaming methods emit invocation lifecycle documents in order: `accepted`, `result`, any stream `item`/terminal events, and `finished`. Scalar fields accompanying streams are in the `result` event's `value`.

## Streaming Parameters and Results

Use `-` as the argument for exactly one direct stream parameter to bind it to stdin. By default, `--stdin-format value` reads one Scala value per line, strips only the line terminator, preserves blank lines as values, and does not accept multiline values. `--stdin-format raw` is available only for a direct `stream<binary>` or `stream<u8>` parameter. For `stream<binary>`, each logical item is a fixed 64 KiB chunk except the final shorter chunk. For `stream<u8>`, each byte is one logical item.

```shell
printf 'Some(1)\nNone\n' | golem agent invoke 'MyAgent()' consumeValues -
cat input.bin | golem agent invoke 'MyAgent()' consumeBytes - --stdin-format raw
```

`--stdout-format value` is the default and renders stream items as Scala values. `--stdout-format raw` writes only bytes and requires exactly one direct `stream<binary>` or `stream<u8>` result.

```shell
golem agent invoke 'MyAgent()' produceValues --stdout-format value
golem agent invoke 'MyAgent()' produceBytes --stdout-format raw > output.bin
```

In structured CLI formats, invocation output is a sequence of lifecycle documents rather than one object or array. Streaming invocation is durable: disconnecting or pressing Ctrl-C detaches the current transport without cancelling the invocation. The CLI does not retry automatically. Use `--save-session <PATH>` on the initial invocation, then `--resume-session <PATH>` after a detach or `--takeover-session <PATH>` to fence an attached transport explicitly.

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

### Invoke with an agent that has constructor parameters

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
| `--stdin-format value\|raw` | Select stdin stream framing; defaults to `value` |
| `--stdout-format value\|raw` | Select stream result rendering; defaults to `value` |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (requires `--trigger`; ISO 8601 format) |

## Idempotency

Every invocation uses an idempotency key. If not provided, one is generated automatically. The same idempotency key guarantees that the invocation is executed at most once, even if the CLI call is retried.

```shell
golem agent invoke -i my-unique-key 'MyAgent()' doWork
```

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, `golem agent invoke` will automatically build and deploy the component before invoking.

## Value Syntax

The agent ID parameters and method arguments use **Scala syntax**:

- Field names use `camelCase` with `=` separator
- Options: `Some(value)` / `None`
- Records: `MyRecord(fieldOne = 1, fieldTwo = "hello")`
- Enums/Variants: `MyEnum.VariantName` or `MyEnum.VariantName(value)`
- Tuples: `(1, "hello")` or `Tuple1(value)` for single-element
- Results: `WitResult.Ok(value)` / `WitResult.Err(value)`

```shell
golem agent invoke 'MyAgent("user-123")' updateProfile 'MyProfile(displayName = "Alice", age = Some(30))'
```
