---
name: golem-add-http-endpoint-scala
description: "Exposing a Scala Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, or expose agent methods as a REST API."
---

# Adding HTTP Endpoints to a Scala Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` parameter to `@agentDefinition`
2. Annotating methods with `@endpoint`
3. Adding an `httpApi` deployment section to `golem.yaml` (load the `golem-configure-api-domain` skill)

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-http-params-scala` | Path/query/header variable mapping, body mapping, supported types, response mapping |
| `golem-add-http-auth-scala` | Enabling authentication |
| `golem-add-cors-scala` | Configuring CORS allowed origins |
| `golem-configure-api-domain` | Setting up `httpApi` in `golem.yaml`, security schemes, domain deployments |

## Steps

1. Add `mount = "/path/{param}"` to `@agentDefinition(...)`
2. Add `@endpoint(method = "GET", path = "/...")` to trait methods
3. Add `httpApi` deployment to `golem.yaml` (see `golem-configure-api-domain` skill)
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

For details on how path variables, query parameters, headers, and request bodies map to method parameters, load the `golem-http-params-scala` skill.

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

## Custom Types

All types used in endpoint parameters and return values must have a `zio.blocks.schema.Schema` instance:

```scala
import zio.blocks.schema.Schema

final case class Task(id: String, title: String, done: Boolean) derives Schema
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
- Catch-all path variables `{*name}` can only appear as the last path segment
- The endpoint path must start with `/`
- Custom types require a `zio.blocks.schema.Schema` instance (use `derives Schema` in Scala 3)
- The `scalacOptions += "-experimental"` flag is required for macro annotations
