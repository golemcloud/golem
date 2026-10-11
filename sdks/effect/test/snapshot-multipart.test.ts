import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Ref, Schema } from "effect"
import { DatabaseSync } from "node:sqlite"
import {
  __resetAgents,
  defineAgent,
  dispatchLoadSnapshot,
  dispatchSaveSnapshot,
} from "../src/Agent.js"
import { method } from "../src/Method.js"
import * as Snapshot from "../src/Snapshot.js"
import { guest } from "../src/internal/guest.js"
import { decodeMultipart, encodeMultipart } from "../src/internal/multipart.js"
import { decodeEnvelope, encodeMultipartJsonEnvelope } from "../src/internal/snapshotEnvelope.js"
import { schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"
import { __setParseAgentIdImpl, __resetParseAgentIdImpl } from "./mocks/golem-agent-host.js"
import { __setEnvironment, __resetEnvironment } from "./mocks/wasi-cli-environment.js"
import { __setRestoreDatabaseSync, __resetSqliteExtensions } from "./mocks/node-sqlite.js"

const principal = {
  tag: "oidc",
  val: { sub: "Árvíztűrő 🦀", issuer: "https://example.test", claims: "{}" },
} as const
const wire = schemaValueToWit(v.record([]))
const bytes: Uint8Array = new Uint8Array([0, 255, 13, 10])
let initCalls = 0
let restoreCalls = 0
let methodsCalls = 0
let failRestore = false
let restoredBytes: Uint8Array | undefined
const spec = defineAgent({
  name: "MultipartIndex",
  id: {},
  snapshotting: Snapshot.multipart({
    schema: Schema.Struct({ revision: Schema.NumberFromString }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: { ping: method({ input: {}, success: Schema.Void }) },
})
const agent = spec.implement({
  init: () =>
    Effect.gen(function* () {
      initCalls++
      return yield* Ref.make({ revision: 7, index: bytes })
    }),
  methods: () => {
    methodsCalls++
    return { ping: () => Effect.void }
  },
  snapshot: {
    save: (ref) =>
      Ref.get(ref).pipe(
        Effect.map((state) => ({
          state: { revision: state.revision },
          parts: new Map([
            ["index", { bytes: state.index, contentType: "Application/Octet-Stream" }],
            ["__proto__", { bytes: new Uint8Array(), contentType: "text/plain" }],
            ["state", { bytes: new Uint8Array([1]), contentType: "application/json" }],
          ]),
        })),
      ),
    restore: (saved, context) =>
      Effect.gen(function* () {
        restoreCalls++
        expect(context.principal).toEqual(principal)
        expect(saved.state.revision).toBe(7)
        expect([...saved.parts.keys()]).toEqual(["index", "__proto__", "state"])
        const index = yield* Snapshot.requirePart(saved.parts, "index", "APPLICATION/OCTET-STREAM")
        if (failRestore) return yield* Effect.fail(new Error("restore failed"))
        restoredBytes = index
        return yield* Ref.make({ revision: saved.state.revision, index })
      }),
  },
})

const binaryOnly = defineAgent({
  name: "MultipartNull",
  id: {},
  snapshotting: Snapshot.multipart({ schema: Schema.Null, policy: Snapshot.policy.default }),
  methods: { ping: method({ input: {}, success: Schema.Void }) },
}).implement({
  init: () => Effect.succeed(null),
  methods: () => ({ ping: () => Effect.void }),
  snapshot: {
    save: () => Effect.succeed({ state: null, parts: new Map() }),
    restore: (saved) => Effect.succeed(saved.state),
  },
})

let dbHydrated = false
let dbRestoreCalls = 0
const sqlite = defineAgent({
  name: "MultipartSqlite",
  id: {},
  snapshotting: Snapshot.multipart({
    schema: Schema.Null,
    policy: Snapshot.policy.default,
    databases: ["main"],
  }),
  methods: { ping: method({ input: {}, success: Schema.Void }) },
}).implement({
  init: () => Effect.sync(() => new DatabaseSync(":memory:")),
  methods: () => {
    expect(dbHydrated).toBe(true)
    return { ping: () => Effect.void }
  },
  snapshot: {
    save: () => Effect.succeed({ state: null, parts: new Map() }),
    restore: (saved) =>
      Effect.gen(function* () {
        dbRestoreCalls++
        expect(dbHydrated).toBe(false)
        expect(
          yield* Snapshot.requirePart(saved.parts, "index", "application/octet-stream"),
        ).toEqual(bytes)
        return new DatabaseSync(":memory:")
      }),
    databases: (db) => ({ main: db }),
  },
})

const setLoad = (name: string) => {
  __setEnvironment([["GOLEM_AGENT_ID", name]])
  __setParseAgentIdImpl(() => [name, { value: wire } as never, undefined])
}

describe("explicit multipart snapshots", () => {
  beforeEach(async () => {
    await __resetAgents()
    void agent
    void binaryOnly
    void sqlite
    initCalls = 0
    restoreCalls = 0
    methodsCalls = 0
    failRestore = false
    restoredBytes = undefined
    dbHydrated = false
    dbRestoreCalls = 0
  })
  afterEach(() => {
    __resetEnvironment()
    __resetParseAgentIdImpl()
    __resetSqliteExtensions()
  })

  it("requires a complete strategy at compile time", () => {
    const invalidStrategies = () => {
      // @ts-expect-error Multipart always needs an explicit projection and restoration.
      spec.implement({
        init: () => Effect.succeed({}),
        methods: () => ({ ping: () => Effect.void }),
      })
      spec.implement({
        init: () => Effect.succeed({}),
        methods: () => ({ ping: () => Effect.void }),
        // @ts-expect-error A saver alone cannot reconstruct live state.
        snapshot: { save: () => Effect.succeed({ state: { revision: 7 }, parts: new Map() }) },
      })
    }
    expect(invalidStrategies).toBeTypeOf("function")
  })

  it("projects saved state, restores fresh Ref state without initialization, and preserves all parts/principal", async () => {
    await guest.initialize("MultipartIndex", wire, principal)
    const snapshot = await dispatchSaveSnapshot()
    const decoded = decodeEnvelope(snapshot, { tag: "anonymous" })
    expect(decoded.kind).toBe("multipart")
    if (decoded.kind !== "multipart") throw new Error()
    expect(decoded.state).toEqual({ revision: "7" })
    expect(decoded.parts.get("index")).toEqual({ bytes, contentType: "application/octet-stream" })
    await __resetAgents()
    setLoad("MultipartIndex")
    await dispatchLoadSnapshot(snapshot)
    expect(initCalls).toBe(1)
    expect(restoreCalls).toBe(1)
    expect(methodsCalls).toBe(2)
    expect(restoredBytes).toEqual(bytes)
    await guest.invoke("ping", wire, principal)
  })

  it("always emits multipart for JSON null and an empty part map", async () => {
    await guest.initialize("MultipartNull", wire, principal)
    const snapshot = await dispatchSaveSnapshot()
    expect(snapshot.mimeType).toMatch(/^multipart\/mixed;/)
    const decoded = decodeEnvelope(snapshot, principal)
    expect(decoded).toMatchObject({ kind: "multipart", state: null, parts: new Map() })
    await __resetAgents()
    setLoad("MultipartNull")
    await dispatchLoadSnapshot(snapshot)
  })

  it("failed restoration installs no active instance or methods", async () => {
    await guest.initialize("MultipartIndex", wire, principal)
    const snapshot = await dispatchSaveSnapshot()
    await __resetAgents()
    setLoad("MultipartIndex")
    methodsCalls = 0
    failRestore = true
    await expect(dispatchLoadSnapshot(snapshot)).rejects.toThrow(/restore failed/)
    expect(methodsCalls).toBe(0)
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/not initialized/)
    failRestore = false
    await dispatchLoadSnapshot(snapshot)
  })

  it("validates the declared DB inventory before restoration, then hydrates before methods", async () => {
    setLoad("MultipartSqlite")
    const parts = new Map([["index", { bytes, contentType: "application/octet-stream" }]])
    await expect(
      dispatchLoadSnapshot(encodeMultipartJsonEnvelope(principal, null, [], parts)),
    ).rejects.toThrow(/SnapshotDatabaseMissingPartError/)
    expect(dbRestoreCalls).toBe(0)
    __setRestoreDatabaseSync((_db, image) => {
      expect(image).toEqual(bytes)
      dbHydrated = true
    })
    await dispatchLoadSnapshot(
      encodeMultipartJsonEnvelope(principal, null, [{ name: "main", bytes }], parts),
    )
    expect(dbRestoreCalls).toBe(1)
    expect(dbHydrated).toBe(true)
  })

  it("rejects nonmultipart and reserved DBs before user restoration", async () => {
    setLoad("MultipartNull")
    await expect(
      dispatchLoadSnapshot({
        payload: new TextEncoder().encode("{}"),
        mimeType: "application/json",
      }),
    ).rejects.toBeDefined()
    await expect(
      dispatchLoadSnapshot(
        encodeMultipartJsonEnvelope(principal, null, [{ name: "other", bytes }]),
      ),
    ).rejects.toThrow(/SnapshotDatabaseUnknownPartError/)
  })

  it.effect("required-part errors are typed and MIME parameters are not silently stripped", () =>
    Effect.gen(function* () {
      const parts = new Map([["index", { bytes, contentType: "Application/Octet-Stream" }]])
      expect(yield* Snapshot.requirePart(parts, "index", "application/octet-stream")).toBe(bytes)
      expect((yield* Effect.flip(Snapshot.requirePart(parts, "missing", "text/plain")))._tag).toBe(
        "SnapshotEnvelopeError",
      )
      expect((yield* Effect.flip(Snapshot.requirePart(parts, "index", "text/plain")))._tag).toBe(
        "SnapshotEnvelopeError",
      )
      expect(
        (yield* Effect.flip(
          Snapshot.requirePart(parts, "index", "application/octet-stream; charset=utf-8"),
        ))._tag,
      ).toBe("SnapshotEnvelopeError")
    }),
  )

  it("rejects duplicate JSON metadata, version spellings, namespaces, names, MIME parameters and invalid UTF-8", () => {
    const enc = new TextEncoder()
    const envelope = '{"version":1,"principal":{"tag":"anonymous"},"state":null}'
    const wrap = (
      state: Uint8Array,
      extras: Array<{ name: string; contentType: string; body: Uint8Array }> = [],
    ) => {
      const { data, boundary } = encodeMultipart([
        { name: "state", contentType: "application/json", body: state },
        ...extras,
      ])
      return { payload: data, mimeType: `multipart/mixed; boundary=${boundary}` }
    }
    for (const text of [
      envelope.replace('"version":1', '"version":1,"version":1'),
      envelope.replace('"version":1', '"version":1.0'),
      envelope.replace('"version":1', '"version":1e0'),
      envelope.replace('"tag":"anonymous"', '"tag":"anonymous","tag":"anonymous"'),
    ]) {
      expect(() => decodeEnvelope(wrap(enc.encode(text)), principal)).toThrow(
        Snapshot.SnapshotEnvelopeError,
      )
    }
    expect(() => decodeEnvelope(wrap(new Uint8Array([255])), principal)).toThrow(
      Snapshot.SnapshotEnvelopeError,
    )
    for (const name of ["unknown:x", "part:", "part:a/b", "db:"]) {
      expect(() =>
        decodeEnvelope(
          wrap(enc.encode(envelope), [
            { name, contentType: "application/octet-stream", body: bytes },
          ]),
          principal,
        ),
      ).toThrow(Snapshot.SnapshotEnvelopeError)
    }
    expect(() =>
      decodeEnvelope(
        wrap(enc.encode(envelope), [
          { name: "part:index", contentType: "text/plain; charset=utf-8", body: bytes },
        ]),
        principal,
      ),
    ).toThrow(Snapshot.SnapshotEnvelopeError)
    const valid = wrap(enc.encode(envelope), [
      { name: "part:opaque", contentType: "application/json", body: bytes },
    ])
    expect(decodeEnvelope(valid, principal).kind).toBe("multipart")
    expect(decodeMultipart(valid.payload, valid.mimeType.split("boundary=")[1]!)[1]!.body).toEqual(
      bytes,
    )
  })
})
