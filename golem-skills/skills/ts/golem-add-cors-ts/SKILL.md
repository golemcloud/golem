---
name: golem-add-cors-ts
description: "Configuring CORS for TypeScript HTTP endpoints. Use when the user asks to enable CORS, allow cross-origin requests, or configure allowed origins for HTTP endpoints."
---

# Configuring CORS for TypeScript HTTP Endpoints

## Mount-Level CORS

Set `cors` on `@agent()` to apply allowed origins to **all** endpoints:

```typescript
@agent({
  mount: '/api/{name}',
  cors: ['https://app.example.com']
})
class MyAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/data' })
  async getData(): Promise<Data> { ... }
  // Allows https://app.example.com
}
```

## Endpoint-Level CORS

Set `cors` on `@endpoint()` to add allowed origins for a specific endpoint. Origins are **unioned** with mount-level CORS:

```typescript
@agent({
  mount: '/api/{name}',
  cors: ['https://app.example.com']
})
class MyAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }

  @endpoint({ get: '/data', cors: ['*'] })
  async getData(): Promise<Data> { ... }
  // Allows BOTH https://app.example.com AND * (all origins)

  @endpoint({ get: '/other' })
  async getOther(): Promise<Data> { ... }
  // Inherits mount-level: only https://app.example.com
}
```

## Wildcard

Use `'*'` to allow all origins:

```typescript
@agent({
  mount: '/public/{name}',
  cors: ['*']
})
class PublicAgent extends BaseAgent {
  constructor(readonly name: string) { super(); }
}
```

## CORS Preflight

Golem automatically handles `OPTIONS` preflight requests for endpoints that have CORS configured. The preflight response includes `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, and `Access-Control-Allow-Headers` headers.
