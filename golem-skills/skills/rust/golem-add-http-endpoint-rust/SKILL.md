---
name: golem-add-http-endpoint-rust
description: "Exposing a Rust Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, map request parameters to method arguments, or configure CORS/authentication for HTTP access."
---

# Adding HTTP Endpoints to a Rust Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` parameter to `#[agent_definition]`
2. Annotating trait methods with `#[endpoint(...)]`
3. Adding an `httpApi` deployment section to `golem.yaml`

## Steps

1. Add `mount = "/path/{param}"` to `#[agent_definition(...)]`
2. Add `#[endpoint(get = "/...")]` (or `post`, `put`, `delete`) to trait methods
3. Add `httpApi` deployment to `golem.yaml`
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
- System variables `{agent-type}` and `{agent-version}` are also available
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

## Path Variables

Path variables `{var_name}` in the endpoint path map to method parameters by name:

```rust
#[endpoint(get = "/users/{user_id}/posts/{post_id}")]
fn get_post(&self, user_id: String, post_id: String) -> Post;
```

Remaining (catch-all) path variables capture everything after a prefix:

```rust
#[endpoint(get = "/files/{*path}")]
fn get_file(&self, path: String) -> FileContent;
// GET /api/tasks/t1/files/docs/readme.md → path = "docs/readme.md"
```

## Query Parameters

Query parameters are specified in the endpoint path using `?key={var}` syntax:

```rust
#[endpoint(get = "/search?q={query}&limit={max_results}")]
fn search(&self, query: String, max_results: u64) -> Vec<SearchResult>;
// GET /api/tasks/t1/search?q=hello&limit=10
```

Each query parameter value must be a `{variable}` reference matching a method parameter.

## Header Variables

Map HTTP headers to method parameters using the `headers(...)` block:

```rust
#[endpoint(
    get = "/data",
    headers("X-Request-Id" = "request_id", "Authorization" = "token")
)]
fn get_data(&self, request_id: String, token: String) -> Data;
```

## Supported Types for Path, Query, and Header Variables

Path variables, query parameters, and header values are extracted from URL strings and parsed into typed values. Only the following types can be used for parameters bound to path/query/header variables:

| Rust Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `char` | Single character |
| `bool` | Parsed from `"true"` / `"false"` |
| `u8`, `u16`, `u32`, `u64` | Parsed as unsigned integer |
| `i8`, `i16`, `i32`, `i64` | Parsed as signed integer |
| `f32`, `f64` | Parsed as floating-point number |
| Enum (unit variants only) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| Rust Type | Behavior |
|---|---|
| `Option<T>` (where `T` is a supported type above) | Optional — absent query param or header produces `None` |
| `Vec<T>` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (structs, tuples, enums with data, `HashMap`, etc.) can only be used as **body parameters** — they cannot be bound to path, query, or header variables.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters that are **not** mapped to path variables, query parameters, or headers are populated from the JSON request body:

```rust
#[endpoint(post = "/items/{id}")]
fn update_item(&mut self, id: String, name: String, count: u64) -> Item;
// POST /api/tasks/t1/items/123
// Body: { "name": "Widget", "count": 5 }
// → id comes from path, name and count come from body fields
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. Field names in JSON use the snake_case parameter names converted to camelCase.

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `()` (unit / no return) | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `Option<T>` | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |
| `Result<T, E>` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `T` or JSON `E` |
| `Result<(), E>` | 204 No Content if `Ok`, 500 if `Err` | empty or JSON `E` |
| `Result<T, ()>` | 200 OK if `Ok`, 500 if `Err` | JSON `T` or empty |
| `UnstructuredBinary<M>` | 200 OK | Raw binary with Content-Type |

## Data Type to JSON Mapping

| Rust Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `u8`–`u64`, `i8`–`i64` | JSON number (integer) |
| `f32`, `f64` | JSON number (float) |
| `bool` | JSON boolean |
| `Vec<T>` | JSON array |
| Struct (with `Schema`) | JSON object (camelCase field names) |
| `Option<T>` | value or `null` |
| `Result<T, E>` | value (see response mapping above) |
| Enum (unit variants) | JSON string |
| Enum (with data) | JSON object with tag |
| `HashMap<K, V>` | JSON array of `[key, value]` tuples |

## Custom Types

All types used in endpoint parameters and return values must derive `Schema`:

```rust
use golem_rust::Schema;

#[derive(Schema)]
pub struct CreateTaskRequest {
    pub title: String,
}

#[derive(Schema)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub done: bool,
}
```

## Binary Request and Response Bodies

Use `UnstructuredBinary` from the SDK for raw binary payloads:

```rust
use golem_rust::agentic::UnstructuredBinary;

// Accepting any binary content type
#[endpoint(post = "/upload/{bucket}")]
fn upload(&self, bucket: String, payload: UnstructuredBinary<String>) -> i64;

// Restricting to specific MIME types using AllowedMimeTypes derive
use golem_rust::AllowedMimeTypes;

#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum ImageTypes {
    #[mime_type("image/gif")]
    ImageGif,
    #[mime_type("image/png")]
    ImagePng,
}

#[endpoint(post = "/upload-image/{bucket}")]
fn upload_image(&self, bucket: String, payload: UnstructuredBinary<ImageTypes>) -> i64;

// Returning binary data
#[endpoint(get = "/download")]
fn download(&self) -> UnstructuredBinary<String>;
```

In the implementation:
```rust
fn upload(&self, _bucket: String, payload: UnstructuredBinary<String>) -> i64 {
    match payload {
        UnstructuredBinary::Url(_) => -1,
        UnstructuredBinary::Inline { data, .. } => data.len() as i64,
    }
}

fn download(&self) -> UnstructuredBinary<String> {
    UnstructuredBinary::Inline {
        data: vec![1, 2, 3, 4],
        mime_type: "application/octet-stream".to_string(),
    }
}
```

## CORS

CORS origins can be set at the mount level (applies to all endpoints) and/or per endpoint (union of both):

```rust
#[agent_definition(
    mount = "/api/{name}",
    cors = ["https://app.example.com"]
)]
pub trait MyAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/data", cors = ["*"])]
    fn get_data(&self) -> Data;
    // Allows both https://app.example.com AND *
}
```

## Authentication

Enable authentication at the mount level or per endpoint:

```rust
#[agent_definition(mount = "/api/{name}", auth = true)]
pub trait SecureAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/public", auth = false)]
    fn public_data(&self) -> String; // overrides mount-level auth
}
```

## Phantom Agents

Set `phantom_agent = true` to create a new agent instance for each HTTP request, enabling fully parallel processing:

```rust
#[agent_definition(mount = "/gateway/{name}", phantom_agent = true)]
pub trait GatewayAgent {
    fn new(name: String) -> Self;
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
