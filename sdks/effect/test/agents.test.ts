import { Cause, Effect, Exit, Fiber, Stream } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Agents from "../src/agents.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import { uuidToString } from "./mocks/golem-core-types.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

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
  it("getSelfMetadata returns the host's self metadata", async () => {
    const out = await runP(Agents.getSelfMetadata)
    expect(out.agentId.agentId).toBe("Test()")
  })

  it("getAgentMetadata returns undefined for an unknown agent", async () => {
    const out = await runP(Agents.getAgentMetadata(makeAgentId("Other()")))
    expect(out).toBeUndefined()
  })

  it("getAgentMetadata returns seeded metadata", async () => {
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
    const out = await runP(Agents.getAgentMetadata(id))
    expect(out?.componentRevision).toBe(5n)
    expect(out?.status).toBe("idle")
  })

  it("wraps host throws as AgentsHostError", async () => {
    Agents.__setGetSelfMetadataForTest(() => {
      throw new Error("nope")
    })
    try {
      const exit = await runExit(Agents.getSelfMetadata)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/AgentsHostError/)
      }
    } finally {
      Agents.__resetGetSelfMetadataForTest()
    }
  })
})

describe("Agents — lifecycle", () => {
  it("updateAgent forwards target revision and mode", async () => {
    const id = makeAgentId("Counter()")
    await runP(Agents.updateAgent({ agentId: id, targetRevision: 7n, mode: "automatic" }))
    expect(ApiHostMock.__getUpdateCalls()).toEqual([
      { agentId: id, targetRevision: 7n, mode: "automatic" },
    ])
  })

  it("updateAgent rejects negative revisions", async () => {
    const exit = await runExit(
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
  })

  it("forkAgent records the call", async () => {
    const source = makeAgentId("A()")
    const target = makeAgentId("B()")
    await runP(Agents.forkAgent({ source, target, oplogIdxCutOff: 42n }))
    expect(ApiHostMock.__getForkCalls()).toEqual([{ source, target, oplogIdxCutOff: 42n }])
  })

  it("revertAgent supports both target variants", async () => {
    const id = makeAgentId("X()")
    await runP(Agents.revertAgent(id, Agents.RevertTarget.toOplogIndex(99n)))
    const target = await runP(Agents.RevertTarget.lastInvocations(3))
    await runP(Agents.revertAgent(id, target))

    expect(ApiHostMock.__getRevertCalls()).toEqual([
      { agentId: id, target: { tag: "revert-to-oplog-index", val: 99n } },
      { agentId: id, target: { tag: "revert-last-invocations", val: 3n } },
    ])
  })

  it("RevertTarget.lastInvocations rejects non-uint64 inputs", async () => {
    const exit = await runExit(Agents.RevertTarget.lastInvocations(-1))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("fork returns the seeded ForkResult", async () => {
    ApiHostMock.__setForkResult({
      tag: "forked",
      val: { forkedPhantomId: { highBits: 0n, lowBits: 99n } },
    })
    const out = await runP(Agents.fork)
    expect(out.tag).toBe("forked")
    if (out.tag === "forked") {
      expect(out.val.forkedPhantomId.lowBits).toBe(99n)
    }
  })
})

describe("Agents — resolution helpers", () => {
  it("resolveComponentId returns seeded ids", async () => {
    ApiHostMock.__seedComponentId("acct/proj/comp", { uuid: { highBits: 0n, lowBits: 7n } })
    const out = await runP(Agents.resolveComponentId("acct/proj/comp"))
    expect(out?.uuid.lowBits).toBe(7n)
  })

  it("resolveAgentId returns undefined for unknown refs", async () => {
    const out = await runP(Agents.resolveAgentId("missing", "X()"))
    expect(out).toBeUndefined()
  })

  it("resolveAgentIdStrict mirrors resolveAgentId via separate seed map", async () => {
    const id = makeAgentId("Y()")
    ApiHostMock.__seedStrictAgentId("acct/proj/comp", "Y()", id)
    const out = await runP(Agents.resolveAgentIdStrict("acct/proj/comp", "Y()"))
    expect(out).toEqual(id)
  })
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
  it("collects paged metadata in order", async () => {
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

    const out = await runP(Stream.runCollect(Agents.getAgents({ componentId })))
    expect(out.map((m) => m.agentId.agentId)).toEqual(["A()", "B()", "C()"])
  })
})

describe("Agents — Promises", () => {
  it("create + complete + poll returns the payload", async () => {
    const id = await runP(Agents.Promises.create)
    const polled1 = await runP(Agents.Promises.poll(id))
    expect(polled1).toBeUndefined()

    ApiHostMock.completePromise(id, new Uint8Array([1, 2, 3]))

    const polled2 = await runP(Agents.Promises.poll(id))
    expect(Array.from(polled2!)).toEqual([1, 2, 3])
  })

  it("complete fails with PromiseAlreadyCompletedError on a second call", async () => {
    const id = await runP(Agents.Promises.create)
    await runP(Agents.Promises.complete(id, new Uint8Array([0])))
    const exit = await runExit(Agents.Promises.complete(id, new Uint8Array([1])))
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      const failure = exit.cause
      expect(String((failure as { failure?: unknown }).failure ?? failure)).toMatch(
        /PromiseAlreadyCompletedError/,
      )
    }
  })

  it("await resolves once the promise is completed", async () => {
    const id = await runP(Agents.Promises.create)
    // Race: complete in a tick.
    queueMicrotask(() => {
      ApiHostMock.completePromise(id, new Uint8Array([7]))
    })
    const payload = await runP(Agents.Promises.await(id))
    expect(Array.from(payload)).toEqual([7])
  })

  it("await returns immediately when already completed", async () => {
    const id = await runP(Agents.Promises.create)
    ApiHostMock.completePromise(id, new Uint8Array([42]))
    const payload = await runP(Agents.Promises.await(id))
    expect(Array.from(payload)).toEqual([42])
  })

  it("await is interruptible: fiber-interrupt unparks the abortable promise", async () => {
    const id = await runP(Agents.Promises.create)
    // The promise is never completed; the awaiting fiber is interrupted.
    // Without `pollable.abortablePromise(signal)` this would hang the test.
    const exit = await runP(
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(Agents.Promises.await(id))
        yield* Effect.sleep("1 millis")
        yield* Fiber.interrupt(fiber)
        return yield* Fiber.await(fiber)
      }) as Effect.Effect<Exit.Exit<Uint8Array, unknown>, never, never>,
    )
    expect(Exit.isFailure(exit)).toBe(true)
    if (!Exit.isFailure(exit)) return
    expect(Cause.hasInterrupts(exit.cause)).toBe(true)
  })

  // Defensive sanity check: keep the unused import alive so a future refactor
  // removing it doesn't go unnoticed.
  it("agent ids are stringifiable for diagnostics", () => {
    const id = makeAgentId("X()")
    expect(uuidToString(id.componentId.uuid)).toMatch(/^[0-9a-f]{8}-/)
  })
})
