---
name: golem-create-agent-instance-go
description: "Creating a new Go Golem agent instance with golem agent new. Use when the user asks to create, instantiate, or pre-create an agent without invoking a method in a Go Golem project."
---

# Creating a Go Golem Agent Instance with `golem agent new`

## Overview

`golem agent new` **creates a new agent instance** without invoking any method on it. The agent is initialized with its constructor parameters and starts in an idle state, ready to receive invocations.

Unlike `golem agent invoke` — which implicitly creates the agent on first call — `golem agent new` explicitly pre-creates it, which is useful when you need to set environment variables or configuration at creation time.

Both `golem` and `golem-cli` can be used — every command below works with either binary.

## Steps

1. **Identify the agent** by its type name and constructor parameters (`CounterAgent("my-counter")`).
2. **Choose any creation-time settings** — `--env`, `--config` — if needed.
3. **Run `golem agent new`** to pre-create the instance.
4. **Invoke it later** with `golem agent invoke` using the same agent ID.

## Usage

```shell
golem agent new <AGENT_ID> [OPTIONS]
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

### Create an agent with constructor parameters

```shell
golem agent new 'CounterAgent("my-counter")'
```

### Create a singleton agent (no constructor parameters)

```shell
golem agent new 'CounterAgent()'
```

### Create an agent with environment variables

```shell
golem agent new 'CounterAgent("my-counter")' --env API_KEY=sk-abc123 --env LOG_LEVEL=debug
```

### Create an agent with configuration

```shell
golem agent new 'CounterAgent("my-counter")' --config max_retries=5 --config timeout_seconds=30
```

### Create an agent in a specific environment

```shell
golem agent new 'staging/CounterAgent("my-counter")'
```

### Combine environment variables and configuration

```shell
golem agent new 'CounterAgent("my-counter")' \
  --env DATABASE_URL=postgres://... \
  --config batch_size=100 \
  --config retry_policy.max_attempts=3
```

## Available Options

| Option | Description |
|--------|-------------|
| `-e, --env <ENV=VAL>` | Environment variables visible to the agent (can be repeated) |
| `-c, --config <PATH=VALUE>` | Configuration entries for the agent, using dot-separated paths (can be repeated). Only configuration declared by the agent can be provided. If not provided, the default from the manifest (`agents.*.config`) is used. |

## When to Use `golem agent new` vs `golem agent invoke`

- Use `golem agent new` when you need to **pre-create** an agent with specific environment variables or configuration before any invocation.
- Use `golem agent invoke` when you want to call a method — the agent is created automatically on first invocation if it doesn't exist.
- An agent created with `golem agent new` can be invoked later with `golem agent invoke` using the same agent ID.

## Value Syntax

Agent ID parameters use Go syntax, read against the agent's schema:

- Strings `"my-counter"`, runes `'x'`, integers `5`, floats `2.5`, booleans `true` / `false`.
- Each exported field of a Go input struct is one positional parameter, named by lower-casing the first letter of the Go field name (`AmountCents` → `amountCents`).
- Structs, lists and tuples are composite literals without their type, with field names as published: `{zone: "a", level: 2}`, `{"x", "y"}`.
- Maps: `{ "k" => 1 }`. Options (pointer fields): the value itself, or `nil`. Results: `Ok(v)` / `Err(e)`.
- Enum constants and variant cases by their published name: `closed`, `card{number: "1"}`, `amount(42)`.
- Rich values as constructors: `Uuid("…")`, `Datetime("2026-01-01T00:00:00Z")`, `Duration("PT30S")` (or `30 * time.Second`).

For example: `golem agent invoke 'ShelfAgent({zone: "a", level: 2}, closed, {"x"})' describe '"fragile"' '{ "k" => 2 }'`.

## Key Constraints

- The constructor arguments must match the fields of the Go `ID` struct in order.
- If the component is not deployed yet and the CLI runs from an application directory, `golem agent new` auto-builds and deploys it before creating the agent.
- Only configuration declared by the agent can be provided via `-c`.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-invoke-agent-go` | Invoke a method on the agent (creates it if absent) |
| `golem-trigger-agent-go` | Fire-and-forget invocation from the CLI |
| `golem-add-agent-go` | Define the agent, its `ID`, and its methods |
| `golem-multi-instance-agent-go` | Address many instances of an agent by ID |
