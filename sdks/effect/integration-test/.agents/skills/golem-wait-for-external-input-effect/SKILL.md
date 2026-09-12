---
name: golem-wait-for-external-input-effect
description: "Waiting for external input with durable promises in an Effect-based Golem agent. Use for human-in-the-loop workflows, callbacks, external decisions, or suspending an @golemcloud/effect-golem invocation until another agent supplies data."
---

# Waiting for External Input in an Effect Golem Agent

Use `Agents.Promises` when an invocation must durably suspend until another agent or external
system supplies a one-shot byte payload. Golem owns the promise and suspension, so the waiting
agent consumes no compute while idle and can recover through normal durable execution.

These are Golem host promises, not JavaScript `Promise` values or Effect `Deferred` values. An
ordinary `Deferred`, fiber, queue, or callback is in-memory coordination and must not be used as a
durable external waiting handle.

## API

Import the public namespace from the Effect SDK:

```typescript
import { Effect } from "effect";
import { Agents } from "@golemcloud/effect-golem";
```

| API                                   | Behavior                                                               |
| ------------------------------------- | ---------------------------------------------------------------------- |
| `Agents.Promises.create`              | Effect that allocates a durable promise and returns `Agents.PromiseId` |
| `Agents.Promises.await(id)`           | Effect that durably waits and returns the completed `Uint8Array`       |
| `Agents.Promises.poll(id)`            | Non-blocking Effect returning `Uint8Array \| undefined`                |
| `Agents.Promises.complete(id, bytes)` | Effect that completes the promise once and returns `true`              |

`create` is itself an Effect value, so yield it without parentheses:

```typescript
const promiseId = yield * Agents.Promises.create;
const bytes = yield * Agents.Promises.await(promiseId);
```

Only the agent that created a promise may await or poll it. Another agent may complete the shared
`PromiseId`. Completion is one-shot; another completion fails with
`Agents.PromiseAlreadyCompletedError`.

## Encode and Decode the Payload

Golem promises carry bytes. Define the application protocol explicitly at the boundary:

```typescript
const payload = new TextEncoder().encode("approved");
yield * Agents.Promises.complete(promiseId, payload);

const bytes = yield * Agents.Promises.await(promiseId);
const decision = new TextDecoder().decode(bytes);
```

For structured data, encode JSON before completion and parse and validate it after awaiting. Do
not hide malformed external input by substituting an empty or default value.

## Pass a Promise ID Through Agent RPC

`Agents.PromiseId` is a public TypeScript type but this SDK revision does not export a ready-made
Effect Schema for it. Agent method parameters need a Schema, so reproduce the host type's exact
structural shape:

```typescript
import { Schema } from "effect";
import { WitTypes } from "@golemcloud/effect-golem";

const PromiseIdSchema = Schema.Struct({
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

Use `WitTypes.Uint64`, not `Schema.BigInt`: the UUID halves and oplog index are WIT `u64` values.
Pass only this plain data over RPC. Do not pass an Effect, fiber, host pollable, or JavaScript
promise.

## Full Example: Human-in-the-Loop Approval

This pattern uses two durable agents. `ApprovalAgent.request` creates the promise, hands its ID to
the matching `DeciderAgent`, and then awaits it. A later HTTP call to `DeciderAgent.decide`
completes the promise and resumes the approval invocation.

```typescript
// src/approval-agents.ts
import { Effect, Ref, Schema } from "effect";
import {
  Agents,
  defineAgent,
  Http,
  method,
  WitTypes,
} from "@golemcloud/effect-golem";

const PromiseIdSchema = Schema.Struct({
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

export const DeciderAgent = defineAgent({
  name: "DeciderAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  http: Http.mount("/deciders/{name}"),
  methods: {
    receive: method({
      params: { promiseId: PromiseIdSchema },
      success: Schema.Void,
      http: [Http.post("/receive")],
    }),
    decide: method({
      params: { decision: Schema.String },
      success: Schema.Boolean,
      http: [Http.post("/decide")],
    }),
  },
}).implement(() =>
  Effect.gen(function* () {
    const pending = yield* Ref.make<Agents.PromiseId | undefined>(undefined);

    return {
      receive: ({ promiseId }) => Ref.set(pending, promiseId),

      decide: ({ decision }) =>
        Effect.gen(function* () {
          const promiseId = yield* Ref.get(pending);
          if (promiseId === undefined) return false;

          const completed = yield* Agents.Promises.complete(
            promiseId,
            new TextEncoder().encode(decision),
          ).pipe(
            Effect.catchTag("PromiseAlreadyCompletedError", () =>
              Effect.succeed(false),
            ),
          );

          if (completed) yield* Ref.set(pending, undefined);
          return completed;
        }).pipe(Effect.orDie),
    };
  }),
);

export const ApprovalAgent = defineAgent({
  name: "ApprovalAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  http: Http.mount("/approvals/{name}"),
  methods: {
    request: method({
      params: {},
      success: Schema.String,
      http: [Http.post("/request")],
    }),
    getResult: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/result")],
    }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    const result = yield* Ref.make("");

    return {
      request: () =>
        Effect.gen(function* () {
          const promiseId = yield* Agents.Promises.create;
          const decider = yield* DeciderAgent.client.get({ name });

          // Await this short handoff so the decider has stored the ID before we suspend.
          yield* decider.receive({ promiseId });

          const bytes = yield* Agents.Promises.await(promiseId);
          const decision = new TextDecoder().decode(bytes);
          yield* Ref.set(result, decision);
          return decision;
        }).pipe(Effect.orDie),

      getResult: () => Ref.get(result),
    };
  }),
);
```

Register the module for side effects:

```typescript
// src/main.ts
import "./approval-agents.js";
```

Expose both agents in the HTTP API without removing existing deployment entries:

```yaml
httpApi:
  deployments:
    local:
      - domain: test-app.localhost:9006
        agents:
          ApprovalAgent: {}
          DeciderAgent: {}
```

With these method schemas, `POST /deciders/a1/decide` accepts the normal JSON body
`{ "decision": "approved" }`. The waiting `/approvals/a1/request` invocation resumes, stores
`"approved"`, and returns it.

## Handoff Choices

An awaited handoff confirms that the receiver stored the ID before the creator starts waiting:

```typescript
yield * decider.receive({ promiseId });
```

If acknowledgement is unnecessary, fire-and-forget handoff is also available:

```typescript
yield * decider.receive.trigger({ promiseId });
```

Both `client.get(...)` and the remote call are lazy Effects and must be yielded. Avoid synchronous
RPC cycles: the completing agent must not call back and await a method on the already-suspended
creator.

## Lifecycle and Error Rules

- Create a fresh Golem promise for each one-shot wait; do not reuse a completed ID.
- Ensure every created promise has a completion path. Interrupting `Agents.Promises.await` stops
  only the JavaScript wait; the host promise remains pending.
- Do not add polling loops or manual retries around `await`; let Golem persist the suspension and
  replay durable execution.
- Keep `PromiseId`, URLs, decisions, and other plain values in agent state. Never store Effects,
  `Deferred`, fibers, or host resource handles in snapshot state.
- `create`, `await`, and `complete` expose typed SDK host errors. Map them to a declared domain
  error when callers should recover, or deliberately use `Effect.orDie` when infrastructure
  failure should fail and retry the invocation.
- Use `poll` only when a non-blocking readiness check is required. Prefer `await` for durable
  suspension instead of repeatedly polling.
- Import every implemented agent module from `src/main.ts`, and do not edit generated files under
  `golem-temp/`.
