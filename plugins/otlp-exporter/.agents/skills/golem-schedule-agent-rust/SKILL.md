---
name: golem-schedule-agent-rust
description: "Scheduling a future invocation on a Rust Golem agent. Use when asked to schedule, delay, or plan a future agent method invocation using golem agent invoke --trigger --schedule-at."
---

# Scheduling a Future Agent Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke --trigger --schedule-at <DATETIME> <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

The `--schedule-at` flag schedules the invocation to execute at a specific future time. It **requires** the `--trigger` flag because scheduled invocations are always fire-and-forget — the CLI returns immediately after the invocation is enqueued.

## DateTime Format

The `--schedule-at` value must be in **ISO 8601 / RFC 3339** format with a timezone:

```
2026-03-15T10:30:00Z          # UTC
2026-03-15T10:30:00+02:00     # With timezone offset
```

## Examples

### Schedule a method to run at a specific time

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z 'MyAgent()' run_daily_report
```

### Schedule with parameters

```shell
golem agent invoke --trigger --schedule-at 2026-04-01T00:00:00Z 'BatchProcessor("daily")' generate_report '"Q1-2026"'
```

### Schedule in a specific environment

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T08:00:00Z 'production/NotificationAgent()' send_reminders
```

### Schedule with an idempotency key for deduplication

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z -i 'report-2026-03-15' 'ReportAgent()' generate_daily
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | **Required** with `--schedule-at`. Fire-and-forget mode |
| `--schedule-at <DATETIME>` | The time to execute the invocation (ISO 8601 / RFC 3339) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |

## How It Works

1. The CLI sends the invocation request with the scheduled time to the Golem server
2. The server enqueues the invocation to execute at the specified time
3. The CLI returns immediately with the idempotency key
4. At the scheduled time, the Golem runtime executes the invocation

## Idempotency

Scheduled invocations use idempotency keys just like regular invocations. Providing the same idempotency key for a scheduled invocation ensures it is not executed more than once, even if the CLI command is retried.

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, the command will automatically build and deploy the component before scheduling.

## Value Syntax

The agent ID parameters and method arguments use **Rust syntax**:

- Field names use `snake_case`
- Options: `Some(value)` / `None`
- Records: `MyRecord { field_one: 1, field_two: "hello" }`
- Enums/Variants: `MyEnum::VariantName(value)`
- Tuples: `(1, "hello")`

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z 'MyAgent("user-123")' run_task 'TaskConfig { priority: 1, retry: true }'
```
