---
name: golem-add-config-effect
description: "Adding typed configuration to an Effect-based Golem agent. Use when an @golemcloud/effect-golem agent needs settings or parameters supplied through golem.yaml, the CLI, or RPC overrides."
---

# Adding Typed Configuration to an Effect Golem Agent

Effect agents model configuration as an Effect service created by `defineConfig`. Each field is
declared with Effect Schema, and the service is attached to the agent definition with the
`config` property. The runtime supplies the service to the constructor Effect and every method
handler.

## Steps

1. Define nested configuration records with `Schema.Struct`.
2. Create a config service class with `defineConfig(name, fields)`.
3. Attach the class to `defineAgent` as `config: MyAgentConfig`.
4. Keep durable identity in `constructorParams`; config is not a constructor parameter.
5. Yield the config service and its field Effects inside method handlers.
6. Set values in `golem.yaml`, at agent creation, or through typed RPC overrides.

## Agent with Typed Config

```typescript
import { Effect, Schema } from "effect";
import {
  defineAgent,
  defineConfig,
  method,
  WitTypes,
} from "@golemcloud/effect-golem";

const ServerConfig = Schema.Struct({
  host: Schema.String,
  port: WitTypes.Int32,
});

export class MyAgentConfig extends defineConfig("MyAgent.Config", {
  appName: Schema.String,
  maxRetries: WitTypes.Int32,
  server: ServerConfig,
}) {}

const Settings = Schema.Struct({
  appName: Schema.String,
  maxRetries: WitTypes.Int32,
  serverHost: Schema.String,
  serverPort: WitTypes.Int32,
});

export const MyAgent = defineAgent({
  name: "MyAgent",
  mode: "durable",
  config: MyAgentConfig,
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    getSettings: method({
      params: {},
      success: Settings,
    }),
  },
}).implement(({ name }) =>
  Effect.succeed({
    getSettings: () =>
      Effect.gen(function* () {
        const config = yield* MyAgentConfig;
        const appName = yield* config.appName;
        const maxRetries = yield* config.maxRetries;
        const serverHost = yield* config.server.host;
        const serverPort = yield* config.server.port;

        return { appName, maxRetries, serverHost, serverPort };
      }).pipe(Effect.annotateLogs({ agentName: name })),
  }),
);
```

Import the implementation module from the component entry point so the top-level
`.implement(...)` call registers it:

```typescript
// src/main.ts
import "./my-agent.js";
```

## How Config Reaches the Implementation

The first `.implement(...)` argument is only the decoded `constructorParams` record. With a
snapshot, the snapshot binding is the second argument. There is no positional `Config<T>` input:
the complete example above correctly receives `({ name })` and yields `MyAgentConfig` from the
handler Effect. Do not write `.implement(({ name }, config) => ...)`; Effect Golem never passes
config as that second argument.

Prefer yielding the service inside a handler when methods should observe configuration changes.
Non-secret leaves are loaded when their Effects are evaluated, cached within that invocation,
and read from a fresh config service on the next invocation.

## Providing Config Values

Set defaults under the agent in `golem.yaml`:

```yaml
agents:
  MyAgent:
    config:
      appName: "My Application"
      maxRetries: 3
      server:
        host: "localhost"
        port: 8080
```

Override values when creating an individual agent. Dot-separated keys address nested fields:

```shell
golem agent new 'MyAgent("agent-1")' \
  --config appName="CLI Application" \
  --config maxRetries=5 \
  --config server.host=example.com \
  --config server.port=8443
```

For an RPC-created remote agent, use the typed `overrides` option. Overrides are recursively
partial, apply when the remote agent is first created, and cannot override secret fields:

```typescript
const program = Effect.gen(function* () {
  const remote = yield* MyAgent.client.get(
    { name: "agent-2" },
    {
      overrides: {
        appName: "RPC Application",
        server: { host: "rpc.example.com" },
      },
    },
  );

  return remote;
});
```

## Schema and Loading Rules

- Use `Schema.String`, `Schema.Boolean`, and `Schema.Array(...)` for ordinary values.
- Use `WitTypes.Int32` for a WIT `s32`; plain `Schema.Number` maps to `f64`.
- A `Schema.Struct` directly inside `defineConfig` becomes nested config paths such as
  `server.host`; access each leaf as an Effect such as `yield* config.server.host`.
- Use `Schema.Option(inner)` for an optional config leaf. Evaluating it yields an Effect
  `Option.Option<T>` rather than trapping when the key is absent.
- Config keys retain TypeScript camelCase in `golem.yaml` and CLI paths.
- Config source precedence is `componentTemplates` → `components` → `agents` → `presets`, with
  agent-creation or RPC overrides taking precedence over manifest defaults.
- Do not use `Config<T>` from `@golemcloud/golem-ts-sdk`, manually provide a config Layer, or
  implement handlers as plain `async` functions.
- Run `golem build` after changing the schemas or agent definition.
