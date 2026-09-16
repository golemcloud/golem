import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Effect, Fiber, Layer } from "effect"
import * as Agents from "../src/Agents.js"
import * as Durability from "../src/Durability.js"
import { AgentHostLive } from "../src/host/AgentHostClient.js"
import { DurabilityModeLive } from "../src/host/DurabilityModeClient.js"
import { OplogLive } from "../src/host/OplogClient.js"
import { SelfAgentId } from "../src/SelfAgentId.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

const self: Agents.AgentId = {
  componentId: { uuid: { highBits: 0n, lowBits: 1n } },
  agentId: 'Counter("x")',
}

const hostLayer = Layer.mergeAll(OplogLive, DurabilityModeLive, AgentHostLive)

const provideSelf = <A, E, R>(eff: Effect.Effect<A, E, R>): Effect.Effect<A, E, never> =>
  Effect.provide(
    Effect.provideService(eff as Effect.Effect<A, E, R | SelfAgentId>, SelfAgentId, self),
    hostLayer,
  ) as Effect.Effect<A, E, never>

beforeEach(() => {
  ApiHostMock.__resetAll()
})
afterEach(() => {
  ApiHostMock.__resetAll()
})

describe("Durability.checkpoint", () => {
  it.effect("returns ok on success without reverting", () =>
    Effect.gen(function* () {
      const program = Durability.checkpoint(Effect.succeed(42))
      const out = yield* provideSelf(program)
      expect(out._tag).toBe("ok")
      if (out._tag === "ok") expect(out.value).toBe(42)
      expect(ApiHostMock.__getRevertCalls()).toEqual([])
    }),
  )

  it.effect("issues a self-revert with the captured oplog index on failure", () =>
    Effect.gen(function* () {
      // Bump the oplog index to something distinct before checkpoint runs.
      ApiHostMock.__setOplogIndex(99n)
      const program = Durability.checkpoint(Effect.fail("boom" as const))
      const out = yield* provideSelf(program)
      expect(out._tag).toBe("reverted")
      if (out._tag === "reverted") {
        const fail = out.cause.reasons.find(Cause.isFailReason)
        expect(fail?.error).toBe("boom")
      }
      const calls = ApiHostMock.__getRevertCalls()
      expect(calls.length).toBe(1)
      expect(calls[0]!.agentId).toEqual(self)
      expect(calls[0]!.target.tag).toBe("revert-to-oplog-index")
      if (calls[0]!.target.tag === "revert-to-oplog-index") {
        // currentIndex bumps the counter, so the captured index is 100n.
        expect(calls[0]!.target.val).toBe(100n)
      }
    }),
  )
})

describe("Durability.unwrapOrRevert", () => {
  it.effect("returns the value on success", () =>
    Effect.gen(function* () {
      const program = Durability.unwrapOrRevert(Effect.succeed("ok"))
      const out = yield* provideSelf(program)
      expect(out).toBe("ok")
      expect(ApiHostMock.__getRevertCalls()).toEqual([])
    }),
  )

  it.effect("issues a revert and never returns on body failure", () =>
    Effect.gen(function* () {
      // Body fails; unwrapOrRevert calls revertAgent then Effect.never.
      // We run the program in a fiber so we can inspect the host state
      // without waiting for it to terminate.
      const program = Durability.unwrapOrRevert(Effect.fail("boom" as const))
      const fiber = Effect.runFork(provideSelf(program) as Effect.Effect<never, never, never>)
      // Yield once to let the body run + revertAgent fire.
      yield* Effect.promise(() => new Promise<void>((r) => setTimeout(r, 5)))
      expect(ApiHostMock.__getRevertCalls().length).toBe(1)
      // Tear the fiber down so vitest doesn't hang.
      yield* Fiber.interrupt(fiber)
    }),
  )
})

describe("Durability.checkpoint — defects bypass revert", () => {
  it.effect("propagates defects without reverting", () =>
    Effect.gen(function* () {
      const program = Durability.checkpoint(Effect.die("kaboom" as const))
      const exit = yield* Effect.exit(provideSelf(program))
      expect(exit._tag).toBe("Failure")
      expect(ApiHostMock.__getRevertCalls()).toEqual([])
    }),
  )
})

describe("Durability.compensable", () => {
  it.effect("returns the body's value on success and skips compensate", () =>
    Effect.gen(function* () {
      let compensated = false
      const program = Durability.compensable({
        acquire: Effect.succeed("token"),
        body: (a) => Effect.succeed(a.length),
        compensate: () =>
          Effect.sync(() => {
            compensated = true
          }),
      })
      const out = yield* provideSelf(program)
      expect(out).toBe(5)
      expect(compensated).toBe(false)
      expect(ApiHostMock.__getRevertCalls()).toEqual([])
    }),
  )

  it.effect("runs compensate and revertAgent on body failure", () =>
    Effect.gen(function* () {
      let compensated = false
      const program = Durability.compensable({
        acquire: Effect.succeed("token"),
        body: () => Effect.fail("boom" as const),
        compensate: () =>
          Effect.sync(() => {
            compensated = true
          }),
      })
      const fiber = Effect.runFork(provideSelf(program) as Effect.Effect<never, never, never>)
      yield* Effect.promise(() => new Promise<void>((r) => setTimeout(r, 5)))
      expect(compensated).toBe(true)
      expect(ApiHostMock.__getRevertCalls().length).toBe(1)
      yield* Fiber.interrupt(fiber)
    }),
  )
})
