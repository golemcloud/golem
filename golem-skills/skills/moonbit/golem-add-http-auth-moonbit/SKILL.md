---
name: golem-add-http-auth-moonbit
description: "Enabling authentication on HTTP endpoints in MoonBit Golem agents. Use when the user asks to add auth, security, or access control to HTTP endpoints."
---

# Enabling Authentication on MoonBit HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments.

## Enabling Auth on All Endpoints (Mount Level)

Set `#derive.mount_auth(true)` on the agent struct to require authentication for all endpoints:

```moonbit
#derive.agent
#derive.mount("/secure/{name}")
#derive.mount_auth(true)
pub(all) struct SecureAgent {
  name : String
}

fn SecureAgent::new(name : String) -> SecureAgent {
  { name }
}
```

Note: when `#derive.mount_auth` is not specified, authentication defaults to disabled.

## Enabling Auth on Individual Endpoints

Set `#derive.endpoint_auth(true)` on specific endpoint methods:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_auth(false)
pub(all) struct ApiAgent {
  name : String
}

fn ApiAgent::new(name : String) -> ApiAgent {
  { name }
}

#derive.endpoint(get="/public")
pub fn ApiAgent::public_data(self : Self) -> String {
  "public"
}

#derive.endpoint(get="/private")
#derive.endpoint_auth(true)
pub fn ApiAgent::private_data(self : Self) -> String {
  "private"
}
```

## Overriding Mount-Level Auth

Per-endpoint `#derive.endpoint_auth` overrides the mount-level `#derive.mount_auth` setting:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_auth(true)
pub(all) struct MostlySecureAgent {
  name : String
}

fn MostlySecureAgent::new(name : String) -> MostlySecureAgent {
  { name }
}

///|
/// No auth required (overrides mount-level auth)
#derive.endpoint(get="/health")
#derive.endpoint_auth(false)
pub fn MostlySecureAgent::health(self : Self) -> String {
  "ok"
}

///|
/// Auth required (inherited from mount)
#derive.endpoint(get="/data")
pub fn MostlySecureAgent::get_data(self : Self) -> String {
  "secret data"
}
```

## Accessing the Authenticated Principal

When auth is enabled, add a `Principal` parameter to the `new` constructor or endpoint methods to receive the authenticated identity. The SDK automatically injects the principal value:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_auth(true)
pub(all) struct AuthedAgent {
  name : String
  principal : Principal
}

fn AuthedAgent::new(name : String, principal : Principal) -> AuthedAgent {
  { name, principal }
}
```

## Deployment Configuration

After enabling auth in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details. Quick reference:

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
