---
name: golem-add-http-auth-rust
description: "Enabling authentication on Rust HTTP endpoints. Use when the user asks to add auth, require authentication, or protect HTTP endpoints."
---

# Enabling Authentication on Rust HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments.

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

## Deployment Configuration

After enabling `auth = true` in code, you must configure a security scheme in `golem.yaml`. Load the `golem-configure-api-domain` skill for the full details. Quick reference:

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
