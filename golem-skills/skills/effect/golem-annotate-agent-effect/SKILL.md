---
name: golem-annotate-agent-effect
description: "Adding prompt hints and descriptions to Effect-based Golem agents and methods. Use when an @golemcloud/effect-golem agent needs AI/LLM discovery metadata or human-readable method documentation."
---

# Annotating Effect Agents and Methods

Effect Golem agents declare AI/LLM discovery metadata as fields in their `defineAgent(...)` and
`method(...)` specs. Do not translate decorator syntax from another SDK: Effect agents do not use
annotation decorators.

## Metadata Fields

- Top-level `description` describes the agent type and its overall purpose.
- Top-level `promptHint`, when needed, tells an LLM when to construct or select an agent instance.
- Method-level `promptHint` tells an LLM when to invoke that method.
- Method-level `description` documents the method's behavior, inputs, outputs, and edge cases.

The method name exposed by Golem is the key in the `methods` record, so use TypeScript camelCase
there. A request for a method's “prompt” annotation maps to the Effect SDK's `promptHint` field.

## Agent and Method Metadata

Set agent metadata directly on the `defineAgent` spec and method metadata directly inside
`method(...)`:

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const CounterAgent = defineAgent({
  name: "CounterAgent",
  description: "A test counter agent for tracking numeric values",
  mode: "durable",
  constructorParams: {
    count: Schema.Number,
  },
  methods: {
    getDouble: method({
      params: {},
      success: Schema.Number,
      promptHint: "Get the doubled value of the counter",
      description:
        "Returns the current count multiplied by two. Useful for scaling operations.",
    }),
  },
}).implement(({ count }) =>
  Effect.succeed({
    getDouble: () => Effect.succeed(count * 2),
  }),
);
```

When editing an existing stateful agent, preserve its snapshot and implementation structure. Add
the method spec under `methods`, then add a matching Effect-returning handler to the object returned
by `.implement(...)`. Metadata belongs only on the specs; it does not change handler behavior.

## Key Constraints

- Import `defineAgent` and `method` from `@golemcloud/effect-golem`, and `Effect` and `Schema` from
  `effect`.
- Use `description` and `promptHint`, not `prompt`, decorator syntax, or helpers from
  `@golemcloud/golem-ts-sdk`.
- Keep method `params`, `success`, and optional `error` schemas unchanged when adding metadata.
- Both metadata fields are optional; omit them for methods that should not carry discovery hints.
- Keep the top-level `.implement(...)` registration and the implementation module's side-effect
  import from `src/main.ts`.
- Run `golem build` and redeploy after changing metadata so discovery reports the new values.
