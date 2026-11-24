---
name: golem-add-http-auth-moonbit
description: "Enabling authentication on HTTP endpoints in MoonBit Golem agents. Use when the user asks to add auth, security, or access control to HTTP endpoints."
---

# Enabling Authentication on MoonBit HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments, including when to use `subdomain` versus `domain`.

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

## Reading the Caller

When auth is enabled, add an `@agents.InvocationContext` parameter to the `new` constructor or to an endpoint method. The SDK supplies it; it is not part of the input schema and is never mapped to path, query, header or body. `ctx.caller().original` is the principal that started the call chain (the signed-in user); `ctx.caller().immediate` is the principal that made this call.

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_auth(true)
pub(all) struct AuthedAgent {
  name : String
  // Who created the agent. A PrincipalRef is data: it grants nothing by itself.
  owner : @agents.PrincipalRef?
}

fn AuthedAgent::new(name : String, ctx : @agents.InvocationContext) -> AuthedAgent {
  { name, owner: ctx.caller().original.principal_ref() }
}

#derive.endpoint(get="/whoami")
pub fn AuthedAgent::who_am_i(self : Self, ctx : @agents.InvocationContext) -> String {
  // A signed-in user prints as `user ("<issuer>","<subject>")`, never with its claims.
  ctx.caller().original.to_string()
}

#derive.endpoint(get="/data")
pub fn AuthedAgent::get_data(self : Self, ctx : @agents.InvocationContext) -> String {
  match self.owner {
    Some(owner) if ctx.caller().original.equals_ref(owner) => "secret data"
    _ => abort("only the creator can read this data")
  }
}
```

The principal is one of `User` (a signed-in user; its `claims` field holds the claim set as JSON text), `Account` (a Golem account, for example the CLI), `Agent`, `Tool` and `Anonymous`. A `Principal` or `Caller` cannot appear in a method, constructor or tool signature or inside a value type: the build fails with a message that points to `ctx.caller()`. Store `principal_ref()` (a `PrincipalRef`) instead.

## Deployment Configuration

After enabling auth in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details, including when to use `subdomain` versus `domain`. Quick reference:

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
