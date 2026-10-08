import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { vi } from "vitest"
import { existsSync } from "node:fs"
import { SqliteClient, __getUnderlyingDatabase } from "../src/Sqlite/SqliteClient.js"
import { DatabaseSync as FakeDatabaseSync } from "./mocks/node-sqlite.js"
import { decodeEnvelope, encodeMultipartJsonEnvelope } from "../src/internal/snapshotEnvelope.js"
import { decodeMultipart, encodeMultipart, extractBoundary } from "../src/internal/multipart.js"
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
import { guest } from "../src/internal/guest.js"
import * as Snapshot from "../src/Snapshot.js"
import { toWitCodec } from "../src/WitCodec.js"
import { DatabaseSync } from "node:sqlite"
import { schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v, type SchemaValue } from "../src/internal/schema-model/model.js"

const oidcZoe = {
  tag: "oidc",
  val: { sub: "zoe", issuer: "https://example.test", claims: "{}" },
} as const

const wire = (...fields: Array<SchemaValue>) => schemaValueToWit(v.record(fields))
const typed = (value: ReturnType<typeof wire>) => ({ value }) as never

vi.mock("node:fs", async (original) => ({
  ...(await original<typeof import("node:fs")>()),
  existsSync: vi.fn(() => true),
}))

// ---------------------------------------------------------------------------
// Auto + databases agent (single DB)
// ---------------------------------------------------------------------------

const SqliteCounter = defineAgent({
  name: "SqliteCounterTest",
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ note: Schema.String }),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(5),
  }),
  methods: {
    note: method({ input: {}, success: Schema.String }),
    setNote: method({ input: { v: Schema.String }, success: Schema.Void }),
  },
}).implement({
  init: ({ name }) => Effect.sync(() => ({ note: name, db: new DatabaseSync(":memory:") })),
  methods: (state) => ({
    note: () => Effect.sync(() => state.note),
    setNote: ({ v }) => Effect.sync(() => void (state.note = v)),
  }),
  snapshot: {
    save: (state) => Effect.succeed({ note: state.note }),
    restore: (saved) => Effect.sync(() => ({ ...saved, db: new DatabaseSync(":memory:") })),
    databases: (state) => ({ counters: state.db }),
  },
})

// ---------------------------------------------------------------------------
// Agent that declares a DB but never attaches it (negative test)
// ---------------------------------------------------------------------------

const SqliteForgetfulAttach = defineAgent({
  name: "SqliteForgetfulAttach",
  id: {},
  snapshotting: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.default,
  }),
  methods: {
    ping: method({ input: {}, success: Schema.Void }),
  },
}).implement({
  init: () => Effect.succeed({}),
  methods: () => ({ ping: () => Effect.void }),
  snapshot: {
    save: (state) => Effect.succeed(state),
    restore: (saved) => Effect.succeed(saved),
    // @ts-expect-error Deliberately bypass the required attachment to test runtime validation.
    databases: () => ({}),
  },
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
    vi.mocked(existsSync).mockReturnValue(true)
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

    await guest.initialize("SqliteCounterTest", wire(xWv), oidcZoe)

    const snapshot = await dispatchSaveSnapshot()
    expect(snapshot.mimeType).toMatch(/^multipart\/mixed; boundary=/)
    expect(captured).toHaveLength(1)

    await __resetAgents()
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => ["SqliteCounterTest", typed(wire(xWv)), undefined])

    await dispatchLoadSnapshot(snapshot)
    expect(restoreCalled).not.toBeNull()
    expect(new TextDecoder().decode(restoreCalled!.bytes)).toBe("SQLITE-DB#1")
  })

  it("save fails fast if the user declared a DB but never attached it", async () => {
    await guest.initialize("SqliteForgetfulAttach", wire(), oidcZoe)
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/SnapshotDatabaseMissingPartError/)
  })

  it("save fails when isAutocommitDatabaseSync returns false", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))

    __setIsAutocommitDatabaseSyncForTest(() => false)
    __setSerializeDatabaseSyncForTest(() => new Uint8Array([0]))

    await guest.initialize("SqliteCounterTest", wire(xWv), oidcZoe)
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/SnapshotDatabaseNotInAutocommitError/)
  })

  it("rejects an unknown 'db:<name>' part on load", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))
    __setIsAutocommitDatabaseSyncForTest(() => true)
    __setSerializeDatabaseSyncForTest(() => new Uint8Array([1, 2]))
    __setRestoreDatabaseSyncForTest(() => {})

    await guest.initialize("SqliteCounterTest", wire(xWv), oidcZoe)
    const snap = await dispatchSaveSnapshot()

    // Hand-craft an envelope with an extra 'db:bogus' part by editing
    // the multipart body. Simpler: build via encodeMultipartJsonEnvelope
    // directly (re-importing for the test).
    const { encodeMultipartJsonEnvelope } = await import("../src/internal/snapshotEnvelope.js")
    const tampered = encodeMultipartJsonEnvelope({ tag: "anonymous" }, { note: "zoe" }, [
      { name: "counters", bytes: new Uint8Array([1]) },
      { name: "bogus", bytes: new Uint8Array([2]) },
    ])

    await __resetAgents()
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => ["SqliteCounterTest", typed(wire(xWv)), undefined])
    void snap
    await expect(dispatchLoadSnapshot(tampered)).rejects.toThrow(/SnapshotDatabaseUnknownPartError/)
  })

  it("rejects a missing 'db:<name>' part on load", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const xWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("zoe"))
    __setIsAutocommitDatabaseSyncForTest(() => true)
    __setRestoreDatabaseSyncForTest(() => {})
    const { encodeMultipartJsonEnvelope } = await import("../src/internal/snapshotEnvelope.js")
    const tampered = encodeMultipartJsonEnvelope({ tag: "anonymous" }, { note: "zoe" }, [])
    __setEnvironment([["GOLEM_AGENT_ID", "SqliteCounterTest:zoe"]])
    __setParseAgentIdForTest(() => ["SqliteCounterTest", typed(wire(xWv)), undefined])
    await expect(dispatchLoadSnapshot(tampered)).rejects.toThrow(/SnapshotDatabaseMissingPartError/)
  })
})

describe.each(["auto", "multipart"] as const)("managed SQLite ownership (%s)", (mode) => {
  let sequence = 0
  const userBytes = new Uint8Array([0, 255, 13, 10])
  const parts = new Map([["index", { bytes: userBytes, contentType: "application/octet-stream" }]])
  const configure = (
    names: readonly string[],
    open: () => Effect.Effect<
      Record<string, Snapshot.AttachableDatabase>,
      unknown,
      import("effect").Scope.Scope
    >,
  ) => {
    const name = `Ownership${mode}${++sequence}`
    const restored = vi.fn()
    const methods = vi.fn()
    const spec = {
      schema: Schema.Struct({ note: Schema.String }),
      policy: Snapshot.policy.default,
      databases: names,
    }
    const restoration = {
      restore: (_saved: unknown, context: Snapshot.SnapshotRestorationContext) => {
        restored(context.principal)
        return open()
      },
      databases: (state: Record<string, Snapshot.AttachableDatabase>) => state,
    }
    const metadata = {
      name,
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }
    const implementation = {
      init: open,
      methods: () => {
        methods()
        return { ping: () => Effect.void }
      },
    }
    if (mode === "auto") {
      defineAgent({ ...metadata, snapshotting: Snapshot.define(spec) }).implement({
        ...implementation,
        snapshot: { ...restoration, save: () => Effect.succeed({ note: "saved" }) },
      })
    } else {
      defineAgent({ ...metadata, snapshotting: Snapshot.multipart(spec) }).implement({
        ...implementation,
        snapshot: {
          ...restoration,
          save: () => Effect.succeed({ state: { note: "saved" }, parts }),
        },
      })
    }
    const select = () => {
      __setEnvironment([["GOLEM_AGENT_ID", name]])
      __setParseAgentIdForTest(() => [name, typed(wire()), undefined])
    }
    const envelope = (
      images: Array<{ name: string; bytes: Uint8Array }>,
      files: Record<string, string> = {},
    ) =>
      encodeMultipartJsonEnvelope(
        oidcZoe,
        { note: "saved" },
        images,
        mode === "multipart" ? parts : new Map(),
        files,
      )
    return { name, restored, methods, select, envelope }
  }
  beforeEach(async () => {
    await __resetAgents()
    vi.mocked(existsSync).mockReturnValue(true)
  })
  afterEach(() => {
    __resetGetEnvironmentForTest()
    __resetParseAgentIdForTest()
    __resetSqliteExtensions()
    vi.mocked(existsSync).mockReturnValue(true)
  })

  it.each([false, true])(
    "saves only memory/temp images and reopens file-only/mixed handles (wrapped=%s)",
    async (wrapped) => {
      for (const locations of [["/data/app.db"], [":memory:", "", "/data/app.db"]]) {
        const names = locations.map((_, i) => `db${i}`)
        let handles: FakeDatabaseSync[] = []
        const serialized: FakeDatabaseSync[] = []
        const hydrated: FakeDatabaseSync[] = []
        const warmed: FakeDatabaseSync[] = []
        __setSerializeDatabaseSyncForTest((db) => {
          serialized.push(db as unknown as FakeDatabaseSync)
          return userBytes
        })
        __setRestoreDatabaseSyncForTest((db, bytes) => {
          expect(bytes).toEqual(userBytes)
          hydrated.push(db as unknown as FakeDatabaseSync)
        })
        const agent = configure(names, () =>
          Effect.gen(function* () {
            handles = locations.map((location) => {
              const db = new FakeDatabaseSync(location)
              const prepare = db.prepare.bind(db)
              db.prepare = (sql) => {
                if (sql === "SELECT count(*) FROM sqlite_master") warmed.push(db)
                return prepare(sql)
              }
              return db
            })
            const values: Array<[string, Snapshot.AttachableDatabase]> = []
            for (const [i, db] of handles.entries()) {
              const handle = db as unknown as DatabaseSync
              values.push([
                names[i]!,
                wrapped
                  ? yield* SqliteClient.fromDatabase(handle, { closeOnScopeClose: true })
                  : handle,
              ])
            }
            return Object.fromEntries(values)
          }),
        )
        await guest.initialize(agent.name, wire(), oidcZoe)
        const snapshot = await dispatchSaveSnapshot()
        const decoded = decodeEnvelope(snapshot, oidcZoe)
        if (decoded.kind !== "multipart") throw new Error("expected multipart")
        expect(decoded.databases.map((db) => db.name)).toEqual(
          names.filter((_, i) => locations[i] !== "/data/app.db"),
        )
        expect(decoded.fileDatabases).toEqual({ [names[names.length - 1]!]: "/data/app.db" })
        expect(serialized.map((db) => db.location())).toEqual(
          locations.filter((path) => path !== "/data/app.db").map(() => null),
        )
        await __resetAgents()
        serialized.length = 0
        agent.select()
        await dispatchLoadSnapshot(snapshot)
        expect(agent.restored).toHaveBeenCalledWith(oidcZoe)
        expect(hydrated).toEqual(handles.filter((db) => db.location() === null))
        expect(warmed).toEqual(handles)
        // This read is discarded cache warming, not a saved file image.
        expect(serialized).toEqual(handles.filter((db) => db.location() !== null))
        expect(agent.methods).toHaveBeenCalledTimes(2)
        const savedAgain = decodeEnvelope(await dispatchSaveSnapshot(), oidcZoe)
        if (savedAgain.kind !== "multipart") throw new Error("expected multipart")
        expect(savedAgain.principal).toEqual(oidcZoe)
        expect(savedAgain.parts).toEqual(mode === "multipart" ? parts : new Map())
        await __resetAgents()
      }
    },
  )

  it("rejects malformed, missing, unknown and overlapping inventories before the factory", async () => {
    const agent = configure(["mem", "file"], () =>
      Effect.succeed({ mem: new DatabaseSync(":memory:"), file: new DatabaseSync("/data/app.db") }),
    )
    agent.select()
    const valid = agent.envelope([{ name: "mem", bytes: userBytes }], { file: "/data/app.db" })
    const raw = decodeMultipart(valid.payload, extractBoundary(valid.mimeType)!)
    const state = JSON.parse(new TextDecoder().decode(raw[0]!.body))
    for (const files of [
      undefined,
      null,
      [],
      { file: 17 },
      { file: "" },
      { file: ":memory:" },
      { file: "/data/app.db", mem: "/data/other.db" },
      { bogus: "/data/app.db" },
      {},
    ]) {
      const encoded = encodeMultipart([
        {
          ...raw[0]!,
          body: new TextEncoder().encode(JSON.stringify({ ...state, fileDatabases: files })),
        },
        ...raw.slice(1),
      ])
      await expect(
        dispatchLoadSnapshot({
          payload: encoded.data,
          mimeType: `multipart/mixed; boundary=${encoded.boundary}`,
        }),
      ).rejects.toBeDefined()
    }
    expect(agent.restored).not.toHaveBeenCalled()
    expect(agent.methods).not.toHaveBeenCalled()
  })

  it("checks recorded files before a restore factory can create empty replacements", async () => {
    const agent = configure(["file"], () =>
      Effect.succeed({ file: new DatabaseSync("/data/app.db") }),
    )
    agent.select()
    vi.mocked(existsSync).mockReturnValue(false)
    await expect(
      dispatchLoadSnapshot(agent.envelope([], { file: "/data/app.db" })),
    ).rejects.toBeDefined()
    expect(agent.restored).not.toHaveBeenCalled()
  })

  it.each(["closed", "transaction", "attached"])(
    "retains file-backed save restrictions on %s",
    async (fault) => {
      const db = new FakeDatabaseSync("/data/app.db")
      const serialized = vi.fn(() => userBytes)
      __setSerializeDatabaseSyncForTest(serialized)
      const agent = configure(["file"], () =>
        Effect.succeed({ file: db as unknown as DatabaseSync }),
      )
      await guest.initialize(agent.name, wire(), oidcZoe)
      if (fault === "closed") db.close()
      if (fault === "transaction") db._autocommit = false
      if (fault === "attached") {
        db._returnArrays = true
        db._attachments.push({ name: "extra" })
      }
      await expect(dispatchSaveSnapshot()).rejects.toBeDefined()
      expect(serialized).not.toHaveBeenCalled()
    },
  )

  it.each(["hydrate", "warm"])(
    "closes managed acquisitions if %s fails and never publishes methods",
    async (fault) => {
      let handle: DatabaseSync | undefined
      const agent = configure(["mem"], () =>
        Effect.gen(function* () {
          const client = yield* SqliteClient.make({ filename: ":memory:" })
          handle = __getUnderlyingDatabase(client)
          if (fault === "warm")
            vi.spyOn(handle, "prepare").mockImplementation((sql) => {
              if (sql === "SELECT count(*) FROM sqlite_master") throw new Error("warming failed")
              return new FakeDatabaseSync(":memory:").prepare(sql) as never
            })
          return { mem: client }
        }),
      )
      if (fault === "hydrate")
        __setRestoreDatabaseSyncForTest(() => {
          throw new Error("hydration failed")
        })
      agent.select()
      await expect(
        dispatchLoadSnapshot(agent.envelope([{ name: "mem", bytes: userBytes }])),
      ).rejects.toBeDefined()
      expect(handle!.isOpen).toBe(false)
      expect(agent.methods).not.toHaveBeenCalled()
      await expect(dispatchSaveSnapshot()).rejects.toThrow(/not initialized/)
    },
  )

  it.each([
    "missing",
    "wrong-file",
    "memory-for-file",
    "file-for-memory",
    "closed",
    "transaction",
    "attached",
  ])("validates all handles before hydration and closes scoped handles on %s", async (fault) => {
    let mem: FakeDatabaseSync | undefined
    let file: FakeDatabaseSync | undefined
    const hydrated = vi.fn()
    __setRestoreDatabaseSyncForTest(hydrated)
    const agent = configure(["mem", "file"], () =>
      Effect.gen(function* () {
        mem = new FakeDatabaseSync(fault === "file-for-memory" ? "/data/app.db" : ":memory:")
        file = new FakeDatabaseSync(
          fault === "wrong-file"
            ? "/data/other.db"
            : fault === "memory-for-file"
              ? ""
              : "/data/app.db",
        )
        const first = yield* SqliteClient.fromDatabase(mem as unknown as DatabaseSync, {
          closeOnScopeClose: true,
        })
        const second = yield* SqliteClient.fromDatabase(file as unknown as DatabaseSync, {
          closeOnScopeClose: true,
        })
        if (fault === "closed") file.close()
        if (fault === "transaction") file._autocommit = false
        if (fault === "attached") {
          file._returnArrays = true
          file._attachments.push({ name: "extra" })
        }
        return Object.fromEntries(
          fault === "missing"
            ? [["mem", first]]
            : [
                ["mem", first],
                ["file", second],
              ],
        )
      }),
    )
    agent.select()
    await expect(
      dispatchLoadSnapshot(
        agent.envelope([{ name: "mem", bytes: userBytes }], { file: "/data/app.db" }),
      ),
    ).rejects.toBeDefined()
    expect(hydrated).not.toHaveBeenCalled()
    expect(agent.methods).not.toHaveBeenCalled()
    expect(mem!._closed).toBe(true)
    expect(file!._closed).toBe(true)
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/not initialized/)
  })

  it.each([
    [488, 4096, -2000, true],
    [489, 4096, -2000, false],
    [1840, 1024, -2000, true],
    [1841, 1024, -2000, false],
    [99, 4096, 100, true],
    [100, 4096, 100, false],
    [0, 4096, -2000, false],
  ])(
    "warms file cache before methods for pageCount=%s pageSize=%s cacheSize=%s",
    async (pageCount, pageSize, cacheSize, shouldRead) => {
      const serialized = vi.fn(() => userBytes)
      __setSerializeDatabaseSyncForTest(serialized)
      const events: string[] = []
      const agent = configure(["file"], () =>
        Effect.sync(() => {
          const db = new FakeDatabaseSync("/data/app.db")
          db._returnArrays = true
          db._pragmas = {
            page_count: pageCount as number,
            page_size: pageSize as number,
            cache_size: cacheSize as number,
          }
          const prepare = db.prepare.bind(db)
          db.prepare = (sql) => {
            events.push(sql)
            return prepare(sql)
          }
          return { file: db as unknown as DatabaseSync }
        }),
      )
      agent.methods.mockImplementation(() => {
        expect(events).toContain("SELECT count(*) FROM sqlite_master")
        expect(serialized).toHaveBeenCalledTimes(shouldRead ? 1 : 0)
      })
      agent.select()
      await dispatchLoadSnapshot(agent.envelope([], { file: "/data/app.db" }))
    },
  )

  it("preserves prototype-like logical file names", async () => {
    const agent = configure(["__proto__"], () =>
      Effect.succeed(Object.fromEntries([["__proto__", new DatabaseSync("/data/app.db")]])),
    )
    agent.select()
    const snapshot = agent.envelope([], Object.fromEntries([["__proto__", "/data/app.db"]]))
    await dispatchLoadSnapshot(snapshot)
    const decoded = decodeEnvelope(await dispatchSaveSnapshot(), oidcZoe)
    if (decoded.kind !== "multipart") throw new Error("expected multipart")
    expect(Object.entries(decoded.fileDatabases!)).toEqual([["__proto__", "/data/app.db"]])
    await __resetAgents()
    await expect(
      dispatchLoadSnapshot(
        agent.envelope(
          [{ name: "__proto__", bytes: userBytes }],
          Object.fromEntries([["__proto__", "/data/app.db"]]),
        ),
      ),
    ).rejects.toBeDefined()
  })
})
