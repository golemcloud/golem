---
name: golem-invoke-agent-go
description: "Invoking a Go Golem agent method from the CLI and waiting for its result. Use when the user asks to call, invoke, or run a method on a deployed agent in a Go Golem project."
---

# Invoking a Go Golem Agent with `golem agent invoke`

## Overview

`golem agent invoke` calls a method on a deployed agent and **waits for the result**. The agent is created automatically on first invocation if it does not exist yet. Standard output, error, and log streams from the agent are streamed live to the terminal by default.

Both `golem` and `golem-cli` can be used — every command below works with either binary.

## Steps

1. **Identify the agent** by its type name and constructor parameters (`CounterAgent("my-counter")`).
2. **Pick the method** to call (the WIT name, e.g. `increment`, `add`, `value`).
3. **Run `golem agent invoke`** with the agent ID, method name, and any arguments.
4. **Read the result** — text by default, or `--format json` / `--format yaml` for machine output.

## Usage

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

## Agent ID Format

The agent ID identifies the agent type and its constructor parameters:

```
AgentTypeName(param1, param2, ...)
```

For a Go agent, the constructor parameters are the fields of its `ID` struct in declaration order. `CounterAgent`'s `ID` is `struct{ Name string }`, so its ID is `CounterAgent("my-counter")`. For an agent whose `ID` has no fields (a singleton), use empty parentheses: `AgentTypeName()`.

The agent ID can optionally be prefixed with environment or application paths:

| Format | Description |
|--------|-------------|
| `AgentTypeName(params)` | Standalone agent name |
| `env/AgentTypeName(params)` | Environment-specific |
| `app/env/AgentTypeName(params)` | Application and environment-specific |
| `account/app/env/AgentTypeName(params)` | Account, application, and environment-specific |

## Examples

### Invoke a method with no parameters

```shell
golem agent invoke 'CounterAgent("my-counter")' increment
```

### Invoke a method with an argument

`add` takes one parameter (the `By` field of `AddIn`), passed positionally:

```shell
golem agent invoke 'CounterAgent("my-counter")' add 5
```

### Read the current value

```shell
golem agent invoke 'CounterAgent("my-counter")' value
```

### Invoke in a specific environment

```shell
golem agent invoke 'staging/CounterAgent("my-counter")' value
```

### Invoke with an explicit idempotency key

```shell
golem agent invoke -i my-unique-key 'CounterAgent("my-counter")' increment
```

## Output

Text output renders return values in Go syntax, as in the value syntax below. Methods returning `golem.Unit` or no value print `void` in text mode.

For machine-readable output, use `--format json` or `--format yaml`. A single return value includes `result` plus `resultJson`; multiple return values include `result` plus `resultsJson`; methods returning no value omit result fields.

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | Only trigger the invocation without waiting for the result (fire-and-forget) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (requires `--trigger`; ISO 8601 format) |

## Value Syntax

Agent ID parameters and method arguments use Go syntax, read against the agent's schema:

- Strings `"my-counter"`, runes `'x'`, integers `5`, floats `2.5`, booleans `true` / `false`.
- Each exported field of a Go input struct is one positional parameter, named as the Go field name spelled unexported (`AmountCents` → `amountCents`, `UserID` → `userID`, `APIKey` → `apiKey`).
- Structs, lists and tuples are composite literals without their type, with field names as published: `{zone: "a", level: 2}`, `{"x", "y"}`.
- Maps: `{ "k" => 1 }`. Options (pointer fields): the value itself, or `nil`. Results: `Ok(v)` / `Err(e)`.
- Enum constants and variant cases by their published name: `closed`, `card{number: "1"}`, `amount(42)`.
- Rich values as constructors: `Uuid("…")`, `Datetime("2026-01-01T00:00:00Z")`, `Duration("PT30S")` (or `30 * time.Second`).

For example: `golem agent invoke 'ShelfAgent({zone: "a", level: 2}, closed, {"x"})' describe '"fragile"' '{ "k" => 2 }'`.

## Key Constraints

- `golem agent invoke` **blocks** until the agent returns; use `--trigger` for fire-and-forget.
- Every invocation uses an idempotency key (auto-generated if not supplied), guaranteeing at-most-once execution even if the CLI retries.
- If the component is not deployed yet and the CLI runs from an application directory, the command auto-builds and deploys it before invoking.
- The agent ID's constructor arguments must match the fields of the Go `ID` struct in order.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-trigger-agent-go` | Fire-and-forget invocation from the CLI (`--trigger`) |
| `golem-schedule-agent-go` | Schedule a future invocation from the CLI |
| `golem-create-agent-instance-go` | Pre-create an agent instance (`golem agent new`) |
| `golem-add-agent-go` | Define the agent and its methods |
