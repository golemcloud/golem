import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Fiber, Layer } from "effect"
import type { QuotaToken as RawQuotaToken } from "golem:core/types@2.0.0"
import * as Quota from "../src/Quota.js"
import { QuotaClient } from "../src/host/QuotaClient.js"
import { compile } from "../src/WitCodec.js"

/**
 * Local host fake used by the schema and operational tests below.
 */
type Event =
  | { tag: "construct"; resourceName: string; expectedUse: bigint }
  | { tag: "reserve"; resourceName: string; amount: bigint }
  | { tag: "commit"; resourceName: string; used: bigint; reservedAmount: bigint }
  | { tag: "split"; resourceName: string; childExpectedUse: bigint }
  | { tag: "merge"; resourceName: string; otherResource: string }
type Raw = RawQuotaToken & { resourceName: string; expectedUse: bigint }
type RawReservation = { token: Raw; amount: bigint; consumed: boolean }
const events: Event[] = []
let reserveFailure: unknown
const fake = QuotaClient.of({
  newToken: (resourceName, expectedUse) => {
    events.push({ tag: "construct", resourceName, expectedUse })
    return { resourceName, expectedUse } as Raw
  },
  reserve: (token, amount) => {
    if (reserveFailure !== undefined) throw reserveFailure
    const raw = token as Raw
    events.push({ tag: "reserve", resourceName: raw.resourceName, amount })
    return { token: raw, amount, consumed: false } as never
  },
  commit: (value, used) => {
    const r = value as unknown as RawReservation
    if (r.consumed) throw new Error("already consumed")
    r.consumed = true
    events.push({
      tag: "commit",
      resourceName: r.token.resourceName,
      used,
      reservedAmount: r.amount,
    })
  },
  split: (token, childExpectedUse) => {
    const raw = token as Raw
    if (childExpectedUse > raw.expectedUse) throw new Error("split overflow")
    raw.expectedUse -= childExpectedUse
    events.push({ tag: "split", resourceName: raw.resourceName, childExpectedUse })
    return { resourceName: raw.resourceName, expectedUse: childExpectedUse } as Raw
  },
  merge: (token, other) => {
    const target = token as Raw
    const source = other as Raw
    if (target.resourceName !== source.resourceName) throw new Error("resource mismatch")
    events.push({
      tag: "merge",
      resourceName: target.resourceName,
      otherResource: source.resourceName,
    })
  },
})
const QuotaTestLive: Layer.Layer<QuotaClient> = Layer.succeed(QuotaClient, fake)

describe("QuotaToken schema", () => {
  it.effect("compiles to an opaque quota-token schema graph", () =>
    Effect.gen(function* () {
      const codec = yield* compile(Quota.QuotaTokenSchema)
      expect(codec.graph.defs.size).toBe(0)
      expect(codec.graph.root.body).toEqual({ tag: "quota-token", spec: {} })
    }),
  )

  it.effect("transfers a token once through its schema codec", () =>
    Effect.gen(function* () {
      const token = yield* Quota.acquireQuotaToken("cpu", 100n)
      const codec = yield* compile(Quota.QuotaTokenSchema)
      const wire = yield* codec.encode(token)
      expect(wire.valueNodes[wire.root]?.tag).toBe("quota-token-handle")
      const received = yield* codec.decode(wire)
      expect((yield* codec.encode(received)).valueNodes[0]?.tag).toBe("quota-token-handle")
      expect(Exit.isFailure(yield* Effect.exit(codec.encode(token)))).toBe(true)
      expect(Exit.isFailure(yield* Effect.exit(codec.encode(received)))).toBe(true)
    }).pipe(Effect.provide(QuotaTestLive)),
  )
})

describe("Quota — operational API", () => {
  beforeEach(() => {
    events.length = 0
    reserveFailure = undefined
  })
  afterEach(() => {
    events.length = 0
    reserveFailure = undefined
  })

  it.effect("acquireQuotaToken constructs a QuotaToken via the host class", () =>
    Effect.gen(function* () {
      const token = yield* Quota.acquireQuotaToken("api-calls", 100n)
      expect(token).toBeInstanceOf(Quota.QuotaToken)
      expect(events).toEqual([{ tag: "construct", resourceName: "api-calls", expectedUse: 100n }])
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("acquireQuotaToken surfaces host failures as QuotaHostError", () =>
    Effect.gen(function* () {
      // Per-test override: `acquireQuotaToken` throws; the other
      // methods stay at their live (mock-routed) impls. Replaces the
      // legacy `__setAcquireQuotaTokenForTest` indirection.
      const failingAcquireLayer = Layer.succeed(
        QuotaClient,
        QuotaClient.of({
          newToken: () => {
            throw new Error("manifest does not declare resource")
          },
          reserve: fake.reserve,
          commit: fake.commit,
          split: fake.split,
          merge: fake.merge,
        }),
      )
      const exit = yield* Effect.exit(
        Quota.acquireQuotaToken("missing", 1n).pipe(Effect.provide(failingAcquireLayer)),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
      }
    }),
  )

  it.effect("withReservation reserves, runs body, and commits the body's used", () =>
    Effect.gen(function* () {
      const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
      const value = yield* Quota.withReservation(token, 100n, () =>
        Effect.succeed({ used: 42n, value: "ok" }),
      )
      expect(value).toBe("ok")

      const eventTags = events.map((e) => e.tag)
      expect(eventTags).toEqual(["construct", "reserve", "commit"])
      const commitEv = events[2]
      if (commitEv.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(42n)
      expect(commitEv.reservedAmount).toBe(100n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("withReservation commits 0 on body failure (drop semantics)", () =>
    Effect.gen(function* () {
      class Boom {
        readonly _tag = "Boom"
      }
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          return yield* Quota.withReservation(token, 50n, () => Effect.fail(new Boom()))
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/Boom/)
      }
      const commitEv = events.find((e) => e.tag === "commit")
      expect(commitEv).toBeDefined()
      if (commitEv?.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(0n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.live("withReservation commits 0 on body interruption", () =>
    Effect.gen(function* () {
      let bodyEntered = false
      const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
      const fiber = yield* Effect.forkChild(
        Quota.withReservation(token, 50n, () =>
          Effect.gen(function* () {
            bodyEntered = true
            yield* Effect.never
            return { used: 999n, value: "unreachable" }
          }),
        ),
      )
      // Wait for the body to be entered, then interrupt.
      yield* Effect.sleep("10 millis")
      expect(bodyEntered).toBe(true)
      yield* Fiber.interrupt(fiber)

      const commitEv = events.find((e) => e.tag === "commit")
      expect(commitEv).toBeDefined()
      if (commitEv?.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(0n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("withReservation surfaces failed-reservation as FailedReservationError", () =>
    Effect.gen(function* () {
      reserveFailure = { estimatedWaitNanos: 5000n }
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          return yield* Quota.withReservation(token, 100n, () =>
            Effect.succeed({ used: 0n, value: "never" }),
          )
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        const text = JSON.stringify(exit.cause)
        expect(text).toMatch(/FailedReservationError/)
        expect(text).toMatch(/5000/)
      }
      expect(events.find((e) => e.tag === "commit")).toBeUndefined()
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("manual reserve + commit emits the expected event sequence", () =>
    Effect.gen(function* () {
      yield* Effect.scoped(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          const reservation = yield* Quota.reserve(token, 10n)
          yield* Quota.commit(reservation, 7n)
        }),
      )
      const eventTags = events.map((e) => e.tag)
      expect(eventTags).toEqual(["construct", "reserve", "commit"])
      const commitEv = events[2]
      if (commitEv.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(7n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("reserve auto-commits 0 on scope close when not explicitly committed", () =>
    Effect.gen(function* () {
      yield* Effect.scoped(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          yield* Quota.reserve(token, 10n)
          // No explicit commit — scope close fires the finalizer.
        }),
      )
      const commits = events.filter((e) => e.tag === "commit")
      expect(commits).toHaveLength(1)
      if (commits[0].tag !== "commit") throw new Error("unreachable")
      expect(commits[0].used).toBe(0n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("commit twice on the same reservation fails with QuotaHostError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
            const reservation = yield* Quota.reserve(token, 10n)
            yield* Quota.commit(reservation, 5n)
            yield* Quota.commit(reservation, 1n)
          }),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/already committed/)
      }
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("manual reserve surfaces failed-reservation typed error", () =>
    Effect.gen(function* () {
      reserveFailure = { estimatedWaitNanos: undefined }
      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
            return yield* Quota.reserve(token, 10n)
          }),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/FailedReservationError/)
      }
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("manual reserve surfaces non-failed-reservation throws as QuotaHostError", () =>
    Effect.gen(function* () {
      reserveFailure = new Error("host invariant violated")
      const exit = yield* Effect.exit(
        Effect.scoped(
          Effect.gen(function* () {
            const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
            return yield* Quota.reserve(token, 10n)
          }),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        const text = JSON.stringify(exit.cause)
        expect(text).toMatch(/QuotaHostError/)
        expect(text).not.toMatch(/FailedReservationError/)
      }
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("split delegates to the host and returns a child token", () =>
    Effect.gen(function* () {
      const parent = yield* Quota.acquireQuotaToken("api-calls", 1000n)
      const child = yield* Quota.split(parent, 300n)
      expect(child).toBeInstanceOf(Quota.QuotaToken)
      const splits = events.filter((e) => e.tag === "split")
      expect(splits).toHaveLength(1)
      if (splits[0].tag !== "split") throw new Error("unreachable")
      expect(splits[0].childExpectedUse).toBe(300n)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("split overflow surfaces as QuotaHostError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const parent = yield* Quota.acquireQuotaToken("api-calls", 100n)
          return yield* Quota.split(parent, 999n)
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
      }
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("merge delegates to the host", () =>
    Effect.gen(function* () {
      const a = yield* Quota.acquireQuotaToken("api-calls", 500n)
      const b = yield* Quota.acquireQuotaToken("api-calls", 500n)
      yield* Quota.merge(a, b)
      const merges = events.filter((e) => e.tag === "merge")
      expect(merges).toHaveLength(1)
    }).pipe(Effect.provide(QuotaTestLive)),
  )

  it.effect("merge of mismatched resources surfaces as QuotaHostError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const a = yield* Quota.acquireQuotaToken("api-calls", 500n)
          const b = yield* Quota.acquireQuotaToken("storage", 500n)
          yield* Quota.merge(a, b)
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
      }
    }).pipe(Effect.provide(QuotaTestLive)),
  )
})
