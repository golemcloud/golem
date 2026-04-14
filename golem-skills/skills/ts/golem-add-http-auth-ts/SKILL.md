---
name: golem-add-http-auth-ts
description: "Enabling authentication on TypeScript HTTP endpoints. Use when the user asks to add auth, require authentication, use Principal, or protect HTTP endpoints."
---

# Enabling Authentication on TypeScript HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments.

## Enabling Auth on All Endpoints (Mount Level)

Set `auth: true` on the `@agent()` decorator to require authentication for all endpoints:

```typescript
@agent({
  mount: '/secure/{name}',
  auth: true
})
class SecureAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }
  // All endpoints require authentication
}
```

## Enabling Auth on Individual Endpoints

Set `auth: true` on specific `@endpoint()` decorators:

```typescript
@agent({ mount: '/api/{name}' })
class ApiAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/public' })
  async publicData(): Promise<string> { return "open"; }

  @endpoint({ get: '/private', auth: true })
  async privateData(): Promise<string> { return "secret"; }
}
```

## Overriding Mount-Level Auth

Per-endpoint `auth` overrides the mount-level setting:

```typescript
@agent({ mount: '/api/{name}', auth: true })
class MostlySecureAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/health', auth: false })
  async health(): Promise<string> { return "ok"; } // No auth required

  @endpoint({ get: '/data' })
  async getData(): Promise<Data> { ... } // Auth required (inherited)
}
```

## Receiving the Principal

When auth is enabled, methods can receive a `Principal` parameter with information about the authenticated user. `Principal` is automatically populated and must **not** be mapped to path/query/header variables:

```typescript
import { Principal } from '@golemcloud/golem-ts-sdk';

@endpoint({ get: '/whoami', auth: true })
async whoAmI(principal: Principal): Promise<{ value: Principal }> {
  return { value: principal };
}

// Principal can appear at any position among parameters
@endpoint({ get: '/data/{id}' })
async getData(id: string, principal: Principal): Promise<Data> { ... }
```

## Deployment Configuration

After enabling `auth: true` in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details. Quick reference:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        SecureAgent:
          securityScheme: my-oidc            # For production OIDC
        # or for development:
        # SecureAgent:
        #   testSessionHeaderName: X-Test-Auth
```
