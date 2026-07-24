---
name: golem-schedule-agent-effect
description: "Scheduling a future CLI invocation on an Effect-based Golem agent. Use when asked to schedule, delay, or plan an agent method call using golem agent invoke --trigger --schedule-at."
---

# Scheduling a Future Effect Agent Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary. Effect
agents use the ordinary Golem CLI; do not replace this CLI operation with an in-agent typed-client
call.

## Usage

```shell
golem agent invoke --trigger --schedule-at <DATETIME> <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

The `--schedule-at` flag schedules the invocation to execute at a specific future time. It
**requires** `--trigger` because scheduled CLI invocations are fire-and-forget: the CLI returns
after the invocation is enqueued rather than waiting for the method result.

When the task is only to schedule a call, return after the CLI accepts it. Do not wait for the
scheduled time or invoke the method again as a verification step; either action can change the
agent state the caller expects to observe before execution.

## Effect Agent Names and Arguments

For an agent declared with `defineAgent`:

- The agent type name is exactly the `name` passed to `defineAgent`.
- Constructor arguments follow the declaration order in `constructorParams`.
- The function name is exactly the key declared in `methods`, including its TypeScript casing.
- Method arguments follow the declaration order in that method's `params`.

Do not infer an agent type from the exported TypeScript binding or translate method names to
snake_case. The generated Effect starter currently declares its counter with the exact runtime
name `Counter`:

```typescript
export const Counter = defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  methods: {
    increment: method({
      params: {},
      success: Schema.Number,
    }),
  },
});
```

Schedule that starter agent as `Counter("c1")`, not `CounterAgent("c1")`:

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z \
  'Counter("c1")' increment
```

If an exported TypeScript binding and the `name` metadata differ, the metadata still controls the
CLI agent type.

The shell arguments are positional CLI values. Do not pass the SDK client's named input objects,
such as `{ name: "c1" }` or `{ amount: 5 }`, as wrappers around constructor or method argument
lists.

## DateTime Format

The `--schedule-at` value must be a future **ISO 8601 / RFC 3339** timestamp with a timezone:

```text
2026-03-15T10:30:00Z
2026-03-15T10:30:00+02:00
```

When the request is relative, such as "15 seconds from now," compute the future timestamp at the
time the command runs and pass the resulting RFC 3339 value to `--schedule-at`.

## Examples

### Schedule a method with no parameters

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z \
  'MyAgent()' runDailyReport
```

### Schedule with constructor and method parameters

```shell
golem agent invoke --trigger --schedule-at 2026-04-01T00:00:00Z \
  'BatchProcessor("daily")' generateReport '"Q1-2026"'
```

### Schedule in a specific environment

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T08:00:00Z \
  'production/NotificationAgent()' sendReminders
```

### Schedule with an idempotency key

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z \
  -i 'report-2026-03-15' 'ReportAgent()' generateDaily
```

## Available Options

| Option                        | Description                                                 |
| ----------------------------- | ----------------------------------------------------------- |
| `-t, --trigger`               | **Required** with `--schedule-at`; fire-and-forget mode     |
| `--schedule-at <DATETIME>`    | Future execution time in ISO 8601 / RFC 3339 format         |
| `-i, --idempotency-key <KEY>` | Set an idempotency key; use `"-"` for an auto-generated key |
| `--no-stream`                 | Disable live streaming of agent stdout, stderr, and logs    |

## How It Works

1. The CLI submits the invocation and scheduled time to the Golem server.
2. The server enqueues the invocation for the requested time.
3. The CLI returns immediately with the invocation's idempotency key.
4. At the scheduled time, the Golem runtime invokes the exact declared agent method.

Providing the same idempotency key when retrying the scheduling command prevents the invocation
from being executed more than once. If the component has not been deployed and the CLI runs from
its application directory, the command can automatically build and deploy it before scheduling.

## Effect CLI Value Syntax

Effect components report TypeScript as their source language, so agent IDs and method arguments
use **TypeScript syntax** even though their contracts use Effect Schema:

- Field names use `camelCase`.
- Options use the raw value or `undefined`.
- Records use `{ fieldOne: 1, fieldTwo: "hello" }`.
- Enums use `"variant-name"`.
- Variants use `{ tag: "variant-name", value: 42 }`.
- Tuples use `[1, "hello"]`.
- Results use `{ ok: value }` or `{ error: value }`.

Quote structured values as one shell argument. String values need TypeScript string quotes inside
the shell quotes:

```shell
golem agent invoke --trigger --schedule-at 2026-03-15T10:30:00Z \
  'MyAgent("user-123")' runTask '{ priority: 1, retry: true }'
```

## CLI Scheduling vs Effect SDK Scheduling

This skill covers scheduling from a shell. The Effect SDK separately exposes
`remote.method.schedule(scheduledAt, input)` for calls made from inside Effect agent code. That API
uses an SDK datetime value and a named input object and returns a cancellation handle; it is not a
replacement syntax for `golem agent invoke --trigger --schedule-at`.
