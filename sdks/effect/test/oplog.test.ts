import { Effect, Exit, Stream } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Oplog from "../src/oplog.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import * as OplogMock from "./mocks/golem-api-oplog.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

const mkEntry = (tag: string): Oplog.PublicOplogEntry =>
  ({ tag }) as unknown as Oplog.PublicOplogEntry
const mkRawEntry = (tag: string): Oplog.OplogEntry => ({ tag }) as unknown as Oplog.OplogEntry

const sampleAgent: Oplog.AgentId = {
  componentId: { uuid: { highBits: 0n, lowBits: 1n } },
  agentId: 'Counter("x")',
}
const sampleEnvironment: Oplog.EnvironmentId = { uuid: { highBits: 0n, lowBits: 1n } }

beforeEach(() => {
  ApiHostMock.__resetAll()
  OplogMock.__reset()
})
afterEach(() => {
  ApiHostMock.__resetAll()
  OplogMock.__reset()
})

describe("Oplog — currentIndex / setIndex", () => {
  it("currentIndex returns the host's monotonic counter", async () => {
    const a = await runP(Oplog.currentIndex)
    const b = await runP(Oplog.currentIndex)
    expect(b).toBeGreaterThan(a)
  })

  it("setIndex forwards the requested index to the host", async () => {
    await runP(Oplog.setIndex(42n))
    expect(ApiHostMock.__getOplogIndex()).toBe(42n)
  })

  it("wraps host throws as OplogHostError", async () => {
    Oplog.__setGetOplogIndexForTest(() => {
      throw new Error("nope")
    })
    try {
      const exit = await runExit(Oplog.currentIndex)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/OplogHostError/)
      }
    } finally {
      Oplog.__resetGetOplogIndexForTest()
    }
  })
})

describe("Oplog — read stream", () => {
  it("reads paged chunks until the host returns undefined", async () => {
    OplogMock.__seedReadChunks(sampleAgent, 0n, [
      [mkEntry("create"), mkEntry("host-call")],
      [mkEntry("agent-invocation-started")],
    ])

    const arr = await runP(Stream.runCollect(Oplog.read({ agentId: sampleAgent, start: 0n })))
    expect(arr.map((e) => (e as { tag: string }).tag)).toEqual([
      "create",
      "host-call",
      "agent-invocation-started",
    ])
  })

  it("reader exposes manual paging", async () => {
    OplogMock.__seedReadChunks(sampleAgent, 5n, [[mkEntry("log")], [mkEntry("no-op")]])
    const r = await runP(Oplog.reader({ agentId: sampleAgent, start: 5n }))
    const first = await runP(r.next)
    const second = await runP(r.next)
    const third = await runP(r.next)
    expect(first?.map((e) => (e as { tag: string }).tag)).toEqual(["log"])
    expect(second?.map((e) => (e as { tag: string }).tag)).toEqual(["no-op"])
    expect(third).toBeUndefined()
  })

  it("returns an empty stream when no chunks are seeded", async () => {
    const arr = await runP(Stream.runCollect(Oplog.read({ agentId: sampleAgent, start: 0n })))
    expect(arr).toEqual([])
  })
})

describe("Oplog — search stream", () => {
  it("yields tuples in chunk order", async () => {
    OplogMock.__seedSearchChunks(sampleAgent, "boom", [
      [
        [10n, mkEntry("error")],
        [11n, mkEntry("log")],
      ],
      [[20n, mkEntry("log")]],
    ])
    const arr = await runP(Stream.runCollect(Oplog.search({ agentId: sampleAgent, text: "boom" })))
    expect(arr.map(([i, e]) => [i, (e as { tag: string }).tag])).toEqual([
      [10n, "error"],
      [11n, "log"],
      [20n, "log"],
    ])
  })
})

describe("Oplog — enrich", () => {
  it("delegates to the seeded enricher", async () => {
    OplogMock.__setEnrichImpl((entries) =>
      entries.map(
        ([i, e]) =>
          ({
            tag: `enriched:${(e as { tag: string }).tag}`,
            idx: i,
          }) as unknown as Oplog.PublicOplogEntry,
      ),
    )
    const out = await runP(
      Oplog.enrich({
        environmentId: sampleEnvironment,
        agentId: sampleAgent,
        entries: [
          [0n, mkRawEntry("create")],
          [1n, mkRawEntry("log")],
        ],
        componentRevision: 0n,
      }),
    )
    expect(out.map((e) => (e as { tag: string }).tag)).toEqual(["enriched:create", "enriched:log"])
  })

  it("surfaces host throws as OplogHostError", async () => {
    const exit = await runExit(
      Oplog.enrich({
        environmentId: sampleEnvironment,
        agentId: sampleAgent,
        entries: [],
        componentRevision: 0n,
      }),
    )
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      expect(JSON.stringify(exit.cause)).toMatch(/OplogHostError/)
    }
  })
})
