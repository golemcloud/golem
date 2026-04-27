import { Cause, Effect, Fiber } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Agents from "../src/agents.js"
import * as Durability from "../src/durability.js"
import { SelfAgentId } from "../src/self-agent-id.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)

const self: Agents.AgentId = {
  componentId: { uuid: { highBits: 0n, lowBits: 1n } },
  agentId: 'Counter("x")',
}

const provideSelf = <A, E, R>(
  eff: Effect.Effect<A, E, R | SelfAgentId>,
): Effect.Effect<A, E, Exclude<R, SelfAgentId>> =>
  Effect.provideService(eff, SelfAgentId, self) as Effect.Effect<A, E, Exclude<R, SelfAgentId>>

beforeEach(() => {
  ApiHostMock.__resetAll()
})
afterEach(() => {
  ApiHostMock.__resetAll()
})

describe("Durability.checkpoint", () => {
  it("returns ok on success without reverting", async () => {
    const program = Durability.checkpoint(Effect.succeed(42))
    const out = await runP(
      provideSelf(program) as Effect.Effect<
        Durability.CheckpointResult<number, never>,
        never,
        never
      >,
    )
    expect(out._tag).toBe("ok")
    if (out._tag === "ok") expect(out.value).toBe(42)
    expect(ApiHostMock.__getRevertCalls()).toEqual([])
  })

  it("issues a self-revert with the captured oplog index on failure", async () => {
    // Bump the oplog index to something distinct before checkpoint runs.
    ApiHostMock.__setOplogIndex(99n)
    const program = Durability.checkpoint(Effect.fail("boom" as const))
    const out = await runP(
      provideSelf(program) as Effect.Effect<
        Durability.CheckpointResult<never, "boom">,
        never,
        never
      >,
    )
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
  })
})

describe("Durability.unwrapOrRevert", () => {
  it("returns the value on success", async () => {
    const program = Durability.unwrapOrRevert(Effect.succeed("ok"))
    const out = await runP(provideSelf(program) as Effect.Effect<string, never, never>)
    expect(out).toBe("ok")
    expect(ApiHostMock.__getRevertCalls()).toEqual([])
  })

  it("issues a revert and never returns on body failure", async () => {
    // Body fails; unwrapOrRevert calls revertAgent then Effect.never.
    // We run the program in a fiber so we can inspect the host state
    // without waiting for it to terminate.
    const program = Durability.unwrapOrRevert(Effect.fail("boom" as const))
    const fiber = Effect.runFork(provideSelf(program) as Effect.Effect<never, never, never>)
    // Yield once to let the body run + revertAgent fire.
    await new Promise<void>((r) => setTimeout(r, 5))
    expect(ApiHostMock.__getRevertCalls().length).toBe(1)
    // Tear the fiber down so vitest doesn't hang.
    await runP(Fiber.interrupt(fiber))
  })
})

describe("Durability.checkpoint — defects bypass revert", () => {
  it("propagates defects without reverting", async () => {
    const program = Durability.checkpoint(Effect.die("kaboom" as const))
    const exit = await Effect.runPromiseExit(
      provideSelf(program) as Effect.Effect<
        Durability.CheckpointResult<never, never>,
        never,
        never
      >,
    )
    expect(exit._tag).toBe("Failure")
    expect(ApiHostMock.__getRevertCalls()).toEqual([])
  })
})

describe("Durability.compensable", () => {
  it("returns the body's value on success and skips compensate", async () => {
    let compensated = false
    const program = Durability.compensable({
      acquire: Effect.succeed("token"),
      body: (a) => Effect.succeed(a.length),
      compensate: () =>
        Effect.sync(() => {
          compensated = true
        }),
    })
    const out = await runP(provideSelf(program) as Effect.Effect<number, never, never>)
    expect(out).toBe(5)
    expect(compensated).toBe(false)
    expect(ApiHostMock.__getRevertCalls()).toEqual([])
  })

  it("runs compensate and revertAgent on body failure", async () => {
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
    await new Promise<void>((r) => setTimeout(r, 5))
    expect(compensated).toBe(true)
    expect(ApiHostMock.__getRevertCalls().length).toBe(1)
    await runP(Fiber.interrupt(fiber))
  })
})
