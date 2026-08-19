---
name: golem-trigger-agent-go
description: "Triggering a fire-and-forget invocation on a Go Golem agent from the CLI. Use when the user asks to trigger, enqueue, or fire-and-forget an agent method invocation in a Go Golem project."
---

# Triggering a Fire-and-Forget Agent Invocation

## Overview

The `--trigger` (or `-t`) flag on `golem agent invoke` sends the invocation request to the agent and **returns immediately** without waiting for the result. The invocation is enqueued and executed asynchronously by the Golem runtime.

Both `golem` and `golem-cli` can be used — every command below works with either binary.

## Steps

1. **Identify the agent** by its type name and constructor parameters (`CounterAgent("my-counter")`).
2. **Pick the method** to enqueue (e.g. `increment`, `add`).
3. **Run `golem agent invoke --trigger`** — the CLI returns as soon as the invocation is accepted.

## Usage

```shell
golem agent invoke --trigger <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

## When to Use Trigger

- When the caller does not need the return value
- When you want to start a long-running operation without blocking
- When enqueuing work for background processing
- When combined with `--schedule-at` for future execution (see `golem-schedule-agent-go`)

## Examples

### Trigger a method with no wait

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```

### Trigger with an argument

`add` takes one parameter (the `By` field of `AddIn`), passed positionally:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' add 5
```

### Trigger in a specific environment

```shell
golem agent invoke --trigger 'staging/CounterAgent("my-counter")' increment
```

### Trigger with an explicit idempotency key

```shell
golem agent invoke --trigger -i 'increment-2026-01-15' 'CounterAgent("my-counter")' increment
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

## Value Syntax

Agent ID parameters and method arguments use the CLI's WIT value syntax:

- Strings are quoted: `"my-counter"`; integers are bare: `5`; booleans are `true` / `false`.
- Each exported field of a Go input struct is one positional WIT parameter, named by lower-camel-casing the Go field name (`AmountCents` → `amountCents`).
- Options: `some(value)` / `none`. Records: `{ field-one: 1, field-two: "hello" }`. Tuples: `(1, "hello")`.

## Key Constraints

- Triggered invocations still use idempotency keys — the same key is not executed twice.
- There is no way to read a return value from a triggered invocation; use plain `golem agent invoke` when you need the result.
- If the component is not deployed yet and the CLI runs from an application directory, the command auto-builds and deploys it before triggering.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-invoke-agent-go` | Synchronous invocation that waits for the result |
| `golem-schedule-agent-go` | Combine `--trigger` with `--schedule-at` for future execution |
| `golem-fire-and-forget-go` | Fire-and-forget invocation from *agent code* (`Trigger`) |
| `golem-create-agent-instance-go` | Pre-create an agent instance (`golem agent new`) |
