import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Duration, Effect, Ref, Schema } from "effect"
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
import { method } from "../src/Method.js"
import { guest } from "../src/internal/guest.js"
import * as Snapshot from "../src/Snapshot.js"
import { defineConfig } from "../src/Config.js"
import {
  __resetEnvironment as __resetGetEnvironmentForTest,
  __setEnvironment as __setGetEnvironmentForTest,
} from "./mocks/wasi-cli-environment.js"
import {
  __resetGetConfigValueImpl as __resetGetConfigValueForTest,
  __setGetConfigValueImpl as __setGetConfigValueForTest,
} from "./mocks/golem-agent-host.js"
import { toWitCodec } from "../src/WitCodec.js"
import {
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "../src/internal/snapshotEnvelope.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v, type SchemaValue } from "../src/internal/schema-model/model.js"

const anonymous = { tag: "anonymous" } as const
const wire = (...fields: Array<SchemaValue>) => schemaValueToWit(v.record(fields))
const typed = (value: ReturnType<typeof wire>) => ({ value }) as never
const read = <A>(value: NonNullable<Awaited<ReturnType<typeof guest.invoke>>>): A => {
  const decoded = schemaValueFromWit(value)
  if (decoded.tag !== "string" && decoded.tag !== "f64") {
    throw new Error(`Expected a scalar value, got ${decoded.tag}`)
  }
  return decoded.value as A
}
const oidcBob = {
  tag: "oidc",
  val: { sub: "bob", issuer: "https://example.test", claims: "{}" },
} as const

// ---------------------------------------------------------------------------
// Auto-snapshot agent
// ---------------------------------------------------------------------------

const AutoSnapshotCounter = defineAgent({
  name: "AutoSnapshotCounter",
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({
      count: Schema.Number,
      owner: Schema.String,
      groups: Schema.Record(
        Schema.String,
        Schema.Struct({
          values: Schema.Array(Schema.Number),
          note: Schema.optional(Schema.String),
        }),
      ),
    }),
    policy: Snapshot.policy.everyN(5),
  }),
  methods: {
    value: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
    owner: method({ input: {}, success: Schema.String }),
  },
}).implement(
  ({ name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({
        count: 0,
        owner: name,
        groups: {
          primary: { values: [1, 2, 3], note: "nested record" },
          secondary: { values: [] },
        },
      })
      return {
        value: () => Ref.get(state).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(state, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
        owner: () => Ref.get(state).pipe(Effect.map((s) => s.owner)),
      }
    }),
  (_context, { name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ count: 0, owner: name, groups: {} })
      return {
        value: () => Ref.get(state).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(state, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
        owner: () => Ref.get(state).pipe(Effect.map((s) => s.owner)),
      }
    }),
)

// ---------------------------------------------------------------------------
// Custom-snapshot agent
// ---------------------------------------------------------------------------

const customStore = {
  saveCalls: 0,
  loadCalls: 0,
}

const CustomSnapshotAgent = defineAgent({
  name: "CustomSnapshotAgent",
  id: { name: Schema.String },
  snapshotting: Snapshot.custom({ policy: Snapshot.policy.periodic(Duration.seconds(30)) }),
  methods: {
    value: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
  },
}).implement(
  ({ name }, snap) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make({ count: 0, owner: name })
      yield* snap.register({
        save: Effect.gen(function* () {
          customStore.saveCalls++
          const s = yield* Ref.get(ref)
          return new TextEncoder().encode(JSON.stringify(s))
        }),
        load: (bytes) =>
          Effect.gen(function* () {
            customStore.loadCalls++
            const decoded = JSON.parse(new TextDecoder().decode(bytes)) as {
              count: number
              owner: string
            }
            yield* Ref.set(ref, decoded)
          }),
      })
      return {
        value: () => Ref.get(ref).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(ref, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
      }
    }),
  (_context, { name }, snap) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make({ count: 0, owner: name })
      yield* snap.register({
        save: Ref.get(ref).pipe(Effect.map((s) => new TextEncoder().encode(JSON.stringify(s)))),
        load: (bytes) =>
          Effect.gen(function* () {
            customStore.loadCalls++
            yield* Ref.set(ref, JSON.parse(new TextDecoder().decode(bytes)))
          }),
      })
      return {
        value: () => Ref.get(ref).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(ref, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
      }
    }),
)

// ---------------------------------------------------------------------------
// Agent that declares snapshot but never binds (negative test)
// ---------------------------------------------------------------------------

const ForgetfulSnapshotAgent = defineAgent({
  name: "ForgetfulSnapshotAgent",
  id: {},
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.default,
  }),
  methods: {
    noop: method({ input: {}, success: Schema.Void }),
  },
})
  // Intentionally never call snap.init: triggers SnapshotNotBoundError.
  .implement(
    (_, _snap) =>
      Effect.succeed({
        noop: () => Effect.void,
      }),
    (_context, _input, _snap) => Effect.succeed({ noop: () => Effect.void }),
  )

// ---------------------------------------------------------------------------
// Custom-snapshot agent that reads a config service inside its save/load
// handlers — exercises the §E.2.a wiring in `dispatchSaveSnapshot` /
// `dispatchLoadSnapshot` (see notes/config-audit-and-plan.md).
// ---------------------------------------------------------------------------

class ConfigCustomCfg extends defineConfig("ConfigCustomCfg", {
  prefix: Schema.String,
}) {}

const configCustomStore: { saveCalls: number; loadCalls: number; lastBytes: Uint8Array | null } = {
  saveCalls: 0,
  loadCalls: 0,
  lastBytes: null,
}

const ConfigCustomAgent = defineAgent({
  name: "ConfigCustomAgent",
  id: { name: Schema.String },
  config: ConfigCustomCfg,
  snapshotting: Snapshot.custom({ policy: Snapshot.policy.default }),
  methods: {
    value: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
  },
}).implement(
  ({ name }, snap) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make({ count: 0, owner: name })
      yield* snap.register({
        save: Effect.gen(function* () {
          configCustomStore.saveCalls++
          // Read the config service from inside the save handler.
          const cfg = yield* ConfigCustomCfg
          const prefix = yield* cfg.prefix
          const s = yield* Ref.get(ref)
          return new TextEncoder().encode(`${prefix}:${JSON.stringify(s)}`)
        }),
        load: (bytes) =>
          Effect.gen(function* () {
            configCustomStore.loadCalls++
            // Read the config service from inside the load handler too.
            const cfg = yield* ConfigCustomCfg
            const prefix = yield* cfg.prefix
            const text = new TextDecoder().decode(bytes)
            if (!text.startsWith(`${prefix}:`)) {
              throw new Error(
                `load handler: payload prefix '${text.slice(0, 20)}' does not match config prefix '${prefix}'`,
              )
            }
            const decoded = JSON.parse(text.slice(prefix.length + 1)) as {
              count: number
              owner: string
            }
            yield* Ref.set(ref, decoded)
          }),
      })
      return {
        value: () => Ref.get(ref).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(ref, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
      }
    }),
  (_context, { name }, snap) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make({ count: 0, owner: name })
      yield* snap.register({
        save: Ref.get(ref).pipe(Effect.map((s) => new TextEncoder().encode(JSON.stringify(s)))),
        load: (bytes) =>
          Effect.gen(function* () {
            configCustomStore.loadCalls++
            const cfg = yield* ConfigCustomCfg
            const prefix = yield* cfg.prefix
            const text = new TextDecoder().decode(bytes)
            if (!text.startsWith(`${prefix}:`))
              throw new Error(`does not match config prefix '${prefix}'`)
            yield* Ref.set(ref, JSON.parse(text.slice(prefix.length + 1)))
          }),
      })
      return {
        value: () => Ref.get(ref).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(ref, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
      }
    }),
)

describe("snapshotting", () => {
  beforeEach(async () => {
    await __resetAgents()
    customStore.saveCalls = 0
    customStore.loadCalls = 0
    configCustomStore.saveCalls = 0
    configCustomStore.loadCalls = 0
    configCustomStore.lastBytes = null
    void AutoSnapshotCounter
    void CustomSnapshotAgent
    void ConfigCustomAgent
    void ForgetfulSnapshotAgent
  })

  afterEach(() => {
    __resetGetEnvironmentForTest()
    __resetParseAgentIdForTest()
    __resetGetConfigValueForTest()
  })

  // -------------------------------------------------------------------------
  // Metadata
  // -------------------------------------------------------------------------

  it.effect("reflects auto snapshot policy in AgentType metadata", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const t = types.find((t) => t.typeName === "AutoSnapshotCounter")!
      expect(t.snapshotting).toEqual({
        tag: "enabled",
        val: { tag: "every-n-invocation", val: 5 },
      })
    }),
  )

  it.effect("reflects custom snapshot policy in AgentType metadata", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const t = types.find((t) => t.typeName === "CustomSnapshotAgent")!
      expect(t.snapshotting.tag).toBe("enabled")
      if (t.snapshotting.tag !== "enabled") throw new Error()
      expect(t.snapshotting.val.tag).toBe("periodic")
      if (t.snapshotting.val.tag !== "periodic") throw new Error()
      // 30 s in nanoseconds
      expect(t.snapshotting.val.val).toBe(30_000_000_000n)
    }),
  )

  it.effect("agents without `snapshot` keep snapshotting=disabled (regression)", () =>
    Effect.gen(function* () {
      const NoSnapAgent = defineAgent({
        name: "NoSnapAgent",
        id: {},
        methods: { ping: method({ input: {}, success: Schema.Void }) },
      }).implement(() => Effect.succeed({ ping: () => Effect.void }))
      void NoSnapAgent
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const t = types.find((t) => t.typeName === "NoSnapAgent")!
      expect(t.snapshotting).toEqual({ tag: "disabled" })
    }),
  )

  // -------------------------------------------------------------------------
  // Auto round-trip
  // -------------------------------------------------------------------------

  it.effect("auto: save → load round-trips state and uses JSON envelope", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      const numberCodec = yield* toWitCodec(Schema.Number)
      const aliceWv = yield* Schema.encodeEffect(stringCodec.codec)("alice")
      const sevenWv = yield* Schema.encodeEffect(numberCodec.codec)(7)

      yield* Effect.promise(() => guest.initialize("AutoSnapshotCounter", wire(aliceWv), oidcBob))
      yield* Effect.promise(() => guest.invoke("add", wire(sevenWv), oidcBob))
      yield* Effect.promise(() => guest.invoke("add", wire(sevenWv), oidcBob))

      const snapshot = yield* Effect.promise(() => dispatchSaveSnapshot())
      expect(snapshot.mimeType).toBe("application/json")
      const obj = JSON.parse(new TextDecoder().decode(snapshot.payload))
      expect(obj.version).toBe(1)
      expect(obj.principal).toEqual({
        tag: "oidc",
        val: {
          sub: "bob",
          issuer: "https://example.test",
          email: null,
          name: null,
          emailVerified: null,
          givenName: null,
          familyName: null,
          picture: null,
          preferredUsername: null,
          claims: "{}",
        },
      })
      expect(obj.state).toEqual({
        count: 14,
        owner: "alice",
        groups: {
          primary: { values: [1, 2, 3], note: "nested record" },
          secondary: { values: [] },
        },
      })

      // Reset (simulating new container) and load.
      yield* Effect.promise(() => __resetAgents())

      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:alice"]])
      __setParseAgentIdForTest(() => ["AutoSnapshotCounter", typed(wire(aliceWv)), undefined])

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))

      const out = yield* Effect.promise(() => guest.invoke("value", wire(), oidcBob))
      const value = read<number>(out!)
      expect(value).toBe(14)

      const ownerOut = yield* Effect.promise(() => guest.invoke("owner", wire(), oidcBob))
      const owner = read<string>(ownerOut!)
      expect(owner).toBe("alice")
    }),
  )

  it("auto: load rejects an envelope whose mime type is binary", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const aliceWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("alice"))

    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:alice"]])
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", typed(wire(aliceWv)), undefined])

    const wrong = encodeBinaryEnvelope(anonymous, new Uint8Array([1, 2, 3]))
    await expect(dispatchLoadSnapshot(wrong)).rejects.toThrow(SnapshotEnvelopeError)
  })

  it("auto: load rejects record state that does not match the private snapshot schema", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const aliceWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("alice"))

    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:alice"]])
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", typed(wire(aliceWv)), undefined])

    const malformed = encodeJsonEnvelope(anonymous, {
      count: 1,
      owner: "alice",
      groups: { broken: { values: ["not-a-number"] } },
    })
    await expect(dispatchLoadSnapshot(malformed)).rejects.toThrow()
  })

  // -------------------------------------------------------------------------
  // Custom round-trip
  // -------------------------------------------------------------------------

  it.effect("custom: save → load round-trips via user-supplied bytes", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      const numberCodec = yield* toWitCodec(Schema.Number)
      const carolWv = yield* Schema.encodeEffect(stringCodec.codec)("carol")
      const threeWv = yield* Schema.encodeEffect(numberCodec.codec)(3)

      yield* Effect.promise(() => guest.initialize("CustomSnapshotAgent", wire(carolWv), oidcBob))
      yield* Effect.promise(() => guest.invoke("add", wire(threeWv), oidcBob))
      yield* Effect.promise(() => guest.invoke("add", wire(threeWv), oidcBob))

      const snapshot = yield* Effect.promise(() => dispatchSaveSnapshot())
      expect(snapshot.mimeType).toBe("application/octet-stream")
      expect(customStore.saveCalls).toBe(1)

      yield* Effect.promise(() => __resetAgents())
      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "CustomSnapshotAgent:carol"]])
      __setParseAgentIdForTest(() => ["CustomSnapshotAgent", typed(wire(carolWv)), undefined])

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))
      expect(customStore.loadCalls).toBe(1)

      const out = yield* Effect.promise(() => guest.invoke("value", wire(), oidcBob))
      const value = read<number>(out!)
      expect(value).toBe(6)
    }),
  )

  it.effect("custom: save/load handlers can read the agent's config service", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      const numberCodec = yield* toWitCodec(Schema.Number)
      const danWv = yield* Schema.encodeEffect(stringCodec.codec)("dan")
      const fourWv = yield* Schema.encodeEffect(numberCodec.codec)(4)

      const prefixWv = yield* Schema.encodeEffect(stringCodec.codec)("snapshot-prefix")
      __setGetConfigValueForTest((path) => {
        if (path.length === 1 && path[0] === "prefix") return schemaValueToWit(prefixWv)
        throw new Error(`unexpected config path: ${path.join(".")}`)
      })

      yield* Effect.promise(() => guest.initialize("ConfigCustomAgent", wire(danWv), oidcBob))
      yield* Effect.promise(() => guest.invoke("add", wire(fourWv), oidcBob))

      const snapshot = yield* Effect.promise(() => dispatchSaveSnapshot())
      expect(snapshot.mimeType).toBe("application/octet-stream")
      expect(configCustomStore.saveCalls).toBe(1)

      yield* Effect.promise(() => __resetAgents())
      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "ConfigCustomAgent:dan"]])
      __setParseAgentIdForTest(() => ["ConfigCustomAgent", typed(wire(danWv)), undefined])
      __setGetConfigValueForTest((path) => {
        if (path.length === 1 && path[0] === "prefix") return schemaValueToWit(prefixWv)
        throw new Error(`unexpected config path: ${path.join(".")}`)
      })

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))
      expect(configCustomStore.loadCalls).toBe(1)

      const out = yield* Effect.promise(() => guest.invoke("value", wire(), oidcBob))
      const value = read<number>(out!)
      expect(value).toBe(4)
    }),
  )

  it("custom: load handler that misuses config surfaces the failure as a thrown error", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const danWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("dan"))

    // First save with prefix=A, then load with prefix=B → load handler
    // throws because the embedded bytes don't begin with "B:".
    const prefixA = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("A"))
    __setGetConfigValueForTest(() => schemaValueToWit(prefixA))

    await guest.initialize("ConfigCustomAgent", wire(danWv), oidcBob)
    const snapshot = await dispatchSaveSnapshot()

    await __resetAgents()
    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "ConfigCustomAgent:dan"]])
    __setParseAgentIdForTest(() => ["ConfigCustomAgent", typed(wire(danWv)), undefined])
    const prefixB = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("B"))
    __setGetConfigValueForTest(() => schemaValueToWit(prefixB))

    await expect(dispatchLoadSnapshot(snapshot)).rejects.toThrow(/does not match config prefix/)
  })

  it("custom: load rejects an envelope whose mime type is JSON", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const carolWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("carol"))

    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "CustomSnapshotAgent:carol"]])
    __setParseAgentIdForTest(() => ["CustomSnapshotAgent", typed(wire(carolWv)), undefined])

    const wrong = encodeJsonEnvelope(anonymous, { count: 0, owner: "carol" })
    await expect(dispatchLoadSnapshot(wrong)).rejects.toThrow(SnapshotEnvelopeError)
  })

  // -------------------------------------------------------------------------
  // Mutual exclusion + binding errors
  // -------------------------------------------------------------------------

  it("save without an active agent fails", async () => {
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/not initialized/)
  })

  it("save fails for an active agent that did not declare a snapshot", async () => {
    const NoSnap = defineAgent({
      name: "NoSnap",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }).implement(() => Effect.succeed({ ping: () => Effect.void }))
    void NoSnap
    await guest.initialize("NoSnap", wire(), anonymous)
    await expect(dispatchSaveSnapshot()).rejects.toThrow(/did not declare a snapshot/)
  })

  it("load fails when an agent is already initialized", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const aliceWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("alice"))
    await guest.initialize("AutoSnapshotCounter", wire(aliceWv), oidcBob)
    const fake = encodeJsonEnvelope(anonymous, { count: 0, owner: "alice" })
    await expect(dispatchLoadSnapshot(fake)).rejects.toThrow(/already initialized/)
  })

  it("load fails when GOLEM_AGENT_ID is missing", async () => {
    __setGetEnvironmentForTest([])
    const fake = encodeJsonEnvelope(anonymous, { count: 0, owner: "x" })
    await expect(dispatchLoadSnapshot(fake)).rejects.toThrow(/GOLEM_AGENT_ID/)
  })

  it("load rejects malformed multipart/mixed envelopes (no body)", async () => {
    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:x"]])
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", typed(wire()), undefined])
    await expect(
      dispatchLoadSnapshot({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed; boundary=abc",
      }),
    ).rejects.toThrow(SnapshotEnvelopeError)
  })

  it("load rejects multipart/mixed without a boundary parameter", async () => {
    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:x"]])
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", typed(wire()), undefined])
    await expect(
      dispatchLoadSnapshot({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed",
      }),
    ).rejects.toThrow(UnsupportedSnapshotFormatError)
  })

  it("declared snapshot but unbound impl → SnapshotNotBoundError on initialize", async () => {
    await expect(guest.initialize("ForgetfulSnapshotAgent", wire(), anonymous)).rejects.toThrow(
      /SnapshotNotBoundError|did not bind/,
    )
  })

  it("auto: snap.init called twice fails the second time", async () => {
    const TwiceBound = defineAgent({
      name: "TwiceBound",
      id: {},
      snapshotting: Snapshot.define({
        schema: Schema.Number,
        policy: Snapshot.policy.default,
      }),
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }).implement(
      (_input, snap) =>
        Effect.gen(function* () {
          yield* snap.init(0)
          yield* snap.init(1) // boom
          return { ping: () => Effect.void }
        }),
      (_context, _input, snap) =>
        snap.init(0).pipe(Effect.map(() => ({ ping: () => Effect.void }))),
    )
    void TwiceBound
    await expect(guest.initialize("TwiceBound", wire(), anonymous)).rejects.toThrow(
      /SnapshotAlreadyBoundError|already bound/,
    )
  })
})
