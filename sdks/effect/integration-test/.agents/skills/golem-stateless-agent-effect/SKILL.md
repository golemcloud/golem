---
name: golem-stateless-agent-effect
description: "Creates ephemeral, stateless Effect Golem agents with fresh handlers for every invocation. Use for pure request handlers, independent transformations, or agents that must not share state between calls."
---

# Creating Ephemeral Agents with Effect

An ephemeral agent gets a fresh implementation for every invocation. Its constructor parameters
still describe its logical identity, but values captured by one invocation's handlers are discarded
when that invocation completes. The oplog remains available for inspection, but Golem does not
replay it to recover an ephemeral agent.

Use ephemeral agents for independent request handling, transformations, validation, adapters, and
other work where one call must not depend on an earlier call. Use a durable agent instead for
counters, carts, workflows, long-running orchestration, or external operations that require durable
recovery.

## Define and Implement the Agent

Set `mode: "ephemeral"` in `defineAgent(...)`. Declare constructor and method contracts with Effect
Schema, return `Effect` values from every handler, and omit `snapshot` for a stateless agent:

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const TextProcessorAgent = defineAgent({
  name: "TextProcessorAgent",
  mode: "ephemeral",
  constructorParams: {
    processorName: Schema.String,
  },
  methods: {
    toUpper: method({
      params: { input: Schema.String },
      success: Schema.String,
    }),
    toLower: method({
      params: { input: Schema.String },
      success: Schema.String,
    }),
  },
}).implement(() =>
  Effect.succeed({
    toUpper: ({ input }) => Effect.succeed(input.toUpperCase()),
    toLower: ({ input }) => Effect.succeed(input.toLowerCase()),
  }),
);
```

The `.implement(...)` call registers the agent eagerly when the module is evaluated. Import the
implementation module from the component entry point so registration occurs:

```typescript
// src/main.ts
import "./text-processor-agent.js";
```

Generated Effect projects use ESM and NodeNext module resolution, so local imports use the emitted
`.js` suffix.

## Fresh Invocation Semantics

For every invocation, Golem:

1. Starts a fresh ephemeral agent instance.
2. Runs the `.implement(...)` Effect to obtain that instance's handlers.
3. Runs the requested handler.
4. Discards the instance and its captured in-memory values.

Do not add `Snapshot.define(...)`, call `snapshot.init(...)`, or create mutable state merely to
represent a stateless handler. Omitting `snapshot` disables snapshotting. If calls must observe or
update shared state, the agent is not stateless; use a durable agent and the normal snapshot-backed
state pattern instead.

## Effect and CLI Conventions

- Import Effect APIs from `effect` and Golem APIs from `@golemcloud/effect-golem`.
- Use `Schema` values for constructor parameters, method parameters, successes, and typed errors.
- Handlers receive named parameter records and return Effects; do not use plain `async` or
  Promise-returning handlers from the non-Effect TypeScript SDK.
- Method names use TypeScript casing. For the example above, invoke `toUpper` and `toLower`, not
  snake_case names.
- Effect components report TypeScript as their source language, so CLI agent IDs and method
  arguments use TypeScript value syntax.
- Constructor parameters remain part of the CLI agent ID. The example is addressed as
  `TextProcessorAgent("processor-1")`.
- Ephemeral agents can still expose HTTP routes, call other agents, and use Golem host APIs;
  ephemeral mode changes instance lifetime and recovery, not the available capabilities.
- Do not call a runner or registration helper. The top-level `.implement(...)` call plus the
  `src/main.ts` side-effect import is the registration path.

## Choosing Ephemeral Mode

Choose `mode: "ephemeral"` when every invocation is independent and losing its in-memory values at
the end of the call is intentional. Keep the default durable mode when:

- a result depends on an earlier invocation;
- an agent coordinates a workflow or saga;
- state must survive suspension, failure, or update; or
- host-operation replay and durable recovery are part of the correctness model.
