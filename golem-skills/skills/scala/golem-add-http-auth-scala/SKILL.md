---
name: golem-add-http-auth-scala
description: "Enabling authentication on Scala HTTP endpoints. Use when the user asks to add auth, require authentication, or protect HTTP endpoints."
---

# Enabling Authentication on Scala HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments.

## Enabling Auth on All Endpoints (Mount Level)

Set `auth = true` on `@agentDefinition` to require authentication for all endpoints:

```scala
@agentDefinition(mount = "/secure/{value}", auth = true)
trait SecureAgent extends BaseAgent {
  class Id(val value: String)
  // All endpoints require authentication
}
```

## Enabling Auth on Individual Endpoints

Set `auth = true` on specific `@endpoint` annotations:

```scala
@agentDefinition(mount = "/api/{value}")
trait ApiAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/public")
  def publicData(): Future[String]

  @endpoint(method = "GET", path = "/private", auth = true)
  def privateData(): Future[String]
}
```

## Overriding Mount-Level Auth

Per-endpoint `auth` overrides the mount-level setting:

```scala
@agentDefinition(mount = "/api/{value}", auth = true)
trait MostlySecureAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/health", auth = false)
  def health(): Future[String] // No auth required

  @endpoint(method = "GET", path = "/data")
  def getData(): Future[Data] // Auth required (inherited)
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
