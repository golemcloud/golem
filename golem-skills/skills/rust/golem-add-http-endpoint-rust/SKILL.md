---
name: golem-add-http-endpoint-rust
description: "Exposing a Rust Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, or expose agent methods as a REST API."
---

# Adding HTTP Endpoints to a Rust Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` parameter to `#[agent_definition]`
2. Annotating trait methods with `#[endpoint(...)]`
3. Adding an `httpApi` deployment section to `golem.yaml` (load the `golem-configure-api-domain` skill)

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-http-params-rust` | Path/query/header variable mapping, body mapping, supported types, response mapping |
| `golem-make-http-request-rust` | Making outgoing HTTP requests from agent code, especially when calling other Golem agent endpoints (required for correct JSON body formatting) |
| `golem-add-http-auth-rust` | Enabling authentication |
| `golem-add-cors-rust` | Configuring CORS allowed origins |
| `golem-configure-api-domain` | Setting up `httpApi` in `golem.yaml`, security schemes, domain deployments |

## Steps

1. Add `mount = "/path/{param}"` to `#[agent_definition(...)]`
2. Add `#[endpoint(get = "/...")]` (or `post`, `put`, `delete`) to trait methods
3. Add `httpApi` deployment to `golem.yaml` (see `golem-configure-api-domain` skill)
4. Build and deploy

## Mount Path

The `mount` parameter on `#[agent_definition]` defines the base HTTP path. Path variables in `{braces}` map to constructor parameters:

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/api/tasks/{task_name}")]
pub trait TaskAgent {
    fn new(task_name: String) -> Self;

    // methods...
}
```

Rules:
- Path must start with `/`
- Every constructor parameter must appear as a `{variable}` in the mount path (using the parameter name)
- Every `{variable}` must match a constructor parameter name
- Catch-all `{*rest}` variables are **not** allowed in mount paths

## Endpoint Annotation

The `#[endpoint(...)]` attribute marks a trait method as an HTTP endpoint. Specify one HTTP method with its path:

```rust
#[endpoint(get = "/items")]
fn list_items(&self) -> Vec<Item>;

#[endpoint(post = "/items")]
fn create_item(&mut self, name: String, count: u64) -> Item;

#[endpoint(put = "/items/{id}")]
fn update_item(&mut self, id: String, name: String) -> Item;

#[endpoint(delete = "/items/{id}")]
fn delete_item(&mut self, id: String);
```

Endpoint paths are relative to the mount path. A method can have multiple `#[endpoint(...)]` attributes to expose it under different routes.

For details on how path variables, query parameters, headers, and request bodies map to method parameters, load the `golem-http-params-rust` skill.

## Phantom Agents

Set `phantom_agent = true` to create a new agent instance for each HTTP request, enabling fully parallel processing:

```rust
#[agent_definition(mount = "/gateway/{name}", phantom_agent = true)]
pub trait GatewayAgent {
    fn new(name: String) -> Self;
    // Each HTTP request gets its own agent instance
}
```

## Custom Types

All types used in endpoint parameters and return values must derive `Schema`:

```rust
use golem_rust::Schema;

#[derive(Clone, Schema)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub done: bool,
}
```

## Complete Example

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint, Schema};

#[derive(Clone, Schema)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub done: bool,
}

#[derive(Schema)]
pub struct ErrorResponse {
    pub error: String,
}

#[agent_definition(mount = "/task-agents/{name}")]
pub trait TaskAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/tasks")]
    fn get_tasks(&self) -> Vec<Task>;

    #[endpoint(post = "/tasks")]
    fn create_task(&mut self, title: String) -> Task;

    #[endpoint(get = "/tasks/{id}")]
    fn get_task(&self, id: String) -> Option<Task>;

    #[endpoint(post = "/tasks/{id}/complete")]
    fn complete_task(&mut self, id: String) -> Result<Task, ErrorResponse>;
}

struct TaskAgentImpl {
    name: String,
    tasks: Vec<Task>,
}

#[agent_implementation]
impl TaskAgent for TaskAgentImpl {
    fn new(name: String) -> Self {
        Self { name, tasks: vec![] }
    }

    fn get_tasks(&self) -> Vec<Task> {
        self.tasks.clone()
    }

    fn create_task(&mut self, title: String) -> Task {
        let task = Task {
            id: format!("{}", self.tasks.len() + 1),
            title,
            done: false,
        };
        self.tasks.push(task.clone());
        task
    }

    fn get_task(&self, id: String) -> Option<Task> {
        self.tasks.iter().find(|t| t.id == id).cloned()
    }

    fn complete_task(&mut self, id: String) -> Result<Task, ErrorResponse> {
        match self.tasks.iter_mut().find(|t| t.id == id) {
            Some(task) => {
                task.done = true;
                Ok(task.clone())
            }
            None => Err(ErrorResponse { error: "not found".to_string() }),
        }
    }
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

- A `mount` path is required on `#[agent_definition]` before any `#[endpoint]` attributes can be used
- All constructor parameters must be provided via mount path variables
- Path/query/header variable names must exactly match method parameter names
- Catch-all path variables `{*name}` can only appear as the last path segment
- The endpoint path must start with `/`
- Exactly one HTTP method must be specified per `#[endpoint]` attribute
- All custom types used in parameters or return values must derive `Schema`
