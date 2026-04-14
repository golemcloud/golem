---
name: golem-http-params-ts
description: "Mapping HTTP request elements to TypeScript agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (TypeScript)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which types are supported for each, and how return types map to HTTP responses.

## Path Variables

Path variables `{varName}` in mount or endpoint paths map to parameters by name:

```typescript
// Mount path variables → constructor parameters
@agent({ mount: '/api/tasks/{name}' })
class TaskAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  // Endpoint path variables → method parameters
  @endpoint({ get: '/items/{itemId}' })
  async getItem(itemId: string): Promise<Item> { ... }
}
```

Remaining (catch-all) path variables capture everything after a prefix:

```typescript
@endpoint({ get: '/files/{*path}' })
async getFile(path: string): Promise<FileContent> { ... }
// GET .../files/docs/readme.md → path = "docs/readme.md"
```

Catch-all variables can only appear as the **last** path segment and are **not** allowed in mount paths.

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```typescript
@endpoint({ get: '/search?q={query}&limit={maxResults}' })
async search(query: string, maxResults: number): Promise<SearchResult[]> { ... }
// GET .../search?q=hello&limit=10
```

## Header Variables

Map HTTP headers to parameters using the `headers` option on `@endpoint()`:

```typescript
@endpoint({
  get: '/data',
  headers: { 'X-Request-Id': 'requestId', 'Authorization': 'token' }
})
async getData(requestId: string, token: string): Promise<Data> { ... }
```

Headers can also be mapped to **constructor** parameters at the mount level:

```typescript
@agent({
  mount: '/api',
  headers: { 'X-Api-Key': 'apiKey' }
})
class ApiAgent extends BaseAgent {
  constructor(readonly apiKey: string) { super(); }
}
```

When using mount-level headers, all constructor parameters must be satisfied by either path variables or header variables.

## Supported Types for Path, Query, and Header Variables

Only these types can be used for parameters bound to path/query/header variables (the value is parsed from the URL/header string):

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

**All other types** (objects, interfaces, nested arrays, `Map`, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters **not** bound to path variables, query parameters, or headers are populated from the JSON request body:

```typescript
@endpoint({ post: '/items/{id}' })
async updateItem(id: string, name: string, count: number): Promise<Item> { ... }
// POST .../items/123
// Body: { "name": "Widget", "count": 5 }
// → id from path, name and count from body
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. Field names use the original camelCase parameter names.

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

## Principal Parameter

When authentication is enabled, methods can receive a `Principal` parameter with info about the authenticated user. `Principal` parameters are automatically populated and must **not** be mapped to path/query/header variables:

```typescript
import { Principal } from '@golemcloud/golem-ts-sdk';

@endpoint({ get: '/whoami', auth: true })
async whoAmI(principal: Principal): Promise<{ value: Principal }> {
  return { value: principal };
}

// Principal can appear at any position among parameters
@endpoint({ get: '/data/{id}' })
async getData(id: string, principal: Principal): Promise<Data> { ... }
```
