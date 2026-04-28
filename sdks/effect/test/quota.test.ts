import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Fiber, Schema } from "effect"
import * as Quota from "../src/quota.js"
import { QuotaToken as QuotaTokenSchema, QuotaTokenRecord } from "../src/quota.js"
import { toWitCodec } from "../src/wit-codec.js"
import { QuotaToken } from "golem:quota/types@1.5.0"
import * as QuotaMock from "./mocks/golem-quota-types.js"

const sample = {
  environmentId: { uuid: { highBits: 1n, lowBits: 2n } },
  resourceName: "cpu",
  expectedUse: 100n,
  lastCredit: 50n,
  lastCreditAt: { seconds: 1_700_000_000n, nanoseconds: 123 },
}

describe("QuotaToken schema", () => {
  it.effect("encodes a QuotaToken instance to its record shape", () =>
    Effect.gen(function* () {
      const token = QuotaToken.fromRecord(sample)
      const enc = yield* Schema.encodeEffect(QuotaTokenSchema)(token)
      expect(enc).toEqual(sample)
    }),
  )

  it.effect("decodes a record into a QuotaToken instance", () =>
    Effect.gen(function* () {
      const token = yield* Schema.decodeEffect(QuotaTokenSchema)(sample)
      expect(token).toBeInstanceOf(QuotaToken)
      expect(token.toRecord()).toEqual(sample)
    }),
  )

  it.effect("compiles to a WIT record with the expected field types", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(QuotaTokenSchema as any)
      const root = wc.witType.nodes[0]?.type as any
      expect(root.tag).toBe("record-type")
      const fieldNames = root.val.map((p: [string, number]) => p[0])
      expect(fieldNames).toEqual([
        "environmentId",
        "resourceName",
        "expectedUse",
        "lastCredit",
        "lastCreditAt",
      ])

      // expectedUse must be u64; lastCredit is s64; lastCreditAt.nanoseconds is u32.
      const find = (name: string) => root.val.find((p: [string, number]) => p[0] === name)[1]
      expect(wc.witType.nodes[find("expectedUse")]?.type.tag).toBe("prim-u64-type")
      expect(wc.witType.nodes[find("lastCredit")]?.type.tag).toBe("prim-s64-type")
    }),
  )

  it.effect("round-trips QuotaToken end-to-end through the WIT codec", () =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(QuotaTokenSchema as any)
      const codec = wc.codec as Schema.Codec<QuotaToken, any, never, never>
      const token = QuotaToken.fromRecord(sample)
      const wv = yield* Schema.encodeEffect(codec)(token)
      const back = yield* Schema.decodeEffect(codec)(wv)
      expect(back).toBeInstanceOf(QuotaToken)
      expect(back.toRecord()).toEqual(sample)
    }),
  )

  it.effect("QuotaTokenRecord schema matches the host record shape", () =>
    Effect.gen(function* () {
      const rec = yield* Schema.decodeEffect(QuotaTokenRecord)(sample)
      expect(rec).toEqual(sample)
    }),
  )
})

describe("Quota — operational API", () => {
  beforeEach(() => {
    QuotaMock.__reset()
  })
  afterEach(() => {
    QuotaMock.__reset()
  })

  it.effect("acquireQuotaToken constructs a QuotaToken via the host class", () =>
    Effect.gen(function* () {
      const token = yield* Quota.acquireQuotaToken("api-calls", 100n)
      expect(token).toBeInstanceOf(QuotaToken)
      expect(QuotaMock.events).toEqual([
        { tag: "construct", resourceName: "api-calls", expectedUse: 100n },
      ])
    }),
  )

  it.effect("acquireQuotaToken surfaces host failures as QuotaHostError", () =>
    Effect.gen(function* () {
      Quota.__setAcquireQuotaTokenForTest(() => {
        throw new Error("manifest does not declare resource")
      })
      try {
        const exit = yield* Effect.exit(Quota.acquireQuotaToken("missing", 1n))
        expect(Exit.isFailure(exit)).toBe(true)
        if (Exit.isFailure(exit)) {
          expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
        }
      } finally {
        Quota.__resetAcquireQuotaTokenForTest()
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

      const eventTags = QuotaMock.events.map((e) => e.tag)
      expect(eventTags).toEqual(["construct", "reserve", "commit"])
      const commitEv = QuotaMock.events[2]
      if (commitEv.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(42n)
      expect(commitEv.reservedAmount).toBe(100n)
    }),
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
      const commitEv = QuotaMock.events.find((e) => e.tag === "commit")
      expect(commitEv).toBeDefined()
      if (commitEv?.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(0n)
    }),
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

      const commitEv = QuotaMock.events.find((e) => e.tag === "commit")
      expect(commitEv).toBeDefined()
      if (commitEv?.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(0n)
    }),
  )

  it.effect("withReservation surfaces failed-reservation as FailedReservationError", () =>
    Effect.gen(function* () {
      QuotaMock.__setReserveFails({ estimatedWaitNanos: 5000n })
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
      expect(QuotaMock.events.find((e) => e.tag === "commit")).toBeUndefined()
    }),
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
      const eventTags = QuotaMock.events.map((e) => e.tag)
      expect(eventTags).toEqual(["construct", "reserve", "commit"])
      const commitEv = QuotaMock.events[2]
      if (commitEv.tag !== "commit") throw new Error("unreachable")
      expect(commitEv.used).toBe(7n)
    }),
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
      const commits = QuotaMock.events.filter((e) => e.tag === "commit")
      expect(commits).toHaveLength(1)
      if (commits[0].tag !== "commit") throw new Error("unreachable")
      expect(commits[0].used).toBe(0n)
    }),
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
    }),
  )

  it.effect("manual reserve surfaces failed-reservation typed error", () =>
    Effect.gen(function* () {
      QuotaMock.__setReserveFails({ estimatedWaitNanos: undefined })
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
    }),
  )

  it.effect("manual reserve surfaces non-failed-reservation throws as QuotaHostError", () =>
    Effect.gen(function* () {
      QuotaMock.__setReserveThrows(new Error("host invariant violated"))
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
    }),
  )

  it.effect("split delegates to the host and returns a child token", () =>
    Effect.gen(function* () {
      const parent = yield* Quota.acquireQuotaToken("api-calls", 1000n)
      const child = yield* Quota.split(parent, 300n)
      expect(child).toBeInstanceOf(QuotaToken)
      const splits = QuotaMock.events.filter((e) => e.tag === "split")
      expect(splits).toHaveLength(1)
      if (splits[0].tag !== "split") throw new Error("unreachable")
      expect(splits[0].childExpectedUse).toBe(300n)
    }),
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
    }),
  )

  it.effect("merge delegates to the host", () =>
    Effect.gen(function* () {
      const a = yield* Quota.acquireQuotaToken("api-calls", 500n)
      const b = yield* Quota.acquireQuotaToken("api-calls", 500n)
      yield* Quota.merge(a, b)
      const merges = QuotaMock.events.filter((e) => e.tag === "merge")
      expect(merges).toHaveLength(1)
    }),
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
    }),
  )
})
