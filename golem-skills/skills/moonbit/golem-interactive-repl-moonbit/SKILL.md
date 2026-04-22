---
name: golem-interactive-repl-moonbit
description: "Using the Golem REPL for interactive testing and scripting of agents. Use when asked to test agents interactively, run a REPL, or execute test scripts against deployed agents."
---

# Golem Interactive REPL (MoonBit)

The `golem repl` command starts an interactive REPL for testing and scripting agents. It supports both an **interactive mode** and a **script execution mode**.

**There is no MoonBit REPL language** in Golem 1.5. Use the **TypeScript REPL** (the default) to interact with your MoonBit agents — all agent types are available as TypeScript classes regardless of the agent's source language.

## Interactive Mode

```shell
golem repl
```

Starts a TypeScript REPL with all agent client classes preconfigured in the global scope — including your MoonBit agents. You can get or create agent instances, invoke methods, and see logs streamed during invocations, all with full type safety.

### Example: Invoking a MoonBit counter agent from the TypeScript REPL

```typescript
const c1 = await CounterAgent.get("c1")
await c1.increment()
await c1.increment()
const value = await c1.getValue()
```

### Built-in Commands

The REPL provides built-in commands that mirror CLI functionality. Commands accept both `.` and `:` prefixes:

- `.build` / `:build` — build the project
- `.deploy` / `:deploy` — deploy components
- `.help` / `:help` — show available commands

This lets you build, deploy, and test agents without leaving the REPL.

## Script Mode

Run a TypeScript file non-interactively:

```shell
golem repl --script-file test.ts --yes
```

The `--yes` flag auto-confirms any prompts. The script has the same global scope as the interactive REPL — all agent client classes are available, including MoonBit agents.

### Example script (`test.ts`)

```typescript
const c2 = await CounterAgent.get("c2")
await c2.increment()
await c2.increment()
```

Run it:

```shell
golem repl --script-file test.ts --yes
```

## Available REPL Languages

| Language | Flag | Interactive | Script | Notes |
|----------|------|-------------|--------|-------|
| TypeScript | (default) | ✅ | ✅ | Recommended — works with all agent languages |
| Rust | `--language rust` | ✅ | ✅ | Slow interactive (recompiles); scripts are practical |

No MoonBit REPL is available yet. The TypeScript REPL is recommended for interacting with MoonBit agents.

## Prerequisites

- The Golem server must be running (`golem server run`)
- Components must be deployed (`golem deploy`)
