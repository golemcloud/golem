---
name: golem-add-secret-ts
description: "Adding secrets to TypeScript Golem agents. Use when the user asks to add secrets, store API keys, manage sensitive config values, or use Secret<T> in TypeScript agents."
---

# Adding Secrets to TypeScript Golem Agents

## Overview

Secrets are sensitive configuration values (API keys, passwords, tokens) stored per-environment. In the fluent SDK a secret is just a config field marked with the `s.secret(...)` schema marker. The config value carries an opaque, log-safe `Secret<T>` handle; the plaintext is revealed only when agent code calls `.get()`.

## Declaring Secrets in the Config Record

Wrap a sensitive field's schema with `s.secret(inner)` in the agent's `config` record. Secret markers work at any depth — including a whole nested object:

```typescript
import { z } from 'zod';
import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';

export const MyAgent = defineAgent({
    name: 'MyAgent',
    id: { name: z.string() },
    config: {
        // A plain local field, read fresh on each access.
        greeting: z.string(),
        // A top-level secret → `this.config.apiKey` is a `Secret<string>` handle.
        apiKey: s.secret(z.string()),
        // A nested object; its `.password` sub-field is a nested secret.
        db: z.object({
            host: z.string(),
            port: z.number(),
            password: s.secret(z.string()),
        }),
    },
    methods: {
        connect: method({ input: {}, returns: z.string() }),
    },
});
```

`this.config` is statically typed from the record: `greeting` is a `string`, `apiKey` is a `Secret<string>`, and `db.password` is a `Secret<string>` (a secret wrapping a whole object would surface as `Secret<{...}>`).

## Using Secrets in Agent Code

Call `.get()` on a `Secret<T>` field to explicitly reveal the current plaintext:

```typescript
export const MyAgentImpl = MyAgent.implement({
    init: () => ({}),
    methods: {
        connect() {
            const key = this.config.apiKey.get();     // string
            const pwd = this.config.db.password.get(); // string
            return `Connecting to ${this.config.db.host}:${this.config.db.port}`;
        },
    },
});
```

`Secret` handles are log-safe: `JSON.stringify` / logging the whole config object throws rather than leaking the plaintext. Call `.get()` only where you actually need the value.

## Managing Secrets via CLI

```shell
# Create secrets (--secret-type uses language-native type names)
golem secret create apiKey --secret-type string --secret-value "sk-abc123"
golem secret create db.password --secret-type string --secret-value "s3cret"

# List, update, and delete
golem secret list
golem secret update-value apiKey --secret-value "new-value"
golem secret delete apiKey
```

> **Note:** For `update-value` and `delete`, you can also use `--id <uuid>` instead of the positional path.

## Secret Defaults in golem.yaml

For development environments, define secret defaults in `golem.yaml`. These are **not** used in production:

```yaml
secretDefaults:
  local:
    apiKey: "dev-key-123"
    db:
      password: "dev-password"
```

## Key Points

- Mark a config field secret with `s.secret(inner)` — no separate declaration; the field becomes a `Secret<T>` on `this.config`.
- `Secret<T>` values are **not** revealed eagerly — call `.get()` to read the current plaintext (it re-reads the live value each call).
- `Secret` handles refuse serialization, so a stray log of `this.config` cannot leak a secret.
- Secret values are stored **per-environment**, not per-agent-instance.
- Secrets are **not** stored in the `config` section of `golem.yaml` — use `secretDefaults` for dev environments only.
- Missing required secrets cause agent creation to fail.
- The `--secret-type` flag accepts TypeScript type names: `string`, `s32`, `boolean`, `string[]` (JSON-encoded analysed types like `'{"type":"Str"}'` are also supported as a fallback).
- If the agent also needs non-secret typed config guidance, use `golem-add-config-ts` alongside this skill.
