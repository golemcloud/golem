---
name: golem-add-http-endpoint-effect
description: "Exposing an Effect-based Golem agent over HTTP. Use when adding HTTP mounts, REST endpoints, request parameter bindings, or an httpApi deployment to an @golemcloud/effect-golem agent."
---

# Adding HTTP Endpoints to an Effect Golem Agent

Effect agents publish HTTP route metadata with the `Http` namespace from
`@golemcloud/effect-golem`. The Golem host serves the routes, decodes path, query, header, and JSON
body values into method parameters, invokes the agent, and maps the method result to an HTTP
response.

## Steps

1. Define the agent and its methods with `defineAgent`, `method`, and Effect Schema.
2. Add `http: Http.mount(...)` to the agent definition.
3. Add an `http: [Http.get(...) | Http.post(...) | ...]` array to every exposed method.
4. Implement every handler as an `Effect` and import its module from `src/main.ts`.
5. Add the agent type to an `httpApi` domain deployment in `golem.yaml`.
6. Run `golem build`, then deploy with `golem deploy --yes`.

### Related Skills

| Skill                            | When to Load                                                 |
| -------------------------------- | ------------------------------------------------------------ |
| `golem-add-agent-effect`         | Defining the Effect agent, method schemas, and durable state |
| `golem-http-params-effect`       | Detailed path, query, header, body, and response mapping     |
| `golem-make-http-request-effect` | Making outgoing HTTP requests from an Effect handler         |
| `golem-add-http-auth-effect`     | Enabling authentication and reading the caller principal     |
| `golem-add-cors-effect`          | Configuring mount-level or endpoint-level CORS               |
| `golem-configure-api-domain`     | Configuring the `httpApi` deployment domain                  |

## Mount Path

Import `Http` from the Effect Golem SDK and put one mount on the agent definition:

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, Http, method } from "@golemcloud/effect-golem";

export const TaskAgent = defineAgent({
  name: "TaskAgent",
  mode: "durable",
  constructorParams: {
    taskName: Schema.String,
  },
  http: Http.mount("/api/tasks/{taskName}"),
  methods: {
    // ...
  },
}).implement(() =>
  Effect.succeed({
    // ...
  }),
);
```

Mount rules:

- The path starts with `/` and does not end with `/` unless it is exactly `/`.
- Every constructor parameter appears as a `{variable}` in the mount path.
- Variable names use the exact TypeScript `constructorParams` keys, including casing.
- Mount paths cannot contain query parameters or `{*rest}` catch-all variables.
- Use `{taskName}`, not `{task-name}`, for a constructor field named `taskName`. This changes only
  the placeholder name; both forms would match the same concrete URL segment.

For a fresh phantom agent instance per HTTP request, set the mount option rather than changing the
agent mode:

```typescript
http: Http.mount("/gateway/{name}", { phantomAgent: true }),
```

`mode` still accepts only `"durable"` or `"ephemeral"`; it is independent of
`phantomAgent`.

## Endpoint Declarations

Declare routes on the corresponding method. Endpoint paths are relative to the mount:

```typescript
methods: {
  listItems: method({
    params: {},
    success: Schema.Array(Item),
    http: [Http.get("/items")],
  }),

  createItem: method({
    params: {
      name: Schema.String,
      count: Schema.Number,
    },
    success: Item,
    http: [Http.post("/items")],
  }),

  updateItem: method({
    params: {
      id: Schema.String,
      name: Schema.String,
    },
    success: Schema.NullOr(Item),
    http: [Http.put("/items/{id}")],
  }),

  deleteItem: method({
    params: { id: Schema.String },
    success: Schema.Void,
    http: [Http.del("/items/{id}")],
  }),
},
```

Use these exact helpers:

| HTTP route    | Effect Golem helper                     |
| ------------- | --------------------------------------- |
| GET           | `Http.get(path, options?)`              |
| POST          | `Http.post(path, options?)`             |
| PUT           | `Http.put(path, options?)`              |
| DELETE        | `Http.del(path, options?)`              |
| Custom method | `Http.custom("METHOD", path, options?)` |

One method can have multiple routes by adding multiple entries to its `http` array.

## Parameter Mapping

Bindings always refer to exact method `params` keys:

```typescript
searchItems: method({
  params: {
    category: Schema.String,
    query: Schema.String,
    minPrice: Schema.NullOr(Schema.Number),
    tenant: Schema.String,
  },
  success: Schema.Array(Item),
  http: [
    Http.get(
      "/categories/{category}/search?q={query}&min-price={minPrice}",
      { headers: { "X-Tenant": "tenant" } as const },
    ),
  ],
}),
```

- `{category}` binds a path segment to `params.category`.
- `q={query}` binds query key `q` to `params.query`; the URL key and TypeScript variable may have
  different names.
- Missing optional query or header values decode to `null` when declared with
  `Schema.NullOr(...)`.
- `headers` maps HTTP header names to method parameter names.
- A parameter can be bound from only one of path, query, or headers in a given endpoint.
- `Http.get` has no body, so every parameter must be bound explicitly.
- For `post`, `put`, `del`, and other bodyful endpoints, every unbound parameter comes from a JSON
  object field with the same TypeScript name. For example, unbound `inStock` expects
  `{ "inStock": true }`.

Endpoint paths start with `/`. A `{*rest}` catch-all is allowed only as the final endpoint path
segment.

## HTTP Response Mapping

The Golem HTTP host maps Effect method result schemas as follows:

| Method success schema  | Handler success value | HTTP response   |
| ---------------------- | --------------------- | --------------- |
| `Schema.Void`          | `Effect.void`         | 204, empty body |
| `T`                    | `Effect<T>`           | 200, JSON `T`   |
| `Schema.NullOr(T)`     | `T`                   | 200, JSON `T`   |
| `Schema.NullOr(T)`     | `null`                | 404, empty body |
| method with `error: E` | `Effect.fail(E)`      | 500, JSON `E`   |

`Schema.NullOr(T)` is lowered by the Effect SDK to WIT `option<T>`, which is what enables the
host's 200/404 mapping. Use a declared `error` schema and `Effect.fail(...)` for expected typed
failures; do not use defects for normal not-found behavior.

## Complete Durable Example

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, Http, method, Snapshot } from "@golemcloud/effect-golem";

const TodoItem = Schema.Struct({
  id: Schema.String,
  title: Schema.String,
  done: Schema.Boolean,
});

const TodoState = Schema.Struct({
  items: Schema.Array(TodoItem),
});

export const TodoAgent = defineAgent({
  name: "TodoAgent",
  mode: "durable",
  constructorParams: {
    listName: Schema.String,
  },
  http: Http.mount("/todos/{listName}"),
  snapshot: Snapshot.define({
    schema: TodoState,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    createItem: method({
      params: { title: Schema.String },
      success: TodoItem,
      http: [Http.post("/items")],
    }),
    listItems: method({
      params: {},
      success: Schema.Array(TodoItem),
      http: [Http.get("/items")],
    }),
    completeItem: method({
      params: { id: Schema.String },
      success: Schema.NullOr(TodoItem),
      http: [Http.post("/items/{id}/complete")],
    }),
  },
}).implement((_constructor, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({ items: [] });

    return {
      createItem: ({ title }) =>
        Ref.modify(state, ({ items }) => {
          const item = {
            id: String(items.length + 1),
            title,
            done: false,
          };
          return [item, { items: [...items, item] }] as const;
        }),

      listItems: () => Ref.get(state).pipe(Effect.map(({ items }) => items)),

      completeItem: ({ id }) =>
        Ref.modify(state, ({ items }) => {
          const existing = items.find((item) => item.id === id);
          if (existing === undefined) return [null, { items }] as const;

          const updated = { ...existing, done: true };
          return [
            updated,
            {
              items: items.map((item) => (item.id === id ? updated : item)),
            },
          ] as const;
        }),
    };
  }),
);
```

Register the top-level implementation:

```typescript
// src/main.ts
import "./todo-agent.js";
```

Local imports use the emitted `.js` suffix in generated ESM projects.

## Domain Deployment

Add the agent to the existing `httpApi` deployment without removing other agents:

```yaml
httpApi:
  deployments:
    local:
      - domain: my-app.localhost:9006
        agents:
          TodoAgent: {}
```

Current Golem 1.5 manifests contain deployment configuration only. Do not add legacy
`apiDefinitions`, route lists, OpenAPI extension bindings, or Rib response scripts. Route metadata
comes from `Http.mount(...)` and the method-level `Http.*(...)` declarations. Golem serves the
generated OpenAPI document at `/openapi.yaml` after deployment.

## Key Constraints

- Import Effect APIs from `effect` and `defineAgent`, `Http`, `method`, and `Snapshot` from
  `@golemcloud/effect-golem`.
- Do not use decorators or classes from `@golemcloud/golem-ts-sdk` in an Effect component.
- Every constructor parameter must appear in the mount path with exact TypeScript casing.
- Every bound path, query, or header variable must match a method parameter.
- Unbound bodyful-method parameters use same-named camelCase JSON body fields.
- Handlers return Effects, not plain values or `async` functions.
- Call `snapshot.init(...)` exactly once when the definition declares a snapshot.
- Import the implementation module from `src/main.ts`; otherwise it is not registered.
- Do not edit generated files under `golem-temp/`.
