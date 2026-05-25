---
title: "Golem 1.5 features — Part 12: REPL"
date: "2026-04-20T20:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-12-repl"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part12-repl/"
---

## Introduction

This post is part of a series showcasing new Golem 1.5 features (releasing end of April 2026). It assumes reader familiarity with Golem and references prior posts in the series for background. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Testing agents with a REPL

The Golem REPL serves as an important element of testing agents during development. Previously, it relied on Rib, a custom scripting language. However, since Golem moved to code-first routes, maintaining a separate scripting language became redundant.

## TypeScript REPL

Golem removed Rib entirely and made it available as a standalone tool for potential wasmtime integration. The new primary REPL is a real TypeScript REPL with agent client classes from bridge libraries preconfigured globally.

The TypeScript REPL works just like you would expect and supports calling any agent type in a type-safe manner, regardless of the agent's implementation language.

**CLI commands from the REPL:**

Users can run all the usual commands such as `build`, `deploy`, or inspecting agent logs without leaving the REPL using either `.` or `:` prefixes.

**Running scripts:**

TypeScript scripts can execute non-interactively:

```typescript
const c2 = await CounterAgent.get("c2");
await c2.increment();
await c2.increment();
```

```bash
$ cat test.ts
const c2 = await CounterAgent.get("c2")
await c2.increment()
await c2.increment()

$ golem repl --script-file test.ts --yes
2
```

## Rust REPL

A Rust REPL exists but is very slow because it has to continuously recompile things. It's retained primarily for Rust script execution where performance impact is less significant.

```rust
let c3 = CounterAgent::get("c3").await.unwrap();
c3.increment().await;
c3.increment().await;
```

```bash
$ cat test.rs
let c3 = CounterAgent::get("c3").await.unwrap();
c3.increment().await;
c3.increment().await;

$ golem repl --script-file test.rs --language rust --yes
2
```

## Scala and MoonBit

Scala and MoonBit don't have any REPL implementation in Golem 1.5, though the TypeScript and Rust REPLs fully support agents written in those languages.
