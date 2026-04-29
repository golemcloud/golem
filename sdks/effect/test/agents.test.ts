import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Effect, Layer, Exit, Fiber, Stream } from "effect"
import * as Agents from "../src/agents.js"
import { AgentHostClient, AgentHostLive } from "../src/host/AgentHostClient.js"
import { PromiseLive } from "../src/host/PromiseClient.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import { uuidToString } from "./mocks/golem-core-types.js"

/**
 * Layer-based replacement for the deleted `__setX/__resetX`
 * indirection in `src/agents.ts`. Production layers go through the
 * vitest-aliased mock modules, so seeded state in `ApiHostMock` is
 * visible to any effect that resolves these services.
 */
const HostLayer = Layer.mergeAll(AgentHostLive, PromiseLive)

const makeAgentId = (label: string): Agents.AgentId => ({
  componentId: { uuid: { highBits: 0n, lowBits: 1n } },
  agentId: label,
})

beforeEach(() => {
  ApiHostMock.__resetAll()
})
afterEach(() => {
  ApiHostMock.__resetAll()
})

describe("Agents — metadata", () => {
  it.effect("getSelfMetadata returns the host's self metadata", () =>
    Effect.gen(function* () {
      const out = yield* Agents.getSelfMetadata
      expect(out.agentId.agentId).toBe("Test()")
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("getAgentMetadata returns undefined for an unknown agent", () =>
    Effect.gen(function* () {
      const out = yield* Agents.getAgentMetadata(makeAgentId("Other()"))
      expect(out).toBeUndefined()
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("getAgentMetadata returns seeded metadata", () =>
    Effect.gen(function* () {
      const id = makeAgentId("Other()")
      ApiHostMock.__setAgentMetadata(id, {
        agentId: id,
        args: [],
        env: [],
        config: [],
        status: "idle",
        componentRevision: 5n,
        retryCount: 0n,
        environmentId: { uuid: { highBits: 0n, lowBits: 1n } },
      })
      const out = yield* Agents.getAgentMetadata(id)
      expect(out?.componentRevision).toBe(5n)
      expect(out?.status).toBe("idle")
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("wraps host throws as AgentsHostError", () => {
    // Replaces the deleted `__setGetSelfMetadataForTest` indirection
    // with a Layer-level override of `AgentHostClient.getSelfMetadata`
    // that throws on call.
    const ThrowingAgentHost = Layer.succeed(
      AgentHostClient,
      AgentHostClient.of({
        parseAgentId: (() => {
          throw new Error("not used by this test")
        }) as never,
        getSelfMetadata: () => {
          throw new Error("nope")
        },
        createWebhook: (() => {
          throw new Error("not used by this test")
        }) as never,
        getAgentMetadata: (() => {
          throw new Error("not used by this test")
        }) as never,
        updateAgent: (() => {
          throw new Error("not used by this test")
        }) as never,
        forkAgent: (() => {
          throw new Error("not used by this test")
        }) as never,
        revertAgent: (() => {
          throw new Error("not used by this test")
        }) as never,
        fork: (() => {
          throw new Error("not used by this test")
        }) as never,
        resolveComponentId: (() => {
          throw new Error("not used by this test")
        }) as never,
        resolveAgentId: (() => {
          throw new Error("not used by this test")
        }) as never,
        resolveAgentIdStrict: (() => {
          throw new Error("not used by this test")
        }) as never,
        getAgentsCtor: (() => {
          throw new Error("not used by this test")
        }) as never,
      }),
    )
    return Effect.gen(function* () {
      const exit = yield* Effect.exit(Agents.getSelfMetadata)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/AgentsHostError/)
      }
    }).pipe(Effect.provide(ThrowingAgentHost))
  })
})

describe("Agents — lifecycle", () => {
  it.effect("updateAgent forwards target revision and mode", () =>
    Effect.gen(function* () {
      const id = makeAgentId("Counter()")
      yield* Agents.updateAgent({ agentId: id, targetRevision: 7n, mode: "automatic" })
      expect(ApiHostMock.__getUpdateCalls()).toEqual([
        { agentId: id, targetRevision: 7n, mode: "automatic" },
      ])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("updateAgent rejects negative revisions", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Agents.updateAgent({
          agentId: makeAgentId("Counter()"),
          targetRevision: -1,
          mode: "automatic",
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/AgentsValidationError/)
      }
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("forkAgent records the call", () =>
    Effect.gen(function* () {
      const source = makeAgentId("A()")
      const target = makeAgentId("B()")
      yield* Agents.forkAgent({ source, target, oplogIdxCutOff: 42n })
      expect(ApiHostMock.__getForkCalls()).toEqual([{ source, target, oplogIdxCutOff: 42n }])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("revertAgent supports both target variants", () =>
    Effect.gen(function* () {
      const id = makeAgentId("X()")
      yield* Agents.revertAgent(id, Agents.RevertTarget.toOplogIndex(99n))
      const target = yield* Agents.RevertTarget.lastInvocations(3)
      yield* Agents.revertAgent(id, target)

      expect(ApiHostMock.__getRevertCalls()).toEqual([
        { agentId: id, target: { tag: "revert-to-oplog-index", val: 99n } },
        { agentId: id, target: { tag: "revert-last-invocations", val: 3n } },
      ])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("RevertTarget.lastInvocations rejects non-uint64 inputs", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(Agents.RevertTarget.lastInvocations(-1))
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("fork returns the seeded ForkResult", () =>
    Effect.gen(function* () {
      ApiHostMock.__setForkResult({
        tag: "forked",
        val: { forkedPhantomId: { highBits: 0n, lowBits: 99n } },
      })
      const out = yield* Agents.fork
      expect(out.tag).toBe("forked")
      if (out.tag === "forked") {
        expect(out.val.forkedPhantomId.lowBits).toBe(99n)
      }
    }).pipe(Effect.provide(HostLayer)),
  )
})

describe("Agents — resolution helpers", () => {
  it.effect("resolveComponentId returns seeded ids", () =>
    Effect.gen(function* () {
      ApiHostMock.__seedComponentId("acct/proj/comp", { uuid: { highBits: 0n, lowBits: 7n } })
      const out = yield* Agents.resolveComponentId("acct/proj/comp")
      expect(out?.uuid.lowBits).toBe(7n)
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("resolveAgentId returns undefined for unknown refs", () =>
    Effect.gen(function* () {
      const out = yield* Agents.resolveAgentId("missing", "X()")
      expect(out).toBeUndefined()
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("resolveAgentIdStrict mirrors resolveAgentId via separate seed map", () =>
    Effect.gen(function* () {
      const id = makeAgentId("Y()")
      ApiHostMock.__seedStrictAgentId("acct/proj/comp", "Y()", id)
      const out = yield* Agents.resolveAgentIdStrict("acct/proj/comp", "Y()")
      expect(out).toEqual(id)
    }).pipe(Effect.provide(HostLayer)),
  )
})

describe("Agents — Filter DSL", () => {
  it("compiles a single leaf filter", () => {
    const f = Agents.Filter.name("equal", "Counter()")
    const raw = Agents.toRawFilter(f)
    expect(raw.filters).toEqual([
      { filters: [{ tag: "name", val: { comparator: "equal", value: "Counter()" } }] },
    ])
  })

  it("compiles AND chains into a single AllFilter", () => {
    const f = Agents.Filter.name("equal", "X")
      .and(Agents.Filter.status("equal", "running"))
      .and(Agents.Filter.version("greater-equal", 1n))
    const raw = Agents.toRawFilter(f)
    expect(raw.filters.length).toBe(1)
    expect(raw.filters[0].filters.length).toBe(3)
  })

  it("compiles OR chains into multiple AllFilters", () => {
    const f = Agents.Filter.status("equal", "running").or(Agents.Filter.status("equal", "idle"))
    const raw = Agents.toRawFilter(f)
    expect(raw.filters.length).toBe(2)
  })

  it("expands `(A or B) and C` into proper DNF", () => {
    // (name=A or name=B) and status=running
    // -> (name=A and status=running) or (name=B and status=running)
    const f = Agents.Filter.name("equal", "A")
      .or(Agents.Filter.name("equal", "B"))
      .and(Agents.Filter.status("equal", "running"))
    const raw = Agents.toRawFilter(f)
    expect(raw.filters.length).toBe(2)
    for (const conj of raw.filters) {
      expect(conj.filters.length).toBe(2)
      const tags = conj.filters.map((p) => p.tag).sort()
      expect(tags).toEqual(["name", "status"])
    }
  })

  it("expands `A and (B or C)` into proper DNF", () => {
    const f = Agents.Filter.status("equal", "running").and(
      Agents.Filter.name("equal", "A").or(Agents.Filter.name("equal", "B")),
    )
    const raw = Agents.toRawFilter(f)
    expect(raw.filters.length).toBe(2)
    for (const conj of raw.filters) {
      expect(conj.filters.length).toBe(2)
    }
  })

  it("expands `(A or B) and (C or D)` into 4 conjunctions", () => {
    const f = Agents.Filter.name("equal", "A")
      .or(Agents.Filter.name("equal", "B"))
      .and(Agents.Filter.status("equal", "running").or(Agents.Filter.status("equal", "idle")))
    const raw = Agents.toRawFilter(f)
    expect(raw.filters.length).toBe(4)
  })
})

describe("Agents — getAgents stream", () => {
  it.effect("collects paged metadata in order", () =>
    Effect.gen(function* () {
      const componentId: Agents.ComponentId = { uuid: { highBits: 0n, lowBits: 7n } }
      const a: Agents.AgentMetadata = {
        agentId: makeAgentId("A()"),
        args: [],
        env: [],
        config: [],
        status: "running",
        componentRevision: 0n,
        retryCount: 0n,
        environmentId: { uuid: { highBits: 0n, lowBits: 1n } },
      }
      const b: Agents.AgentMetadata = {
        ...a,
        agentId: makeAgentId("B()"),
      }
      const c: Agents.AgentMetadata = {
        ...a,
        agentId: makeAgentId("C()"),
      }
      ApiHostMock.__seedAgentsForComponent(componentId, [[a, b], [c]])

      const out = yield* Stream.runCollect(Agents.getAgents({ componentId }))
      expect(out.map((m) => m.agentId.agentId)).toEqual(["A()", "B()", "C()"])
    }).pipe(Effect.provide(HostLayer)),
  )
})

describe("Agents — Promises", () => {
  it.effect("create + complete + poll returns the payload", () =>
    Effect.gen(function* () {
      const id = yield* Agents.Promises.create
      const polled1 = yield* Agents.Promises.poll(id)
      expect(polled1).toBeUndefined()

      ApiHostMock.completePromise(id, new Uint8Array([1, 2, 3]))

      const polled2 = yield* Agents.Promises.poll(id)
      expect(Array.from(polled2!)).toEqual([1, 2, 3])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("complete fails with PromiseAlreadyCompletedError on a second call", () =>
    Effect.gen(function* () {
      const id = yield* Agents.Promises.create
      yield* Agents.Promises.complete(id, new Uint8Array([0]))
      const exit = yield* Effect.exit(Agents.Promises.complete(id, new Uint8Array([1])))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        const failure = exit.cause
        expect(String((failure as { failure?: unknown }).failure ?? failure)).toMatch(
          /PromiseAlreadyCompletedError/,
        )
      }
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("await resolves once the promise is completed", () =>
    Effect.gen(function* () {
      const id = yield* Agents.Promises.create
      // Race: complete in a tick.
      queueMicrotask(() => {
        ApiHostMock.completePromise(id, new Uint8Array([7]))
      })
      const payload = yield* Agents.Promises.await(id)
      expect(Array.from(payload)).toEqual([7])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("await returns immediately when already completed", () =>
    Effect.gen(function* () {
      const id = yield* Agents.Promises.create
      ApiHostMock.completePromise(id, new Uint8Array([42]))
      const payload = yield* Agents.Promises.await(id)
      expect(Array.from(payload)).toEqual([42])
    }).pipe(Effect.provide(HostLayer)),
  )

  it.live("await is interruptible: fiber-interrupt unparks the abortable promise", () =>
    Effect.gen(function* () {
      const id = yield* Agents.Promises.create
      // The promise is never completed; the awaiting fiber is interrupted.
      // Without `pollable.abortablePromise(signal)` this would hang the test.
      const fiber = yield* Effect.forkChild(Agents.Promises.await(id))
      yield* Effect.sleep("1 millis")
      yield* Fiber.interrupt(fiber)
      const exit = yield* Fiber.await(fiber)
      expect(Exit.isFailure(exit)).toBe(true)
      if (!Exit.isFailure(exit)) return
      expect(Cause.hasInterrupts(exit.cause)).toBe(true)
    }).pipe(Effect.provide(HostLayer)),
  )

  // Defensive sanity check: keep the unused import alive so a future refactor
  // removing it doesn't go unnoticed.
  it("agent ids are stringifiable for diagnostics", () => {
    const id = makeAgentId("X()")
    expect(uuidToString(id.componentId.uuid)).toMatch(/^[0-9a-f]{8}-/)
  })
})
