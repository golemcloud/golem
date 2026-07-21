---
name: golem-fire-and-forget-effect
description: "Triggering agent-to-agent invocations without waiting for their results in an Effect-based Golem project. Use for fire-and-forget RPC, breaking awaited RPC cycles, background work, fan-out, or event notifications with @golemcloud/effect-golem."
---

# Fire-and-Forget Agent Invocation with Effect

A fire-and-forget call submits an invocation to another agent and continues without waiting for
that method's result. Every typed remote method obtained through an Effect Golem agent's `client`
has a `.trigger(input)` form for this purpose.

## Trigger a Remote Method

Obtain the typed remote handle and yield the trigger Effect:

```typescript
const counter = yield* CounterAgent.client.get({ name: "my-counter" });

// A method declared with params: {} still requires an empty named-input record.
yield* counter.increment.trigger({});
```

Pass the same named-input record used by an awaited call when the method has parameters:

```typescript
const processor = yield* DataProcessor.client.get({ name: "pipeline-1" });
yield* processor.processBatch.trigger({ batch: batchData });
```

Both `client.get(...)` and `.trigger(...)` return lazy `Effect` values. Inside `Effect.gen`, use
`yield*` as shown. Calling `processor.processBatch.trigger(...)` and discarding the returned Effect
does not execute the RPC. Do not add an extra `.await()`, use Promise-based Golem SDK examples, or
invent a separate client helper.

## Semantics

`remote.method.trigger(input)` returns `Effect.Effect<void, RemoteCallError>`:

- success means the Golem host accepted the invocation;
- it does not wait for the remote method to finish or return its success value;
- the remote method's declared domain error is not returned to the triggering caller;
- request encoding or submission can still fail with `RemoteCallError`;
- after acceptance, there is no caller-side cancellation handle.

Handle the client and submission errors when the calling method exposes a matching typed error. If
an infrastructure failure should fail and durably retry a caller method whose contract has no error
schema, convert those errors to defects around the composed operation:

```typescript
runTest: () =>
  Effect.gen(function* () {
    const callback = yield* Callback.client.get({ name });
    yield* callback.run.trigger({});
    return yield* incrementLocalState;
  }).pipe(Effect.orDie),
```

Do not use `Effect.fork` as a substitute for `.trigger()`: an awaited remote invocation forked into
a fiber has different cancellation and completion semantics. Use the SDK's trigger form when the
result is intentionally not needed.

## Break an Awaited RPC Cycle

Durable agents process invocations sequentially. If agent A awaits agent B and B then awaits A,
both can wait forever. Trigger the callback so B does not wait for A to become available:

```typescript
// Agent A may await work performed by B.
const worker = yield* AgentB.client.get({ name: "b1" });
const result = yield* worker.doWork({ data });

// While handling that work, B submits the callback but does not await its result.
const caller = yield* AgentA.client.get({ name: "a1" });
yield* caller.onWorkDone.trigger({ result });
```

The callback can run only after the currently active invocation on A finishes. Do not read its
result or assume its side effects are already visible when `.trigger(...)` returns.

## Background Work and Fan-Out

Triggering is also appropriate for background work and event notifications. Enqueue one invocation
per target without awaiting remote completion:

```typescript
for (const region of ["us-east", "us-west", "eu-central"]) {
  const processor = yield* RegionProcessor.client.get({ region });
  yield* processor.runReport.trigger({ reportId });
}
```

Each `yield*` waits only for that submission to be accepted. The target agents execute the reports
asynchronously.

## Agent Definitions and Imports

The typed client is attached to the value returned by `defineAgent(...)`. Import that exported
agent spec to access `.client`. For agents shared across components or involved in module cycles,
keep the spec and implementation separate:

- `agents/CounterAgent.ts` exports the `defineAgent(...)` spec without calling `.implement(...)`;
- `agents/CounterAgent.impl.ts` imports the spec and calls `CounterAgent.implement(...)`;
- the hosting component's `src/main.ts` imports `CounterAgent.impl.js` for registration;
- RPC callers import only `CounterAgent.js`, avoiding implementation side effects.

Use emitted `.js` suffixes for local imports in generated Effect projects. There is no public SDK
helper that creates this typed client from a component URI or an arbitrary `AgentId`; use the
imported agent definition's `.client`.

## When to Use

- **Breaking RPC cycles** when a callback must not wait for the original caller
- **Background work** whose return value and remote domain error are intentionally ignored
- **Fan-out** to enqueue work on multiple target agents
- **Event notifications** that should not couple the sender to receiver completion time

Use a normal `yield* remote.method(input)` call instead when the result or typed domain error is
required before the caller can continue.

## CLI Equivalent

This skill covers agent-to-agent calls in Effect code. From the CLI, `--trigger` performs a
fire-and-forget invocation; Effect applications use TypeScript casing and value syntax:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```
