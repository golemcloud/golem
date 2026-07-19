---
name: golem-http-params-effect
description: "Mapping HTTP path, query, header, and body values to Effect Golem agent parameters and mapping method results to responses. Use when defining request bindings, optional parameters, JSON bodies, response schemas, or unstructured HTTP inputs with @golemcloud/effect-golem."
---

# HTTP Request and Response Parameter Mapping (Effect)

Effect Golem endpoints declare request bindings with the `Http` namespace and describe every
method parameter and result with Effect Schema. The Golem host performs the HTTP parsing and
response rendering; handlers receive typed values and return `Effect` values rather than reading
or constructing raw HTTP requests and responses.

Load `golem-add-http-endpoint-effect` when the agent still needs a mount, endpoint declarations,
an implementation import in `src/main.ts`, or an `httpApi` deployment.

## Path Variables

Place constructor identity fields in the mount path and method fields in endpoint paths. Variable
names must exactly match the corresponding `constructorParams` or `params` keys, including
TypeScript casing:

```typescript
import { Schema } from "effect";
import { defineAgent, Http, method } from "@golemcloud/effect-golem";

export const TaskAgent = defineAgent({
  name: "TaskAgent",
  mode: "durable",
  constructorParams: {
    taskName: Schema.String,
  },
  http: Http.mount("/api/tasks/{taskName}"),
  methods: {
    getItem: method({
      params: { itemId: Schema.String },
      success: Item,
      http: [Http.get("/items/{itemId}")],
    }),
  },
});
```

Every constructor parameter must appear in the mount path. This Effect SDK has no API for binding
a mount header to a constructor parameter. Mount paths also cannot contain query parameters or
catch-all variables.

An endpoint catch-all captures the remaining path and must be its final segment:

```typescript
serveFile: method({
  params: { path: Schema.String },
  success: FileContent,
  http: [Http.get("/files/{*path}")],
}),
// GET .../files/docs/readme.md supplies "docs/readme.md" as path.
```

## Query Parameters

Declare query bindings in the endpoint path with `key={parameterName}`. The HTTP key may differ
from the TypeScript parameter name:

```typescript
search: method({
  params: {
    query: Schema.String,
    minPrice: Schema.NullOr(Schema.Number),
    inStockOnly: Schema.NullOr(Schema.Boolean),
  },
  success: Schema.Array(Product),
  http: [
    Http.get(
      "/products/search?q={query}&min-price={minPrice}&in-stock-only={inStockOnly}",
    ),
  ],
}),
```

For query and header parameters, `Schema.NullOr(T)` represents an optional value: an omitted value
is supplied as `null`. Keep path variables non-optional because a matching path always contains
the segment.

## Header Variables

The endpoint options map each HTTP header name to one method parameter name:

```typescript
submitReport: method({
  params: {
    tenantId: Schema.String,
    report: Report,
  },
  success: Receipt,
  http: [
    Http.post("/reports", {
      headers: { "X-Tenant": "tenantId" } as const,
    }),
  ],
}),
```

Here `tenantId` comes from `X-Tenant`, while the unbound `report` parameter comes from the JSON
body field named `report`. Header names are case-insensitive for duplicate detection. Query keys
and all parameter names are case-sensitive.

A parameter may be bound only once within an endpoint. Do not bind the same parameter from two
path positions, two query keys, two headers, or a mixture of those sources.

## Schemas Allowed in Path, Query, and Headers

Route-bound values must use a string-bindable schema:

| Effect Schema                                                 | Parsed value                  |
| ------------------------------------------------------------- | ----------------------------- |
| `Schema.String`                                               | string                        |
| `Schema.Number`                                               | JavaScript number / WIT `f64` |
| `Schema.Boolean`                                              | boolean                       |
| `Schema.BigInt`                                               | bigint / WIT `s64`            |
| `Schema.Literal(...)` or a union of scalar literals           | validated literal value       |
| scalar template literal, refinement, brand, or transformation | decoded scalar value          |

For a string enum, use the Effect v4 union form:

```typescript
const SortOrder = Schema.Union([
  Schema.Literal("ascending"),
  Schema.Literal("descending"),
]);
```

Wrap a scalar with `Schema.NullOr(...)` only for an optional query or header. Structs, tuples,
arrays, records, unstructured values, and multimodal values are not route-bindable. In particular,
`Schema.Array(T)` cannot collect repeated query parameters or comma-separated headers in this SDK;
model the value as a JSON body field or define separate scalar parameters instead.

## JSON Body Mapping

Use `Http.post`, `Http.put`, `Http.del`, `Http.patch`, or another bodyful helper when parameters
must come from a body. Every parameter not bound to a path, query key, or header becomes a
top-level field in the JSON object, using the exact TypeScript parameter name:

```typescript
updateItem: method({
  params: {
    id: Schema.String,
    name: Schema.String,
    count: Schema.Number,
  },
  success: Item,
  http: [Http.put("/items/{id}")],
}),
```

The request body is:

```json
{ "name": "Widget", "count": 5 }
```

The body is always an object keyed by method parameter name. A single unbound parameter
`decision: Schema.String` expects `{ "decision": "approved" }`, not the bare JSON string
`"approved"`. Likewise, `params: { item: Item }` expects `{ "item": { ... } }`; the fields of the
`Item` struct are not flattened into the top-level body.

`Http.get` and `Http.head` are bodyless, so every parameter of those methods must be bound from
the path, query, or headers. Prefer the standard verb helpers over `Http.custom(...)` for standard
methods so the SDK can apply its bodyless validation.

## Structured Data and JSON

Use schemas that the SDK can lower to WIT:

| Effect Schema            | JSON representation                  |
| ------------------------ | ------------------------------------ |
| `Schema.String`          | string                               |
| `Schema.Number`          | number                               |
| `Schema.Boolean`         | boolean                              |
| `Schema.Array(T)`        | array                                |
| `Schema.Struct({ ... })` | object with the declared field names |
| `Schema.Tuple([...])`    | array                                |
| `Schema.NullOr(T)`       | value or `null`                      |
| union of string literals | string enum value                    |
| tagged union             | tagged variant object                |

Use `Schema.Struct` for fixed object fields. Open-ended `Schema.Record(...)` index signatures are
not supported by the pinned Effect SDK's WIT schema compiler. Keep transformations and refinements
serializable through their encoded schemas.

## HTTP Response Mapping

Declare the HTTP outcome through the method's `success` and optional `error` schemas:

| Method contract and handler result           | HTTP response   |
| -------------------------------------------- | --------------- |
| `success: Schema.Void`, return `Effect.void` | 204, empty body |
| `success: T`, return `Effect<T>`             | 200, JSON `T`   |
| `success: Schema.NullOr(T)`, return `T`      | 200, JSON `T`   |
| `success: Schema.NullOr(T)`, return `null`   | 404, empty body |
| declare `error: E`, return `Effect.fail(E)`  | 500, JSON `E`   |

Use `Schema.NullOr(T)` for ordinary not-found behavior. Use a declared error schema and
`Effect.fail(...)` for expected typed failures. Defects such as `Effect.die(...)` are unexpected
invocation failures and may be retried; do not use them to select an HTTP status.

The SDK does not expose a raw response builder or per-error status mapping. Do not return an
Effect Platform `HttpServerResponse` or invent response-header/status helpers.

## Unstructured Request Bodies

The Effect SDK exposes top-level unstructured method parameters under the `Unstructured`
namespace:

```typescript
import { Effect, Schema } from "effect";
import { Http, method, Unstructured } from "@golemcloud/effect-golem";

const upload = method({
  params: {
    payload: Unstructured.UnstructuredBinary({
      restrictions: [{ mimeType: "image/png" }],
    }),
  },
  success: Schema.Number,
  http: [Http.post("/upload")],
});

// In the implementation:
const uploadHandler = ({
  payload,
}: {
  payload: Unstructured.BinaryReferenceValue;
}) =>
  Effect.succeed(payload._tag === "inline" ? payload.val.data.byteLength : -1);
```

Use `Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] })` for a restricted
text input, or omit `restrictions` to accept any declared text language or binary MIME type. An
unstructured value cannot bind to path, query, or headers; keep it as the only body parameter.

At the pinned SDK version, `method.success` accepts Effect Schema rather than an unstructured
element specification. There is therefore no top-level raw binary or raw text HTTP response API;
do not mechanically translate another SDK's `UnstructuredBinary` or `UnstructuredText` return
type.

## Authenticated Principal

The authenticated caller is an Effect Context service, not a method parameter or HTTP binding:

```typescript
import { Effect } from "effect";
import { Principal } from "@golemcloud/effect-golem";

const currentCaller = Effect.gen(function* () {
  return yield* Principal.Principal;
});
```

Enable authentication with `Http.mount(path, { auth: true })` or an endpoint `auth` option, then
read `Principal.Principal` inside the handler for the current invocation. Load
`golem-add-http-auth-effect` for deployment security configuration and principal variants.

## Key Constraints

- Import Effect APIs from `effect` and Golem APIs from `@golemcloud/effect-golem`.
- Match every placeholder and header target to an exact `params` or `constructorParams` key.
- Use `Schema.NullOr(...)` and `null` for optional query/header values and 404 success results.
- Keep GET/HEAD parameters fully bound and bodyful request bodies as named JSON objects.
- Return Effects from handlers; do not use plain values, `async` handlers, or raw HTTP middleware.
- Do not use `@golemcloud/golem-ts-sdk` decorators or invent APIs from another language SDK.
