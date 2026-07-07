---
name: golem-http-params-ts
description: "Mapping HTTP request elements to TypeScript agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (TypeScript)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to the agent `id` record and to method inputs. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which schema types are supported for each, and how return schemas map to HTTP responses.

## Path Variables

Path variables `{varName}` map by name — mount-path variables bind to the agent's `id` fields, endpoint-path variables bind to the method's `input` keys:

```typescript
import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const TaskAgent = defineAgent({
  name: 'TaskAgent',
  id: { name: z.string() },                 // {name} in the mount path
  http: http.mount('/api/tasks/{name}'),
  methods: {
    // {itemId} in the endpoint path binds to the `itemId` input
    getItem: method({ input: { itemId: z.string() }, returns: Item, http: http.get('/items/{itemId}') }),
  },
});
```

Remaining (catch-all) path variables capture everything after a prefix:

```typescript
getFile: method({ input: { path: z.string() }, returns: FileContent, http: http.get('/files/{*path}') }),
// GET .../files/docs/readme.md → path = "docs/readme.md"
```

Catch-all variables can only appear as the **last** endpoint path segment and are **not** allowed in mount paths.

## Query Parameters

Bind query parameters either inline in the endpoint path with `?key={var}` syntax, or explicitly via the `query` option map (its values are input keys):

```typescript
// Inline form
search: method({
  input: { query: z.string(), maxResults: z.number() },
  returns: z.array(SearchResult),
  http: http.get('/search?q={query}&limit={maxResults}'),
}),

// Explicit map form (query-param name → input key)
search2: method({
  input: { query: z.string(), maxResults: z.number() },
  returns: z.array(SearchResult),
  http: http.get('/search', { query: { q: 'query', limit: 'maxResults' } }),
}),
```

## Header Variables

Bind HTTP headers to inputs with the `headers` option map (header name → input key):

```typescript
getData: method({
  input: { requestId: z.string(), token: z.string() },
  returns: Data,
  http: http.get('/data', { headers: { 'X-Request-Id': 'requestId', 'Authorization': 'token' } }),
}),
```

> **Note:** Unlike the old decorator API, the fluent `http.mount(...)` does **not** support a mount-level `headers` map. Every agent `id` field must be supplied by a mount **path** variable. Header binding is available on endpoints only, and binds to method inputs.

## Supported Schema Types for Path, Query, and Header Variables

Only scalar schemas can be used for inputs bound to path/query/header variables (the value is parsed from the URL/header string):

| Schema | Parsed From |
|---|---|
| `z.string()` | Used as-is |
| `z.number()` | Parsed as float (`f64`) |
| `z.boolean()` | Parsed from `"true"` / `"false"` |
| `z.enum(['red', 'green'])` | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper forms are supported:

| Schema | Behavior |
|---|---|
| `z.string().optional()` (any supported scalar) | Optional — absent query param or header produces `undefined` |
| `z.array(z.string())` (any supported scalar) | Repeated query params or comma-separated header values |

**All other schemas** (`z.object(...)`, nested arrays, maps, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE`/`PATCH` endpoints, any method input **not** bound to a path variable, query parameter, or header is populated from the JSON request body:

```typescript
updateItem: method({
  input: { id: z.string(), name: z.string(), count: z.number() },
  returns: Item,
  http: http.post('/items/{id}'),
}),
// POST .../items/123
// Body: { "name": "Widget", "count": 5 }
// → id from path, name and count from body
```

Each unbound input becomes a top-level field in the expected JSON body object. Field names use the input key names.

> **⚠️ Important:** The request body is **always** a JSON object with input keys as fields — even when there is only a single body parameter. For example, a method with `input: { decision: z.string() }` expects `{"decision": "approved"}`, **never** a bare string like `"approved"`. Sending a non-object JSON value or plain text will fail with `REQUEST_JSON_BODY_PARSING_FAILED`.

## Binary Request and Response Bodies

Use the `s.unstructuredBinary()` schema marker for raw binary payloads. Its decoded value is a reference: `{ tag: 'url', val: string }` or `{ tag: 'inline', val: Uint8Array, mimeType?: string }`. A method using it may have **only one** body parameter, and that input cannot also be bound to a path/query/header variable.

```typescript
import { z } from 'zod';
import { defineAgent, method, http, s } from '@golemcloud/golem-ts-sdk';

export const UploadAgent = defineAgent({
  name: 'UploadAgent',
  id: { name: z.string() },
  http: http.mount('/uploads/{name}'),
  methods: {
    // Accepting any binary content type
    upload: method({
      input: { bucket: z.string(), payload: s.unstructuredBinary() },
      returns: z.number(),
      http: http.post('/upload/{bucket}'),
    }),
    // Restricting to specific MIME types
    uploadImage: method({
      input: { bucket: z.string(), payload: s.unstructuredBinary({ mimeTypes: ['image/gif'] }) },
      returns: z.number(),
      http: http.post('/upload-image/{bucket}'),
    }),
    // Returning binary data
    download: method({ input: {}, returns: s.unstructuredBinary(), http: http.get('/download') }),
  },
});

export const UploadAgentImpl = UploadAgent.implement({
  init: () => ({}),
  methods: {
    upload({ payload }) {
      if (payload.tag === 'url') return -1;
      return payload.val.byteLength;
    },
    uploadImage({ payload }) {
      return payload.tag === 'inline' ? payload.val.byteLength : -1;
    },
    download() {
      return { tag: 'inline', val: new Uint8Array([1, 2, 3]), mimeType: 'application/octet-stream' };
    },
  },
});
```

## Plain Text Request and Response Bodies

Use the `s.unstructuredText()` marker for raw `text/plain` payloads. Its decoded value is `{ tag: 'url', val: string }` or `{ tag: 'inline', val: string, languageCode?: string }`. Like binary, a method using it may have **only one body parameter**, not bound to any path/query/header variable. The body is decoded as UTF-8.

```typescript
methods: {
  // Accepting any text/plain content
  addNote: method({
    input: { id: z.string(), body: s.unstructuredText() },
    returns: z.number(),
    http: http.post('/notes/{id}'),
  }),
  // Restricting to specific language codes
  translate: method({
    input: { id: z.string(), body: s.unstructuredText({ languages: ['en', 'de'] }) },
    returns: z.string(),
    http: http.post('/translate/{id}'),
  }),
  // Returning text/plain
  getNote: method({ input: { id: z.string() }, returns: s.unstructuredText(), http: http.get('/notes/{id}') }),
}
```

```typescript
// Handler returning inline text with a language code:
getNote({ id }) {
  return { tag: 'inline', val: 'hello', languageCode: 'en' };
}
```

HTTP-level rules:
- The request must have either no `Content-Type`, `text/plain`, or
  `text/plain; charset=utf-8` (case-insensitive). Any other content type is
  rejected with `415 Unsupported Media Type`.
- `Content-Language` is **always optional**, even when language codes are
  restricted. If present, it must be a single value (multi-valued or
  comma-separated headers are rejected with `400 Bad Request`).
- When restricted, the supplied `Content-Language` is matched
  case-insensitively against the allowed list; otherwise `415 Unsupported Media Type`.
- A non-UTF-8 request body is rejected with `400 Bad Request`.
- `Content-Language` cannot also be bound as an endpoint header parameter when
  the body is unstructured text — that header is reserved for declaring the
  body language.

The response is sent as `Content-Type: text/plain; charset=utf-8`. If the
returned inline text has a `languageCode`, it is forwarded as the
`Content-Language` response header.

## Return Schema to HTTP Response Mapping

| `returns` schema (and returned value) | HTTP Status | Response Body |
|---|---|---|
| `z.void()` / no value | 204 No Content | empty |
| any schema `T` | 200 OK | JSON-serialized `T` |
| `T.nullable()` / `T.optional()` | 200 OK if value, 404 Not Found if `null` / `undefined` | JSON `T` or empty |
| `s.result(ok, err)` returning `Result.ok` / `Result.err` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `ok` or JSON `err` |
| `s.unstructuredBinary()` | 200 OK | Raw binary with Content-Type |
| `s.unstructuredText()` | 200 OK | `text/plain; charset=utf-8` (+ optional `Content-Language`) |

## Data Type to JSON Mapping

| Schema | JSON Representation |
|---|---|
| `z.string()` | JSON string |
| `z.number()` | JSON number |
| `z.boolean()` | JSON boolean |
| `z.array(T)` | JSON array |
| `z.object({...})` | JSON object |
| `T.nullable()` / `T.optional()` | value or `null` |
| `z.enum(['a', 'b'])` | JSON string |

## Accessing the Principal

When authentication is enabled, access the authenticated user via `this.getPrincipal()` inside the handler (it is not a method input). See the `golem-add-http-auth-ts` skill:

```typescript
whoAmI() {
  const principal = this.getPrincipal();
  return { value: principal };
}
```

## Related Skills

- Load `golem-add-http-endpoint-ts` for the high-level workflow of defining and mounting HTTP endpoints on an agent
