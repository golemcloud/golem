---
name: golem-add-secret-effect
description: "Adding secrets to Effect-based Golem agents. Use when an @golemcloud/effect-golem agent needs API keys, passwords, tokens, or other sensitive typed configuration without exposing plaintext in Effect values or logs."
---

# Adding Secrets to an Effect Golem Agent

Effect agents declare secrets as redacted fields in a `defineConfig` service. The host supplies a
secret field on demand as an Effect `Redacted.Redacted<T>` value. Keep that wrapper intact through
Effect composition and logging, and call `Redacted.value(...)` only at the immediate boundary that
must consume the plaintext.

## Steps

1. Create a config service class with `defineConfig(name, fields)`.
2. Mark each sensitive field with `Schema.Redacted(innerSchema)`.
3. Attach the config class to `defineAgent` with `config: MyAgentConfig`.
4. Yield the config service inside the method handler.
5. Evaluate a regular field directly; evaluate a secret field's `.get` Effect.
6. Keep the resulting `Redacted.Redacted<T>` wrapped until the narrowest possible use site.
7. Provision production values with `golem secret`; use `secretDefaults` only for development.

## Agent with Regular and Secret Config

```typescript
import { Effect, Redacted, Schema } from "effect";
import { defineAgent, defineConfig, method } from "@golemcloud/effect-golem";

export class SecureAgentConfig extends defineConfig("SecureAgent.Config", {
  label: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
}) {}

const Info = Schema.Struct({
  label: Schema.String,
  apiKeyPrefix: Schema.String,
});

export const SecureAgent = defineAgent({
  name: "SecureAgent",
  mode: "durable",
  config: SecureAgentConfig,
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    getInfo: method({
      params: {},
      success: Info,
    }),
  },
}).implement(() =>
  Effect.succeed({
    getInfo: () =>
      Effect.gen(function* () {
        const config = yield* SecureAgentConfig;
        const label = yield* config.label;
        const apiKey = yield* config.apiKey.get;

        return {
          label,
          // Reveal only long enough to derive the non-secret result.
          apiKeyPrefix: Redacted.value(apiKey).slice(0, 4),
        };
      }),
  }),
);
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./secure-agent.js";
```

The config service has different access shapes for regular and secret leaves:

```typescript
const readConfig = Effect.gen(function* () {
  const config = yield* SecureAgentConfig;

  const label = yield* config.label; // string
  const apiKey = yield* config.apiKey.get; // Redacted.Redacted<string>
  return { label, apiKey };
});
```

`.get` is an Effect property, not a method: write `yield* config.apiKey.get`, not
`config.apiKey.get()`.

## Nested and Structured Secrets

A `Schema.Struct` inside `defineConfig` creates nested paths. Mark only the sensitive leaves when
the surrounding values are ordinary config:

```typescript
export class ServiceConfig extends defineConfig("Service.Config", {
  database: Schema.Struct({
    host: Schema.String,
    password: Schema.Redacted(Schema.String),
  }),
}) {}

const readDatabaseConfig = Effect.gen(function* () {
  const config = yield* ServiceConfig;
  const host = yield* config.database.host;
  const password = yield* config.database.password.get;
  return { host, password };
});
```

This produces the regular path `database.host` and secret path `database.password`. To make an
entire object one secret value instead, wrap its struct:

```typescript
credentials: Schema.Redacted(
  Schema.Struct({
    username: Schema.String,
    password: Schema.String,
  }),
),
```

That produces one secret path named `credentials` whose `.get` Effect yields a redacted object.

## Preserve Redaction

`Schema.Redacted` is both the secret declaration marker and the guest-side protection around the
decoded value. Ordinary inspection, string conversion, JSON conversion, and logging render the
wrapper as redacted. That protection ends as soon as `Redacted.value(secret)` returns plaintext.

- Keep secret variables typed as `Redacted.Redacted<T>` while composing Effects.
- Reveal directly at the external API or minimal transformation that needs plaintext.
- Never put revealed plaintext in log messages, log annotations, errors, snapshots, `Ref` state,
  method results, or long-lived implementation closures.
- If a secret must be associated with a log event, retain the redacted wrapper; do not log
  `Redacted.value(secret)`.
- Returning a deliberately non-secret derivative, such as a four-character prefix, is safe only
  when the application explicitly requires that disclosure.

## Runtime Reads and Rotation

Yield the config service and evaluate the secret `.get` Effect inside the method handler when the
method should observe updates:

```typescript
const readCurrentPrefix = Effect.gen(function* () {
  const config = yield* SecureAgentConfig;
  const currentApiKey = yield* config.apiKey.get;
  return Redacted.value(currentApiKey).slice(0, 4);
});
```

The SDK does not cache secret `.get` Effects: each evaluation asks the host for the value. A fresh
config service is also supplied for each invocation. Do not reveal and capture a secret during
agent initialization if later invocations should be able to observe rotation. Actual propagation
and consistency of an external secret update remain host concerns.

## Managing Secrets with the CLI

Effect components use TypeScript casing and value syntax. Config paths retain camelCase, and
string values passed through the shell include the TypeScript string literal quotes:

```shell
# Create secret values in the current environment
golem secret create apiKey --secret-type string --secret-value '"sk-abc123"'
golem secret create database.password --secret-type string --secret-value '"s3cret"'

# List, update, and delete
golem secret list
golem secret update-value apiKey --secret-value '"new-value"'
golem secret delete apiKey
```

For `update-value` and `delete`, `--id <uuid>` can be used instead of the positional path.

## Defaults in `golem.yaml`

Put ordinary defaults under the agent's `config`, but keep secret defaults in the environment's
`secretDefaults` map:

```yaml
agents:
  SecureAgent:
    config:
      label: "production"

secretDefaults:
  local:
    apiKey: "dev-key-123"
    database:
      password: "dev-password"
```

Use `secretDefaults` for local development only. Manage deployed secret values separately with
the CLI rather than checking them into the manifest.

## Key Constraints

- Pass the class returned by `defineConfig` to `defineAgent.config`. A raw schema record or a
  `Schema.Struct` is not a valid `config` value.
- Do not use `Config<T>` or `Secret<T>` from the plain TypeScript SDK; Effect agents use
  `defineConfig`, `Schema.Redacted`, and Effect `Redacted`.
- Config is not a positional `.implement(...)` argument. Yield the config service in an Effect.
- Regular fields are field Effects; secret fields provide a `.get` Effect that yields a redacted
  value.
- Secret paths retain TypeScript camelCase and are scoped per environment, not per agent instance.
- A missing required secret prevents the agent from being created successfully.
- Keep versions of `effect` and `@golemcloud/effect-golem` aligned with the generated project.
- If the agent also needs detailed non-secret config guidance, use `golem-add-config-effect`.
- Run `golem build` after changing config schemas or agent definitions.
