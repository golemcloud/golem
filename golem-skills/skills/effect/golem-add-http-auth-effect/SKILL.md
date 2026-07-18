---
name: golem-add-http-auth-effect
description: "Enabling authentication on Effect-based Golem HTTP endpoints. Use when protecting mounts or individual routes, adding public-route overrides, or reading the authenticated caller in an @golemcloud/effect-golem agent."
---

# Enabling Authentication on Effect HTTP Endpoints

Effect Golem agents publish authentication requirements as route metadata through the `Http`
namespace from `@golemcloud/effect-golem`. The Golem host authenticates requests and supplies the
principal; do not add application-side authentication middleware or parse authentication headers
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
  constructorParams: {
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
    params: {},
    success: Schema.String,
    http: [Http.get("/public")],
  }),
  privateData: method({
    params: {},
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
  constructorParams: {
    name: Schema.String,
  },
  http: Http.mount("/api/{name}", { auth: true }),
  methods: {
    health: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/health", { auth: false })],
    }),
    getData: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/data")],
    }),
  },
});
```

Here, `/health` is public because it explicitly sets `auth: false`, while `/data` requires
authentication because it inherits `auth: true` from the mount.

## Reading the Authenticated Principal

Import the `Principal` namespace and yield the `Principal.Principal` Context service inside a
handler to read the caller for that invocation:

```typescript
import { Effect, Schema } from "effect";
import {
  defineAgent,
  Http,
  method,
  Principal,
} from "@golemcloud/effect-golem";

export const CallerAgent = defineAgent({
  name: "CallerAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  http: Http.mount("/callers/{name}", { auth: true }),
  methods: {
    whoAmI: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/whoami")],
    }),
  },
}).implement(() =>
  Effect.succeed({
    whoAmI: () =>
      Effect.gen(function* () {
        const caller = yield* Principal.Principal;
        return caller.tag === "oidc" ? caller.val.sub : caller.tag;
      }),
  }),
);
```

For an OIDC principal, narrow `caller.tag === "oidc"` before reading the subject from
`caller.val.sub`. Principals are host-supplied services, not method parameters, request-body
fields, or path/query/header bindings.

The principal available while the `.implement(...)` initialization Effect runs is the principal
that created the agent. The principal yielded inside a returned method handler is the current
invocation's caller. For caller-based authorization, always read `Principal.Principal` inside the
handler rather than capturing the initialization-time principal.

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

- Import `Http` and `Principal` from `@golemcloud/effect-golem`; do not use decorators or classes
  from `@golemcloud/golem-ts-sdk`.
- Configure auth with `Http.mount(path, { auth: true })` and endpoint helpers such as
  `Http.get(path, { auth: true | false })`.
- Endpoint `auth` omission inherits the mount; an explicit `true` or `false` overrides it.
- Read the current caller with `yield* Principal.Principal` inside the Effect handler.
- Do not bind the principal to an HTTP variable or trust a caller-supplied identity field.
- Keep handlers as Effects and import the implemented agent module from `src/main.ts`.
- Run `golem build` after changing route metadata, then redeploy the application.
