---
name: golem-create-agent-instance-effect
description: "Pre-creating Effect-based Golem agent instances with golem agent new. Use when asked to create, instantiate, or initialize an agent without invoking a method."
---

# Creating an Effect Golem Agent Instance with `golem agent new`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent new <AGENT_ID> [OPTIONS]
```

This **creates a new agent instance** without invoking any method on it. The agent is initialized
with its constructor parameters and starts idle, ready to receive invocations. If the agent's
component has not been deployed and the CLI is run from an application directory, the command
automatically builds and deploys the component first.

Unlike `golem agent invoke`, which implicitly creates an agent on its first call, `golem agent new`
explicitly pre-creates it. Use this when environment variables or typed configuration must be
supplied before any invocation.

The Effect SDK has no ordinary programmatic create-agent helper. Do not replace this command with a
typed RPC client or `newPhantom`; those APIs serve different purposes.

## Agent ID Format

The agent ID identifies the agent type and its constructor parameters:

```
AgentTypeName(param1, param2, ...)
```

For an Effect agent declared with `defineAgent`, use the exact `name` string as `AgentTypeName` and
pass values in the declaration order of `constructorParams`. Do not infer the type name from the
exported TypeScript constant, filename, or description, and do not append an `Agent` suffix unless
it is present in `name`.

For example, this declaration is addressed as `Counter("c1")`, not `CounterAgent("c1")`:

```typescript
export const Counter = defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  // ...
});
```

Before creating an instance, inspect the agent's `defineAgent` declaration or discover deployed
constructors with `golem agent-type list` and `golem agent-type get <AGENT_TYPE>`. If the CLI reports
that a type is not found, use the exact name from its list of available deployed constructors.

The agent ID can optionally be prefixed with environment or application paths:

| Format | Description |
|--------|-------------|
| `AgentTypeName(params)` | Standalone agent name |
| `env/AgentTypeName(params)` | Environment-specific |
| `app/env/AgentTypeName(params)` | Application and environment-specific |
| `account/app/env/AgentTypeName(params)` | Account, application, and environment-specific |

For agents with no constructor parameters, use empty parentheses: `AgentTypeName()`.

## Examples

### Create an agent with no constructor parameters

```shell
golem agent new 'MyAgent()'
```

### Create an agent with constructor parameters

```shell
golem agent new 'Counter("c1")'
```

### Create an agent with environment variables

```shell
golem agent new 'MyAgent("user-123")' --env API_KEY=[REDACTED:api-key] --env LOG_LEVEL=debug
```

### Create an agent with configuration

```shell
golem agent new 'MyAgent()' --config maxRetries=5 --config timeoutSeconds=30
```

### Create an agent in a specific environment

```shell
golem agent new 'staging/MyAgent("user-123")'
```

### Combine environment variables and configuration

```shell
golem agent new 'OrderProcessor("us-east")' \
  --env DATABASE_URL=postgres://... \
  --config batchSize=100 \
  --config retryPolicy.maxAttempts=3
```

## Available Options

| Option | Description |
|--------|-------------|
| `-e, --env <ENV=VAL>` | Environment variables visible to the agent (can be repeated) |
| `-c, --config <PATH=VALUE>` | Typed configuration entries using dot-separated paths (can be repeated). Only configuration declared by the agent can be provided. If omitted, defaults come from `agents.*.config` in the manifest. |

## Auto-Deploy

When run from an application directory, `golem agent new` automatically builds and deploys an
agent component that has not been deployed yet.

## Verify Creation Without Invoking a Method

Use `golem agent get` with the same agent ID to verify that the pre-created instance exists:

```shell
golem agent get 'Counter("c1")'
```

A successful metadata lookup verifies existence without calling an agent method. Do not require a
particular textual entry from `golem agent oplog`: pre-creation does not guarantee that the CLI's
rendered oplog contains a `create` line.

## Effect CLI Value Syntax

Effect components are reported as TypeScript source, so constructor values use **TypeScript
syntax**, even though the agent implementation uses Effect Schema:

- Field names use `camelCase`
- Options: raw value or `undefined`
- Records: `{ fieldOne: 1, fieldTwo: "hello" }`
- Enums: `"variant-name"`
- Variants: `{ tag: "variant-name", value: 42 }`
- Tuples: `[1, "hello"]`
- Results: `{ ok: value }` / `{ error: value }`

```shell
golem agent new 'UserManager({ region: "us-east", maxConnections: 10 })'
```

These are CLI expressions, not in-process Effect values. Do not put `Option.some(...)`,
`Result.succeed(...)`, or an Effect Schema tagged-union object into the shell command.

Quote agent IDs containing parentheses, strings, objects, or spaces so the shell passes the entire
ID as one argument.

## When to Use `golem agent new` vs `golem agent invoke`

- Use `golem agent new` to **pre-create** an agent with specific environment variables or
  configuration before any invocation.
- Use `golem agent invoke` to call a method; a durable agent is created automatically on first
  invocation if it does not exist.
- Invoke a pre-created agent later with the same agent ID.
