import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Stream } from "effect"
import { OplogLive } from "../src/host/OplogClient.js"
import * as Oplog from "../src/Oplog.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import * as OplogMock from "./mocks/golem-api-oplog.js"

const mkEntry = (tag: string): Oplog.AttributedPublicOplogEntry =>
  ({ attribution: { tag: "agent" }, entry: { tag } }) as unknown as Oplog.AttributedPublicOplogEntry
const mkRawEntry = (tag: string): Oplog.OplogEntry => ({ tag }) as unknown as Oplog.OplogEntry

const sampleAgent: Oplog.AgentId = { uuid: { highBits: 0x1n, lowBits: 0x8000000000000001n } }
const sampleEnvironment: Oplog.EnvironmentId = { uuid: { highBits: 0n, lowBits: 1n } }

beforeEach(() => {
  ApiHostMock.__resetAll()
  OplogMock.__reset()
})
afterEach(() => {
  ApiHostMock.__resetAll()
  OplogMock.__reset()
})

describe("Oplog — read stream", () => {
  it.effect("reads paged chunks until the host returns undefined", () =>
    Effect.gen(function* () {
      OplogMock.__seedReadChunks(sampleAgent, 0n, [
        [mkEntry("create"), mkEntry("host-call")],
        [mkEntry("agent-invocation-started")],
      ])

      const arr = yield* Stream.runCollect(Oplog.read({ agentId: sampleAgent, start: 0n }))
      expect(arr.map((e) => e.entry.tag)).toEqual([
        "create",
        "host-call",
        "agent-invocation-started",
      ])
    }).pipe(Effect.provide(OplogLive)),
  )

  it.effect("reader exposes manual paging", () =>
    Effect.gen(function* () {
      OplogMock.__seedReadChunks(sampleAgent, 5n, [[mkEntry("log")], [mkEntry("suspend")]])
      const r = yield* Oplog.reader({ agentId: sampleAgent, start: 5n })
      const first = yield* r.next
      const second = yield* r.next
      const third = yield* r.next
      expect(first?.map((e) => e.entry.tag)).toEqual(["log"])
      expect(second?.map((e) => e.entry.tag)).toEqual(["suspend"])
      expect(third).toBeUndefined()
    }).pipe(Effect.provide(OplogLive)),
  )

  it.effect("returns an empty stream when no chunks are seeded", () =>
    Effect.gen(function* () {
      const arr = yield* Stream.runCollect(Oplog.read({ agentId: sampleAgent, start: 0n }))
      expect(arr).toEqual([])
    }).pipe(Effect.provide(OplogLive)),
  )
})

describe("Oplog — search stream", () => {
  it.effect("yields tuples in chunk order", () =>
    Effect.gen(function* () {
      OplogMock.__seedSearchChunks(sampleAgent, "boom", [
        [
          [10n, mkEntry("error")],
          [11n, mkEntry("log")],
        ],
        [[20n, mkEntry("log")]],
      ])
      const arr = yield* Stream.runCollect(Oplog.search({ agentId: sampleAgent, text: "boom" }))
      expect(arr.map(([i, e]) => [i, e.entry.tag])).toEqual([
        [10n, "error"],
        [11n, "log"],
        [20n, "log"],
      ])
    }).pipe(Effect.provide(OplogLive)),
  )
})

describe("Oplog — enrich", () => {
  it.effect("delegates to the seeded enricher", () =>
    Effect.gen(function* () {
      OplogMock.__setEnrichImpl((entries) =>
        entries.map(
          ([i, e]) =>
            ({
              attribution: { tag: "agent" },
              entry: { tag: `enriched:${e.tag}`, idx: i },
            }) as unknown as Oplog.AttributedPublicOplogEntry,
        ),
      )
      const out = yield* Oplog.enrich({
        environmentId: sampleEnvironment,
        agentId: sampleAgent,
        entries: [
          [0n, mkRawEntry("create")],
          [1n, mkRawEntry("log")],
        ],
        componentRevision: 0n,
      })
      expect(out.map((e) => e.entry.tag)).toEqual(["enriched:create", "enriched:log"])
    }).pipe(Effect.provide(OplogLive)),
  )

  it.effect("surfaces host throws as OplogHostError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
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
    }).pipe(Effect.provide(OplogLive)),
  )
})
