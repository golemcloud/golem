import { afterEach, beforeEach, describe, it, expect } from "vitest"
import { Effect, Exit, Fiber, Schema } from "effect"
import * as Quota from "../src/quota.js"
import { QuotaToken as QuotaTokenSchema, QuotaTokenRecord } from "../src/quota.js"
import { toWitCodec } from "../src/wit-codec.js"
import { QuotaToken } from "golem:quota/types@1.5.0"
import * as QuotaMock from "./mocks/golem-quota-types.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

const sample = {
  environmentId: { uuid: { highBits: 1n, lowBits: 2n } },
  resourceName: "cpu",
  expectedUse: 100n,
  lastCredit: 50n,
  lastCreditAt: { seconds: 1_700_000_000n, nanoseconds: 123 },
}

describe("QuotaToken schema", () => {
  it("encodes a QuotaToken instance to its record shape", async () => {
    const token = QuotaToken.fromRecord(sample)
    const enc = await Effect.runPromise(Schema.encodeEffect(QuotaTokenSchema)(token))
    expect(enc).toEqual(sample)
  })

  it("decodes a record into a QuotaToken instance", async () => {
    const token = await Effect.runPromise(Schema.decodeEffect(QuotaTokenSchema)(sample))
    expect(token).toBeInstanceOf(QuotaToken)
    expect(token.toRecord()).toEqual(sample)
  })

  it("compiles to a WIT record with the expected field types", async () => {
    const wc = await Effect.runPromise(toWitCodec(QuotaTokenSchema as any))
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
  })

  it("round-trips QuotaToken end-to-end through the WIT codec", async () => {
    const wc = await Effect.runPromise(toWitCodec(QuotaTokenSchema as any))
    const codec = wc.codec as Schema.Codec<QuotaToken, any, never, never>
    const token = QuotaToken.fromRecord(sample)
    const wv = await Effect.runPromise(Schema.encodeEffect(codec)(token))
    const back = await Effect.runPromise(Schema.decodeEffect(codec)(wv))
    expect(back).toBeInstanceOf(QuotaToken)
    expect(back.toRecord()).toEqual(sample)
  })

  it("QuotaTokenRecord schema matches the host record shape", async () => {
    const rec = await Effect.runPromise(Schema.decodeEffect(QuotaTokenRecord)(sample))
    expect(rec).toEqual(sample)
  })
})

describe("Quota — operational API", () => {
  beforeEach(() => {
    QuotaMock.__reset()
  })
  afterEach(() => {
    QuotaMock.__reset()
  })

  it("acquireQuotaToken constructs a QuotaToken via the host class", async () => {
    const token = await runP(Quota.acquireQuotaToken("api-calls", 100n))
    expect(token).toBeInstanceOf(QuotaToken)
    expect(QuotaMock.events).toEqual([
      { tag: "construct", resourceName: "api-calls", expectedUse: 100n },
    ])
  })

  it("acquireQuotaToken surfaces host failures as QuotaHostError", async () => {
    Quota.__setAcquireQuotaTokenForTest(() => {
      throw new Error("manifest does not declare resource")
    })
    try {
      const exit = await runExit(Quota.acquireQuotaToken("missing", 1n))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
      }
    } finally {
      Quota.__resetAcquireQuotaTokenForTest()
    }
  })

  it("withReservation reserves, runs body, and commits the body's used", async () => {
    const value = await runP(
      Effect.gen(function* () {
        const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
        return yield* Quota.withReservation(token, 100n, () =>
          Effect.succeed({ used: 42n, value: "ok" }),
        )
      }),
    )
    expect(value).toBe("ok")

    const eventTags = QuotaMock.events.map((e) => e.tag)
    expect(eventTags).toEqual(["construct", "reserve", "commit"])
    const commitEv = QuotaMock.events[2]
    if (commitEv.tag !== "commit") throw new Error("unreachable")
    expect(commitEv.used).toBe(42n)
    expect(commitEv.reservedAmount).toBe(100n)
  })

  it("withReservation commits 0 on body failure (drop semantics)", async () => {
    class Boom {
      readonly _tag = "Boom"
    }
    const exit = await runExit(
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
  })

  it("withReservation commits 0 on body interruption", async () => {
    let bodyEntered = false
    const token = await runP(Quota.acquireQuotaToken("api-calls", 1n))
    const fiber = Effect.runFork(
      Quota.withReservation(token, 50n, () =>
        Effect.gen(function* () {
          bodyEntered = true
          yield* Effect.never
          return { used: 999n, value: "unreachable" }
        }),
      ),
    )
    // Wait for the body to be entered, then interrupt.
    await new Promise((r) => setTimeout(r, 10))
    expect(bodyEntered).toBe(true)
    await runP(Fiber.interrupt(fiber))

    const commitEv = QuotaMock.events.find((e) => e.tag === "commit")
    expect(commitEv).toBeDefined()
    if (commitEv?.tag !== "commit") throw new Error("unreachable")
    expect(commitEv.used).toBe(0n)
  })

  it("withReservation surfaces failed-reservation as FailedReservationError", async () => {
    QuotaMock.__setReserveFails({ estimatedWaitNanos: 5000n })
    const exit = await runExit(
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
  })

  it("manual reserve + commit emits the expected event sequence", async () => {
    await runP(
      Effect.scoped(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          const reservation = yield* Quota.reserve(token, 10n)
          yield* Quota.commit(reservation, 7n)
        }),
      ),
    )
    const eventTags = QuotaMock.events.map((e) => e.tag)
    expect(eventTags).toEqual(["construct", "reserve", "commit"])
    const commitEv = QuotaMock.events[2]
    if (commitEv.tag !== "commit") throw new Error("unreachable")
    expect(commitEv.used).toBe(7n)
  })

  it("reserve auto-commits 0 on scope close when not explicitly committed", async () => {
    await runP(
      Effect.scoped(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("api-calls", 1n)
          yield* Quota.reserve(token, 10n)
          // No explicit commit — scope close fires the finalizer.
        }),
      ),
    )
    const commits = QuotaMock.events.filter((e) => e.tag === "commit")
    expect(commits).toHaveLength(1)
    if (commits[0].tag !== "commit") throw new Error("unreachable")
    expect(commits[0].used).toBe(0n)
  })

  it("commit twice on the same reservation fails with QuotaHostError", async () => {
    const exit = await runExit(
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
  })

  it("manual reserve surfaces failed-reservation typed error", async () => {
    QuotaMock.__setReserveFails({ estimatedWaitNanos: undefined })
    const exit = await runExit(
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
  })

  it("manual reserve surfaces non-failed-reservation throws as QuotaHostError", async () => {
    QuotaMock.__setReserveThrows(new Error("host invariant violated"))
    const exit = await runExit(
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
  })

  it("split delegates to the host and returns a child token", async () => {
    const child = await runP(
      Effect.gen(function* () {
        const parent = yield* Quota.acquireQuotaToken("api-calls", 1000n)
        return yield* Quota.split(parent, 300n)
      }),
    )
    expect(child).toBeInstanceOf(QuotaToken)
    const splits = QuotaMock.events.filter((e) => e.tag === "split")
    expect(splits).toHaveLength(1)
    if (splits[0].tag !== "split") throw new Error("unreachable")
    expect(splits[0].childExpectedUse).toBe(300n)
  })

  it("split overflow surfaces as QuotaHostError", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        const parent = yield* Quota.acquireQuotaToken("api-calls", 100n)
        return yield* Quota.split(parent, 999n)
      }),
    )
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      expect(JSON.stringify(exit.cause)).toMatch(/QuotaHostError/)
    }
  })

  it("merge delegates to the host", async () => {
    await runP(
      Effect.gen(function* () {
        const a = yield* Quota.acquireQuotaToken("api-calls", 500n)
        const b = yield* Quota.acquireQuotaToken("api-calls", 500n)
        yield* Quota.merge(a, b)
      }),
    )
    const merges = QuotaMock.events.filter((e) => e.tag === "merge")
    expect(merges).toHaveLength(1)
  })

  it("merge of mismatched resources surfaces as QuotaHostError", async () => {
    const exit = await runExit(
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
  })
})
