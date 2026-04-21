---
name: golem-interactive-repl-ts
description: "Using the Golem REPL for interactive testing and scripting of agents. Use when asked to test agents interactively, run a REPL, or execute test scripts against deployed agents."
---

# Golem Interactive REPL (TypeScript)

The `golem repl` command starts an interactive REPL for testing and scripting agents. It supports both an **interactive mode** and a **script execution mode**.

## Interactive Mode

```shell
golem repl
```

Starts a TypeScript REPL with all agent client classes from the bridge libraries preconfigured in the global scope. You can get or create agent instances, invoke methods, and see logs streamed during invocations — all with full type safety.

### Example: Invoking a counter agent

```typescript
const c1 = await CounterAgent.get("c1")
await c1.increment()
await c1.increment()
const value = await c1.getValue()
```

The REPL uses TypeScript syntax and has all agent types available automatically. No imports needed.

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

The `--yes` flag auto-confirms any prompts. The script has the same global scope as the interactive REPL — all agent client classes are available.

Run the script after creating it, unless the user explicitly asked not to.

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

## Recommended REPL Language

The **TypeScript REPL** is the recommended choice for TypeScript projects. It uses the same language as your agents, so the syntax feels natural.

Note: the REPL can interact with agents written in **any language** (Rust, Scala, MoonBit) — the REPL language is independent of the agent's implementation language. All agent types are available as TypeScript classes regardless of the agent's source language.

## Available REPL Languages

| Language | Flag | Interactive | Script | Notes |
|----------|------|-------------|--------|-------|
| TypeScript | (default) | ✅ | ✅ | Recommended — fast, type-safe |
| Rust | `--language rust` | ✅ | ✅ | Slow interactive (recompiles); scripts are practical |

## Prerequisites

- The Golem server must be running (`golem server run`)
- Components must be deployed (`golem deploy`)
