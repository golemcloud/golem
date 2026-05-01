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
import { guest } from "../src/Exports.js"
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
} from "../src/SnapshotEnvelope.js"

const anonymous = { tag: "anonymous" } as const
const oidcBob = {
  tag: "oidc",
  val: { sub: "bob", issuer: "https://example.test", claims: "{}" },
} as const

// ---------------------------------------------------------------------------
// Auto-snapshot agent
// ---------------------------------------------------------------------------

const AutoSnapshotCounter = defineAgent({
  name: "AutoSnapshotCounter",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number, owner: Schema.String }),
    policy: Snapshot.policy.everyN(5),
  }),
  methods: {
    value: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
    owner: method({ params: {}, success: Schema.String }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ count: 0, owner: name })
      return {
        value: () => Ref.get(state).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(state, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
        owner: () => Ref.get(state).pipe(Effect.map((s) => s.owner)),
      }
    }),
})

// ---------------------------------------------------------------------------
// Custom-snapshot agent
// ---------------------------------------------------------------------------

const customStore = {
  saveCalls: 0,
  loadCalls: 0,
}

const CustomSnapshotAgent = defineAgent({
  name: "CustomSnapshotAgent",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.custom({ policy: Snapshot.policy.periodic(Duration.seconds(30)) }),
  methods: {
    value: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
  },
  impl: ({ name }, snap) =>
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
})

// ---------------------------------------------------------------------------
// Agent that declares snapshot but never binds (negative test)
// ---------------------------------------------------------------------------

const ForgetfulSnapshotAgent = defineAgent({
  name: "ForgetfulSnapshotAgent",
  constructorParams: {},
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.default,
  }),
  methods: {
    noop: method({ params: {}, success: Schema.Void }),
  },
  // Intentionally never call snap.init: triggers SnapshotNotBoundError.
  impl: (_, _snap) =>
    Effect.succeed({
      noop: () => Effect.void,
    }),
})

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
  constructorParams: { name: Schema.String },
  config: ConfigCustomCfg,
  snapshot: Snapshot.custom({ policy: Snapshot.policy.default }),
  methods: {
    value: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
  },
  impl: ({ name }, snap) =>
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
})

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
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const t = types.find((t) => t.typeName === "AutoSnapshotCounter")!
      expect(t.snapshotting).toEqual({
        tag: "enabled",
        val: { tag: "every-n-invocation", val: 5 },
      })
    }),
  )

  it.effect("reflects custom snapshot policy in AgentType metadata", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
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
        constructorParams: {},
        methods: { ping: method({ params: {}, success: Schema.Void }) },
        impl: () => Effect.succeed({ ping: () => Effect.void }),
      })
      void NoSnapAgent
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
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

      yield* Effect.promise(() =>
        guest.initialize(
          "AutoSnapshotCounter",
          { tag: "tuple", val: [{ tag: "component-model", val: aliceWv }] },
          oidcBob,
        ),
      )
      yield* Effect.promise(() =>
        guest.invoke(
          "add",
          { tag: "tuple", val: [{ tag: "component-model", val: sevenWv }] },
          oidcBob,
        ),
      )
      yield* Effect.promise(() =>
        guest.invoke(
          "add",
          { tag: "tuple", val: [{ tag: "component-model", val: sevenWv }] },
          oidcBob,
        ),
      )

      const snapshot = yield* Effect.promise(() => Promise.resolve(dispatchSaveSnapshot()))
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
      expect(obj.state).toEqual({ count: 14, owner: "alice" })

      // Reset (simulating new container) and load.
      yield* Effect.promise(() => __resetAgents())

      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:alice"]])
      __setParseAgentIdForTest(() => [
        "AutoSnapshotCounter",
        { tag: "tuple", val: [{ tag: "component-model", val: aliceWv }] },
        undefined,
      ])

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))

      const out = yield* Effect.promise(() =>
        guest.invoke("value", { tag: "tuple", val: [] }, oidcBob),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") throw new Error()
      const value = yield* Schema.decodeEffect(numberCodec.codec)(out.val[0].val)
      expect(value).toBe(14)

      const ownerOut = yield* Effect.promise(() =>
        guest.invoke("owner", { tag: "tuple", val: [] }, oidcBob),
      )
      if (ownerOut.tag !== "tuple" || ownerOut.val[0]?.tag !== "component-model") throw new Error()
      const owner = yield* Schema.decodeEffect(stringCodec.codec)(ownerOut.val[0].val)
      expect(owner).toBe("alice")
    }),
  )

  it("auto: load rejects an envelope whose mime type is binary", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const aliceWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("alice"))

    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:alice"]])
    __setParseAgentIdForTest(() => [
      "AutoSnapshotCounter",
      { tag: "tuple", val: [{ tag: "component-model", val: aliceWv }] },
      undefined,
    ])

    const wrong = encodeBinaryEnvelope(anonymous, new Uint8Array([1, 2, 3]))
    await expect(dispatchLoadSnapshot(wrong)).rejects.toThrow(SnapshotEnvelopeError)
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

      yield* Effect.promise(() =>
        guest.initialize(
          "CustomSnapshotAgent",
          { tag: "tuple", val: [{ tag: "component-model", val: carolWv }] },
          oidcBob,
        ),
      )
      yield* Effect.promise(() =>
        guest.invoke(
          "add",
          { tag: "tuple", val: [{ tag: "component-model", val: threeWv }] },
          oidcBob,
        ),
      )
      yield* Effect.promise(() =>
        guest.invoke(
          "add",
          { tag: "tuple", val: [{ tag: "component-model", val: threeWv }] },
          oidcBob,
        ),
      )

      const snapshot = yield* Effect.promise(() => Promise.resolve(dispatchSaveSnapshot()))
      expect(snapshot.mimeType).toBe("application/octet-stream")
      expect(customStore.saveCalls).toBe(1)

      yield* Effect.promise(() => __resetAgents())
      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "CustomSnapshotAgent:carol"]])
      __setParseAgentIdForTest(() => [
        "CustomSnapshotAgent",
        { tag: "tuple", val: [{ tag: "component-model", val: carolWv }] },
        undefined,
      ])

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))
      expect(customStore.loadCalls).toBe(1)

      const out = yield* Effect.promise(() =>
        guest.invoke("value", { tag: "tuple", val: [] }, oidcBob),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") throw new Error()
      const value = yield* Schema.decodeEffect(numberCodec.codec)(out.val[0].val)
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
        if (path.length === 1 && path[0] === "prefix") return prefixWv
        throw new Error(`unexpected config path: ${path.join(".")}`)
      })

      yield* Effect.promise(() =>
        guest.initialize(
          "ConfigCustomAgent",
          { tag: "tuple", val: [{ tag: "component-model", val: danWv }] },
          oidcBob,
        ),
      )
      yield* Effect.promise(() =>
        guest.invoke(
          "add",
          { tag: "tuple", val: [{ tag: "component-model", val: fourWv }] },
          oidcBob,
        ),
      )

      const snapshot = yield* Effect.promise(() => Promise.resolve(dispatchSaveSnapshot()))
      expect(snapshot.mimeType).toBe("application/octet-stream")
      expect(configCustomStore.saveCalls).toBe(1)

      yield* Effect.promise(() => __resetAgents())
      __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "ConfigCustomAgent:dan"]])
      __setParseAgentIdForTest(() => [
        "ConfigCustomAgent",
        { tag: "tuple", val: [{ tag: "component-model", val: danWv }] },
        undefined,
      ])
      __setGetConfigValueForTest((path) => {
        if (path.length === 1 && path[0] === "prefix") return prefixWv
        throw new Error(`unexpected config path: ${path.join(".")}`)
      })

      yield* Effect.promise(() => dispatchLoadSnapshot(snapshot))
      expect(configCustomStore.loadCalls).toBe(1)

      const out = yield* Effect.promise(() =>
        guest.invoke("value", { tag: "tuple", val: [] }, oidcBob),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") throw new Error()
      const value = yield* Schema.decodeEffect(numberCodec.codec)(out.val[0].val)
      expect(value).toBe(4)
    }),
  )

  it("custom: load handler that misuses config surfaces the failure as a thrown error", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const danWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("dan"))

    // First save with prefix=A, then load with prefix=B → load handler
    // throws because the embedded bytes don't begin with "B:".
    const prefixA = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("A"))
    __setGetConfigValueForTest(() => prefixA)

    await guest.initialize(
      "ConfigCustomAgent",
      { tag: "tuple", val: [{ tag: "component-model", val: danWv }] },
      oidcBob,
    )
    const snapshot = await dispatchSaveSnapshot()

    await __resetAgents()
    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "ConfigCustomAgent:dan"]])
    __setParseAgentIdForTest(() => [
      "ConfigCustomAgent",
      { tag: "tuple", val: [{ tag: "component-model", val: danWv }] },
      undefined,
    ])
    const prefixB = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("B"))
    __setGetConfigValueForTest(() => prefixB)

    await expect(dispatchLoadSnapshot(snapshot)).rejects.toThrow(/does not match config prefix/)
  })

  it("custom: load rejects an envelope whose mime type is JSON", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const carolWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("carol"))

    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "CustomSnapshotAgent:carol"]])
    __setParseAgentIdForTest(() => [
      "CustomSnapshotAgent",
      { tag: "tuple", val: [{ tag: "component-model", val: carolWv }] },
      undefined,
    ])

    const wrong = encodeJsonEnvelope(anonymous, { count: 0, owner: "carol" })
    await expect(dispatchLoadSnapshot(wrong)).rejects.toThrow(SnapshotEnvelopeError)
  })

  // -------------------------------------------------------------------------
  // Mutual exclusion + binding errors
  // -------------------------------------------------------------------------

  it("save without an active agent fails", async () => {
    // dispatchSaveSnapshot is intentionally synchronous on its error
    // paths and on the auto path (see comment on dispatchSaveSnapshot
    // for why); use `expect(fn).toThrow` instead of
    // `await expect(promise).rejects.toThrow`.
    expect(() => dispatchSaveSnapshot()).toThrow(/not initialized/)
  })

  it("save fails for an active agent that did not declare a snapshot", async () => {
    const NoSnap = defineAgent({
      name: "NoSnap",
      constructorParams: {},
      methods: { ping: method({ params: {}, success: Schema.Void }) },
      impl: () => Effect.succeed({ ping: () => Effect.void }),
    })
    void NoSnap
    await guest.initialize("NoSnap", { tag: "tuple", val: [] }, anonymous)
    expect(() => dispatchSaveSnapshot()).toThrow(/did not declare a snapshot/)
  })

  it("load fails when an agent is already initialized", async () => {
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const aliceWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("alice"))
    await guest.initialize(
      "AutoSnapshotCounter",
      { tag: "tuple", val: [{ tag: "component-model", val: aliceWv }] },
      oidcBob,
    )
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
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", { tag: "tuple", val: [] }, undefined])
    await expect(
      dispatchLoadSnapshot({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed; boundary=abc",
      }),
    ).rejects.toThrow(SnapshotEnvelopeError)
  })

  it("load rejects multipart/mixed without a boundary parameter", async () => {
    __setGetEnvironmentForTest([["GOLEM_AGENT_ID", "AutoSnapshotCounter:x"]])
    __setParseAgentIdForTest(() => ["AutoSnapshotCounter", { tag: "tuple", val: [] }, undefined])
    await expect(
      dispatchLoadSnapshot({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed",
      }),
    ).rejects.toThrow(UnsupportedSnapshotFormatError)
  })

  it("declared snapshot but unbound impl → SnapshotNotBoundError on initialize", async () => {
    await expect(
      guest.initialize("ForgetfulSnapshotAgent", { tag: "tuple", val: [] }, anonymous),
    ).rejects.toThrow(/SnapshotNotBoundError|did not bind/)
  })

  it("auto: snap.init called twice fails the second time", async () => {
    const TwiceBound = defineAgent({
      name: "TwiceBound",
      constructorParams: {},
      snapshot: Snapshot.define({
        schema: Schema.Number,
        policy: Snapshot.policy.default,
      }),
      methods: { ping: method({ params: {}, success: Schema.Void }) },
      impl: (_input, snap) =>
        Effect.gen(function* () {
          yield* snap.init(0)
          yield* snap.init(1) // boom
          return { ping: () => Effect.void }
        }),
    })
    void TwiceBound
    await expect(
      guest.initialize("TwiceBound", { tag: "tuple", val: [] }, anonymous),
    ).rejects.toThrow(/SnapshotAlreadyBoundError|already bound/)
  })
})
