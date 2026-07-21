---
name: golem-make-http-request-effect
description: "Making outgoing HTTP requests from an Effect-based Golem agent with Effect HttpClient and FetchHttpClient.layer. Use when calling external REST APIs, sending JSON, reading HTTP responses, or integrating an HTTP service from @golemcloud/effect-golem code."
---

# Making Outgoing HTTP Requests from Effect Golem Agents

Use the Effect v4 HTTP client from `effect/unstable/http`. `FetchHttpClient.layer` supplies the
canonical `HttpClient.HttpClient` service through QuickJS's WASI-backed `globalThis.fetch`, while
keeping request construction, failures, response decoding, and dependency provision in Effect.

```typescript
import { Effect, Schema } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
```

Do not wrap `globalThis.fetch` in `Effect.tryPromise` for ordinary application HTTP. Do not use
`node:http`, `node:https`, `@effect/platform-node`, or another Node networking transport.

```diagram
┌───────────────┐    HttpClient request    ┌───────────────────────┐    WASI HTTP    ┌────────────┐
│ Agent handler │─────────────────────────▶│ FetchHttpClient.layer │────────────────▶│ Remote API │
└───────────────┘                          └───────────────────────┘                 └────────────┘
```

The `Http` namespace from `@golemcloud/effect-golem` is separate: `Http.mount`, `Http.get`, and
`Http.post` declare **incoming** agent routes. They do not execute outbound requests.

## Provide the Fetch Client

Provide `FetchHttpClient.layer` at the narrowest common application boundary. A one-off request can
provide it directly:

```typescript
const readText = HttpClient.get("https://api.example.com/health").pipe(
  Effect.flatMap(HttpClientResponse.filterStatusOk),
  Effect.flatMap((response) => response.text),
  Effect.provide(FetchHttpClient.layer),
);
```

For agent handlers, provide the layer to the composed HTTP effect returned by the handler. If a
larger application has several HTTP-dependent services, compose them into an application layer and
provide `FetchHttpClient.layer` once at that layer boundary.

## GET and Decode JSON

Build immutable requests, execute them through the service, require a `2xx` status, and decode the
JSON body with Effect Schema:

```typescript
const ApiUser = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
});

const getUser = (id: number) =>
  HttpClientRequest.get(`https://api.example.com/users/${id}`).pipe(
    HttpClientRequest.setHeaders({
      Accept: "application/json",
      Authorization: "Bearer my-token",
    }),
    HttpClient.execute,
    Effect.flatMap(HttpClientResponse.filterStatusOk),
    Effect.flatMap(HttpClientResponse.schemaBodyJson(ApiUser)),
    Effect.provide(FetchHttpClient.layer),
  );
```

`HttpClientResponse.schemaBodyJson` both parses JSON and validates its shape. Transport, status,
body-decoding, and schema failures remain in the Effect error channel.

For several calls, filter the client once:

```typescript
const program = Effect.gen(function* () {
  const client = (yield* HttpClient.HttpClient).pipe(HttpClient.filterStatusOk);
  const response = yield* client.get("https://api.example.com/users/1", {
    headers: { Accept: "application/json" },
  });
  return yield* HttpClientResponse.schemaBodyJson(ApiUser)(response);
}).pipe(Effect.provide(FetchHttpClient.layer));
```

Choose either client-level `HttpClient.filterStatusOk` or response-level
`HttpClientResponse.filterStatusOk`; do not apply both to the same request.

## POST JSON

Use `HttpClientRequest.bodyJson` so serialization and the JSON content type are handled by the
Effect HTTP request model:

```typescript
const createUser = (name: string, email: string) =>
  HttpClientRequest.post("https://api.example.com/users").pipe(
    HttpClientRequest.setHeader("Accept", "application/json"),
    HttpClientRequest.bodyJson({ name, email }),
    Effect.flatMap(HttpClient.execute),
    Effect.flatMap(HttpClientResponse.filterStatusOk),
    Effect.flatMap((response) => response.text),
    Effect.provide(FetchHttpClient.layer),
  );
```

`bodyJson` is effectful because JSON encoding can fail. When a request body has an Effect Schema,
prefer `HttpClientRequest.schemaBodyJson(InputSchema)(input)`. Other request helpers include
`bodyText`, `bodyUint8Array`, `bodyUrlParams`, `bodyFormData`, and `bodyStream`.

Use `HttpClientRequest.put`, `patch`, or `delete` for the corresponding HTTP method.

## Read Responses

Status and headers are immediate response fields. Body readers are Effects:

```typescript
const inspectResponse = (url: string) =>
  Effect.gen(function* () {
    const response = yield* HttpClient.get(url);
    const body = yield* response.text;
    return {
      status: response.status,
      contentType: response.headers["content-type"],
      body,
    };
  }).pipe(Effect.provide(FetchHttpClient.layer));
```

Use:

- `response.text` for text;
- `response.json` for an unvalidated parsed JSON value;
- `HttpClientResponse.schemaBodyJson(Schema)` for validated JSON;
- `response.arrayBuffer` for binary data; or
- `response.stream` for streaming consumption.

Apply `HttpClientResponse.filterStatusOk` when non-`2xx` responses are failures. If the application
must inspect a non-`2xx` body before deciding, read the response and branch on `response.status`
instead.

## Agent Handler Error Types

The HTTP client keeps transport, non-`2xx`, body-decoding, timeout, and schema failures typed in the
Effect error channel. An Effect-Golem method with no `error` schema cannot return those failures.
Either:

1. declare an Effect Schema for caller-visible failures and map the HTTP errors to that type; or
2. use `Effect.orDie` for genuinely unexpected failures that should become defects.

```typescript
const infallibleHandlerEffect = getUser(1).pipe(Effect.orDie);
```

Do not use defects for expected business outcomes.

## Timeouts and Cancellation

Put the timeout around request execution **and body consumption**, because receiving response
headers does not mean the body is complete:

```typescript
const timed = HttpClient.get("https://api.example.com/slow").pipe(
  Effect.flatMap(HttpClientResponse.filterStatusOk),
  Effect.flatMap((response) => response.text),
  Effect.timeout("5 seconds"),
  Effect.provide(FetchHttpClient.layer),
);
```

`FetchHttpClient` passes Effect interruption to a fetch `AbortSignal`. Prompt cancellation of the
underlying WASI HTTP operation is currently blocked by the QuickJS runtime issue tracked in
[`GOL-325`](https://linear.app/golem-cloud/issue/GOL-325/propagate-fetch-abortsignal-cancellation-to-the-underlying-wasi-http).
Do not claim that a timeout has already stopped the remote request until that runtime fix is
consumed and verified.

## Durability, Retries, and Duplicate Requests

Outgoing WASI HTTP calls are recorded by Golem. During replay of the **same agent invocation**,
Golem normally reuses the recorded response rather than sending the request again unless a
durability boundary explicitly requires re-execution. Using `FetchHttpClient.layer` preserves that
host behavior because it still executes through the same WASI-backed fetch implementation.

This is not cross-invocation deduplication. Two calls to the same agent method are independent and
can send two requests. `Durability.atomically` controls recovery within one invocation; it is not a
distributed transaction with the remote HTTP server.

For state-changing requests that callers may retry:

- accept a stable caller-supplied request ID;
- send it in the remote API's idempotency header; and
- require the remote API to deduplicate that key.

Do not assume `Durability.generateIdempotencyKey` deduplicates separate method invocations. Do not
manually invoke a state-changing method when an automated verification will invoke it later.

### Host status retry policy

Use a named host retry policy when retry depends only on HTTP status. The host applies it below
`FetchHttpClient` and returns the final response to application code:

```yaml
# golem.yaml — under retryPolicyDefaults / <environment>:
http-5xx-retry:
  priority: 20
  predicate:
    propIn: { property: "status-code", values: [500, 502, 503, 504] }
  policy:
    countBox:
      maxRetries: 3
      inner:
        exponential:
          baseDelay: "200ms"
          factor: 2.0
```

A retry can repeat a state-changing operation. Use it only when the operation is naturally
idempotent or the remote server deduplicates a stable key.

### Atomic application-level validation

When retry depends on decoded content or several side effects, include request execution, complete
body consumption, and validation in the atomic region:

```typescript
import { Durability } from "@golemcloud/effect-golem";

const validatedCall = Durability.atomically(
  HttpClient.get("https://api.example.com/users/1").pipe(
    Effect.flatMap(HttpClientResponse.filterStatusOk),
    Effect.flatMap(HttpClientResponse.schemaBodyJson(ApiUser)),
    Effect.provide(FetchHttpClient.layer),
  ),
);
```

A failed atomic region can re-execute the complete region and repeat a remote side effect. Do not
use it as an exactly-once wrapper or merely for status-code retries. Load `golem-atomic-block-effect`
for the full replay constraints.

## Calling a Golem HTTP Endpoint

For an incoming Golem endpoint, unbound method parameters come from a JSON object whose keys match
the method's camelCase `params` keys. Even one body parameter requires an object.

Given an incoming method:

```typescript
record: method({
  params: { message: Schema.String },
  success: Schema.Void,
  http: [Http.post("/record")],
});
```

send `{ "message": "..." }`, not a raw string.

## Complete Agent Method

This durable agent performs the POST through one shared `HttpClient` service:

```typescript
import { Effect, Schema } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const MessageForwarder = defineAgent({
  name: "MessageForwarder",
  mode: "durable",
  constructorParams: { name: Schema.String },
  methods: {
    recordMessageViaHttp: method({
      params: { message: Schema.String },
      success: Schema.String,
    }),
  },
}).implement(() =>
  Effect.succeed({
    recordMessageViaHttp: ({ message }) =>
      HttpClientRequest.post(
        "http://test-app.localhost:9006/requests/main/record",
      ).pipe(
        HttpClientRequest.bodyJson({ message }),
        Effect.flatMap(HttpClient.execute),
        Effect.flatMap(HttpClientResponse.filterStatusOk),
        Effect.flatMap((response) => response.text),
        Effect.as(message),
        Effect.provide(FetchHttpClient.layer),
        Effect.orDie,
      ),
  }),
);
```

Import the implementation module from `src/main.ts` using its emitted `.js` suffix so the
top-level registration runs.

For provider-backed LLM tasks, load `golem-add-llm-effect` and use `LanguageModel` plus the selected
Effect AI provider instead of hand-writing provider REST calls.

## Key Constraints

- Import root Effect APIs from `effect` and HTTP APIs from `effect/unstable/http`.
- Use the canonical `HttpClient.HttpClient` service with `FetchHttpClient.layer`.
- Do not use direct `globalThis.fetch`, Node networking modules, or native HTTP client packages.
- Keep handlers as Effects; do not return raw Promises or mark handlers `async`.
- Use Effect request-body helpers instead of manually setting a fetch body.
- Filter or inspect HTTP statuses explicitly and decode unknown JSON with Effect Schema.
- Bound body consumption as well as request execution when applying a timeout.
- Let Golem handle durable retries; do not add manual retry loops around host operations.
- Durable replay does not deduplicate separate agent method invocations.
- Use named camelCase JSON fields when posting to another Golem endpoint.

## Authoritative API Sources

- [Effect v4 HTTP client](https://github.com/Effect-TS/effect/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/http/HttpClient.ts)
- [Effect v4 HTTP request](https://github.com/Effect-TS/effect/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/http/HttpClientRequest.ts)
- [Effect v4 HTTP response](https://github.com/Effect-TS/effect/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/http/HttpClientResponse.ts)
- [Effect v4 fetch client layer](https://github.com/Effect-TS/effect/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/http/FetchHttpClient.ts)
- [Effect-Golem durability modes](https://github.com/golemcloud/effect-golem/blob/HEAD/src/internal/durabilityMode.ts)
- [GOL-325 cancellation follow-up](https://linear.app/golem-cloud/issue/GOL-325/propagate-fetch-abortsignal-cancellation-to-the-underlying-wasi-http)
