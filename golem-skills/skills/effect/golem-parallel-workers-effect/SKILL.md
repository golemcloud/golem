---
name: golem-parallel-workers-effect
description: "Fans out work to multiple durable agents and collects results with Effect structured concurrency. Use for parallel execution, fan-out/fan-in, bounded concurrency, child fibers, agent forks, or aggregating remote agent results in an @golemcloud/effect-golem project."
---

# Parallel Workers with Effect

Golem runs invocations to one durable agent instance sequentially. For actual parallel work, send
the work to distinct agent instances and compose their typed remote-call Effects with Effect v4
structured-concurrency operators. Do not translate `Promise.all`, floating promises, or `async`
handlers from the Promise-based TypeScript SDK.

Use the mechanisms according to their semantics:

| Mechanism                         | Purpose                                                             |
| --------------------------------- | ------------------------------------------------------------------- |
| `Effect.all`                      | Run remote-call Effects concurrently and collect ordered results    |
| `Effect.forkChild` / `Fiber.join` | Manage individual child fibers within the current invocation        |
| `remote.method.trigger(input)`    | Submit durable background work when no result is needed             |
| `Agents.fork`                     | Clone the current Golem agent and its state into a phantom instance |

## Fan Out to Durable Child Agents

Define each worker as a normal durable agent. Constructor parameters are its durable identity, so
different `id` values address different worker instances.

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

const Numbers = Schema.Array(Schema.Number);

export const Worker = defineAgent({
  name: "Worker",
  mode: "durable",
  constructorParams: {
    id: Schema.Number,
  },
  methods: {
    compute: method({
      params: { value: Schema.Number },
      success: Schema.Number,
    }),
  },
}).implement(() =>
  Effect.succeed({
    compute: ({ value }) => Effect.succeed(value * 2),
  }),
);

export const Coordinator = defineAgent({
  name: "Coordinator",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    fanOut: method({
      params: { items: Numbers },
      success: Numbers,
    }),
  },
}).implement(() =>
  Effect.succeed({
    fanOut: ({ items }) =>
      Effect.all(
        items.map((value, id) =>
          Effect.gen(function* () {
            const worker = yield* Worker.client.get({ id });
            return yield* worker.compute({ value });
          }),
        ),
        { concurrency: "unbounded" },
      ).pipe(Effect.orDie),
  }),
);
```

`Worker.client.get({ id })` and `worker.compute({ value })` both return lazy Effects. `Effect.all`
executes those Effects concurrently, retains input order in its output, and returns an empty array
for empty input. In a stateful existing application, add the method contract and handler to the
existing coordinator instead of registering a duplicate agent type.

Import every implementation module from `src/main.ts` using its emitted `.js` suffix:

```typescript
import "./worker.js";
import "./coordinator.js";
```

## Bound Concurrency

Use a numeric `concurrency` value when the input can be large or a downstream system has a capacity
limit:

```typescript
const calls = items.map((value, id) =>
  Effect.gen(function* () {
    const worker = yield* Worker.client.get({ id });
    return yield* worker.compute({ value });
  }),
);

const results = yield * Effect.all(calls, { concurrency: 5 });
```

This runs at most five lookup-and-call Effects at once. It is preferable to manually slicing the
input into batches because Effect owns scheduling, interruption, and result ordering. Pick stable worker
identities when calls across invocations should reach the same worker; include a job identity when
each fan-out must use a separate durable worker set.

## Fail-Fast and Partial Results

By default, `Effect.all` fails on the first typed failure. Effects that have not started are
skipped, and already-started siblings may be interrupted. Remote cancellation is best-effort: a
worker invocation that already began can still finish and apply side effects even when its caller
fiber is interrupted. Make remote work idempotent when retries or interruption can overlap.

When every worker outcome must be retained, turn each typed failure into data before collecting:

```typescript
const outcomes =
  yield *
  Effect.all(
    calls.map((call) =>
      Effect.match(call, {
        onFailure: (error) => ({
          _tag: "Failure" as const,
          error: String(error),
        }),
        onSuccess: (value) => ({
          _tag: "Success" as const,
          value,
        }),
      }),
    ),
    { concurrency: 8 },
  );
```

`Effect.match` handles typed failures, not defects or interruption. If those outcomes must also be
returned as data, use `Effect.exit` for each call and declare a schema-safe public result. Do not
silently apply `Effect.orDie` when the caller is expected to recover; either handle the remote
error or map it to a domain error declared in the coordinator method.

## Child Fibers Are Structured, Not Background Agents

Use child fibers when individual calls need separate handles, cancellation, or joins:

```typescript
import { Effect, Fiber } from "effect";

const left = yield * Effect.forkChild(firstWorker.compute({ value: 1 }));
const right = yield * Effect.forkChild(secondWorker.compute({ value: 2 }));

const results =
  yield *
  Effect.all([Fiber.join(left), Fiber.join(right)], {
    concurrency: "unbounded",
  });
```

`Effect.forkChild` creates a fiber in the current agent invocation; it does not create a Golem
agent, clone state, or make work outlive the parent. Prefer direct `Effect.all` unless individual
fiber control is needed.

## Fire-and-Forget Fan-Out

If results are intentionally not needed, submit each remote method through its real `.trigger`
form. Yield the trigger Effects—the calls do nothing if their Effects are discarded:

```typescript
yield *
  Effect.all(
    regions.map((region) =>
      Effect.gen(function* () {
        const worker = yield* RegionWorker.client.get({ region });
        yield* worker.runReport.trigger({ reportId });
      }),
    ),
    { concurrency: "unbounded" },
  );
```

Trigger success means the host accepted the invocation, not that the worker completed. There is no
result or remote domain error to collect afterward. Use awaited calls when fan-in needs the result.
Host-backed `Agents.Promises` can implement an explicit later rendezvous, but it is unrelated to a
JavaScript `Promise`; this SDK version does not export a ready-made Effect Schema for passing its
`PromiseId` through an agent method.

## Clone the Current Agent with `Agents.fork`

`Agents.fork` is an Effect value that asks Golem to clone the current agent at the current execution
point. Both agents resume from it: the source sees `tag: "original"`, and the clone sees
`tag: "forked"`. This is different from `Effect.forkChild`.

Use a host-backed Golem promise created before the fork to return data from the cloned agent:

```typescript
import { Effect } from "effect";
import { Agents } from "@golemcloud/effect-golem";

const parallelCompute = Effect.gen(function* () {
  const promiseId = yield* Agents.Promises.create;
  const branch = yield* Agents.fork;

  if (branch.tag === "forked") {
    const payload = new TextEncoder().encode("forked-result");
    yield* Agents.Promises.complete(promiseId, payload);
    return "forked done";
  }

  const payload = yield* Agents.Promises.await(promiseId);
  const forkedResult = new TextDecoder().decode(payload);
  return `Combined: original + ${forkedResult}`;
}).pipe(Effect.orDie);
```

The forked branch must finish its handler after completing its slice; otherwise it can continue
through coordinator-only logic. `Agents.Promises.await` is durably suspendable. Interrupting the
JavaScript wait does not cancel or complete the host promise, so every created promise still needs
a peer that completes it.

## Choosing a Pattern

| Requirement                                         | Use                                    |
| --------------------------------------------------- | -------------------------------------- |
| Independent work with persistent worker identities  | Distinct durable agents + `Effect.all` |
| Limit pressure on workers or dependencies           | Numeric `Effect.all` concurrency       |
| Need every typed success and failure                | Per-call `Effect.match` + `Effect.all` |
| Work should continue without returning a value      | Remote `.trigger(...)`                 |
| Need individual in-invocation cancellation or joins | `Effect.forkChild` + `Fiber`           |
| Need a copy of the current agent state              | `Agents.fork` + `Agents.Promises`      |

## Key Constraints

- Calls to the same agent identity are still processed sequentially; use distinct identities for
  real worker parallelism.
- Compose SDK-returned Effects. Do not use `Promise.all`, plain `async` handlers, or floating Effects.
- Use named records for `client.get` and remote method inputs, including `{}` for no parameters.
- Avoid synchronous RPC cycles in which two sequential agents await each other; use `.trigger`
  only when the callback result is not required.
- Durable child agents remain addressable after fan-in. The pinned Effect SDK client has no remote
  agent deletion method, so do not invent `delete`, `remove`, or `destroy` on a remote handle.
- Put arbitrary external or nondeterministic side effects behind host-backed SDK APIs or an
  appropriate durable wrapper; Effect concurrency alone does not make a raw JavaScript side effect
  replay-safe.
