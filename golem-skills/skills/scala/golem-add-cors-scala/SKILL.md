---
name: golem-add-cors-scala
description: "Configuring CORS for Scala HTTP endpoints. Use when the user asks to enable CORS, allow cross-origin requests, or configure allowed origins for HTTP endpoints."
---

# Configuring CORS for Scala HTTP Endpoints

## Mount-Level CORS

Set `cors` on `@agentDefinition` to apply allowed origins to **all** endpoints:

```scala
@agentDefinition(
  mount = "/api/{value}",
  cors = Array("https://app.example.com")
)
trait MyAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/data")
  def getData(): Future[String]
  // Allows https://app.example.com
}
```

## Endpoint-Level CORS

Set `cors` on `@endpoint` to add allowed origins for a specific endpoint. Origins are **unioned** with mount-level CORS:

```scala
@agentDefinition(
  mount = "/api/{value}",
  cors = Array("https://app.example.com")
)
trait MyAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/data", cors = Array("*"))
  def getData(): Future[String]
  // Allows BOTH https://app.example.com AND * (all origins)

  @endpoint(method = "GET", path = "/other")
  def getOther(): Future[String]
  // Inherits mount-level: only https://app.example.com
}
```

## Wildcard

Use `"*"` to allow all origins:

```scala
@agentDefinition(
  mount = "/public/{value}",
  cors = Array("*")
)
trait PublicAgent extends BaseAgent {
  class Id(val value: String)
}
```

## CORS Preflight

Golem automatically handles `OPTIONS` preflight requests for endpoints that have CORS configured. The preflight response includes `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, and `Access-Control-Allow-Headers` headers.
