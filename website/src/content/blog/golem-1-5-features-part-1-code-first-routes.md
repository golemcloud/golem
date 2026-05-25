---
title: "Golem 1.5 features — Part 1: Code-first routes"
date: "2026-04-08T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-1-code-first-routes"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part1-code-first-routes/"
---

## Introduction

I am writing a series of _short_ posts showcasing the new features of **Golem 1.5**, to be released at the end of April, 2026. The episodes of this series will be short and assume the reader knows what Golem is. Check my [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information!

## Code-first routes

Previously, Golem required developers to define HTTP endpoints using OpenAPI-like YAML and a custom scripting language called Rib to map between requests and agent interfaces. Version 1.5 eliminates this requirement, allowing everything to be defined directly in code.

### Mount points

Agents require a mount point definition:

```typescript
@agent({
  mount: "/task-agents/{name}",
})
export class Tasks extends BaseAgent {
  // ...
}
```

```rust
#[agent_definition(mount = "/task-agents/{name}")]
pub trait Tasks {
    // ...
}
```

```scala
@agentDefinition(mount = "/task-agents/{name}")
trait Tasks extends BaseAgent {
  // ...
}
```

```moonbit
#derive.agent
#derive.mount("/task-agents/{name}")
pub(all) struct Tasks {
  // ...
}
```

Mount paths can use placeholders like `{name}` that map to agent constructor parameters. All agent parameters must be mapped in the mount path.

### Endpoints

Individual agent methods can be exported as endpoints:

```typescript
@endpoint({ post: "/tasks" })
async createTask(request: CreateTaskRequest): Promise<Task> {
    // ...
}

async getTasks(): Promise<Task[]> {
    // ...
}

@endpoint({ post: "/tasks/{id}/complete" })
async completeTask(id: number): Promise<Task | null> {
    // ...
}
```

```rust
#[endpoint(post = "/tasks")]
fn create_task(&mut self, request: CreateTaskRequest) -> Task;

#[endpoint(get = "/tasks")]
fn get_tasks(&self) -> Vec<Task>;

#[endpoint(post = "/tasks/{id}/complete")]
fn complete_task(&mut self, id: usize) -> Option<Task>;
```

```scala
@endpoint(method = "POST", path = "/tasks")
def createTask(request: CreateTaskRequest): Future[Task]

@endpoint(method = "GET", path = "/tasks")
def getTasks(): Future[Array[Task]]

@endpoint(method = "POST", path = "/tasks/{id}/complete")
def completeTask(id: Int): Future[Option[Task]]
```

```moonbit
#derive.endpoint(post="/tasks")
pub fn Tasks::create_task(self : Self, request : CreateTaskRequest) -> Task {
  // ...
}

#derive.endpoint(get="/tasks")
pub fn Tasks::get_tasks(self: Self) -> Array[Task] {
  // ...
}

#derive.endpoint(post="/tasks/{id}/complete")
pub fn Tasks::complete_task(self: Self, id: UInt32) -> Option[Task] {
  // ...
}
```

Endpoint paths are relative to the mount point and support placeholders mapped to parameters. Unmapped parameters come from the request body, and query parameters are supported in path patterns.

### Additional features

Custom headers can be mapped to function parameters:

```typescript
@endpoint({
    get: '/example',
    headers: { 'X-Foo': 'location', 'X-Bar': 'name' },
  })
async example(location: string, name: string): Promise<String> {
  // ...
}
```

```rust
#[endpoint(get = "/example", headers("X-Foo" = "location", "X-Bar" = "name"))]
fn example(&self, location: String, name: String) -> String;
```

```scala
@endpoint(method = "GET", path = "/example")
def example(@header("X-Foo") location: String, @header("X-Bar") name: String): Future[String]
```

```moonbit
#derive.endpoint(get="/example")
#derive.endpoint_header("X-Foo", "location")
#derive.endpoint_header("X-Bar", "name")
pub fn ExampleAgent::example(
  self : Self,
  location: String,
  name: String
) -> String {
  // ...
}
```

Endpoint decorators support CORS and authentication. CORS can be configured with syntax like `cors = ["*"]`. When authentication is enabled, agent constructors and methods can optionally receive a `Principal` parameter containing authenticated user information.

Developers can configure the HTTP layer to create a **phantom agent** for each request — useful for stateless, ephemeral agents serving as gateways. This is a single-line change (`phantomAgent = true`) that allows requests to be processed in parallel.

### Deployments

A small step in the application manifest file deploys code-first routes:

```yaml
httpApi:
  deployments:
    local:
      - domain: app-name.localhost:9006
        agents:
          Tasks: {}
```

Developers specify which agents deploy to which domains per environment (local, staging, production).

### OpenAPI

Code-first endpoint definitions automatically generate proper OpenAPI specifications. Golem adds an `openapi.yaml` endpoint to each deployment:

```bash
$ curl http://routes.localhost:9006/openapi.yaml
components: {}
info:
  title: Managed api provided by Golem
  version: 1.0.0
openapi: 3.0.0
paths:
  /openapi.yaml:
    get:
      responses:
        "200":
          content:
            application/yaml:
              schema:
                additionalProperties: true
                type: object
          description: Response 200
  /task-agents/{name}/tasks:
    get:
      parameters:
        - description: 'Path parameter: name'
          explode: false
          in: path
          name: name
          required: true
          schema:
            type: string
          style: simple
      responses:
        "200":
```

<!-- WebFetch returned truncated content for this post; OpenAPI sample may continue beyond this point in the original article -->
