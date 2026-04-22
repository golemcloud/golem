---
name: golem-add-http-endpoint-moonbit
description: "Exposing a MoonBit Golem agent over HTTP with mount paths and endpoint annotations. Use when the user asks to add HTTP endpoints, REST APIs, or web interfaces to an agent."
---

# Adding HTTP Endpoints to a MoonBit Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding `#derive.mount("/path/{param}")` to the agent struct
2. Annotating methods with `#derive.endpoint(get="/path")` (or `post`, `put`, `delete`)
3. Adding an `httpApi` deployment section to `golem.yaml` (load the `golem-configure-api-domain` skill)

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-agent-moonbit` | Creating a new agent from scratch before adding HTTP endpoints |
| `golem-configure-api-domain` | Setting up `httpApi` in `golem.yaml`, security schemes, domain deployments |

## Steps

1. Add `#derive.mount("/path/{param}")` to the agent struct (below `#derive.agent`)
2. Add `#derive.endpoint(get="/...")` (or `post`, `put`, `delete`) to public methods
3. Optionally add `#derive.mount_auth(false)` to disable authentication
4. Optionally add `#derive.mount_cors("https://origin.com")` to configure CORS
5. Add `httpApi` deployment to `golem.yaml` (see `golem-configure-api-domain` skill)
6. Build and deploy

## Mount Path

The `#derive.mount("/path")` annotation on the agent struct defines the base HTTP path. Path variables in `{braces}` map to constructor parameters:

```moonbit
#derive.agent
#derive.mount("/api/tasks/{task_name}")
struct TaskAgent {
  task_name : String
  mut tasks : Array[TaskInfo]
}

fn TaskAgent::new(task_name : String) -> TaskAgent {
  { task_name, tasks: [] }
}
```

Rules:
- Path must start with `/`
- Every constructor parameter must appear as a `{variable}` in the mount path (using the parameter name)
- Every `{variable}` must match a constructor parameter name
- Catch-all `{*rest}` variables are **not** allowed in mount paths

## Endpoint Annotation

The `#derive.endpoint(...)` attribute marks a public method as an HTTP endpoint. Specify one HTTP method with its path:

```moonbit
#derive.endpoint(get="/items")
pub fn TaskAgent::list_items(self : Self) -> Array[Item] {
  self.items
}

#derive.endpoint(post="/items")
pub fn TaskAgent::create_item(self : Self, name : String, count : UInt64) -> Item {
  let item = { id: self.items.length().to_string(), name, count }
  self.items.push(item)
  item
}

#derive.endpoint(put="/items/{id}")
pub fn TaskAgent::update_item(self : Self, id : String, name : String) -> Item {
  // ...
}

#derive.endpoint(delete="/items/{id}")
pub fn TaskAgent::delete_item(self : Self, id : String) -> Unit {
  // ...
}
```

Endpoint paths are relative to the mount path.

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```moonbit
#derive.endpoint(get="/search?q={query}&limit={max_results}")
pub fn MyAgent::search(self : Self, query : String, max_results : UInt64) -> Array[SearchResult] {
  // query and max_results are extracted from the URL query string
}
```

## Header Variables

Map HTTP headers to method parameters using `#derive.endpoint_header("Header-Name", "param_name")`:

```moonbit
#derive.endpoint(post="/data")
#derive.endpoint_header("X-Request-Id", "request_id")
pub fn MyAgent::submit_data(self : Self, request_id : String, payload : String) -> String {
  // request_id comes from the X-Request-Id header, payload from the JSON body
}
```

## Authentication

Disable authentication on a mount with `#derive.mount_auth(false)`:

```moonbit
#derive.agent
#derive.mount("/public/{name}")
#derive.mount_auth(false)
struct PublicAgent {
  name : String
}
```

## CORS

Configure allowed CORS origins with `#derive.mount_cors(...)`:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_cors("https://app.example.com", "https://other.example.com")
struct ApiAgent {
  name : String
}
```

Multiple origins can be specified as separate string arguments.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters **not** bound to path variables, query parameters, or headers are populated from the JSON request body:

```moonbit
#derive.endpoint(post="/items/{id}")
pub fn MyAgent::update_item(self : Self, id : String, name : String, count : UInt64) -> Item {
  // id from path, name and count from JSON body: { "name": "Widget", "count": 5 }
}
```

## Custom Types

All types used in endpoint parameters and return values must be annotated with `#derive.golem_schema`:

```moonbit
#derive.golem_schema
pub(all) struct Task {
  id : String
  title : String
  done : Bool
} derive(ToJson, @json.FromJson)

#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq, ToJson, @json.FromJson)
```

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Unit` (no return) | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `T?` (`Option[T]`) | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |
| `Result[T, E]` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `T` or JSON `E` |

## Complete Example

```moonbit
///|
/// Priority level for tasks
#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq, ToJson, @json.FromJson)

///|
/// A task record
#derive.golem_schema
pub(all) struct Task {
  id : String
  title : String
  priority : Priority
  done : Bool
} derive(ToJson, @json.FromJson)

///|
/// A task management agent exposed over HTTP
#derive.agent
#derive.mount("/task-agents/{name}")
#derive.mount_auth(false)
struct TaskAgent {
  name : String
  mut tasks : Array[Task]
}

///|
fn TaskAgent::new(name : String) -> TaskAgent {
  { name, tasks: [] }
}

///|
/// List all tasks
#derive.endpoint(get="/tasks")
pub fn TaskAgent::get_tasks(self : Self) -> Array[Task] {
  self.tasks
}

///|
/// Create a new task
#derive.endpoint(post="/tasks")
pub fn TaskAgent::create_task(self : Self, title : String, priority : Priority) -> Task {
  let task = {
    id: self.tasks.length().to_string(),
    title,
    priority,
    done: false,
  }
  self.tasks.push(task)
  task
}

///|
/// Get a task by ID
#derive.endpoint(get="/tasks/{id}")
pub fn TaskAgent::get_task(self : Self, id : String) -> Task? {
  self.tasks.iter().find_first(fn(t) { t.id == id })
}

///|
/// Mark a task as complete
#derive.endpoint(post="/tasks/{id}/complete")
pub fn TaskAgent::complete_task(self : Self, id : String) -> Task? {
  for t in self.tasks {
    if t.id == id {
      t.done = true
      return Some(t)
    }
  }
  None
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

- `#derive.mount("/path")` is required on the agent struct before any `#derive.endpoint(...)` annotations can be used
- All constructor parameters must be provided via mount path variables
- Path/query/header variable names must exactly match method parameter names
- The endpoint path must start with `/`
- Exactly one HTTP method must be specified per `#derive.endpoint(...)` annotation
- All custom types used in parameters or return values must have `#derive.golem_schema`
- Method names use `snake_case`
- Only `pub fn` methods can be exposed as HTTP endpoints
- **Never edit generated files** — `golem_reexports.mbt`, `golem_agents.mbt`, and `golem_derive.mbt` are auto-generated by `golem build`
