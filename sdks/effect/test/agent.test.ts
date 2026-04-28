import { describe, expect, it, beforeEach } from "@effect/vitest"
import { Effect, Fiber, Redacted, Ref, Schema } from "effect"
import { defineAgent, __resetAgents } from "../src/agent.js"
import { method } from "../src/method.js"
import { guest } from "../src/exports.js"
import { Principal, type PrincipalValue } from "../src/principal.js"
import { toWitCodec } from "../src/wit-codec.js"
import {
  __resetGetConfigValueForTest,
  __setGetConfigValueForTest,
  defineConfig,
} from "../src/config.js"

const Person = Schema.Struct({
  name: Schema.String,
  age: Schema.Number,
})

const Greeter = defineAgent({
  name: "Greeter",
  description: "An agent that greets people",
  constructorParams: {},
  methods: {
    greet: method({
      params: { person: Person, greeting: Schema.String },
      success: Schema.String,
      description: "Greet the given person with the given greeting",
      promptHint: "Use to produce a friendly salutation for a Person.",
    }),
    ping: method({ params: {}, success: Schema.Void }),
  },
  impl: () =>
    Effect.succeed({
      greet: ({ person, greeting }) =>
        Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
      ping: () => Effect.void,
    }),
})

/** A stateful agent that exercises the closure-based state pattern, plus a
 *  side-effect during initialization. */
const Counter = defineAgent({
  name: "Counter",
  constructorParams: { initial: Schema.Number },
  methods: {
    getValue: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Void }),
  },
  impl: ({ initial }) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make(initial)
      return {
        getValue: () => Ref.get(ref),
        add: ({ by }) => Ref.update(ref, (n) => n + by),
      }
    }),
})

const anonymousPrincipal = { tag: "anonymous" } as const

const oidcPrincipal = (sub: string): PrincipalValue => ({
  tag: "oidc",
  val: {
    sub,
    issuer: "https://example.test",
    claims: "{}",
  },
})

const principalTag = (p: PrincipalValue): string => (p.tag === "oidc" ? `oidc:${p.val.sub}` : p.tag)

/**
 * Exercises the Principal service in BOTH the constructor effect (where
 * it should resolve to the initialize-time principal) AND a method
 * handler (where it should resolve to the per-call principal — which
 * may differ from the initialize-time one).
 */
const PrincipalAgent = defineAgent({
  name: "PrincipalAgent",
  constructorParams: {},
  methods: {
    owner: method({ params: {}, success: Schema.String }),
    caller: method({ params: {}, success: Schema.String }),
    callerForked: method({ params: {}, success: Schema.String }),
  },
  impl: () =>
    Effect.gen(function* () {
      const ownerPrincipal = yield* Principal
      const owner = principalTag(ownerPrincipal)
      return {
        owner: () => Effect.succeed(owner),
        caller: () =>
          Effect.gen(function* () {
            const callerPrincipal = yield* Principal
            return principalTag(callerPrincipal)
          }),
        // Verifies that the per-call Principal service propagates to
        // child fibers spawned inside a handler.
        callerForked: () =>
          Effect.gen(function* () {
            const fiber = yield* Effect.forkChild(
              Effect.gen(function* () {
                const childPrincipal = yield* Principal
                return principalTag(childPrincipal)
              }),
            )
            return yield* Fiber.join(fiber)
          }),
      }
    }),
})

/**
 * Exercises an Effect-Context-based config service from BOTH the
 * constructor and a method handler. The mocked host responds via the
 * `__setGetConfigValueForTest` shim; the test verifies that:
 *
 * - regular fields are memoized for the duration of one invocation,
 * - secret fields hit the host every read,
 * - mock changes between invocations are observed (no per-instance
 *   stickiness).
 */
class TestConfig extends defineConfig("ConfigAgent.Cfg", {
  greeting: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
}) {}

const ConfigAgent = defineAgent({
  name: "ConfigAgent",
  config: TestConfig,
  constructorParams: {},
  methods: {
    initialGreeting: method({ params: {}, success: Schema.String }),
    currentGreeting: method({ params: {}, success: Schema.String }),
    keyTail: method({ params: {}, success: Schema.String }),
  },
  impl: () =>
    Effect.gen(function* () {
      const cfg = yield* TestConfig
      // Reads the greeting at *initialize* time and captures it; later
      // shape rebuilds (one per invocation) won't mutate this closure.
      const initial = yield* cfg.greeting
      return {
        initialGreeting: () => Effect.succeed(initial),
        currentGreeting: () =>
          Effect.gen(function* () {
            const c = yield* TestConfig
            // Reading the same field twice inside one invocation must
            // hit the host only once (memoization within one shape).
            const a = yield* c.greeting
            const b = yield* c.greeting
            return `${a}/${b}`
          }),
        keyTail: () =>
          Effect.gen(function* () {
            const c = yield* TestConfig
            const r = yield* c.apiKey.get
            const raw = Redacted.value(r)
            return raw.slice(-4)
          }),
      }
    }),
})

describe("agent-guest exports", () => {
  beforeEach(async () => {
    await __resetAgents()
    __resetGetConfigValueForTest()
    // Touch the agent values so this test file isn't dead-code-eliminated
    // (their `defineAgent` calls auto-register at import time).
    void Greeter
    void Counter
    void PrincipalAgent
    void ConfigAgent
  })

  it.effect("discoverAgentTypes returns the registered agents", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      expect(types.map((t) => t.typeName).sort()).toEqual([
        "ConfigAgent",
        "Counter",
        "Greeter",
        "PrincipalAgent",
      ])
      const greeter = types.find((t) => t.typeName === "Greeter")!
      expect(greeter.methods.map((m) => m.name).sort()).toEqual(["greet", "ping"])
    }),
  )

  it.effect("propagates method-level description and promptHint to AgentMethod", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const greeter = types.find((t) => t.typeName === "Greeter")!
      const greet = greeter.methods.find((m) => m.name === "greet")!
      expect(greet.description).toBe("Greet the given person with the given greeting")
      expect(greet.promptHint).toBe("Use to produce a friendly salutation for a Person.")
      const ping = greeter.methods.find((m) => m.name === "ping")!
      // Unset description defaults to "" (matches WIT `description: string`).
      expect(ping.description).toBe("")
      expect(ping.promptHint).toBeUndefined()
    }),
  )

  it.effect("registers config declarations on the AgentType", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const cfgAgent = types.find((t) => t.typeName === "ConfigAgent")!
      const paths = cfgAgent.config.map((d) => ({ source: d.source, path: d.path }))
      expect(paths).toEqual([
        { source: "local", path: ["greeting"] },
        { source: "secret", path: ["apiKey"] },
      ])
    }),
  )

  it.effect("initialize + invoke + getDefinition round-trip a greet call", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        guest.initialize("Greeter", { tag: "tuple", val: [] }, anonymousPrincipal),
      )

      const def = yield* Effect.promise(() => guest.getDefinition())
      expect(def.typeName).toBe("Greeter")

      const personCodec = yield* toWitCodec(Person)
      const stringCodec = yield* toWitCodec(Schema.String)
      const personWv = yield* Schema.encodeEffect(personCodec.codec)({ name: "Ada", age: 36 })
      const greetingWv = yield* Schema.encodeEffect(stringCodec.codec)("Hello")

      const out = yield* Effect.promise(() =>
        guest.invoke(
          "greet",
          {
            tag: "tuple",
            val: [
              { tag: "component-model", val: personWv },
              { tag: "component-model", val: greetingWv },
            ],
          },
          anonymousPrincipal,
        ),
      )

      if (out.tag !== "tuple" || out.val.length !== 1) throw new Error()
      const elem = out.val[0]!
      if (elem.tag !== "component-model") throw new Error()
      const decoded = yield* Schema.decodeEffect(stringCodec.codec)(elem.val)
      expect(decoded).toBe("Hello, Ada (36)!")
    }),
  )

  it.effect("invoke returns an empty tuple for unit-returning methods", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        guest.initialize("Greeter", { tag: "tuple", val: [] }, anonymousPrincipal),
      )
      const out = yield* Effect.promise(() =>
        guest.invoke("ping", { tag: "tuple", val: [] }, anonymousPrincipal),
      )
      expect(out).toEqual({ tag: "tuple", val: [] })
    }),
  )

  it.effect("preserves state across calls (Counter)", () =>
    Effect.gen(function* () {
      const numberCodec = yield* toWitCodec(Schema.Number)

      // initialize Counter with initial = 10
      const initialWv = yield* Schema.encodeEffect(numberCodec.codec)(10)
      yield* Effect.promise(() =>
        guest.initialize(
          "Counter",
          { tag: "tuple", val: [{ tag: "component-model", val: initialWv }] },
          anonymousPrincipal,
        ),
      )

      // add 5 twice
      const fiveWv = yield* Schema.encodeEffect(numberCodec.codec)(5)
      for (let i = 0; i < 2; i++) {
        yield* Effect.promise(() =>
          guest.invoke(
            "add",
            { tag: "tuple", val: [{ tag: "component-model", val: fiveWv }] },
            anonymousPrincipal,
          ),
        )
      }

      const out = yield* Effect.promise(() =>
        guest.invoke("getValue", { tag: "tuple", val: [] }, anonymousPrincipal),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") throw new Error()
      const value = yield* Schema.decodeEffect(numberCodec.codec)(out.val[0].val)
      expect(value).toBe(20)
    }),
  )

  it.effect("registers the Counter agent type with the expected DataSchemas", () =>
    Effect.gen(function* () {
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const counter = types.find((t) => t.typeName === "Counter")!

      expect(counter).toMatchObject({
        typeName: "Counter",
        sourceLanguage: "typescript",
        mode: "durable",
        dependencies: [],
        snapshotting: { tag: "disabled" },
        config: [],
      })

      // Constructor: tuple<("initial", f64)>
      expect(counter.constructor.inputSchema.tag).toBe("tuple")
      if (counter.constructor.inputSchema.tag !== "tuple") throw new Error()
      expect(counter.constructor.inputSchema.val.length).toBe(1)
      const [ctorName, ctorElement] = counter.constructor.inputSchema.val[0]!
      expect(ctorName).toBe("initial")
      expect(ctorElement.tag).toBe("component-model")
      if (ctorElement.tag !== "component-model") throw new Error()
      expect(ctorElement.val.nodes[0]!.type.tag).toBe("prim-f64-type")

      // Methods: getValue() -> f64; add(by: f64) -> ()
      expect(counter.methods.map((m) => m.name).sort()).toEqual(["add", "getValue"])

      const getValue = counter.methods.find((m) => m.name === "getValue")!
      if (getValue.inputSchema.tag !== "tuple") throw new Error()
      expect(getValue.inputSchema.val).toEqual([])
      if (getValue.outputSchema.tag !== "tuple") throw new Error()
      expect(getValue.outputSchema.val.length).toBe(1)
      const getValueOut = getValue.outputSchema.val[0]![1]
      if (getValueOut.tag !== "component-model") throw new Error()
      expect(getValueOut.val.nodes[0]!.type.tag).toBe("prim-f64-type")

      const add = counter.methods.find((m) => m.name === "add")!
      if (add.inputSchema.tag !== "tuple") throw new Error()
      expect(add.inputSchema.val.map(([k]) => k)).toEqual(["by"])
      const byElem = add.inputSchema.val[0]![1]
      if (byElem.tag !== "component-model") throw new Error()
      expect(byElem.val.nodes[0]!.type.tag).toBe("prim-f64-type")
      if (add.outputSchema.tag !== "tuple") throw new Error()
      // Unit return → empty output tuple.
      expect(add.outputSchema.val).toEqual([])
    }),
  )

  it("invoke fails before initialize", async () => {
    await expect(
      guest.invoke("greet", { tag: "tuple", val: [] }, anonymousPrincipal),
    ).rejects.toThrow(/not initialized/)
  })

  it("initialize twice fails", async () => {
    await guest.initialize("Greeter", { tag: "tuple", val: [] }, anonymousPrincipal)
    await expect(
      guest.initialize("Greeter", { tag: "tuple", val: [] }, anonymousPrincipal),
    ).rejects.toThrow(/already initialized/)
  })

  it.effect("Principal service resolves to the initialize-time principal in impl", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", { tag: "tuple", val: [] }, oidcPrincipal("alice")),
      )
      const out = yield* Effect.promise(() =>
        guest.invoke(
          "owner",
          { tag: "tuple", val: [] },
          // The 'caller' principal here is irrelevant for `owner`, which
          // captured the initialize-time principal in its closure.
          anonymousPrincipal,
        ),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") {
        throw new Error()
      }
      const decoded = yield* Schema.decodeEffect(stringCodec.codec)(out.val[0].val)
      expect(decoded).toBe("oidc:alice")
    }),
  )

  it.effect("Principal service resolves to the per-call principal in method handlers", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", { tag: "tuple", val: [] }, oidcPrincipal("alice")),
      )

      // First call as Bob: should see Bob, not Alice.
      const out1 = yield* Effect.promise(() =>
        guest.invoke("caller", { tag: "tuple", val: [] }, oidcPrincipal("bob")),
      )
      if (out1.tag !== "tuple" || out1.val[0]?.tag !== "component-model") {
        throw new Error()
      }
      const decoded1 = yield* Schema.decodeEffect(stringCodec.codec)(out1.val[0].val)
      expect(decoded1).toBe("oidc:bob")

      // Second call as anonymous, on the SAME initialized agent: per-call
      // principal must update, owner closure must not.
      const out2 = yield* Effect.promise(() =>
        guest.invoke("caller", { tag: "tuple", val: [] }, anonymousPrincipal),
      )
      if (out2.tag !== "tuple" || out2.val[0]?.tag !== "component-model") {
        throw new Error()
      }
      const decoded2 = yield* Schema.decodeEffect(stringCodec.codec)(out2.val[0].val)
      expect(decoded2).toBe("anonymous")

      const ownerOut = yield* Effect.promise(() =>
        guest.invoke("owner", { tag: "tuple", val: [] }, anonymousPrincipal),
      )
      if (ownerOut.tag !== "tuple" || ownerOut.val[0]?.tag !== "component-model") {
        throw new Error()
      }
      const ownerDecoded = yield* Schema.decodeEffect(stringCodec.codec)(ownerOut.val[0].val)
      expect(ownerDecoded).toBe("oidc:alice")
    }),
  )

  it.effect("Principal service propagates to child fibers forked inside a handler", () =>
    Effect.gen(function* () {
      const stringCodec = yield* toWitCodec(Schema.String)
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", { tag: "tuple", val: [] }, oidcPrincipal("alice")),
      )
      const out = yield* Effect.promise(() =>
        guest.invoke("callerForked", { tag: "tuple", val: [] }, oidcPrincipal("carol")),
      )
      if (out.tag !== "tuple" || out.val[0]?.tag !== "component-model") {
        throw new Error()
      }
      const decoded = yield* Schema.decodeEffect(stringCodec.codec)(out.val[0].val)
      expect(decoded).toBe("oidc:carol")
    }),
  )

  // -------------------------------------------------------------------
  // ConfigAgent — Effect-Context-based config
  // -------------------------------------------------------------------

  it.effect(
    "ConfigAgent: provides config service in impl AND handlers; observes mock changes per invocation",
    () =>
      Effect.gen(function* () {
        const stringCodec = yield* toWitCodec(Schema.String)
        const wv = (s: string) =>
          Effect.runSync(
            Schema.encodeEffect(stringCodec.codec)(s) as Effect.Effect<unknown, unknown, never>,
          )

        let greeting = "hello"
        let apiKey = "sk-abcd1234"
        const callLog: Array<string> = []
        __setGetConfigValueForTest((path) => {
          callLog.push(path.join("/"))
          if (path.join("/") === "greeting") return wv(greeting) as never
          if (path.join("/") === "apiKey") return wv(apiKey) as never
          throw new Error(`unknown config path: ${path.join("/")}`)
        })

        yield* Effect.promise(() =>
          guest.initialize("ConfigAgent", { tag: "tuple", val: [] }, anonymousPrincipal),
        )

        // initialize ran impl which captured greeting at init-time = "hello".
        // The mock was queried once for "greeting".
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // First invoke: greeting still "hello", read twice → 1 host call.
        callLog.length = 0
        const out1 = yield* Effect.promise(() =>
          guest.invoke("currentGreeting", { tag: "tuple", val: [] }, anonymousPrincipal),
        )
        if (out1.tag !== "tuple" || out1.val[0]?.tag !== "component-model") {
          throw new Error()
        }
        const decoded1 = yield* Schema.decodeEffect(stringCodec.codec)(out1.val[0].val)
        expect(decoded1).toBe("hello/hello")
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // Mutate the mock between invocations; the next invocation must
        // build a fresh shape and observe the updated value.
        callLog.length = 0
        greeting = "hola"
        const out2 = yield* Effect.promise(() =>
          guest.invoke("currentGreeting", { tag: "tuple", val: [] }, anonymousPrincipal),
        )
        if (out2.tag !== "tuple" || out2.val[0]?.tag !== "component-model") {
          throw new Error()
        }
        const decoded2 = yield* Schema.decodeEffect(stringCodec.codec)(out2.val[0].val)
        expect(decoded2).toBe("hola/hola")

        // initialGreeting captured at init time: still "hello", not "hola".
        const out3 = yield* Effect.promise(() =>
          guest.invoke("initialGreeting", { tag: "tuple", val: [] }, anonymousPrincipal),
        )
        if (out3.tag !== "tuple" || out3.val[0]?.tag !== "component-model") {
          throw new Error()
        }
        const decoded3 = yield* Schema.decodeEffect(stringCodec.codec)(out3.val[0].val)
        expect(decoded3).toBe("hello")

        // Secret read (.get) goes to the host every call — single read → 1 hit.
        callLog.length = 0
        apiKey = "sk-newer-key-xyz789"
        const out4 = yield* Effect.promise(() =>
          guest.invoke("keyTail", { tag: "tuple", val: [] }, anonymousPrincipal),
        )
        if (out4.tag !== "tuple" || out4.val[0]?.tag !== "component-model") {
          throw new Error()
        }
        const decoded4 = yield* Schema.decodeEffect(stringCodec.codec)(out4.val[0].val)
        expect(decoded4).toBe("z789")
        expect(callLog.filter((p) => p === "apiKey").length).toBe(1)
      }),
  )
})
