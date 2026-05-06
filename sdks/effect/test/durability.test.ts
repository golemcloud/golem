import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Layer } from "effect"
import * as Durability from "../src/Durability.js"
import { AgentHostLive } from "../src/host/AgentHostClient.js"
import { DurabilityModeClient, DurabilityModeLive } from "../src/host/DurabilityModeClient.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

/**
 * Combined live layer for tests that exercise `Durability.atomically`
 * (which calls `golem:api/host.trap` on failure via `AgentHostClient`).
 */
const AtomicallyLive = Layer.merge(DurabilityModeLive, AgentHostLive)

beforeEach(() => {
  ApiHostMock.__resetAll()
})
afterEach(() => {
  ApiHostMock.__resetAll()
})

describe("Durability — persistence level", () => {
  it.effect("get returns the current host value", () =>
    Effect.gen(function* () {
      const out = yield* Durability.getPersistenceLevel
      expect(out).toEqual({ tag: "smart" })
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("set persists into host state", () =>
    Effect.gen(function* () {
      yield* Durability.setPersistenceLevel(Durability.PersistenceLevel.persistNothing)
      expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "persist-nothing" })
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("withPersistenceLevel restores the previous value on success", () =>
    Effect.gen(function* () {
      let inside: Durability.PersistenceLevelValue | undefined
      yield* Durability.withPersistenceLevel(
        Durability.PersistenceLevel.persistNothing,
        Effect.sync(() => {
          inside = ApiHostMock.getOplogPersistenceLevel()
        }),
      )
      expect(inside).toEqual({ tag: "persist-nothing" })
      expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "smart" })
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("withPersistenceLevel restores on failure too", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Durability.withPersistenceLevel(
          Durability.PersistenceLevel.persistRemoteSideEffects,
          Effect.fail("boom" as const),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "smart" })
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("wraps host throws as DurabilityHostError", () => {
    const live = DurabilityModeClient.of({
      getOplogPersistenceLevel: () => ApiHostMock.getOplogPersistenceLevel(),
      setOplogPersistenceLevel: () => {
        throw new Error("nope")
      },
      getIdempotenceMode: () => ApiHostMock.getIdempotenceMode(),
      setIdempotenceMode: (v) => ApiHostMock.setIdempotenceMode(v),
      markBeginOperation: () => ApiHostMock.markBeginOperation(),
      markEndOperation: (b) => ApiHostMock.markEndOperation(b),
      oplogCommit: (n) => ApiHostMock.oplogCommit(n),
      generateIdempotencyKey: () => ApiHostMock.generateIdempotencyKey(),
    })
    return Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Durability.setPersistenceLevel(Durability.PersistenceLevel.smart),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/DurabilityHostError/)
      }
    }).pipe(Effect.provide(Layer.succeed(DurabilityModeClient, live)))
  })
})

describe("Durability — idempotence mode", () => {
  it.effect("get/set round-trip", () =>
    Effect.gen(function* () {
      expect(yield* Durability.getIdempotenceMode).toBe(true)
      yield* Durability.setIdempotenceMode(false)
      expect(yield* Durability.getIdempotenceMode).toBe(false)
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("withIdempotenceMode restores previous value on interrupt", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.withIdempotenceMode(false, Effect.interrupt))
      expect(Exit.isFailure(exit)).toBe(true)
      expect(ApiHostMock.getIdempotenceMode()).toBe(true)
    }).pipe(Effect.provide(DurabilityModeLive)),
  )
})

describe("Durability — atomic region", () => {
  it.effect("atomically wraps the body in begin/end markers", () =>
    Effect.gen(function* () {
      let observed: ReadonlyArray<bigint> = []
      yield* Durability.atomically(
        Effect.sync(() => {
          observed = ApiHostMock.__getAtomicMarks()
        }),
      )
      expect(observed.length).toBe(1)
      // After scope close the mark must be gone (success path calls
      // mark-end-operation).
      expect(ApiHostMock.__getAtomicMarks()).toEqual([])
    }).pipe(Effect.provide(AtomicallyLive)),
  )

  it.effect("calls trap and leaves the mark open on failure", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.atomically(Effect.fail("boom" as const)))
      expect(Exit.isFailure(exit)).toBe(true)
      // The atomic region is intentionally left OPEN on failure so the
      // host's replay-time recovery can roll back partial side effects
      // and re-run the block — mirrors Rust's `AtomicOperationGuard`
      // skipping `mark-end-operation` when panicking.
      expect(ApiHostMock.__getAtomicMarks().length).toBe(1)
      // ...and `golem:api/host.trap` was called with a reason naming
      // the failure.
      const traps = ApiHostMock.__getTrapReasons()
      expect(traps.length).toBe(1)
      expect(traps[0]).toMatch(/^atomic block failed:/)
    }).pipe(Effect.provide(AtomicallyLive)),
  )

  it.effect("calls trap on defect failures too", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.atomically(Effect.die("kaboom")))
      expect(Exit.isFailure(exit)).toBe(true)
      expect(ApiHostMock.__getAtomicMarks().length).toBe(1)
      expect(ApiHostMock.__getTrapReasons().length).toBe(1)
    }).pipe(Effect.provide(AtomicallyLive)),
  )

  it.effect("calls trap on interrupt too", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.atomically(Effect.interrupt))
      expect(Exit.isFailure(exit)).toBe(true)
      expect(ApiHostMock.__getAtomicMarks().length).toBe(1)
      expect(ApiHostMock.__getTrapReasons().length).toBe(1)
    }).pipe(Effect.provide(AtomicallyLive)),
  )

  it.effect("supports manual begin/end", () =>
    Effect.gen(function* () {
      const begin = yield* Durability.beginOperation
      expect(typeof begin).toBe("bigint")
      expect(ApiHostMock.__getAtomicMarks()).toContain(begin)
      yield* Durability.endOperation(begin)
      expect(ApiHostMock.__getAtomicMarks()).not.toContain(begin)
    }).pipe(Effect.provide(DurabilityModeLive)),
  )
})

describe("Durability — oplog commit", () => {
  it.effect("forwards the requested replica count", () =>
    Effect.gen(function* () {
      yield* Durability.oplogCommit(3)
      expect(ApiHostMock.__getOplogCommits()).toEqual([3])
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("rejects out-of-range replica counts", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.oplogCommit(-1))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/DurabilityValidationError/)
      }
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("rejects non-integer replica counts", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.oplogCommit(1.5))
      expect(Exit.isFailure(exit)).toBe(true)
    }).pipe(Effect.provide(DurabilityModeLive)),
  )
})

describe("Durability — idempotency key", () => {
  it.effect("returns sequential UUIDs from the mock", () =>
    Effect.gen(function* () {
      const a = yield* Durability.generateIdempotencyKey
      const b = yield* Durability.generateIdempotencyKey
      expect(a.lowBits).toBe(1n)
      expect(b.lowBits).toBe(2n)
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("wraps host throws as DurabilityHostError", () => {
    const live = DurabilityModeClient.of({
      getOplogPersistenceLevel: () => ApiHostMock.getOplogPersistenceLevel(),
      setOplogPersistenceLevel: (v) => ApiHostMock.setOplogPersistenceLevel(v),
      getIdempotenceMode: () => ApiHostMock.getIdempotenceMode(),
      setIdempotenceMode: (v) => ApiHostMock.setIdempotenceMode(v),
      markBeginOperation: () => ApiHostMock.markBeginOperation(),
      markEndOperation: (b) => ApiHostMock.markEndOperation(b),
      oplogCommit: (n) => ApiHostMock.oplogCommit(n),
      generateIdempotencyKey: () => {
        throw new Error("boom")
      },
    })
    return Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.generateIdempotencyKey)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/DurabilityHostError/)
      }
    }).pipe(Effect.provide(Layer.succeed(DurabilityModeClient, live)))
  })
})
