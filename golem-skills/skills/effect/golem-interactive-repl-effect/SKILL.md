---
name: golem-interactive-repl-effect
description: "Using the Golem TypeScript REPL with Effect-based agents for interactive testing and scripts. Use when asked to test Effect agents interactively, run a REPL, or execute test scripts against deployed agents."
---

# Golem Interactive REPL (Effect)

The `golem repl` command starts an interactive TypeScript REPL for testing and scripting agents.
It supports both interactive and script execution modes.

Effect components intentionally use the **TypeScript REPL and generated TypeScript Bridge SDK**.
There is no separate Effect REPL language and `@golemcloud/effect-golem` does not provide a REPL
helper. Do not import `Effect`, call `Effect.runPromise`, or use handler-side Effect values in a
REPL script.

## Interactive Mode

From the Golem application directory, run:

```shell
golem repl
```

For an Effect application, Golem automatically selects the TypeScript REPL. To select it
explicitly, use `--language ts` or `--language typescript`, not `--language effect`.

The REPL puts the generated agent client classes in the global scope. A client's class name is the
agent's exact `defineAgent` `name`. For example, the default Effect project defines an agent named
`Counter`, so its client is `Counter`, not `CounterAgent`:

```typescript
const c1 = await Counter.get("c1")
await c1.increment()
await c1.increment()
const value = await c1.value()
```

No imports are needed. `get(...)` gets or creates a durable agent, and generated client methods
return promises, so await both client creation and method calls.

### TypeScript Names and Values

Use the generated TypeScript client signatures as the source of truth:

- Use the exact `defineAgent` name for the client class.
- Effect method names remain TypeScript-cased, such as `createItem` or `increment`.
- Pass constructor and method parameters positionally in their declared order.
- Use TypeScript values: quoted strings, object literals for records, arrays for lists and tuples,
  and the generated TypeScript shapes for other types.

An Effect handler may receive a named parameter object such as `({ item }) => ...`, but its Bridge
SDK method takes the generated positional arguments, such as `await agent.createItem(item)`. Do not
copy Effect handler signatures or internal `WitCodec` representations into REPL calls. Use REPL
completion or `.agent-type-info` when a generated signature or value shape is unclear.

### Built-in Commands

Built-in commands accept both `.` and `:` prefixes:

- `.build` / `:build` — build the project
- `.deploy` / `:deploy` — deploy components
- `.help` / `:help` — show available commands

## Script Mode

Write a TypeScript file; the generated clients are globals here too:

```typescript
// test.ts
const c2 = await Counter.get("c2")
await c2.increment()
await c2.increment()
```

Run it non-interactively:

```shell
golem repl --script-file test.ts --yes
```

The `--yes` flag auto-confirms prompts. Run the script after creating it unless the user explicitly
asks not to.

## Available REPL Languages

| Language | Flag | Interactive | Script | Notes |
|----------|------|-------------|--------|-------|
| TypeScript | (default) | ✅ | ✅ | Used for Effect and all other component languages |

## Prerequisites

- The Golem server must be running (`golem server run`).
- The application must have a manifest environment and its components must be deployable.
- Run commands from the application directory so Golem can generate and configure the Bridge SDK.
