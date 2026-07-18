---
name: golem-add-webhook-effect
description: "Using one-shot webhooks in an Effect-based Golem agent. Use when an @golemcloud/effect-golem agent must create a public callback URL, durably await an external POST, decode its body, or customize its webhook path."
---

# Using Webhooks in an Effect Golem Agent

`Webhook.create` allocates a Golem promise and mints a temporary public URL for it. Posting to the
URL completes the promise; awaiting the handle durably suspends the current invocation without
consuming compute until the callback arrives.

Use this for asynchronous provider callbacks such as payment, CI/CD, and workflow notifications.
The URL is a one-shot rendezvous, not a permanent incoming HTTP endpoint. Use `Http.post(...)`
instead when an external system needs a stable recurring route.

## Prerequisites

The agent that calls `Webhook.create` must:

1. declare an HTTP mount with `Http.mount(...)`; and
2. be included in the active `httpApi` deployment in `golem.yaml`.

Otherwise `Webhook.create` fails with `WebhookHostError` after allocating its underlying promise.
The error type is exported from `@golemcloud/effect-golem`.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-effect` | Declaring `Http.mount(...)`, endpoint metadata, and the HTTP API deployment |
| `golem-add-agent-effect` | Defining Effect schemas, methods, state, and implementation registration |
| `golem-configure-api-domain` | Configuring the deployment domain and optional `webhookUrl` prefix |

## API

Import the SDK namespace rather than translating APIs from another language SDK:

```typescript
import { Effect, Schema } from "effect";
import {
  defineAgent,
  Http,
  method,
  Webhook,
} from "@golemcloud/effect-golem";
```

| API | Behavior |
|---|---|
| `Webhook.create` | `Effect` that allocates a promise and returns a `WebhookHandle` |
| `handle.url` | Public URL to register or share with the external caller |
| `handle.await` | `Effect` that durably waits for the POST body |
| `handle.poll` | Non-blocking `Effect` returning `WebhookPayload \| undefined` |
| `handle.promiseId` | Underlying `Agents.PromiseId`; normally keep this internal |
| `payload.bytes` | Raw request body as `Uint8Array` |
| `payload.text()` | Request body decoded as UTF-8 |
| `payload.decode(schema)` | Schema-checked JSON decoding in the Effect error channel |
| `payload.json<T>()` | Synchronous JSON parsing that can throw; prefer `decode` |

## Create, Share, and Await

Keep the handle local when one handler creates the webhook and immediately awaits it:

```typescript
const waitForCallback = Effect.gen(function* () {
  const handle = yield* Webhook.create;

  // Register handle.url with the external provider through the intended secure channel.

  const payload = yield* handle.await;
  return payload.text();
}).pipe(Effect.orDie);
```

`Webhook.create` and `handle.await` have typed host errors. If the public method does not declare
matching error schemas, explicitly convert those infrastructure failures to defects with
`Effect.orDie`; do not hide them by returning an empty payload.

For JSON, decode with an Effect Schema:

```typescript
const PaymentEvent = Schema.Struct({
  id: Schema.String,
  status: Schema.String,
});

const event = yield* payload.decode(PaymentEvent).pipe(Effect.orDie);
```

`payload.decode(PaymentEvent)` validates both the JSON syntax and the decoded shape. Avoid
`payload.json<PaymentEvent>()` when a parsing exception would escape the Effect error channel.

## Mount and Webhook URL

Declare the ordinary agent mount and, optionally, a custom webhook suffix together:

```typescript
export const PaymentWatcher = defineAgent({
  name: "PaymentWatcher",
  mode: "durable",
  constructorParams: {
    accountName: Schema.String,
  },
  http: Http.mount("/payments/{accountName}", {
    webhookSuffix: "/payment-callbacks/{accountName}",
  }),
  methods: {
    waitForPayment: method({
      params: {},
      success: PaymentEvent,
      http: [Http.post("/wait")],
    }),
  },
}).implement(() =>
  Effect.succeed({
    waitForPayment: () =>
      Effect.gen(function* () {
        const handle = yield* Webhook.create;
        // Register handle.url with the payment provider here.
        const payload = yield* handle.await;
        return yield* payload.decode(PaymentEvent);
      }).pipe(Effect.orDie),
  }),
);
```

The generated URL has this shape:

```text
https://<domain>/<webhook-prefix>/<webhook-suffix>/<signed-id>
```

- The deployment's `webhookUrl` controls the prefix and defaults to `/webhooks`.
- The suffix defaults to the agent type name in kebab-case.
- `webhookSuffix` accepts literal segments, constructor variables such as `{accountName}`, and
  system variables such as `{agent-type}`.
- Suffixes cannot contain query parameters or catch-all variables.

Deploy the agent without removing existing entries:

```yaml
httpApi:
  deployments:
    local:
      - domain: payments.localhost:9006
        webhookUrl: "/callbacks/"
        agents:
          PaymentWatcher: {}
```

After `golem build` and `golem deploy --yes`, an external HTTP client completes a webhook by
POSTing its payload directly to `handle.url`.

## Sharing a URL with Another Agent

Use the typed client on an agent spec for same-component RPC:

```typescript
const receiver = yield* CallbackRegistry.client.get({ accountName });
yield* receiver.receiveUrl.trigger({ url: handle.url });
const payload = yield* handle.await;
```

Define `receiveUrl` with `method({ params: { url: Schema.String }, ... })`; remote methods and
`client.get(...)` are Effects. Use the remote method's `.trigger(...)` form when URL handoff is
fire-and-forget and the creator should enter `handle.await` immediately. A normal
`yield* receiver.receiveUrl(...)` waits for the remote method to finish before reaching the
webhook wait; use that form only when acknowledgement is required.

A separate agent can store or expose the URL while the webhook creator is suspended. For a real
webhook integration, an external system must still POST to that URL.

## Effect SDK 1.5.0 Outgoing HTTP Limitation

At the pinned Effect SDK revision, component code cannot itself POST to a webhook URL:

- the component world does not import `wasi:http/outgoing-handler`;
- the runtime does not provide global `fetch`;
- the SDK exports no outbound `HttpClient`; its `Http` namespace only declares incoming routes.

Do not use `fetch`, install `@effect/platform` expecting it to add a host transport, import the
plain TypeScript Golem SDK, or invent an Effect Golem HTTP helper. A successful bundle would not
make the missing component capability available at runtime.

Use an external service or test process to POST to `handle.url`. If an internal Effect-only test
must exercise suspension and resumption without validating the HTTP route, it may deliberately
complete the underlying promise as described next. Keep that adaptation clearly labeled: it is
not an outgoing webhook request.

## Internal Promise-Completion Adaptation

`handle.promiseId` can cross a typed RPC boundary, and another agent can complete it with
`Agents.Promises.complete`. The SDK has no ready-made `PromiseId` schema, so reproduce its exact
WIT shape with unsigned 64-bit fields:

```typescript
import { Effect, Ref, Schema } from "effect";
import {
  Agents,
  defineAgent,
  Http,
  method,
  Webhook,
  WitTypes,
} from "@golemcloud/effect-golem";

const PromiseId = Schema.Struct({
  agentId: Schema.Struct({
    componentId: Schema.Struct({
      uuid: Schema.Struct({
        highBits: WitTypes.Uint64,
        lowBits: WitTypes.Uint64,
      }),
    }),
    agentId: Schema.String,
  }),
  oplogIdx: WitTypes.Uint64,
});
```

Use `WitTypes.Uint64`, not plain `Schema.BigInt`: the UUID halves and oplog index are WIT `u64`.
The sender can retain both values in ordinary per-instance state:

```typescript
const delivery = yield* Ref.make<
  { url: string; promiseId: Agents.PromiseId } | undefined
>(undefined);

return {
  receiveUrl: ({ url, promiseId }) =>
    Ref.set(delivery, { url, promiseId }),

  send: ({ message }) =>
    Effect.gen(function* () {
      const current = yield* Ref.get(delivery);
      if (current === undefined) return false;

      return yield* Agents.Promises.complete(
        current.promiseId,
        new TextEncoder().encode(message),
      );
    }).pipe(Effect.orDie),
};
```

The corresponding contracts are:

```typescript
receiveUrl: method({
  params: { url: Schema.String, promiseId: PromiseId },
  success: Schema.Void,
  http: [Http.post("/receive-url")],
}),
send: method({
  params: { message: Schema.String },
  success: Schema.Boolean,
  http: [Http.post("/send")],
}),
```

The webhook creator sends both values, then waits normally:

```typescript
const handle = yield* Webhook.create;
const sender = yield* SenderAgent.client.get({ name });
yield* sender.receiveUrl.trigger({
  url: handle.url,
  promiseId: handle.promiseId,
});
const body = (yield* handle.await).text();
```

Direct completion resumes `handle.await`, but bypasses the webhook URL, its signature, and HTTP
routing. It is suitable for an explicit internal fallback or test, not for claiming an external
provider delivered a webhook. Completion is one-shot; a second completion fails with
`Agents.PromiseAlreadyCompletedError`.

### Verifying Suspension Deterministically

When a test must observe the creator in the `Suspended` state:

1. Invoke and await a short non-blocking method on each participating agent first, so the status
   check cannot sample a cold agent that is still starting.
2. Trigger the method that creates the webhook.
3. Hand off the URL and promise ID with `receiveUrl.trigger(...)`, then immediately yield
   `handle.await`.
4. Observe the creator as suspended before calling the sender's completion method.

Do not complete the promise in the handoff method. `handle.await` polls first, so completion before
the creator reaches the wait takes the already-completed fast path and never produces an
observable suspension.

## State and Lifecycle Rules

- Keep `WebhookHandle` invocation-local when creation and awaiting happen in one handler.
- A plain `Ref` may temporarily hold a handle, but never put a handle in snapshot state: its
  `await` and `poll` fields are Effects and are not schema-serializable.
- Snapshot only plain data such as the URL and result. This SDK revision cannot reconstruct a
  handle from a persisted `promiseId`.
- Only the creating agent may call `handle.await`, `handle.poll`, or the corresponding raw promise
  read operations. Another agent may complete a shared promise ID.
- The URL accepts a POST body only and completes one promise once.
- Treat the signed URL as a one-shot bearer capability: share it only with the intended caller and
  do not write it to application logs.
- Let Golem persist the suspension and replay. Do not add polling loops or manual retries around
  `handle.await`.
- Register every implemented agent module from `src/main.ts` with an emitted `.js` side-effect
  import, and do not edit files under `golem-temp/`.
