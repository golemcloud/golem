---
name: golem-add-config-ts
description: "Adding typed configuration to a TypeScript Golem agent. Use when the user asks to add config, settings, or parameters to a TypeScript agent that can be set via golem.yaml or CLI."
---

# Adding Typed Configuration to a TypeScript Agent

## Overview

TypeScript agents declare typed configuration with the `config` option on `defineAgent(...)`. It is a single record of named fields, each a Standard Schema value (Zod / Valibot / ArkType / Effect Schema). At runtime the config is read through `this.config` inside a handler — one property per declared field, freshly read on each access, and fully typed from the schema.

## Steps

1. **Declare a `config` record** on `defineAgent(...)` — one schema per field
2. **Read config via `this.config.<field>`** in handlers (each access reads the live value)
3. **Set config in `golem.yaml` or via CLI**

## Example

### Agent with Config

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const MyAgent = defineAgent({
    name: 'MyAgent',
    id: { name: z.string() },
    // One config record; nested objects are supported and recursed field-by-field.
    config: {
        foo: z.number(),
        bar: z.string(),
        nested: z.object({
            a: z.boolean(),
            b: z.array(z.number()),
        }),
    },
    methods: {
        getFoo: method({ input: {}, returns: z.number() }),
        describe: method({ input: {}, returns: z.string() }),
    },
});

export const MyAgentImpl = MyAgent.implement({
    init: () => ({}),
    methods: {
        getFoo() {
            // `this.config` is statically typed from the config record — no casts,
            // and `this.config.<typo>` is a compile error.
            return this.config.foo;
        },
        describe() {
            const n = this.config.nested;
            return `${this.config.bar}:${n.a}:${n.b.join(',')}`;
        },
    },
});
```

`this.config` is also available during `init` via the `InitContext` argument (`init: (ctx) => ({ ... ctx.config.foo ... })`).

### Setting Config in `golem.yaml`

```yaml
agents:
  MyAgent:
    config:
      foo: 42
      bar: "hello"
      nested:
        a: true
        b: [1, 2, 3]
```

### Setting Config via CLI

```shell
golem agent new 'MyAgent("agent-1")' \
  --config foo=42 \
  --config bar=hello \
  --config nested.a=true
```

## Config Cascade

Config values are resolved in order of increasing precedence:

`componentTemplates` → `components` → `agents` → `presets`

Values set closer to the agent override those set at broader scopes.

## Key Constraints

- `config` is a single record on `defineAgent(...)`; each field is a Standard Schema value (not a class or interface with methods)
- Read config through `this.config.<field>` — each access reads the current value, so config changes between invocations are observed
- Only object/record schemas are recursed into nested fields; unions, arrays, tuples, maps, and primitives are read whole
- Optional fields use the schema's own optionality (e.g. `z.number().optional()`)
- Config keys in `golem.yaml` use camelCase matching the field names
- Config values are provisioned per environment (via `golem.yaml` / CLI); a caller may ALSO override non-secret config for a remote agent at call time via `clientFor(Def)(id, phantomId?, overrides)` (config-on-RPC — secret overrides are rejected)
- If the config includes secret fields, mark them with `s.secret(...)` and see `golem-add-secret-ts` for secret-specific declaration and CLI guidance
