---
name: golem-add-http-auth-effect
description: "Enabling authentication on Effect-based Golem HTTP endpoints. Use when protecting mounts or individual routes, adding public-route overrides, or reading the authenticated caller in an @golemcloud/effect-golem agent."
---

# Enabling Authentication on Effect HTTP Endpoints

Effect Golem agents publish authentication requirements as route metadata through the `Http`
namespace from `@golemcloud/effect-golem`. The Golem host authenticates requests and supplies the
caller; do not add application-side authentication middleware or parse authentication headers
inside handlers.

Authentication also requires deployment configuration in `golem.yaml`. Load the
`golem-configure-api-domain` skill when a security scheme or HTTP API deployment must be added.

## Mount-Level Authentication

Set `auth: true` in the mount options to require authentication for every endpoint by default:

```typescript
import { Schema } from "effect";
import { defineAgent, Http } from "@golemcloud/effect-golem";

export const SecureAgent = defineAgent({
  name: "SecureAgent",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  http: Http.mount("/secure/{name}", { auth: true }),
  methods: {
    // Endpoints without an explicit auth option inherit auth: true.
  },
});
```

Mount authentication defaults to `false` when the option is omitted. Preserve other mount
options such as `cors`, `phantomAgent`, and `webhookSuffix` when adding `auth` to an existing
options object.

## Endpoint-Level Authentication

Set `auth: true` in an endpoint helper's options to protect only that route:

```typescript
methods: {
  publicData: method({
    input: {},
    success: Schema.String,
    http: [Http.get("/public")],
  }),
  privateData: method({
    input: {},
    success: Schema.String,
    http: [Http.get("/private", { auth: true })],
  }),
},
```

Keep every endpoint declaration in the method's `http` array. Preserve existing endpoint options
such as `headers` and `cors` when adding `auth`.

## Overriding Mount Authentication

An endpoint's explicit `auth` value overrides the mount setting. Omitting endpoint `auth` means
inherit from the mount:

```typescript
export const MostlySecureAgent = defineAgent({
  name: "MostlySecureAgent",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  http: Http.mount("/api/{name}", { auth: true }),
  methods: {
    health: method({
      input: {},
      success: Schema.String,
      http: [Http.get("/health", { auth: false })],
    }),
    getData: method({
      input: {},
      success: Schema.String,
      http: [Http.get("/data")],
    }),
  },
});
```

Here, `/health` is public because it explicitly sets `auth: false`, while `/data` requires
authentication because it inherits `auth: true` from the mount.

## Reading the Authenticated Caller

Read the caller from the `InvocationContext.InvocationContext` service. The SDK provides it to every
constructor and handler; it does not consume an HTTP body, path, query, or header field.
`caller.original` is the principal that started the call chain (the signed-in user), and
`caller.immediate` is the principal that made this call:

```typescript
import { Effect, Schema } from "effect";
import {
  defineAgent,
  Http,
  InvocationContext,
  method,
} from "@golemcloud/effect-golem";

export const CallerAgent = defineAgent({
  name: "CallerAgent",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  http: Http.mount("/callers/{name}", { auth: true }),
  methods: {
    whoAmI: method({
      input: {},
      success: Schema.String,
      http: [Http.get("/whoami")],
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    whoAmI: () =>
      Effect.gen(function* () {
        const { caller } = yield* InvocationContext.InvocationContext;
        const user = caller.original;
        return user.kind === "user" ? user.subject : user.kind;
      }),
  }),
});
```

The principal kinds are `user` (with `claims`), `account`, `agent`, `tool` and `anonymous`; narrow
with `kind`. A `Principal` or `Caller` is a live value: it cannot be a method input, a result or part
of a value type. To remember who called, for example the creator in `init`, store
`caller.original.ref` as an `InvocationContext.PrincipalRefSchema` value and compare it later with
`equals`.

## Deployment Configuration

Code-level `auth` metadata must be paired with authentication configuration for the deployed
agent in `golem.yaml`. For production OIDC, reference a configured security scheme:

```yaml
httpApi:
  deployments:
    local:
      - domain: my-app.localhost:9006
        agents:
          SecureAgent:
            securityScheme: my-oidc
```

For local development and harness scenarios, use a test-session header instead:

```yaml
httpApi:
  deployments:
    local:
      - domain: my-app.localhost:9006
        agents:
          SecureAgent:
            testSessionHeaderName: X-Test-Auth
```

Preserve unrelated deployments and agent entries when editing the manifest. Use the test-session
header only for development; configure an OIDC security scheme for production.

## Key Constraints

- Import `Http` and `InvocationContext` from `@golemcloud/effect-golem`; do not use decorators or classes
  from `@golemcloud/golem-ts-sdk`.
- Configure auth with `Http.mount(path, { auth: true })` and endpoint helpers such as
  `Http.get(path, { auth: true | false })`.
- Endpoint `auth` omission inherits the mount; an explicit `true` or `false` overrides it.
- Read the current caller with `yield* InvocationContext.InvocationContext` inside the Effect handler.
- Do not bind the caller to an HTTP variable or trust a caller-supplied identity field.
- Keep handlers as Effects and import the implemented agent module from `src/main.ts`.
- Run `golem build` after changing route metadata, then redeploy the application.
