---
name: golem-add-secret-ts
description: "Adding secrets to TypeScript Golem agents. Use when the user asks to add secrets, store API keys, manage sensitive config values, or use Secret<T> in TypeScript agents."
---

# Adding Secrets to TypeScript Golem Agents

## Overview

Secrets are sensitive configuration values (API keys, passwords, tokens) stored per-environment and accessed via `Secret<T>` from `@golemcloud/golem-ts-sdk`. Unlike regular config fields, secret values are fetched on demand and managed separately from the agent config.

## Declaring Secrets in the Config Type

Wrap sensitive fields with `Secret<T>` in your config type:

```typescript
import { Config, Secret } from "@golemcloud/golem-ts-sdk";

type MyAgentConfig = {
  name: string;
  apiKey: Secret<string>;
  db: {
    host: string;
    port: number;
    password: Secret<string>;
  };
};
```

## Using Secrets in Agent Code

Call `.get()` on a `Secret<T>` field to fetch the current value:

```typescript
import { agent, BaseAgent, Config, Secret } from "@golemcloud/golem-ts-sdk";

@agent()
export class MyAgent extends BaseAgent {
  constructor(readonly id: string, readonly config: Config<MyAgentConfig>) {
    super();
  }

  connect(): string {
    const cfg = this.config.value;
    const key = cfg.apiKey.get();
    const pwd = cfg.db.password.get();
    return `Connecting to ${cfg.db.host}:${cfg.db.port}`;
  }
}
```

## Managing Secrets via CLI

```shell
# Create secrets (--secret-type uses language-native type names)
golem agent-secret create apiKey --secret-type string --secret-value "sk-abc123"
golem agent-secret create db.password --secret-type string --secret-value "s3cret"

# List, update, and delete
golem agent-secret list
golem agent-secret update-value apiKey --secret-value "new-value"
golem agent-secret delete apiKey
```

> **Note:** For `update-value` and `delete`, you can also use `--id <uuid>` instead of the positional path.

## Secret Defaults in golem.yaml

For development environments, define secret defaults in `golem.yaml`. These are **not** used in production:

```yaml
secretDefaults:
  local:
    - path: [apiKey]
      value: "dev-key-123"
    - path: [db, password]
      value: "dev-password"
```

## Key Points

- `Secret<T>` fields are **not** loaded eagerly — call `.get()` to fetch the current value.
- Secret values are stored **per-environment**, not per-agent-instance.
- Secrets are **not** stored in the `config` section of `golem.yaml` — use `secretDefaults` for dev environments only.
- Missing required secrets cause agent creation to fail.
- The `--secret-type` flag accepts TypeScript type names: `string`, `s32`, `boolean`, `string[]` (JSON-encoded analysed types like `'{"type":"Str"}'` are also supported as a fallback).
- If the agent also needs non-secret typed config guidance, use `golem-add-config-ts` alongside this skill.
