---
name: golem-add-cors-effect
description: "Configuring CORS allowed-origin patterns for Effect-based Golem HTTP endpoints. Use when enabling cross-origin requests or adding mount-level or endpoint-level CORS in an @golemcloud/effect-golem agent."
---

# Configuring CORS for Effect HTTP Endpoints

Effect Golem agents publish CORS as HTTP route metadata through the `Http` namespace from
`@golemcloud/effect-golem`. The Golem host applies the policy and handles browser preflight; do not
add application-side middleware or manually construct CORS responses.

## Mount-Level CORS

Set the mount's `cors` option to allow origins for every endpoint on the agent:

```typescript
import { Http } from "@golemcloud/effect-golem";

http: Http.mount("/api/{name}", {
  cors: ["https://app.example.com"],
}),
```

Pass multiple allowed-origin patterns in the same array:

```typescript
http: Http.mount("/api/{name}", {
  cors: ["https://app.example.com", "https://admin.example.com"],
}),
```

When editing an existing mount options object, preserve its other options such as `auth`,
`phantomAgent`, or `webhookSuffix`.

## Endpoint-Level CORS

Pass `cors` in the endpoint helper's options to add allowed origins for only that route. Golem
combines these endpoint patterns with the mount-level patterns:

```typescript
export const MyAgent = defineAgent({
  name: "MyAgent",
  constructorParams: {
    name: Schema.String,
  },
  http: Http.mount("/api/{name}", {
    cors: ["https://app.example.com"],
  }),
  methods: {
    getData: method({
      params: {},
      success: Data,
      http: [Http.get("/data", { cors: ["*"] })],
    }),
    getOther: method({
      params: {},
      success: Data,
      http: [Http.get("/other")],
    }),
  },
});
```

Here, `/data` allows all origins in addition to the mount-level origin, while `/other` has only
the mount-level origin. Keep endpoint declarations inside the method's `http` array, and preserve
any existing endpoint options such as `headers` or `auth` when adding `cors`.

## Wildcard Origins

Use `"*"` as an allowed-origin pattern to allow every origin:

```typescript
http: Http.mount("/public/{name}", { cors: ["*"] }),
```

Use wildcard CORS only when the endpoint is intentionally public to every browser origin.

## CORS Preflight

Golem's HTTP host handles `OPTIONS` preflight requests for routes with CORS metadata and adds the
appropriate `Access-Control-Allow-*` response headers. Do not add an `Http.options(...)` endpoint
solely for CORS, inspect `Origin` manually, or try to return raw HTTP headers from an Effect
handler.

## Key Constraints

- Import `Http` from `@golemcloud/effect-golem`; do not use decorators from
  `@golemcloud/golem-ts-sdk` or Effect Platform HTTP middleware.
- Configure CORS with `Http.mount(path, { cors: [...] })` and endpoint helpers such as
  `Http.get(path, { cors: [...] })`.
- The Effect SDK exposes allowed-origin patterns only. It does not expose CORS options for
  credentials, allowed methods, allowed headers, exposed headers, or preflight max age.
- CORS metadata does not require changes to Effect handlers, method schemas, snapshots, or state.
- Run `golem build` after changing route metadata, then redeploy the application.
