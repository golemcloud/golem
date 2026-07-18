---
name: golem-atomic-block-effect
description: "Using atomic regions and durability controls in Effect-based Golem agents. Use when an @golemcloud/effect-golem project needs whole-region replay, Durability.atomically, persistence or idempotence modes, oplog replication, idempotency keys, or scoped retry policies."
---

# Atomic Regions and Durability Controls in Effect Golem Agents

Golem agents are durable by default. Use these advanced controls only when replay behavior must
change for a specific group of externally observable effects. Import the Effect SDK API from the
package root; do not translate callback helpers from `@golemcloud/golem-ts-sdk`.

```typescript
import { Duration, Effect } from "effect";
import { Durability, Retry } from "@golemcloud/effect-golem";
```

## Atomic Regions

`Durability.atomically` accepts an `Effect`, not a synchronous or async callback. If execution
fails after only part of the region completed, recovery re-executes the region from its begin
marker instead of replaying from the middle.

```typescript
const placeOrder = Durability.atomically(
  Effect.gen(function* () {
    const reservation = yield* reserveInventory(itemId, quantity);
    const charge = yield* chargePayment(customerId, price);
    return { reservation, charge };
  }),
);
```

`reserveInventory` and `chargePayment` above are assumed to return Effects. Yield every operation
inside the body so the atomic region controls their ordering and failure.

Use an atomic region for:

- two or more external effects that must replay together after a crash; or
- one external effect followed by validation that may fail, when that failure must cause the
  effect itself to run again instead of replaying its recorded response.

External effects can still be repeated. Atomicity here means **whole-region re-execution during
durable recovery**, not ACID isolation or rollback in the external system. Design remote operations
to tolerate duplicates or use a stable idempotency key.

An atomic region only controls one durable execution. It does not deduplicate a later, independent
invocation of the same method: that invocation runs the method, including work after the atomic
region, again. When validating a state-changing recovery example, start the atomic work once
against a fresh agent identity. Do not run it during build/deploy validation if a test harness will
start the same identity afterward.

### Agent-to-Agent Atomic Effects

The pinned Effect component has no outgoing HTTP client or global `fetch`. Call another Golem
agent through its typed client instead. The target methods may also have `Http` declarations, so
the same state remains externally observable over incoming HTTP.

Given a durable `SideEffectRecorder` with `record` and `decide` methods, a separate durable runner
can own the crash-prone atomic work:

```typescript
const runAtomicWork = Effect.gen(function* () {
  const recorder = yield* SideEffectRecorder.client.get({ name: "main" });

  yield* Durability.atomically(
    Effect.gen(function* () {
      yield* recorder.record({ event: "a" });
      yield* recorder.record({ event: "b" });

      if (yield* recorder.decide({})) {
        return yield* Effect.dieMessage("retry the complete atomic region");
      }

      yield* recorder.record({ event: "c" });
    }),
  );

  yield* recorder.record({ event: "d" });
}).pipe(Effect.orDie);
```

`SideEffectRecorder.client.get(...)` and every remote method invocation are Effects. Even methods
with no parameters are called with an empty named-input record, such as `decide({})`.

The typed client input is independent of the recorder's incoming HTTP exposure. With the pinned
SDK, a normal `Schema.String` parameter named `event` on `Http.post("/record")` binds from a JSON
body such as `{ "event": "a" }`; the SDK does not provide a documented `text/plain` body codec.
Do not hand-build `Unstructured.ElementSpec` values to imitate plain-text binding.

### Failure Semantics

`Durability.atomically` has intentionally stronger failure behavior than a normal Effect scope:

1. On success it writes the matching end marker and returns the body's value.
2. On a typed failure, defect, or interruption it leaves the region open and invokes the host's
   uncatchable trap so durable recovery can retry the complete region.
3. An outer `Effect.catchAll`, `Effect.either`, or similar operator cannot handle a body failure and
   continue the same invocation after the region.

The trap-recovery guarantee is a worker execution guarantee, not a promise that the original HTTP,
RPC, or CLI caller waits for replay. Do not externally retry with a new invocation merely because
the first caller observed a failure: that would start the method again and can duplicate work after
the atomic region.

Use `Effect.fail` for a condition that should trigger this trap just as a defect does; the high-level
atomic combinator traps every unsuccessful exit. Do not put caller-visible business failures inside
an atomic region if the invocation should recover from them and continue normally.

### Failure-Path Verification Limit

The pinned SDK has unit tests proving that an unsuccessful body leaves its marker open and calls the
host trap, but its real-runtime integration test covers only a successful atomic region. It exposes
no public Effect that waits for trap recovery, reports the recovery attempt, or guarantees that the
original caller eventually receives the replayed result. A fire-and-forget typed call only confirms
host acceptance; it does not provide a completion handle for the trapped invocation.

Consequently, use a successful `Durability.atomically` body for portable end-to-end application
tests. Verify deliberate crash/recovery behavior with executor-specific worker status and oplog
tooling rather than making a normal awaited method or polling loop depend on the trap completing.
Keep deterministic failures out of production atomic bodies: if the condition is unchanged after
recovery, the region can fail repeatedly.

### What Atomic Regions Are Not

- Do not wrap ordinary `Ref` updates just to make in-memory state “transactional.” Durable replay
  already rebuilds agent state, and agents process invocations sequentially.
- Do not use atomic regions to reduce oplog size or speed up recovery. Use snapshot-based recovery
  for that purpose.
- Do not claim that nested regions are supported, rejected, flattened, or merged; the pinned SDK
  does not define or test nesting behavior.

## Manual Begin and End Markers

Prefer `Durability.atomically`. When imperative control is genuinely required, begin is an Effect
value and end is a function taking the returned `bigint` oplog index:

```typescript
const manuallyBracketed = Effect.gen(function* () {
  const begin = yield* Durability.beginOperation;
  const result = yield* performExternalWork;
  yield* Durability.endOperation(begin);
  return result;
});
```

Manual markers do not trap or pair themselves on failure. Never put `endOperation(begin)` in an
unconditional finalizer: closing the marker after failure commits the region instead of leaving it
open for recovery.

## Persistence Level

Persistence-level controls are primarily for Golem-specific libraries implementing custom durable
behavior, not ordinary application optimization. Prefer the scoped combinator, which restores the
previous level when the Effect exits:

```typescript
const result = Durability.withPersistenceLevel(
  Durability.PersistenceLevel.persistNothing,
  customDurabilityEffect,
);
```

Available values are:

| Value                                                  | Meaning                                                  |
| ------------------------------------------------------ | -------------------------------------------------------- |
| `Durability.PersistenceLevel.smart`                    | Default host-managed durable behavior                    |
| `Durability.PersistenceLevel.persistRemoteSideEffects` | Persist remote side effects for replay                   |
| `Durability.PersistenceLevel.persistNothing`           | No replay or state-restoration guarantee for the section |

`persistNothing` is not an oplog-size switch. Code using it must implement the required live/replay
behavior itself. For low-level integration work, `Durability.getPersistenceLevel` is an Effect
value and `Durability.setPersistenceLevel(level)` changes the host mode directly; ordinary code
should use `withPersistenceLevel` so restoration is scoped.

## Idempotence Mode

The host default is `true`. In that mode, side effects are treated as idempotent and Golem
guarantees at-least-once semantics. This does not make an inherently non-idempotent external
operation safe. Opt out only for a side effect where a duplicate is worse than a missing call:

```typescript
const atMostOnce = Durability.withIdempotenceMode(false, nonIdempotentEffect);
```

`false` selects at-most-once handling; the executor fails the agent when it cannot determine whether
the side effect already ran. The scoped combinator restores the prior mode. Low-level code can yield
`Durability.getIdempotenceMode` and call `Durability.setIdempotenceMode(value)`, but should not
manually change the mode when the scoped form is sufficient.

## Oplog Replication Barrier

Wait for the oplog to reach the requested replica count by yielding the Effect:

```typescript
const replicationBarrier = Effect.gen(function* () {
  yield* Durability.oplogCommit(3);
});
```

The replica count must be a safe integer from `0` through `255`; the SDK forwards that value. The
host waits for at least that many replicas, or its maximum replica count if fewer are available.
This is a replication barrier, not an atomic-region commit.

## Durable Idempotency Keys

`generateIdempotencyKey` is an Effect value, not a function:

```typescript
const makeRequest = Effect.gen(function* () {
  const key = yield* Durability.generateIdempotencyKey;
  return { key };
});
```

The host persists and commits the generated key, and replay returns that same key rather than
generating another one. The generated TypeScript value is a UUID record with `highBits` and
`lowBits` bigint fields, not a UUID string. Convert it explicitly at an external API boundary if
text is required.

## Scoped Retry Policies

Retry policy control belongs to the separate `Retry` namespace. Effect Golem manages named host
policies rather than replacing one anonymous global policy:

```typescript
const paymentRetry = Retry.NamedPolicy.named(
  "payment",
  Retry.Policy.periodic(Duration.seconds(1)),
);

const result = Retry.withPolicy(paymentRetry, paymentEffect);
```

`Retry.withPolicy` attempts to restore the previous policy with the same name, or remove the
temporary one, when the Effect exits; cleanup failures are ignored. Other named policies remain
active. The combinator temporarily changes the host-visible policy set—it does not retry the whole
Effect like `Effect.retry`. Do not substitute a local Effect retry schedule when durable,
host-visible retry behavior is required.

## Key Constraints

- Import `Durability` and `Retry` from `@golemcloud/effect-golem`; do not import internal host
  clients or layers.
- Normal `defineAgent(...).implement(...)` execution supplies the services required by these
  Effects. Do not manually provide SDK-internal layers in application code.
- Keep agent methods Effect-based; do not wrap `Durability.atomically` in an `async` callback.
- Prefer `Durability.atomically` over manual begin/end markers.
- Keep externally observable actions duplicate-safe because recovery may execute them again.
- Use typed agent clients rather than `fetch` for same-application agent calls in the pinned Effect
  component.
- Do not assume an awaited caller or fire-and-forget trigger can observe completion after an
  intentional atomic trap; use executor-specific recovery tooling for that test.

## Authoritative API Sources

- [`Durability` public exports](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Durability.ts)
- [Atomic and durability-mode implementation](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/internal/durabilityMode.ts)
- [Durability tests](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/test/durability.test.ts)
- [`Retry` policy API](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Retry.ts)
- [Typed agent client API](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Client.ts)
- [HTTP declaration and body-binding API](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Http.ts)
- [Pinned component world](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/wit/main.wit)
