---
name: golem-add-http-endpoint-ts
description: "Exposing a TypeScript Golem agent over HTTP. Use when the user asks to add HTTP endpoints, mount an agent to a URL path, or expose agent methods as a REST API."
---

# Adding HTTP Endpoints to a TypeScript Golem Agent

## Overview

Golem agents can be exposed over HTTP using code-first route definitions. This involves:
1. Adding a `mount` path to the `@agent()` decorator
2. Annotating methods with `@endpoint()`
3. Adding an `httpApi` deployment section to `golem.yaml` (load the `golem-configure-api-domain` skill)

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-http-params-ts` | Path/query/header variable mapping, body mapping, supported types, response mapping |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code, especially when calling other Golem agent endpoints (required for correct JSON body formatting) |
| `golem-add-http-auth-ts` | Enabling authentication and receiving `Principal` |
| `golem-add-cors-ts` | Configuring CORS allowed origins |
| `golem-configure-api-domain` | Setting up `httpApi` in `golem.yaml`, security schemes, domain deployments |

## Steps

1. Add `mount` to the `@agent()` decorator
2. Add `@endpoint()` decorators to methods
3. Add `httpApi` deployment to `golem.yaml` (see `golem-configure-api-domain` skill)
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
- Every constructor parameter must appear as a `{variable}` in the mount path (or be mapped via mount-level `headers`)
- Every `{variable}` must match a constructor parameter name
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

For details on how path variables, query parameters, headers, and request bodies map to method parameters, load the `golem-http-params-ts` skill.

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

## Complete Example

```typescript
import { BaseAgent, agent, endpoint, Result } from '@golemcloud/golem-ts-sdk';

type Task = { id: string; title: string; done: boolean };

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
- Catch-all path variables `{*name}` can only appear as the last path segment
- The endpoint path must start with `/`
- Exactly one HTTP method must be specified per `@endpoint()` decorator
