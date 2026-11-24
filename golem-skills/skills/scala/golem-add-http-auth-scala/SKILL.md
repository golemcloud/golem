---
name: golem-add-http-auth-scala
description: "Enabling authentication on Scala HTTP endpoints. Use when the user asks to add auth, require authentication, or protect HTTP endpoints."
---

# Enabling Authentication on Scala HTTP Endpoints

## Overview

Golem supports authentication on HTTP endpoints via OIDC providers. Authentication is enabled in the agent code and configured via security schemes in `golem.yaml`. Load the `golem-configure-api-domain` skill for details on setting up security schemes and domain deployments, including when to use `subdomain` versus `domain`.

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

## Reading the Caller

When auth is enabled, a method reads the signed-in user from an `InvocationContext` parameter. The SDK supplies it in the constructor and in any method; it is not part of the input schema and is never mapped to path, query, header or body. `context.caller.original` is the principal that started the call chain (the signed-in user); `context.caller.immediate` is the principal that made this call.

```scala
import golem.{BaseAgent, InvocationContext, Principal}
import golem.runtime.annotations.{agentDefinition, agentImplementation, endpoint}

import scala.concurrent.Future

@agentDefinition(mount = "/api/{name}", auth = true)
trait ApiAgent extends BaseAgent {
  class Id(val name: String)

  @endpoint(method = "GET", path = "/whoami")
  def whoAmI(context: InvocationContext): Future[String]
}

@agentImplementation()
final class ApiAgentImpl(private val name: String) extends ApiAgent {
  override def whoAmI(context: InvocationContext): Future[String] =
    Future.successful(context.caller.original match {
      case user: Principal.User => user.subject
      case other                => other.kind.toString
    })
}
```

The principal is one of `Principal.User` (a signed-in user; `claimsJson` holds the claim set), `Principal.Account` (a Golem account, for example the CLI), `Principal.Agent`, `Principal.Tool` and `Principal.Anonymous`. A `Principal` or `Caller` cannot appear in a method, constructor or tool signature or inside a value type: the build fails with a message that points to `ctx.caller`. To remember who called, store `context.caller.original.ref` (an `Option[PrincipalRef]`) and compare it later with `isSameAs`.

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
