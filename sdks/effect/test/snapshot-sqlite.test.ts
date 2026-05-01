import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Ref, Schema } from "effect"
import {
  __resetAgents,
  defineAgent,
  dispatchLoadSnapshot,
  dispatchSaveSnapshot,
} from "../src/Agent.js"
import {
  __resetParseAgentIdImpl as __resetParseAgentIdForTest,
  __setParseAgentIdImpl as __setParseAgentIdForTest,
} from "./mocks/golem-agent-host.js"
import {
  __resetSqliteExtensions,
  __setIsAutocommitDatabaseSync as __setIsAutocommitDatabaseSyncForTest,
  __setRestoreDatabaseSync as __setRestoreDatabaseSyncForTest,
  __setSerializeDatabaseSync as __setSerializeDatabaseSyncForTest,
} from "./mocks/node-sqlite.js"
import {
  __resetEnvironment as __resetGetEnvironmentForTest,
  __setEnvironment,
} from "./mocks/wasi-cli-environment.js"
import { method } from "../src/Method.js"
import { guest } from "../src/Exports.js"
import * as Snapshot from "../src/Snapshot.js"
import { toWitCodec } from "../src/WitCodec.js"
import { DatabaseSync } from "node:sqlite"

const oidcZoe = {
  tag: "oidc",
  val: { sub: "zoe", issuer: "https://example.test", claims: "{}" },
} as const

// ---------------------------------------------------------------------------
// Auto + databases agent (single DB)
// ---------------------------------------------------------------------------

const SqliteCounter = defineAgent({
  name: "SqliteCounterTest",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ note: Schema.String }),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(5),
  }),
  methods: {
    note: method({ params: {}, success: Schema.String }),
    setNote: method({ params: { v: Schema.String }, success: Schema.Void }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ note: name })
      const db = new DatabaseSync(":memory:")
      yield* snap.attachDatabase("counters", db)
      return {
        note: () => Ref.get(state).pipe(Effect.map((s) => s.note)),
        setNote: ({ v }) => Ref.set(state, { note: v }),
      }
    }),
})

// ---------------------------------------------------------------------------
// Agent that declares a DB but never attaches it (negative test)
// ---------------------------------------------------------------------------

const SqliteForgetfulAttach = defineAgent({
  name: "SqliteForgetfulAttach",
  constructorParams: {},
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.default,
  }),
  methods: {
    ping: method({ params: {}, success: Schema.Void }),
  },
  impl: (_input, snap) =>
    Effect.gen(function* () {
      yield* snap.init({})
      // Intentionally never call snap.attachDatabase("counters", ...).
      return { ping: () => Effect.void }
    }),
})

describe("snapshot + sqlite databases", () => {
  beforeEach(async () => {
    await __resetAgents()
    void SqliteCounter
    void SqliteForgetfulAttach
  })

  afterEach(() => {
    __resetGetEnvironmentForTest()
    __resetParseAgentIdForTest()
    __resetSqliteExtensions()
  })

  it("declares enabled snapshotting in agent metadata", async () => {
    const types = await guest.discoverAgentTypes()
    const t = types.find((t) => t.typeName === "SqliteCounterTest")!
    expect(t.snapshotting.tag).toBe("enabled")
  })

  it("save → load round-trips with a single declared database", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))

    // Stub the three node:sqlite extensions for in-test determinism.
    const captured: Array<{ id: number; bytes: Uint8Array }> = []
    let nextId = 1
    const dbToId = new WeakMap<object, number>()
    __setIsAutocommitDatabaseSyncForTest(() => true)
    __setSerializeDatabaseSyncForTest((db) => {
      let id = dbToId.get(db)
      if (id === undefined) {
        id = nextId++
        dbToId.set(db, id)
      }
      const bytes = new TextEncoder().encode(`SQLITE-DB#${id}`)
      captured.push({ id, bytes })
      return bytes
    })
    let restoreCalled: { db: object; bytes: Uint8Array } | null = null
    __setRestoreDatabaseSyncForTest((db, bytes) => {
      restoreCalled = { db, bytes }
    })

    await guest.initialize(
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      oidcZoe,
    )

    const snapshot = await dispatchSaveSnapshot()
    expect(snapshot.mimeType).toMatch(/^multipart\/mixed; boundary=/)
    expect(captured).toHaveLength(1)

    await __resetAgents()
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => [
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      undefined,
    ])

    await dispatchLoadSnapshot(snapshot)
    expect(restoreCalled).not.toBeNull()
    expect(new TextDecoder().decode(restoreCalled!.bytes)).toBe("SQLITE-DB#1")
  })

  it("save fails fast if the user declared a DB but never attached it", async () => {
    await guest.initialize("SqliteForgetfulAttach", { tag: "tuple", val: [] }, oidcZoe)
    // Auto-snapshot save throws synchronously — see the long comment on
    // dispatchSaveSnapshot for why the auto path must stay non-async.
    expect(() => dispatchSaveSnapshot()).toThrow(/SnapshotDatabaseMissingPartError/)
  })

  it("save fails when isAutocommitDatabaseSync returns false", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))

    __setIsAutocommitDatabaseSyncForTest(() => false)
    __setSerializeDatabaseSyncForTest(() => new Uint8Array([0]))

    await guest.initialize(
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      oidcZoe,
    )
    expect(() => dispatchSaveSnapshot()).toThrow(/SnapshotDatabaseNotInAutocommitError/)
  })

  it("rejects an unknown 'db:<name>' part on load", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))
    __setIsAutocommitDatabaseSyncForTest(() => true)
    __setSerializeDatabaseSyncForTest(() => new Uint8Array([1, 2]))
    __setRestoreDatabaseSyncForTest(() => {})

    await guest.initialize(
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      oidcZoe,
    )
    const snap = await dispatchSaveSnapshot()

    // Hand-craft an envelope with an extra 'db:bogus' part by editing
    // the multipart body. Simpler: build via encodeMultipartJsonEnvelope
    // directly (re-importing for the test).
    const { encodeMultipartJsonEnvelope } = await import("../src/SnapshotEnvelope.js")
    const tampered = encodeMultipartJsonEnvelope({ tag: "anonymous" }, { note: "zoe" }, [
      { name: "counters", bytes: new Uint8Array([1]) },
      { name: "bogus", bytes: new Uint8Array([2]) },
    ])

    await __resetAgents()
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => [
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      undefined,
    ])
    void snap
    await expect(dispatchLoadSnapshot(tampered)).rejects.toThrow(/SnapshotDatabaseUnknownPartError/)
  })

  it("rejects a missing 'db:<name>' part on load", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))
    __setIsAutocommitDatabaseSyncForTest(() => true)
    __setRestoreDatabaseSyncForTest(() => {})
    const { encodeMultipartJsonEnvelope } = await import("../src/SnapshotEnvelope.js")
    const tampered = encodeMultipartJsonEnvelope({ tag: "anonymous" }, { note: "zoe" }, [])
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => [
      "SqliteCounterTest",
      { tag: "tuple", val: [{ tag: "component-model", val: xWv }] },
      undefined,
    ])
    await expect(dispatchLoadSnapshot(tampered)).rejects.toThrow(/SnapshotDatabaseMissingPartError/)
  })
})
