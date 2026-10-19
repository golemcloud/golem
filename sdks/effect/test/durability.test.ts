import { beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Layer } from "effect"
import * as Durability from "../src/Durability.js"
import { DurabilityModeClient, DurabilityModeLive } from "../src/host/DurabilityModeClient.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

beforeEach(() => ApiHostMock.__resetAll())

describe("Durability — oplog commit", () => {
  it.effect("forwards the requested replica count", () =>
    Effect.gen(function* () {
      yield* Durability.oplogCommit(3)
      expect(ApiHostMock.__getOplogCommits()).toEqual([3])
    }).pipe(Effect.provide(DurabilityModeLive)),
  )

  it.effect("rejects invalid replica counts", () =>
    Effect.gen(function* () {
      for (const replicas of [-1, 256, 1.5]) {
        const exit = yield* Effect.exit(Durability.oplogCommit(replicas))
        expect(Exit.isFailure(exit)).toBe(true)
        if (Exit.isFailure(exit))
          expect(JSON.stringify(exit.cause)).toMatch(/DurabilityValidationError/)
      }
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

  it.effect("wraps host throws as DurabilityHostError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Durability.generateIdempotencyKey)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) expect(JSON.stringify(exit.cause)).toMatch(/DurabilityHostError/)
    }).pipe(
      Effect.provide(
        Layer.succeed(DurabilityModeClient, {
          oplogCommit: () => {},
          generateIdempotencyKey: () => {
            throw new Error("boom")
          },
        }),
      ),
    ),
  )
})
