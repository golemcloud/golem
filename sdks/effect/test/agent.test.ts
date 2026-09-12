import { describe, expect, it, beforeEach } from "@effect/vitest"
import { vi } from "vitest"
import { Effect, Fiber, Redacted, Ref, Schema } from "effect"
import { defineAgent, __resetAgents } from "../src/Agent.js"
import { method } from "../src/Method.js"
import { guest } from "../src/internal/guest.js"
import { Principal, type PrincipalValue } from "../src/Principal.js"
import { toWitCodec } from "../src/WitCodec.js"
import { defineConfig } from "../src/Config.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v, type SchemaValue } from "../src/internal/schema-model/model.js"
import {
  __resetGetConfigValueImpl as __resetGetConfigValueForTest,
  __setGetConfigValueImpl as __setGetConfigValueForTest,
} from "./mocks/golem-agent-host.js"

const secretHost = vi.hoisted(() => ({
  reveal: (_handle: unknown, _expected: unknown): unknown => {
    throw new Error("secret reveal not mocked")
  },
}))

vi.mock("golem:secrets/reveal@0.1.0", () => ({
  reveal: (secret: unknown, expected: unknown) => secretHost.reveal(secret, expected),
  id: () => ({}),
  metadata: () => ({}),
}))

const Person = Schema.Struct({
  name: Schema.String,
  age: Schema.Number,
})

const wire = (...fields: Array<SchemaValue>) => schemaValueToWit(v.record(fields))
const read = <A>(value: NonNullable<Awaited<ReturnType<typeof guest.invoke>>>): A => {
  const decoded = schemaValueFromWit(value)
  if (decoded.tag !== "string" && decoded.tag !== "f64") {
    throw new Error(`Expected a scalar value, got ${decoded.tag}`)
  }
  return decoded.value as A
}

const Greeter = defineAgent({
  name: "Greeter",
  description: "An agent that greets people",
  id: {},
  methods: {
    greet: method({
      input: { person: Person, greeting: Schema.String },
      success: Schema.String,
      description: "Greet the given person with the given greeting",
      promptHint: "Use to produce a friendly salutation for a Person.",
    }),
    ping: method({ input: {}, success: Schema.Void }),
  },
}).implement(() =>
  Effect.succeed({
    greet: ({ person, greeting }) => Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
    ping: () => Effect.void,
  }),
)

/** A stateful agent that exercises the closure-based state pattern, plus a
 *  side-effect during initialization. */
const Counter = defineAgent({
  name: "Counter",
  id: { initial: Schema.Number },
  methods: {
    getValue: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Void }),
  },
}).implement(({ initial }) =>
  Effect.gen(function* () {
    const ref = yield* Ref.make(initial)
    return {
      getValue: () => Ref.get(ref),
      add: ({ by }) => Ref.update(ref, (n) => n + by),
    }
  }),
)

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
  id: {},
  methods: {
    owner: method({ input: {}, success: Schema.String }),
    caller: method({ input: {}, success: Schema.String }),
    callerForked: method({ input: {}, success: Schema.String }),
  },
}).implement(() =>
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
)

/**
 * Exercises an Effect-Context-based config service from BOTH the
 * constructor and a method handler. The mocked host responds via the
 * `__setGetConfigValueForTest` shim; the test verifies that:
 *
 * - regular fields are read through the current invocation's config service,
 * - secret fields are revealed on every read,
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
  id: {},
  methods: {
    initialGreeting: method({ input: {}, success: Schema.String }),
    currentGreeting: method({ input: {}, success: Schema.String }),
    keyTail: method({ input: {}, success: Schema.String }),
  },
}).implement(() =>
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
          // Reading the same field twice inside one invocation must hit the
          // host only once (memoization within one shape).
          const a = yield* c.greeting
          const b = yield* c.greeting
          return `${a}/${b}`
        }),
      keyTail: () =>
        Effect.gen(function* () {
          const c = yield* TestConfig
          return Redacted.value(yield* c.apiKey.get).slice(-4)
        }),
    }
  }),
)

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
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
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
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
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
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
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
      yield* Effect.promise(() => guest.initialize("Greeter", wire(), anonymousPrincipal))

      const def = yield* Effect.sync(() => guest.getDefinition())
      expect(def.typeName).toBe("Greeter")

      const personCodec = yield* toWitCodec(Person)
      const stringCodec = yield* toWitCodec(Schema.String)
      const personWv = yield* Schema.encodeEffect(personCodec.codec)({ name: "Ada", age: 36 })
      const greetingWv = yield* Schema.encodeEffect(stringCodec.codec)("Hello")

      const out = yield* Effect.promise(() =>
        guest.invoke("greet", wire(personWv, greetingWv), anonymousPrincipal),
      )
      const decoded = read<string>(out!)
      expect(decoded).toBe("Hello, Ada (36)!")
    }),
  )

  it.effect("invoke returns an empty tuple for unit-returning methods", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() => guest.initialize("Greeter", wire(), anonymousPrincipal))
      const out = yield* Effect.promise(() => guest.invoke("ping", wire(), anonymousPrincipal))
      expect(out).toBeUndefined()
    }),
  )

  it.effect("preserves state across calls (Counter)", () =>
    Effect.gen(function* () {
      const numberCodec = yield* toWitCodec(Schema.Number)

      // initialize Counter with initial = 10
      const initialWv = yield* Schema.encodeEffect(numberCodec.codec)(10)
      yield* Effect.promise(() => guest.initialize("Counter", wire(initialWv), anonymousPrincipal))

      // add 5 twice
      const fiveWv = yield* Schema.encodeEffect(numberCodec.codec)(5)
      for (let i = 0; i < 2; i++) {
        yield* Effect.promise(() => guest.invoke("add", wire(fiveWv), anonymousPrincipal))
      }

      const out = yield* Effect.promise(() => guest.invoke("getValue", wire(), anonymousPrincipal))
      const value = read<number>(out!)
      expect(value).toBe(20)
    }),
  )

  it.effect("registers the Counter agent type with the expected DataSchemas", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      const counter = types.find((t) => t.typeName === "Counter")!

      expect(counter).toMatchObject({
        typeName: "Counter",
        sourceLanguage: "typescript",
        mode: "durable",
        dependencies: [],
        snapshotting: { tag: "disabled" },
        config: [],
      })

      expect(counter.constructor.inputSchema.tag).toBe("parameters")
      if (counter.constructor.inputSchema.tag !== "parameters") throw new Error()
      expect(counter.constructor.inputSchema.val.length).toBe(1)
      const ctorField = counter.constructor.inputSchema.val[0]!
      expect(ctorField.name).toBe("initial")
      expect(counter.schema.typeNodes[ctorField.schema]!.body.tag).toBe("f64-type")

      // Methods: getValue() -> f64; add(by: f64) -> ()
      expect(counter.methods.map((m) => m.name).sort()).toEqual(["add", "getValue"])

      const getValue = counter.methods.find((m) => m.name === "getValue")!
      if (getValue.inputSchema.tag !== "parameters") throw new Error()
      expect(getValue.inputSchema.val).toEqual([])
      if (getValue.outputSchema.tag !== "single") throw new Error()
      expect(counter.schema.typeNodes[getValue.outputSchema.val]!.body.tag).toBe("f64-type")

      const add = counter.methods.find((m) => m.name === "add")!
      if (add.inputSchema.tag !== "parameters") throw new Error()
      expect(add.inputSchema.val.map((field) => field.name)).toEqual(["by"])
      expect(counter.schema.typeNodes[add.inputSchema.val[0]!.schema]!.body.tag).toBe("f64-type")
      expect(add.outputSchema).toEqual({ tag: "unit" })
    }),
  )

  it("invoke fails before initialize", async () => {
    await expect(guest.invoke("greet", wire(), anonymousPrincipal)).rejects.toThrow(
      /not initialized/,
    )
  })

  it("initialize twice fails", async () => {
    await guest.initialize("Greeter", wire(), anonymousPrincipal)
    await expect(guest.initialize("Greeter", wire(), anonymousPrincipal)).rejects.toThrow(
      /already initialized/,
    )
  })

  it("defining two agents with the same name surfaces from discoverAgentTypes", async () => {
    // `Greeter` is already in the registry from this file's top-level
    // `defineAgent`. Re-registering the same name with a fresh definition
    // must NOT throw at import time — the failure is stashed and
    // re-emitted from the WIT-exported `discoverAgentTypes` host call as
    // a typed `AgentError` (so the Golem CLI can surface it as a proper
    // diagnostic instead of as a WASM instantiation crash).
    defineAgent({
      name: "Greeter",
      id: {},
      methods: { ping: method({ input: {}, success: Schema.Void }) },
    }).implement(() => Effect.succeed({ ping: () => Effect.void }))
    let caught: unknown
    try {
      await guest.discoverAgentTypes()
    } catch (e) {
      caught = e
    }
    if (caught === undefined) throw new Error("expected AgentError from discoverAgentTypes")
    const tag = (caught as { tag?: string }).tag
    const val = (caught as { val?: unknown }).val
    expect(tag).toBe("invalid-type")
    expect(typeof val).toBe("string")
    expect(val).toMatch(/Greeter/)
  })

  it.effect("Principal service resolves to the initialize-time principal in impl", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", wire(), oidcPrincipal("alice")),
      )
      const out = yield* Effect.promise(() =>
        guest.invoke(
          "owner",
          wire(),
          // The 'caller' principal here is irrelevant for `owner`, which
          // captured the initialize-time principal in its closure.
          anonymousPrincipal,
        ),
      )
      const decoded = read<string>(out!)
      expect(decoded).toBe("oidc:alice")
    }),
  )

  it.effect("Principal service resolves to the per-call principal in method handlers", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", wire(), oidcPrincipal("alice")),
      )

      // First call as Bob: should see Bob, not Alice.
      const out1 = yield* Effect.promise(() => guest.invoke("caller", wire(), oidcPrincipal("bob")))
      const decoded1 = read<string>(out1!)
      expect(decoded1).toBe("oidc:bob")

      // Second call as anonymous, on the SAME initialized agent: per-call
      // principal must update, owner closure must not.
      const out2 = yield* Effect.promise(() => guest.invoke("caller", wire(), anonymousPrincipal))
      const decoded2 = read<string>(out2!)
      expect(decoded2).toBe("anonymous")

      const ownerOut = yield* Effect.promise(() =>
        guest.invoke("owner", wire(), anonymousPrincipal),
      )
      const ownerDecoded = read<string>(ownerOut!)
      expect(ownerDecoded).toBe("oidc:alice")
    }),
  )

  it.effect("Principal service propagates to child fibers forked inside a handler", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        guest.initialize("PrincipalAgent", wire(), oidcPrincipal("alice")),
      )
      const out = yield* Effect.promise(() =>
        guest.invoke("callerForked", wire(), oidcPrincipal("carol")),
      )
      const decoded = read<string>(out!)
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
        let revealCount = 0
        const callLog: Array<string> = []
        __setGetConfigValueForTest((path: Array<string>) => {
          callLog.push(path.join("/"))
          if (path.join("/") === "greeting") return schemaValueToWit(wv(greeting) as SchemaValue)
          if (path.join("/") === "apiKey") {
            return { root: 0, valueNodes: [{ tag: "secret-value", val: {} }] } as never
          }
          throw new Error(`unknown config path: ${path.join("/")}`)
        })
        secretHost.reveal = () => {
          revealCount++
          return schemaValueToWit(wv(apiKey) as SchemaValue)
        }

        yield* Effect.promise(() => guest.initialize("ConfigAgent", wire(), anonymousPrincipal))

        // initialize ran impl which captured greeting at init-time = "hello".
        // The mock was queried once for "greeting".
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // First invoke: greeting still "hello", read twice → 1 host call.
        callLog.length = 0
        const out1 = yield* Effect.promise(() =>
          guest.invoke("currentGreeting", wire(), anonymousPrincipal),
        )
        const decoded1 = read<string>(out1!)
        expect(decoded1).toBe("hello/hello")
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // Mutate the mock between invocations; the next invocation must
        // build a fresh shape and observe the updated value.
        callLog.length = 0
        greeting = "hola"
        const out2 = yield* Effect.promise(() =>
          guest.invoke("currentGreeting", wire(), anonymousPrincipal),
        )
        const decoded2 = read<string>(out2!)
        expect(decoded2).toBe("hola/hola")

        // initialGreeting captured at init time: still "hello", not "hola".
        const out3 = yield* Effect.promise(() =>
          guest.invoke("initialGreeting", wire(), anonymousPrincipal),
        )
        const decoded3 = read<string>(out3!)
        expect(decoded3).toBe("hello")

        // Secret reads resolve the current capability value and are never cached.
        apiKey = "sk-newer-key-xyz789"
        const out4 = yield* Effect.promise(() =>
          guest.invoke("keyTail", wire(), anonymousPrincipal),
        )
        expect(read<string>(out4!)).toBe("z789")
        expect(callLog.filter((p) => p === "apiKey")).toHaveLength(1)
        expect(revealCount).toBe(1)
      }),
  )
})
