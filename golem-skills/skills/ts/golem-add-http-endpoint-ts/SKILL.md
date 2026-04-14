---
name: golem-add-http-endpoint-ts
description: "Exposing a TypeScript Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, map request parameters to method arguments, or configure CORS/authentication for HTTP access."
---

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` path to the `@agent()` decorator
2. Annotating methods with `@endpoint()`
3. Adding an `httpApi` deployment section to `golem.yaml`

## Steps

1. Add `mount` to the `@agent()` decorator
2. Add `@endpoint()` decorators to methods
3. Add `httpApi` deployment to `golem.yaml`
4. Build and deploy

## Mount Path

The `mount` option on `@agent()` defines the base HTTP path. Path variables in `{braces}` map to constructor parameters:

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';

@agent({
  mount: '/api/tasks/{name}',
})
class TaskAgent extends BaseAgent {
  constructor(readonly name: string) {
    super();
  }
  // ...
}
```

Rules:
- Path must start with `/`
- Every constructor parameter must appear as a `{variable}` in the mount path
- Every `{variable}` must match a constructor parameter name
- System variables `{agent-type}` and `{agent-version}` are also available
- Catch-all `{*rest}` variables are **not** allowed in mount paths

## Endpoint Decorator

The `@endpoint()` decorator marks a method as an HTTP endpoint. Specify exactly one HTTP method (`get`, `post`, `put`, `delete`, or `custom`):

```typescript
@endpoint({ get: '/items' })
async listItems(): Promise<Item[]> { ... }

@endpoint({ post: '/items' })
async createItem(name: string, count: number): Promise<Item> { ... }

@endpoint({ put: '/items/{id}' })
async updateItem(id: string, name: string): Promise<Item> { ... }

@endpoint({ delete: '/items/{id}' })
async deleteItem(id: string): Promise<void> { ... }

@endpoint({ custom: { method: 'PATCH', path: '/items/{id}' } })
async patchItem(id: string, patch: PatchData): Promise<Item> { ... }
```

Endpoint paths are relative to the mount path. A method can have multiple `@endpoint()` decorators to expose it under different routes.

## Path Variables

Path variables `{varName}` in the endpoint path map to method parameters by name:

```typescript
@endpoint({ get: '/users/{userId}/posts/{postId}' })
async getPost(userId: string, postId: string): Promise<Post> { ... }
```

Remaining (catch-all) path variables capture everything after a prefix:

```typescript
@endpoint({ get: '/files/{*path}' })
async getFile(path: string): Promise<FileContent> { ... }
// GET /api/tasks/t1/files/docs/readme.md → path = "docs/readme.md"
```

## Query Parameters

Query parameters are specified in the endpoint path using standard `?key={var}` syntax:

```typescript
@endpoint({ get: '/search?q={query}&limit={maxResults}' })
async search(query: string, maxResults: number): Promise<SearchResult[]> { ... }
// GET /api/tasks/t1/search?q=hello&limit=10
```

Each query parameter value must be a `{variable}` reference matching a method parameter.

## Header Variables

Map HTTP headers to method parameters using the `headers` option:

```typescript
@endpoint({
  get: '/data',
  headers: { 'X-Request-Id': 'requestId', 'Authorization': 'token' }
})
async getData(requestId: string, token: string): Promise<Data> { ... }
```

Headers can also be mapped to constructor parameters at the mount level in `@agent()`:

```typescript
@agent({
  mount: '/api',
  headers: { 'X-Api-Key': 'apiKey' }
})
class ApiAgent extends BaseAgent {
  constructor(readonly apiKey: string) { super(); }
  // ...
}
```

When using mount-level headers, all constructor parameters must be satisfied by either path variables or header variables.

## Supported Types for Path, Query, and Header Variables

Path variables, query parameters, and header values are extracted from URL strings and parsed into typed values. Only the following types can be used for parameters bound to path/query/header variables:

| TypeScript Type | Parsed From |
|---|---|
| `string` | Used as-is |
| `number` | Parsed as float (`f64`) |
| `boolean` | Parsed from `"true"` / `"false"` |
| String literal union (e.g. `"red" \| "green"`) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| TypeScript Type | Behavior |
|---|---|
| `T \| undefined` (where `T` is a supported type above) | Optional — absent query param or header produces `undefined` |
| `Array<T>` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (objects, interfaces, nested arrays, `Map`, etc.) can only be used as **body parameters** — they cannot be bound to path, query, or header variables.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters that are **not** mapped to path variables, query parameters, or headers are populated from the JSON request body:

```typescript
@endpoint({ post: '/items/{id}' })
async updateItem(id: string, name: string, count: number): Promise<Item> { ... }
// POST /api/tasks/t1/items/123
// Body: { "name": "Widget", "count": 5 }
// → id comes from path, name and count come from body
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. Field names in the JSON body use the original camelCase parameter names.

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `void` / no return | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `T \| undefined` | 200 OK if value, 404 Not Found if `undefined` | JSON `T` or empty |
| `Result<T, E>` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `T` or JSON `E` |
| `Result<void, E>` | 204 No Content if `Ok`, 500 if `Err` | empty or JSON `E` |
| `Result<T, void>` | 200 OK if `Ok`, 500 if `Err` | JSON `T` or empty |
| `UnstructuredBinary` | 200 OK | Raw binary with Content-Type |

## Data Type to JSON Mapping

| TypeScript Type | JSON Representation |
|---|---|
| `string` | JSON string |
| `number` | JSON number |
| `boolean` | JSON boolean |
| `Array<T>` | JSON array |
| `object` / interface / type alias | JSON object (camelCase field names) |
| `T \| undefined` or `T \| null` | value or `null` |
| `string` literal union (e.g. `"a" \| "b"`) | JSON string |
| `Map<K, V>` | JSON array of `[key, value]` tuples |

## Binary Request and Response Bodies

Use `UnstructuredBinary` from the SDK for raw binary payloads:

```typescript
import { UnstructuredBinary } from '@golemcloud/golem-ts-sdk';

// Accepting any binary content type
@endpoint({ post: '/upload/{bucket}' })
async upload(bucket: string, payload: UnstructuredBinary): Promise<number> {
  if (payload.tag === 'url') return -1;
  return payload.val.byteLength;
}

// Restricting to specific MIME types
@endpoint({ post: '/upload-image/{bucket}' })
async uploadImage(
  bucket: string,
  payload: UnstructuredBinary<["image/gif"]>
): Promise<number> { ... }

// Returning binary data
@endpoint({ get: '/download' })
async download(): Promise<UnstructuredBinary> {
  return UnstructuredBinary.fromInline(
    new Uint8Array([1, 2, 3]),
    'application/octet-stream'
  );
}
```

## Principal (Authentication)

When `auth: true` is set on the mount or endpoint, methods can receive a `Principal` parameter with info about the authenticated user. `Principal` parameters are automatically populated and must **not** be mapped to path/query/header variables:

```typescript
import { Principal } from '@golemcloud/golem-ts-sdk';

@agent({ mount: '/secure/{name}' })
class SecureAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/whoami', auth: true })
  async whoAmI(principal: Principal): Promise<{ value: Principal }> {
    return { value: principal };
  }

  // Principal can appear at any position among parameters
  @endpoint({ get: '/data/{id}' })
  async getData(id: string, principal: Principal): Promise<Data> { ... }
}
```

## CORS

CORS origins can be set at the mount level (applies to all endpoints) and/or per endpoint (union of both):

```typescript
@agent({
  mount: '/api/{name}',
  cors: ['https://app.example.com']
})
class MyAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/data', cors: ['*'] })
  async getData(): Promise<Data> { ... }
  // This endpoint allows both https://app.example.com AND *
}
```

## Phantom Agents

Set `phantom: true` on the agent to create a new ephemeral agent instance for each HTTP request. This enables fully parallel request processing:

```typescript
@agent({
  mount: '/gateway/{name}',
  phantom: true
})
class GatewayAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }
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

```typescript
import { BaseAgent, agent, endpoint, Result, UnstructuredBinary } from '@golemcloud/golem-ts-sdk';

type Task = { id: string; title: string; done: boolean };
type CreateTaskRequest = { title: string };

@agent({ mount: '/task-agents/{name}' })
class TaskAgent extends BaseAgent {
  private tasks: Task[] = [];

  constructor(readonly name: string) {
    super();
  }

  @endpoint({ get: '/tasks' })
  async getTasks(): Promise<Task[]> {
    return this.tasks;
  }

  @endpoint({ post: '/tasks' })
  async createTask(title: string): Promise<Task> {
    const task: Task = { id: String(this.tasks.length + 1), title, done: false };
    this.tasks.push(task);
    return task;
  }

  @endpoint({ get: '/tasks/{id}' })
  async getTask(id: string): Promise<Task | undefined> {
    return this.tasks.find(t => t.id === id);
  }

  @endpoint({ post: '/tasks/{id}/complete' })
  async completeTask(id: string): Promise<Result<Task, { error: string }>> {
    const task = this.tasks.find(t => t.id === id);
    if (!task) return Result.err({ error: 'not found' });
    task.done = true;
    return Result.ok(task);
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

- A `mount` path is required on `@agent()` before any `@endpoint()` decorators can be used
- All constructor parameters must be provided via mount path variables or header variables
- Path/query/header variable names must exactly match method parameter names
- `Principal` parameters cannot be mapped to path/query/header variables
- Catch-all path variables `{*name}` can only appear as the last path segment
- The endpoint path must start with `/`
- Exactly one HTTP method must be specified per `@endpoint()` decorator
