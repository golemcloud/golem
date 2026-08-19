---
name: golem-schedule-agent-go
description: "Scheduling a future invocation on a Go Golem agent from the CLI. Use when the user asks to schedule, delay, or plan a future agent method invocation in a Go Golem project."
---

# Scheduling a Future Agent Invocation from the CLI

## Overview

The `--schedule-at` flag on `golem agent invoke` schedules the invocation to execute at a specific future time. It **requires** the `--trigger` flag, because scheduled invocations are always fire-and-forget — the CLI returns immediately after the invocation is enqueued, and the Golem runtime executes it when the scheduled time arrives.

Both `golem` and `golem-cli` can be used — every command below works with either binary.

## Steps

1. **Identify the agent** by its type name and constructor parameters (`CounterAgent("my-counter")`).
2. **Pick the method** and any arguments (e.g. `increment`, `add 5`).
3. **Choose the time** in ISO 8601 / RFC 3339 format with a timezone.
4. **Run `golem agent invoke --trigger --schedule-at <DATETIME>`** — the CLI returns immediately.

## Usage

```shell
golem agent invoke --trigger --schedule-at <DATETIME> <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

## DateTime Format

The `--schedule-at` value must be in **ISO 8601 / RFC 3339** format with a timezone:

```
2026-03-15T10:30:00Z          # UTC
2026-03-15T10:30:00+02:00     # With timezone offset
```

## Examples

### Schedule a method to run at a specific time

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z 'CounterAgent("my-counter")' increment
```

### Schedule with an argument

```shell
golem agent invoke --trigger --schedule-at 2026-04-01T00:00:00Z 'CounterAgent("my-counter")' add 5
```

### Schedule in a specific environment

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T08:00:00Z 'production/CounterAgent("my-counter")' increment
```

### Schedule with an idempotency key for deduplication

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'increment-2026-03-15' 'CounterAgent("my-counter")' increment
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | **Required** with `--schedule-at`. Fire-and-forget mode |
| `--schedule-at <DATETIME>` | The time to execute the invocation (ISO 8601 / RFC 3339) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |

## How It Works

1. The CLI sends the invocation request with the scheduled time to the Golem server.
2. The server enqueues the invocation to execute at the specified time.
3. The CLI returns immediately with the idempotency key.
4. At the scheduled time, the Golem runtime executes the invocation.

## Value Syntax

Agent ID parameters and method arguments use the CLI's WIT value syntax:

- Strings are quoted: `"my-counter"`; integers are bare: `5`; booleans are `true` / `false`.
- Each exported field of a Go input struct is one positional WIT parameter, named by lower-camel-casing the Go field name (`AmountCents` → `amountCents`).
- Options: `some(value)` / `none`. Records: `{ field-one: 1, field-two: "hello" }`. Tuples: `(1, "hello")`.

## Key Constraints

- `--schedule-at` requires `--trigger`; a scheduled invocation is always fire-and-forget.
- The datetime must include a timezone (RFC 3339); use `Z` for UTC.
- Scheduled invocations use idempotency keys just like regular invocations — the same key is not executed more than once.
- To schedule a future call from **inside agent code** (rather than the CLI), use `Method.Schedule` — see `golem-schedule-future-call-go`.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-schedule-future-call-go` | Schedule a future call from *agent code* (`Method.Schedule`) |
| `golem-trigger-agent-go` | Fire-and-forget invocation without a schedule |
| `golem-invoke-agent-go` | Synchronous invocation that waits for the result |
| `golem-recurring-task-go` | A self-rescheduling / periodic task in agent code |
