---
name: golem-interactive-repl-rust
description: "Using the Golem REPL for interactive testing and scripting of agents. Use when asked to test agents interactively, run a REPL, or execute test scripts against deployed agents."
---

# Golem Interactive REPL (Rust)

The `golem repl` command starts an interactive REPL for testing and scripting agents. It supports both an **interactive mode** and a **script execution mode**.

## Interactive Mode

```shell
golem repl --language rust
```

Starts a Rust REPL with all agent client types preconfigured in scope. You can get or create agent instances, invoke methods, and see logs streamed during invocations.

**Note:** The Rust REPL is slow interactively because it recompiles on each input. For quick interactive exploration, consider using the TypeScript REPL instead (`golem repl` without `--language`). For scripted test sequences, the Rust REPL works well.

### Example: Invoking a counter agent

```rust
let c1 = CounterAgent::get("c1").await.unwrap();
c1.increment().await;
c1.increment().await;
```

### Built-in Commands

The REPL provides built-in commands that mirror CLI functionality. Commands accept both `.` and `:` prefixes:

- `.build` / `:build` — build the project
- `.deploy` / `:deploy` — deploy components
- `.help` / `:help` — show available commands

## Script Mode

To run a Rust test script:

1. **Create** the script file (e.g., `test.rs`) with Rust REPL syntax
2. **Execute** it immediately with `golem repl`:

```shell
golem repl --script-file test.rs --language rust --yes
```

The `--yes` flag auto-confirms any prompts. The script has the same scope as the interactive REPL — all agent client types are available.

Run the script after creating it, unless the user explicitly asked not to.

### Example script (`test.rs`)

```rust
let c2 = CounterAgent::get("c2").await.unwrap();
c2.increment().await;
c2.increment().await;
```

## Recommended REPL Language

For Rust projects, the **Rust REPL** is available and lets you use the same language as your agents in scripts. However, the **TypeScript REPL** (`golem repl` without `--language`) is faster for interactive use because it doesn't need to recompile.

Note: the REPL can interact with agents written in **any language** (Rust, TypeScript, Scala, MoonBit) — the REPL language is independent of the agent's implementation language. All agent types are available regardless of the agent's source language.

## Available REPL Languages

| Language | Flag | Interactive | Script | Notes |
|----------|------|-------------|--------|-------|
| TypeScript | (default) | ✅ | ✅ | Fast interactive, recommended for exploration |
| Rust | `--language rust` | ✅ | ✅ | Slow interactive (recompiles); scripts are practical |

## Prerequisites

- The Golem server must be running (`golem server run`)
- Components must be deployed (`golem deploy`)
