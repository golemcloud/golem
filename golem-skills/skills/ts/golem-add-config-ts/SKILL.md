---
name: golem-add-config-ts
description: "Adding typed configuration to a TypeScript Golem agent. Use when the user asks to add config, settings, or parameters to a TypeScript agent that can be set via golem.yaml or CLI."
---

# Adding Typed Configuration to a TypeScript Agent

## Overview

TypeScript agents receive typed configuration via `Config<T>` from `@golemcloud/golem-ts-sdk`. The configuration type is a plain TypeScript type literal that describes the shape of the config data. The `@agent()` decorator automatically detects `Config<T>` constructor parameters.

## Steps

1. **Define a config type** — use a TypeScript type literal (not a class or interface with methods)
2. **Add `Config<T>` to the constructor** — the decorator detects it automatically
3. **Access config via `config.value`** — config is loaded lazily when `.value` is accessed
4. **Set config in `golem.yaml` or via CLI**

## Example

### Config Type and Agent

```typescript
import { agent, BaseAgent, Config } from "@golemcloud/golem-ts-sdk";

type MyAgentConfig = {
  foo: number;
  bar: string;
  nested: {
    a: boolean;
    b: number[];
  };
};

@agent()
class MyAgent extends BaseAgent {
  constructor(readonly name: string, readonly config: Config<MyAgentConfig>) {
    super();
  }

  getFoo(): number {
    return this.config.value.foo;
  }
}
```

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
golem agent new my-ns:my-component/my-agent-1 \
  --config foo=42 \
  --config bar=hello \
  --config nested.a=true
```

### RPC Config Overrides

When calling another agent, pass config overrides via `getWithConfig`:

```typescript
const client = MyAgent.getWithConfig("agent-1", {
  foo: 99,
  nested: { a: false },
});
const result = await client.getFoo();
```

## Config Cascade

Config values are resolved in order of increasing precedence:

`componentTemplates` → `components` → `agents` → `presets`

Values set closer to the agent override those set at broader scopes.

## Key Constraints

- Config types must be plain TypeScript type literals — not classes or interfaces with methods
- The `Config<T>` parameter in the constructor is automatically detected by `@agent()`
- Optional fields use `?` syntax (e.g., `timeout?: number`)
- Config is loaded when `.value` is accessed
- Config keys in `golem.yaml` use camelCase matching the TypeScript property names
- If the config includes `Secret<T>` fields, also use `golem-add-secret-ts` for secret-specific declaration and CLI guidance
