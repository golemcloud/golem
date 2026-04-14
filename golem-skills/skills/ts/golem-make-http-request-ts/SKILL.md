---
name: golem-make-http-request-ts
description: "Making outgoing HTTP requests from a TypeScript Golem agent. Use when the user asks to call an external API, make HTTP requests, use fetch, or send HTTP requests from agent code."
---

# Making Outgoing HTTP Requests (TypeScript)

## Overview

Golem TypeScript agents run in a WebAssembly environment with a built-in `fetch` API. Use the standard `fetch()` function for all outgoing HTTP requests — it is fully supported and works with WASI HTTP under the hood.

> **Note:** The `node:http` and `node:https` modules are also available with comprehensive client-side support (the client API passes the majority of Node.js compatibility tests). They can be used as an alternative, especially when porting existing Node.js code. Server-side APIs (`http.createServer`, `net.listen`) are **not** available in WASM.

## GET Request

```typescript
const response = await fetch('https://api.example.com/data');
const data = await response.json();
console.log(data);
```

## GET with Headers

```typescript
const response = await fetch('https://api.example.com/secure', {
  headers: {
    'Authorization': 'Bearer my-token',
    'Accept': 'application/json',
  },
});

if (!response.ok) {
  throw new Error(`HTTP ${response.status}: ${response.statusText}`);
}

const result = await response.json();
```

## POST with JSON Body

```typescript
const payload = { name: 'Alice', email: 'alice@example.com' };

const response = await fetch('https://api.example.com/users', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Accept': 'application/json',
  },
  body: JSON.stringify(payload),
});

const created = await response.json();
```

## PUT / DELETE

```typescript
// PUT
const response = await fetch('https://api.example.com/users/123', {
  method: 'PUT',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ name: 'Updated Name' }),
});

// DELETE
await fetch('https://api.example.com/users/123', {
  method: 'DELETE',
});
```

## Reading Response

```typescript
const response = await fetch(url);

// Status
response.status;      // e.g. 200
response.ok;          // true if 2xx
response.statusText;  // e.g. "OK"

// Headers
response.headers.get('Content-Type');

// Body (choose one)
const text = await response.text();       // as string
const json = await response.json();       // parsed JSON
const buffer = await response.arrayBuffer(); // raw bytes
```

## Error Handling

```typescript
try {
  const response = await fetch('https://api.example.com/data');

  if (!response.ok) {
    const errorBody = await response.text();
    throw new Error(`API error ${response.status}: ${errorBody}`);
  }

  return await response.json();
} catch (error) {
  console.error('Request failed:', error);
  throw error;
}
```

## Complete Example in an Agent

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';

type WeatherReport = { temperature: number; description: string };

@agent({ mount: '/weather/{city}' })
class WeatherAgent extends BaseAgent {
  constructor(readonly city: string) {
    super();
  }

  @endpoint({ get: '/current' })
  async getCurrent(): Promise<WeatherReport> {
    const response = await fetch(
      `https://api.weather.example.com/current?city=${encodeURIComponent(this.city)}`,
      {
        headers: { 'Accept': 'application/json' },
      }
    );

    if (!response.ok) {
      throw new Error(`Weather API error: ${response.status}`);
    }

    return await response.json();
  }
}
```

## Calling Golem Agent HTTP Endpoints

When making HTTP requests to other Golem agent endpoints (or your own), the request body must match the **Golem HTTP body mapping convention**: non-binary body parameters are always deserialized from a **JSON object** where each top-level field corresponds to a method parameter name. This is true even when the endpoint has a single body parameter.

For example, given this endpoint definition:

```typescript
@endpoint({ post: '/record' })
async record(body: string): Promise<void> { ... }
```

The correct HTTP request must send a JSON object with a `body` field — **not** a raw text string:

```typescript
// ✅ CORRECT — JSON object with field name matching the parameter
await fetch('http://my-app.localhost:9006/recorder/main/record', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ body: 'a' }),
});

// ❌ WRONG — raw text body does NOT match Golem's JSON body mapping
await fetch('http://my-app.localhost:9006/recorder/main/record', {
  method: 'POST',
  headers: { 'Content-Type': 'text/plain' },
  body: 'a',
});
```

> **Rule of thumb:** If the target endpoint is a Golem agent, always send `application/json` with parameter names as JSON keys. Load the `golem-http-params-ts` skill for the full body mapping rules.

## Key Constraints

- Use `fetch()` as the primary HTTP client — it is the standard and recommended API
- `node:http` and `node:https` are available with comprehensive client-side support — useful when porting Node.js code or when npm packages depend on them
- Server-side APIs (`http.createServer`, `net.listen`) are **not** available in WASM
- Third-party HTTP client libraries that use `fetch` or `node:http` internally (e.g., `axios`) generally work; libraries that depend on native C/C++ bindings will not
- All HTTP requests go through the WASI HTTP layer, which provides durable execution guarantees
- Requests are async — always use `await`
