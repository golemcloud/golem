---
name: golem-add-http-auth-rust
description: "Enabling authentication on Rust HTTP endpoints. Use when the user asks to add auth, require authentication, or protect HTTP endpoints."
---

# Enabling Authentication on Rust HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments, including when to use `subdomain` versus `domain`.

## Enabling Auth on All Endpoints (Mount Level)

Set `auth = true` on `#[agent_definition]` to require authentication for all endpoints:

```rust
#[agent_definition(mount = "/secure/{name}", auth = true)]
pub trait SecureAgent {
    fn new(name: String) -> Self;
    // All endpoints require authentication
}
```

## Enabling Auth on Individual Endpoints

Set `auth = true` on specific `#[endpoint]` attributes:

```rust
#[agent_definition(mount = "/api/{name}")]
pub trait ApiAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/public")]
    fn public_data(&self) -> String;

    #[endpoint(get = "/private", auth = true)]
    fn private_data(&self) -> String;
}
```

## Overriding Mount-Level Auth

Per-endpoint `auth` overrides the mount-level setting:

```rust
#[agent_definition(mount = "/api/{name}", auth = true)]
pub trait MostlySecureAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/health", auth = false)]
    fn health(&self) -> String; // No auth required

    #[endpoint(get = "/data")]
    fn get_data(&self) -> Data; // Auth required (inherited)
}
```

## Reading the Caller

When auth is enabled, a method reads the signed-in user from an `InvocationContext` parameter. The SDK supplies it in `new` and in any method; it is not part of the input schema and is never mapped to path, query, header or body. `ctx.caller().original()` is the principal that started the call chain (the signed-in user); `ctx.caller().immediate()` is the principal that made this call.

```rust
use golem_rust::agentic::{InvocationContext, Principal, PrincipalRef};
use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/api/{name}", auth = true)]
pub trait ApiAgent {
    fn new(name: String, ctx: InvocationContext) -> Self;

    #[endpoint(get = "/whoami")]
    fn who_am_i(&self, ctx: InvocationContext) -> String;

    #[endpoint(get = "/data")]
    fn get_data(&self, ctx: InvocationContext) -> String;
}

struct ApiAgentImpl {
    // Who created the agent. A PrincipalRef is data: it grants nothing by itself.
    owner: Option<PrincipalRef>,
}

#[agent_implementation]
impl ApiAgent for ApiAgentImpl {
    fn new(_name: String, ctx: InvocationContext) -> Self {
        Self { owner: ctx.caller().original().principal_ref().cloned() }
    }

    fn who_am_i(&self, ctx: InvocationContext) -> String {
        match ctx.caller().original() {
            Principal::User(user) => user.subject().to_string(),
            other => other.kind().as_str().to_string(),
        }
    }

    fn get_data(&self, ctx: InvocationContext) -> String {
        let caller = ctx.caller().original().principal_ref();
        match (caller, &self.owner) {
            (Some(caller), Some(owner)) if caller.equals(owner) => "secret data".to_string(),
            _ => panic!("only the creator can read this data"),
        }
    }
}
```

The principal is one of `Principal::User` (a signed-in user; `claims()` parses the claim set), `Principal::Account` (a Golem account, for example the CLI), `Principal::Agent`, `Principal::Tool` and `Principal::Anonymous`. A `Principal` or `Caller` cannot appear in a method, constructor or tool signature or inside a value type: the build fails with a message that points to `ctx.caller()`. Store `principal_ref()` (a `PrincipalRef`) instead.

## Deployment Configuration

After enabling `auth = true` in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details, including when to use `subdomain` versus `domain`. Quick reference:

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
