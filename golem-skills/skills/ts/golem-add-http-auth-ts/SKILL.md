---
name: golem-add-http-auth-ts
description: "Enabling authentication on TypeScript HTTP endpoints. Use when the user asks to add auth, require authentication, use Principal, or protect HTTP endpoints."
---

# Enabling Authentication on TypeScript HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments, including when to use `subdomain` versus `domain`.

## Enabling Auth on All Endpoints (Mount Level)

Set `auth: true` in the mount options of `http.mount(...)` to require authentication for all endpoints:

```typescript
import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const SecureAgent = defineAgent({
  name: 'SecureAgent',
  id: { name: z.string() },
  http: http.mount('/secure/{name}', { auth: true }),
  methods: {
    // All endpoints require authentication
  },
});
```

## Enabling Auth on Individual Endpoints

Set `auth: true` in the endpoint options (the second argument to a verb builder):

```typescript
export const ApiAgent = defineAgent({
  name: 'ApiAgent',
  id: { name: z.string() },
  http: http.mount('/api/{name}'),
  methods: {
    publicData: method({ input: {}, returns: z.string(), http: http.get('/public') }),
    privateData: method({ input: {}, returns: z.string(), http: http.get('/private', { auth: true }) }),
  },
});
```

## Overriding Mount-Level Auth

Per-endpoint `auth` overrides the mount-level setting:

```typescript
export const MostlySecureAgent = defineAgent({
  name: 'MostlySecureAgent',
  id: { name: z.string() },
  http: http.mount('/api/{name}', { auth: true }),
  methods: {
    // No auth required — overrides the mount default
    health: method({ input: {}, returns: z.string(), http: http.get('/health', { auth: false }) }),
    // Auth required (inherited from the mount)
    getData: method({ input: {}, returns: Data, http: http.get('/data') }),
  },
});
```

## Receiving the Principal

When auth is enabled, access the authenticated user's `Principal` from the handler's `this` via `this.getPrincipal()`. Unlike the old decorator API, `Principal` is **not** a method parameter, so it is never mapped to path/query/header variables:

```typescript
import { z } from 'zod';
import { defineAgent, method, http, Principal } from '@golemcloud/golem-ts-sdk';

export const ApiAgent = defineAgent({
  name: 'ApiAgent',
  id: { name: z.string() },
  http: http.mount('/api/{name}', { auth: true }),
  methods: {
    whoAmI: method({ input: {}, returns: z.unknown(), http: http.get('/whoami') }),
    getData: method({ input: { id: z.string() }, returns: Data, http: http.get('/data/{id}') }),
  },
});

export const ApiAgentImpl = ApiAgent.implement({
  init: () => ({}),
  methods: {
    whoAmI() {
      const principal: Principal = this.getPrincipal();
      return { value: principal };
    },
    getData({ id }) {
      const principal = this.getPrincipal();
      // ... use principal + id ...
    },
  },
});
```

`this.getPrincipal()` (and `this.getId()`, `this.getPhantomId()`) are available on every handler's `this`. The same `principal` is also available in `init` via the `InitContext` argument: `init: (ctx) => ({ caller: ctx.principal })`.

## Deployment Configuration

After enabling `auth: true` in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details, including when to use `subdomain` versus `domain`. Quick reference:

```yaml
httpApi:
  deployments:
    local:
    - subdomain: my-app  # resolves to my-app.localhost:9006 by default
      agents:
        SecureAgent:
          securityScheme: my-oidc            # For production OIDC
        # or for development:
        # SecureAgent:
        #   testSessionHeaderName: X-Test-Auth
```
