---
name: golem-trigger-agent-scala
description: "Triggering a fire-and-forget invocation on a Scala Golem agent. Use when asked to trigger, enqueue, or fire-and-forget an agent method invocation using golem agent invoke --trigger."
---

# Triggering a Fire-and-Forget Agent Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke --trigger <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

The `--trigger` (or `-t`) flag sends the invocation request to the agent and **returns immediately** without waiting for the result. The invocation is enqueued and executed asynchronously by the Golem runtime.

## When to Use Trigger

- When the caller does not need the return value
- When you want to start a long-running operation without blocking
- When enqueuing work for background processing
- When combined with `--schedule-at` for future execution

## Examples

### Trigger a method with no wait

```shell
golem agent invoke --trigger 'MyAgent()' startBackgroundJob '"job-123"'
```

### Trigger in a specific environment

```shell
golem agent invoke --trigger 'staging/MyAgent("user-123")' sendNotification '"Your order is ready"'
```

### Trigger with an explicit idempotency key

```shell
golem agent invoke --trigger -i 'job-run-2024-01-15' 'BatchProcessor()' runDailyReport
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | **Required.** Trigger the invocation without waiting |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (ISO 8601 format, e.g. `2026-03-15T10:30:00Z`) |

## Difference from Regular Invoke

| | `golem agent invoke` | `golem agent invoke --trigger` |
|---|---|---|
| Waits for result | Yes | No |
| Returns value | Yes | Only the idempotency key |
| Streams output | Yes (by default) | No |
| Use case | Synchronous calls | Fire-and-forget / background work |

## Idempotency

Triggered invocations also use idempotency keys. If the same idempotency key is used, the invocation will not be executed again.

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, the command will automatically build and deploy the component before triggering.

## Value Syntax

The agent ID parameters and method arguments use **Scala syntax**:

- Field names use `camelCase` with `=` separator
- Options: `Some(value)` / `None`
- Records: `MyRecord(fieldOne = 1, fieldTwo = "hello")`
- Variants: `MyEnum.VariantName(value)`
- Tuples: `(1, "hello")`

```shell
golem agent invoke --trigger 'MyAgent("user-123")' sendEmail 'EmailRequest(to = "alice@example.com", subject = "Hello", body = "World")'
```
