import { Effect, Exit } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Durability from "../src/durability.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

beforeEach(() => {
  ApiHostMock.__resetAll()
})
afterEach(() => {
  ApiHostMock.__resetAll()
})

describe("Durability — persistence level", () => {
  it("get returns the current host value", async () => {
    const out = await runP(Durability.getPersistenceLevel)
    expect(out).toEqual({ tag: "smart" })
  })

  it("set persists into host state", async () => {
    await runP(Durability.setPersistenceLevel(Durability.PersistenceLevel.persistNothing))
    expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "persist-nothing" })
  })

  it("withPersistenceLevel restores the previous value on success", async () => {
    let inside: Durability.PersistenceLevelValue | undefined
    await runP(
      Durability.withPersistenceLevel(
        Durability.PersistenceLevel.persistNothing,
        Effect.sync(() => {
          inside = ApiHostMock.getOplogPersistenceLevel()
        }),
      ),
    )
    expect(inside).toEqual({ tag: "persist-nothing" })
    expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "smart" })
  })

  it("withPersistenceLevel restores on failure too", async () => {
    const exit = await runExit(
      Durability.withPersistenceLevel(
        Durability.PersistenceLevel.persistRemoteSideEffects,
        Effect.fail("boom" as const),
      ),
    )
    expect(Exit.isFailure(exit)).toBe(true)
    expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "smart" })
  })

  it("wraps host throws as DurabilityHostError", async () => {
    Durability.__setSetOplogPersistenceLevelForTest(() => {
      throw new Error("nope")
    })
    try {
      const exit = await runExit(Durability.setPersistenceLevel(Durability.PersistenceLevel.smart))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/DurabilityHostError/)
      }
    } finally {
      Durability.__resetSetOplogPersistenceLevelForTest()
    }
  })
})

describe("Durability — idempotence mode", () => {
  it("get/set round-trip", async () => {
    expect(await runP(Durability.getIdempotenceMode)).toBe(true)
    await runP(Durability.setIdempotenceMode(false))
    expect(await runP(Durability.getIdempotenceMode)).toBe(false)
  })

  it("withIdempotenceMode restores previous value on interrupt", async () => {
    const exit = await runExit(Durability.withIdempotenceMode(false, Effect.interrupt))
    expect(Exit.isFailure(exit)).toBe(true)
    expect(ApiHostMock.getIdempotenceMode()).toBe(true)
  })
})

describe("Durability — atomic region", () => {
  it("atomically wraps the body in begin/end markers", async () => {
    let observed: ReadonlyArray<bigint> = []
    await runP(
      Durability.atomically(
        Effect.sync(() => {
          observed = ApiHostMock.__getAtomicMarks()
        }),
      ),
    )
    expect(observed.length).toBe(1)
    // After scope close the mark must be gone.
    expect(ApiHostMock.__getAtomicMarks()).toEqual([])
  })

  it("clears the mark on failure", async () => {
    const exit = await runExit(Durability.atomically(Effect.fail("boom" as const)))
    expect(Exit.isFailure(exit)).toBe(true)
    expect(ApiHostMock.__getAtomicMarks()).toEqual([])
  })

  it("supports manual begin/end", async () => {
    const begin = await runP(Durability.beginOperation)
    expect(typeof begin).toBe("bigint")
    expect(ApiHostMock.__getAtomicMarks()).toContain(begin)
    await runP(Durability.endOperation(begin))
    expect(ApiHostMock.__getAtomicMarks()).not.toContain(begin)
  })
})

describe("Durability — oplog commit", () => {
  it("forwards the requested replica count", async () => {
    await runP(Durability.oplogCommit(3))
    expect(ApiHostMock.__getOplogCommits()).toEqual([3])
  })

  it("rejects out-of-range replica counts", async () => {
    const exit = await runExit(Durability.oplogCommit(-1))
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      expect(JSON.stringify(exit.cause)).toMatch(/DurabilityValidationError/)
    }
  })

  it("rejects non-integer replica counts", async () => {
    const exit = await runExit(Durability.oplogCommit(1.5))
    expect(Exit.isFailure(exit)).toBe(true)
  })
})

describe("Durability — idempotency key", () => {
  it("returns sequential UUIDs from the mock", async () => {
    const a = await runP(Durability.generateIdempotencyKey)
    const b = await runP(Durability.generateIdempotencyKey)
    expect(a.lowBits).toBe(1n)
    expect(b.lowBits).toBe(2n)
  })

  it("wraps host throws as DurabilityHostError", async () => {
    Durability.__setGenerateIdempotencyKeyForTest(() => {
      throw new Error("boom")
    })
    try {
      const exit = await runExit(Durability.generateIdempotencyKey)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/DurabilityHostError/)
      }
    } finally {
      Durability.__resetGenerateIdempotencyKeyForTest()
    }
  })
})
