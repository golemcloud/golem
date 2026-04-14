---
name: golem-add-http-endpoint-scala
description: "Exposing a Scala Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, map request parameters to method arguments, or configure CORS/authentication for HTTP access."
---

# Adding HTTP Endpoints to a Scala Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` parameter to `@agentDefinition`
2. Annotating methods with `@endpoint`
3. Adding an `httpApi` deployment section to `golem.yaml`

## Steps

1. Add `mount = "/path/{param}"` to `@agentDefinition(...)`
2. Add `@endpoint(method = "GET", path = "/...")` to trait methods
3. Add `httpApi` deployment to `golem.yaml`
4. Build and deploy

## Mount Path

The `mount` parameter on `@agentDefinition` defines the base HTTP path. Path variables in `{braces}` map to constructor parameters defined in the `class Id(...)`:

```scala
import golem.runtime.annotations.{agentDefinition, endpoint}
import golem.BaseAgent
import scala.concurrent.Future

@agentDefinition(mount = "/api/tasks/{name}")
trait TaskAgent extends BaseAgent {
  class Id(val name: String)

  // methods...
}
```

### Constructor Parameter Naming in Scala

The path variable names must match the `class Id` parameter names:

- **Single parameter**: `class Id(val name: String)` → use `{name}` in path
- **Multiple parameters**: `class Id(val arg0: String, val arg1: Int)` → use `{arg0}`, `{arg1}` in path
- **Custom Id class with `@id`**: use the parameter names from the annotated class

```scala
// Single parameter
@agentDefinition(mount = "/api/weather/{value}")
trait WeatherAgent extends BaseAgent {
  class Id(val value: String)
}

// Multiple parameters
@agentDefinition(mount = "/api/inventory/{arg0}/{arg1}")
trait InventoryAgent extends BaseAgent {
  class Id(val arg0: String, val arg1: Int)
}

// Named parameters with @id annotation
import golem.runtime.annotations.id

@agentDefinition(mount = "/api/catalog/{region}/{catalog}")
trait CatalogAgent extends BaseAgent {
  @id
  class CatalogParams(val region: String, val catalog: String)
}
```

Rules:
- Path must start with `/`
- Every constructor parameter must appear as a `{variable}` in the mount path
- Every `{variable}` must match a constructor parameter name
- System variables `{agent-type}` and `{agent-version}` are also available
- Catch-all `{*rest}` variables are **not** allowed in mount paths

## Endpoint Annotation

The `@endpoint` annotation marks a method as an HTTP endpoint. Specify the HTTP method and path:

```scala
@endpoint(method = "GET", path = "/items")
def listItems(): Future[Array[Item]]

@endpoint(method = "POST", path = "/items")
def createItem(name: String, count: Int): Future[Item]

@endpoint(method = "PUT", path = "/items/{id}")
def updateItem(id: String, name: String): Future[Item]

@endpoint(method = "DELETE", path = "/items/{id}")
def deleteItem(id: String): Future[Unit]
```

Endpoint paths are relative to the mount path. Supported HTTP methods: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `TRACE`, or any custom string.

## Path Variables

Path variables `{varName}` in the endpoint path map to method parameters by name:

```scala
@endpoint(method = "GET", path = "/users/{userId}/posts/{postId}")
def getPost(userId: String, postId: String): Future[Post]
```

Remaining (catch-all) path variables capture everything after a prefix:

```scala
@endpoint(method = "GET", path = "/files/{*filePath}")
def getFile(filePath: String): Future[String]
// GET /api/tasks/t1/files/docs/readme.md → filePath = "docs/readme.md"
```

## Query Parameters

Query parameters are specified in the endpoint path using `?key={var}` syntax:

```scala
@endpoint(method = "GET", path = "/search?q={query}&limit={n}")
def search(query: String, n: Int): Future[String]
// GET /api/tasks/t1/search?q=hello&limit=10
```

Each query parameter value must be a `{variable}` reference matching a method parameter.

## Header Variables

Map HTTP headers to method parameters using the `@header` annotation on individual parameters:

```scala
import golem.runtime.annotations.header

@endpoint(method = "POST", path = "/report")
def submitReport(
  @header("X-Tenant") tenantId: String,
  data: String
): Future[String]
```

## Supported Types for Path, Query, and Header Variables

Path variables, query parameters, and header values are extracted from URL strings and parsed into typed values. Only the following types can be used for parameters bound to path/query/header variables:

| Scala Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `Boolean` | Parsed from `"true"` / `"false"` |
| `Int` | Parsed as 32-bit signed integer |
| `Long` | Parsed as 64-bit signed integer |
| `Float` | Parsed as 32-bit float |
| `Double` | Parsed as 64-bit float |
| Enum / sealed trait (unit cases only) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| Scala Type | Behavior |
|---|---|
| `Option[T]` (where `T` is a supported type above) | Optional — absent query param or header produces `None` |
| `List[T]` / `Array[T]` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (case classes, tuples, sealed traits with data, etc.) can only be used as **body parameters** — they cannot be bound to path, query, or header variables.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters that are **not** mapped to path variables, query parameters, or headers are populated from the JSON request body:

```scala
@endpoint(method = "POST", path = "/items/{id}")
def updateItem(id: String, name: String, count: Int): Future[Item]
// POST /api/tasks/t1/items/123
// Body: { "name": "Widget", "count": 5 }
// → id comes from path, name and count come from body fields
```

Each unmapped parameter becomes a top-level field in the expected JSON body object.

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Future[Unit]` | 204 No Content | empty |
| `Future[T]` | 200 OK | JSON-serialized `T` |
| `Future[Option[T]]` | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |

## Data Type to JSON Mapping

| Scala Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `Int`, `Long` | JSON number (integer) |
| `Float`, `Double` | JSON number (float) |
| `Boolean` | JSON boolean |
| `Array[T]`, `List[T]` | JSON array |
| case class (with `Schema`) | JSON object (camelCase field names) |
| `Option[T]` | value or `null` |
| sealed trait / enum | JSON variant |
| `Tuple` | JSON array |

## Custom Types

All types used in endpoint parameters and return values must have a `zio.blocks.schema.Schema` instance:

```scala
import zio.blocks.schema.Schema

final case class Task(id: String, title: String, done: Boolean) derives Schema
final case class CreateTaskRequest(title: String) derives Schema
```

## CORS

CORS origins can be set at the mount level (applies to all endpoints) and/or per endpoint:

```scala
@agentDefinition(
  mount = "/api/{value}",
  cors = Array("https://app.example.com")
)
trait MyAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/data", cors = Array("*"))
  def getData(): Future[String]
  // Allows both https://app.example.com AND *
}
```

## Authentication

Enable authentication at the mount level or per endpoint:

```scala
@agentDefinition(mount = "/api/{value}", auth = true)
trait SecureAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/public", auth = false)
  def publicData(): Future[String] // overrides mount-level auth
}
```

## Phantom Agents

Set `phantomAgent = true` to create a new agent instance for each HTTP request, enabling fully parallel processing:

```scala
@agentDefinition(
  mount = "/webhook/{agent-type}/{value}",
  phantomAgent = true
)
trait WebhookHandler extends BaseAgent {
  class Id(val value: String)
  // Each HTTP request gets its own agent instance
}
```

## Deployment Configuration (golem.yaml)

Add an `httpApi` section to `golem.yaml` to deploy HTTP endpoints:

```yaml
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        TaskAgent: {}
```

The `agents` map lists agent types (in PascalCase) that should be exposed. Use `{}` for default settings.

After adding or updating routes, redeploy:
```shell
golem deploy --reset
```

## Auto-Generated OpenAPI

Golem automatically generates an OpenAPI specification at `/openapi.yaml` on each deployment domain. Access it at:
```
http://my-app.localhost:9006/openapi.yaml
```

## Complete Example

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation, endpoint, header}
import golem.BaseAgent
import zio.blocks.schema.Schema
import scala.concurrent.Future

final case class Task(id: String, title: String, done: Boolean) derives Schema

@agentDefinition(mount = "/task-agents/{name}")
trait TaskAgent extends BaseAgent {
  class Id(val name: String)

  @endpoint(method = "GET", path = "/tasks")
  def getTasks(): Future[Array[Task]]

  @endpoint(method = "POST", path = "/tasks")
  def createTask(title: String): Future[Task]

  @endpoint(method = "GET", path = "/tasks/{id}")
  def getTask(id: String): Future[Option[Task]]

  @endpoint(method = "POST", path = "/report")
  def submitReport(@header("X-Tenant") tenantId: String, data: String): Future[String]
}
```

```scala
import golem.runtime.annotations.agentImplementation
import scala.concurrent.Future

@agentImplementation()
final class TaskAgentImpl(private val name: String) extends TaskAgent {
  private var tasks: Array[Task] = Array.empty

  override def getTasks(): Future[Array[Task]] =
    Future.successful(tasks)

  override def createTask(title: String): Future[Task] = Future.successful {
    val task = Task(id = (tasks.length + 1).toString, title = title, done = false)
    tasks = tasks :+ task
    task
  }

  override def getTask(id: String): Future[Option[Task]] =
    Future.successful(tasks.find(_.id == id))

  override def submitReport(tenantId: String, data: String): Future[String] =
    Future.successful(s"Report from $tenantId: $data")
}
```

```yaml
# golem.yaml (add to existing file)
httpApi:
  deployments:
    local:
    - domain: my-app.localhost:9006
      agents:
        TaskAgent: {}
```

## Key Constraints

- A `mount` path is required on `@agentDefinition` before any `@endpoint` annotations can be used
- All constructor parameters (from `class Id`) must be provided via mount path variables
- Path/query variable names must exactly match method parameter names
- Header parameters use `@header("Header-Name")` annotation on individual method parameters
- `Principal` parameters cannot be mapped to path/query/header variables
- Catch-all path variables `{*name}` can only appear as the last path segment
- The endpoint path must start with `/`
- Custom types require a `zio.blocks.schema.Schema` instance (use `derives Schema` in Scala 3)
- The `scalacOptions += "-experimental"` flag is required for macro annotations
