---
name: golem-make-http-request-effect
description: "Making outgoing HTTP requests from an Effect-based Golem agent through the QuickJS runtime's WASI-backed fetch API. Use when calling external REST APIs, sending JSON, reading HTTP responses, or integrating an HTTP service from @golemcloud/effect-golem code."
---

# Making Outgoing HTTP Requests from Effect Golem Agents

Effect Golem components run in a QuickJS-based WebAssembly runtime whose `globalThis.fetch` is
implemented over WASI HTTP. Wrap that promise API with root Effect operators so agent handlers stay
Effect-based:

```typescript
import { Effect, Schema } from "effect";
```

Use `Effect.tryPromise` around `globalThis.fetch`. Do not make the handler itself `async`, and do
not use `node:http`, `node:https`, `@effect/platform-node`, or another Node networking transport.

## Why Not `effect/unstable/http`?

Effect 4.0.0-beta.98 contains `FetchHttpClient`, but the generated Golem Rollup configuration
externalizes only the exact module ID `effect`. Importing `effect/unstable/http` makes Rollup bundle
a second copy of Effect into the user module, while `@golemcloud/effect-golem` and imports from
`effect` use the embedded runtime. Do not mix those runtimes.

The supported application boundary with the current template is:

```diagram
┌───────────────┐    Effect.tryPromise    ┌──────────────────┐    WASI HTTP    ┌────────────┐
│ Agent handler │────────────────────────▶│ globalThis.fetch │────────────────▶│ Remote API │
└───────────────┘                         └──────────────────┘                 └────────────┘
```

The `Http` namespace from `@golemcloud/effect-golem` is unrelated: `Http.mount`, `Http.get`, and
`Http.post` declare **incoming** agent routes. They do not execute outbound requests.

## GET and Decode JSON

Keep the fetch, status check, and response-body read in the promise wrapped by `tryPromise`. Then
decode the unknown JSON value with Effect Schema:

```typescript
import { Effect, Schema } from "effect";

const ApiUser = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
});

const getUser = (id: number) =>
  Effect.gen(function* () {
    const json = yield* Effect.tryPromise({
      try: async (signal) => {
        const response = await globalThis.fetch(
          `https://api.example.com/users/${id}`,
          {
            method: "GET",
            headers: {
              Accept: "application/json",
              Authorization: "Bearer my-token",
            },
            signal,
          },
        );

        if (!response.ok) {
          const errorBody = await response.text();
          throw new Error(`HTTP ${response.status}: ${errorBody}`);
        }

        return response.json();
      },
      catch: (cause) =>
        new Error(`request failed: ${String(cause)}`, { cause }),
    });

    return yield* Schema.decodeUnknownEffect(ApiUser)(json);
  });
```

`response.json()` parses JSON but does not validate its shape. `Schema.decodeUnknownEffect` keeps
that validation in the Effect error channel.

## POST JSON

JSON-stringify the request body and set its content type explicitly. The WASI fetch implementation
does not automatically serialize arbitrary JavaScript objects:

```typescript
const createUser = (name: string, email: string) =>
  Effect.tryPromise({
    try: async (signal) => {
      const response = await globalThis.fetch(
        "https://api.example.com/users",
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Accept: "application/json",
          },
          body: JSON.stringify({ name, email }),
          signal,
        },
      );

      const body = await response.text();
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}: ${body}`);
      }

      return body;
    },
    catch: (cause) =>
      new Error(`create user request failed: ${String(cause)}`, { cause }),
  });
```

Use the same request shape with `method: "PUT"`, `"PATCH"`, or `"DELETE"` as required. Supported
body forms include strings, `ArrayBuffer`, `Uint8Array`, `URLSearchParams`, `Blob`, `FormData`, and
`ReadableStream`. For JSON, always use `JSON.stringify`.

## Reading a Response

The runtime exposes the standard Web response subset:

```typescript
const inspectResponse = (url: string) =>
  Effect.tryPromise({
    try: async (signal) => {
      const response = await globalThis.fetch(url, { signal });

      return {
        status: response.status,
        ok: response.ok,
        contentType: response.headers.get("content-type"),
        body: await response.text(),
      };
    },
    catch: (cause) =>
      new Error(`failed to read response: ${String(cause)}`, { cause }),
  });
```

Choose one body reader because the response body is one-shot:

- `await response.text()` for text;
- `await response.json()` for an unvalidated parsed JSON value; or
- `await response.arrayBuffer()` / `await response.bytes()` for binary data.

`fetch` rejects for request/transport failures, but it resolves normally for HTTP `4xx` and `5xx`
responses. Always inspect `response.ok` or `response.status` when non-2xx responses are failures.

## Agent Handler Error Types

`Effect.tryPromise({ try, catch })` places the value returned by `catch` in the Effect error
channel. An Effect Golem method with no `error` schema cannot return that typed failure. Either:

1. declare an Effect Schema for caller-visible failures and map transport/status/schema errors to
   that type; or
2. use `Effect.orDie` for genuinely unexpected failures that should become defects.

For example, when an upstream failure is unexpected:

```typescript
const infallibleHandlerEffect = getUser(1).pipe(Effect.orDie);
```

Do not use defects for expected business outcomes. Give those outcomes a method `error` schema or
represent them in the success type.

## Durability, Retries, and Duplicate Requests

Outgoing WASI HTTP calls are recorded by Golem. During replay of the **same agent invocation**,
Golem normally reuses the recorded response instead of sending the request again unless a
durability boundary explicitly requires re-execution.

This is not cross-invocation deduplication. Two calls to the same agent method are two independent
invocations and can send two requests, even when their method names and arguments are identical.
`Durability.atomically` does not change that: it controls recovery within one invocation and is not
a distributed transaction with the remote HTTP server.

For a state-changing request that a caller may retry:

- accept a stable caller-supplied request ID and send it in the idempotency header understood by the
  remote API;
- require the remote API to deduplicate that key; and
- do not assume `Durability.generateIdempotencyKey` deduplicates separate method invocations. Its
  generated value is stable when replaying one durable execution, but a new invocation gets a new
  value.

When verification only asks for a build or deployment, do not manually invoke a method that sends a
state-changing HTTP request. A later automated invocation is a separate call and would repeat the
remote side effect.

### Preferred for HTTP statuses: host retry policy

Define a named policy whose predicate references `status-code`. The host retries before returning
the response to `globalThis.fetch`, so application code receives the final attempt. No
Effect-specific HTTP wrapper is required:

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

POST/PUT/PATCH requests are eligible by default because Golem's idempotence mode defaults to
`true`. A host retry can issue another HTTP attempt, so use status retries for state-changing
operations only when the operation is naturally idempotent or the remote API deduplicates a stable
idempotency key.

### Application-level validation: atomic region

When retry depends on parsed content or combines several side effects, put the request, complete
body read, parsing, and validation inside `Durability.atomically`:

```typescript
import { Effect, Schema } from "effect";
import { Durability } from "@golemcloud/effect-golem";

const validatedCall = Durability.atomically(
  Effect.gen(function* () {
    const json = yield* Effect.tryPromise({
      try: async (signal) => {
        const response = await globalThis.fetch(
          "https://api.example.com/result",
          { signal },
        );
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}: ${await response.text()}`);
        }
        return response.json();
      },
      catch: (cause) => new Error(`request failed: ${String(cause)}`, { cause }),
    });

    return yield* Schema.decodeUnknownEffect(ApiUser)(json);
  }),
);
```

A failed atomic region traps so durable recovery can re-execute the complete region. If a remote
server accepted a request before the failure, re-execution can repeat that remote side effect.
Therefore, do not use an atomic region as an exactly-once wrapper and do not put a request inside one
merely for status-code retries: inline host status retries are skipped inside atomic regions. Load
`golem-atomic-block-effect` for the full replay and duplicate-side-effect constraints.

## Calling a Golem HTTP Endpoint

For an incoming Golem endpoint, unbound method parameters are read from a JSON object whose keys
match the Effect method's camelCase `params` keys. Even one body parameter requires an object.

Given:

```typescript
record: method({
  params: { message: Schema.String },
  success: Schema.Void,
  http: [Http.post("/record")],
});
```

send:

```typescript
const recordMessage = (message: string) =>
  Effect.tryPromise({
    try: async (signal) => {
      const response = await globalThis.fetch(
        "http://my-app.localhost:9006/requests/main/record",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ message }),
          signal,
        },
      );

      // Drain the final response body before completing the effect.
      const errorBody = await response.text();
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}: ${errorBody}`);
      }
    },
    catch: (cause) =>
      new Error(`record request failed: ${String(cause)}`, { cause }),
  });
```

Do not send a raw string such as `body: message`; it does not match Golem's named JSON body
mapping.

## Complete Agent Method

This agent method performs a real HTTP POST and remains Effect-based:

```typescript
import { Effect, Schema } from "effect";
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
      Effect.tryPromise({
        try: async (signal) => {
          const response = await globalThis.fetch(
            "http://test-app.localhost:9006/requests/main/record",
            {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ message }),
              signal,
            },
          );

          const errorBody = await response.text();
          if (!response.ok) {
            throw new Error(`HTTP ${response.status}: ${errorBody}`);
          }

          return message;
        },
        catch: (cause) =>
          new Error(`record request failed: ${String(cause)}`, { cause }),
      }).pipe(Effect.orDie),
  }),
);
```

Import the implementation module from `src/main.ts` using its emitted `.js` suffix so the
top-level `defineAgent(...).implement(...)` call registers.

## OpenAI-Compatible REST APIs

An OpenAI-compatible chat endpoint is an ordinary JSON POST. No Node provider client is required:

```typescript
const ChatCompletion = Schema.Struct({
  choices: Schema.Array(
    Schema.Struct({
      message: Schema.Struct({
        content: Schema.NullOr(Schema.String),
      }),
    }),
  ),
});

const askLlm = (baseUrl: string, apiKey: string, question: string) =>
  Effect.gen(function* () {
    const json = yield* Effect.tryPromise({
      try: async (signal) => {
        const response = await globalThis.fetch(
          `${baseUrl.replace(/\/$/, "")}/v1/chat/completions`,
          {
            method: "POST",
            headers: {
              Authorization: `Bearer ${apiKey}`,
              "Content-Type": "application/json",
              Accept: "application/json",
            },
            body: JSON.stringify({
              model: "mock-model",
              messages: [{ role: "user", content: question }],
            }),
            signal,
          },
        );

        if (!response.ok) {
          throw new Error(`HTTP ${response.status}: ${await response.text()}`);
        }
        return response.json();
      },
      catch: (cause) =>
        new Error(`LLM request failed: ${String(cause)}`, { cause }),
    });

    const completion = yield* Schema.decodeUnknownEffect(ChatCompletion)(json);
    const content = completion.choices[0]?.message.content;
    if (content === undefined || content === null) {
      return yield* Effect.fail(new Error("LLM response contained no message"));
    }
    return content;
  });
```

In a method without an `error` schema, apply `Effect.orDie` to this composed Effect or map every
failure to a declared error type. Store real provider credentials with Golem secrets; never embed
them in source or logs.

## Key Constraints

- Import Effect APIs from the exact root module `effect` so the app and SDK share one runtime.
- Use `Effect.tryPromise` around the runtime's WASI-backed `globalThis.fetch`.
- Do not import `effect/unstable/http` with the current generated Rollup/base-WASM setup.
- Do not use Node networking modules or native HTTP client packages.
- Keep handlers as Effects; do not return a raw `Promise` or mark a handler `async`.
- JSON request bodies require `JSON.stringify` and `Content-Type: application/json`.
- `fetch` does not fail for HTTP error statuses; check `response.ok` or `response.status`.
- Consume a response body at most once, and validate parsed JSON with Effect Schema.
- Let Golem handle durable retries; do not add manual retry loops around host operations.
- Durable replay does not deduplicate separate agent method invocations.
- Do not manually invoke a state-changing request method when an automated check will invoke it.
- Use named camelCase fields when posting JSON to another Golem endpoint.

## Authoritative API Sources

- [Pinned Effect dependency and component build scripts](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/package.json)
- [Generated application Rollup externalization](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/integration-test/rollup.config.component.mjs)
- [Embedded root Effect module](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/effect-bundle.mjs)
- [Effect 4.0.0-beta.98 `tryPromise` API](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/Effect.ts)
- [Effect 4.0.0-beta.98 Schema decoding API](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/Schema.ts)
- [Pinned QuickJS runtime fetch implementation](https://github.com/golemcloud/wasm-rquickjs/blob/ec23071c7769be5a240d6a44f6f691972040f466/crates/wasm-rquickjs/skeleton/src/builtin/http.js)
- [`Durability.atomically` implementation](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/internal/durabilityMode.ts)
