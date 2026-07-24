---
name: golem-trigger-agent-effect
description: "Triggering a fire-and-forget CLI invocation on an Effect-based Golem agent. Use when asked to trigger, enqueue, or invoke an agent method without waiting using golem agent invoke --trigger."
---

# Triggering a Fire-and-Forget Effect Agent Invocation

Both `golem` and `golem-cli` can be used — all commands below work with either binary. Effect
agents use the ordinary Golem CLI; do not replace a requested shell operation with an in-agent
typed-client call.

## Usage

```shell
golem agent invoke --trigger <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

The `--trigger` (or `-t`) flag submits the invocation and returns after Golem accepts it. The
target method executes asynchronously; the command does not wait for its result or prove that its
side effects are already visible.

Use trigger mode when the caller does not need the return value, such as for background work,
long-running operations, fan-out, or notifications. Use regular `golem agent invoke` when the
caller needs the method result or must know that the invocation finished.

## Effect Agent Names and Arguments

For an agent declared with `defineAgent` and `method`:

- The agent type name is exactly the `name` passed to `defineAgent`.
- Constructor arguments follow the declaration order in `constructorParams`.
- The function name is exactly the key declared in `methods`, including TypeScript casing.
- Method arguments follow the declaration order in that method's `params`.

Do not infer the runtime name from an exported TypeScript binding or translate method names to
snake_case. The generated Effect starter declares its counter with the exact runtime name
`Counter`:

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

Trigger that starter agent as `Counter("c1")`, not `CounterAgent("c1")`:

```shell
golem agent invoke --trigger 'Counter("c1")' increment
```

CLI constructor and method arguments are positional values. Do not wrap them in the named input
objects used by the Effect SDK client. For example, a method declared with
`params: { amount: Schema.Number }` receives `5` from the CLI, not `{ amount: 5 }`. If a declared
parameter is itself a record, pass that record value as one shell argument.

## Examples

### Trigger a method with an argument

```shell
golem agent invoke --trigger 'MyAgent()' startBackgroundJob '"job-123"'
```

### Trigger in a specific environment

```shell
golem agent invoke --trigger \
  'staging/MyAgent("user-123")' sendNotification '"Your order is ready"'
```

### Trigger with an explicit idempotency key

```shell
golem agent invoke --trigger -i 'job-run-2026-07-19' \
  'BatchProcessor()' runDailyReport
```

## Result and Idempotency

A triggered invocation returns its effective idempotency key, not the method's success value.
Golem generates a key when none is supplied. Reusing an explicit key when retrying the command
prevents that invocation from executing more than once.

Do not invoke the method again merely to verify that trigger mode returned: a second invocation
changes state independently. When verification is required, wait or poll through a separate
read-only method whose contract exposes the relevant state.

## Available Options

| Option                        | Description                                                    |
| ----------------------------- | -------------------------------------------------------------- |
| `-t, --trigger`               | Trigger without waiting for the method result                  |
| `-i, --idempotency-key <KEY>` | Set an idempotency key; use `"-"` for an auto-generated key    |
| `--no-stream`                 | Disable live streaming of agent stdout, stderr, and logs       |
| `--schedule-at <DATETIME>`    | Schedule for an ISO 8601 / RFC 3339 time; requires `--trigger` |

If the component is not deployed and the command runs from its application directory, the CLI
can automatically build and deploy it before submitting the invocation.

## Effect CLI Value Syntax

Effect components report TypeScript as their source language, so agent IDs and method arguments
use **TypeScript syntax** even though contracts are declared with Effect Schema:

- Field names use `camelCase`.
- Options use the raw value or `undefined`.
- Records use `{ fieldOne: 1, fieldTwo: "hello" }`.
- Enums use `"variant-name"`.
- Variants use `{ tag: "variant-name", value: 42 }`.
- Tuples use `[1, "hello"]`.
- Results use `{ ok: value }` or `{ error: value }`.

Quote each structured value as one shell argument. String values need TypeScript string quotes
inside the shell quotes:

```shell
golem agent invoke --trigger 'MyAgent("user-123")' sendEmail \
  '{ to: "alice@example.com", subject: "Hello", body: "World" }'
```

## CLI Triggering vs Effect SDK Triggering

This skill covers invocation from a shell. Inside Effect agent code, a typed remote method instead
provides `remote.method.trigger(input)`, for example `remote.increment.trigger({})`. That returns a
lazy `Effect` and must be executed by composing or yielding it. It is not an alternative CLI
syntax for `golem agent invoke --trigger`.
