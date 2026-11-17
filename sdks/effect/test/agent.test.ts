import { describe, expect, it, beforeEach } from "@effect/vitest"
import { vi } from "vitest"
import { Effect, Fiber, Redacted, Ref, Schema } from "effect"
import { defineAgent, __resetAgents, dispatchSaveSnapshot } from "../src/Agent.js"
import { SelfAgentId } from "../src/SelfAgentId.js"
import * as Snapshot from "../src/Snapshot.js"
import { decodeEnvelope } from "../src/internal/snapshotEnvelope.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import { method } from "../src/Method.js"
import { guest } from "../src/internal/guest.js"
import { InvocationContext } from "../src/InvocationContext.js"
import { accountPrincipal, anonymousPrincipal, toolPrincipal, userPrincipal } from "./callers.js"
import { toWitCodec } from "../src/WitCodec.js"
import { defineConfig } from "../src/Config.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v, type SchemaValue } from "../src/internal/schema-model/model.js"
import {
  __resetGetConfigValueImpl as __resetGetConfigValueForTest,
  __setGetConfigValueImpl as __setGetConfigValueForTest,
} from "./mocks/golem-agent-host.js"
import { __setCaller, callerReads, withCaller } from "./mocks/golem-agent-invocation-context.js"

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
}).implement({
  init: () => Effect.void,
  methods: () => ({
    greet: ({ person, greeting }) => Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
    ping: () => Effect.void,
  }),
})

/** A stateful agent that exercises the closure-based state pattern, plus a
 *  side-effect during initialization. */
const Counter = defineAgent({
  name: "Counter",
  id: { initial: Schema.Number },
  methods: {
    getValue: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Void }),
  },
}).implement({
  init: ({ initial }) => Ref.make(initial),
  methods: (ref) => ({
    getValue: () => Ref.get(ref),
    add: ({ by }) => Ref.update(ref, (n) => n + by),
  }),
})

const callerTag = (context: { readonly caller: { readonly immediate: { toString(): string } } }) =>
  context.caller.immediate.toString()
const userText = (subject: string) => `user ("https://example.test",${JSON.stringify(subject)})`

/**
 * Exercises the InvocationContext service in BOTH the constructor effect (where it resolves to the
 * caller of `initialize`) AND a method handler (where it resolves to the caller of that specific
 * call, which may differ from the initialize-time one).
 */
const CallerAgent = defineAgent({
  name: "CallerAgent",
  id: {},
  methods: {
    owner: method({ input: {}, success: Schema.String }),
    caller: method({ input: {}, success: Schema.String }),
    original: method({ input: {}, success: Schema.String }),
    callerForked: method({ input: {}, success: Schema.String }),
    unread: method({ input: {}, success: Schema.String }),
  },
}).implement({
  init: () =>
    Effect.gen(function* () {
      return callerTag(yield* InvocationContext)
    }),
  methods: (owner) => ({
    owner: () => Effect.succeed(owner),
    caller: () => Effect.map(InvocationContext, callerTag),
    original: () => Effect.map(InvocationContext, (context) => context.caller.original.toString()),
    // Verifies that the per-call InvocationContext service propagates to
    // child fibers spawned inside a handler.
    callerForked: () =>
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(Effect.map(InvocationContext, callerTag))
        return yield* Fiber.join(fiber)
      }),
    // Never reads the caller: the host must not be asked for it.
    unread: () => Effect.succeed("unread"),
  }),
})

const ReadOnlyCallerAgent = defineAgent({
  name: "ReadOnlyCallerAgent",
  id: {},
  methods: {
    whoami: method({ input: {}, success: Schema.String, readOnly: true }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    whoami: () => Effect.map(InvocationContext, callerTag),
  }),
})

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
}).implement({
  init: () =>
    Effect.gen(function* () {
      const cfg = yield* TestConfig
      // Reads the greeting at *initialize* time and captures it; later
      // shape rebuilds (one per invocation) won't mutate this closure.
      return yield* cfg.greeting
    }),
  methods: (initial) => ({
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
    void CallerAgent
    void ReadOnlyCallerAgent
    void ConfigAgent
  })

  it.effect("discoverAgentTypes returns the registered agents", () =>
    Effect.gen(function* () {
      const types = yield* Effect.sync(() => guest.discoverAgentTypes())
      expect(types.map((t) => t.typeName).sort()).toEqual([
        "CallerAgent",
        "ConfigAgent",
        "Counter",
        "Greeter",
        "ReadOnlyCallerAgent",
      ])
      const greeter = types.find((t) => t.typeName === "Greeter")!
      expect(greeter.methods.map((m) => m.name).sort()).toEqual(["greet", "ping"])
      // The invocation context is injected: it is never part of an input schema.
      for (const type of types) {
        for (const m of type.methods) {
          for (const field of m.inputSchema.val) expect(field.source.tag).toBe("user-supplied")
          expect(m.readOnly === undefined || !("usesPrincipal" in m.readOnly)).toBe(true)
        }
      }
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
      yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.initialize("Greeter", wire())),
      )

      const def = yield* Effect.sync(() => guest.getDefinition())
      expect(def.typeName).toBe("Greeter")

      const personCodec = yield* toWitCodec(Person)
      const stringCodec = yield* toWitCodec(Schema.String)
      const personWv = yield* Schema.encodeEffect(personCodec.codec)({ name: "Ada", age: 36 })
      const greetingWv = yield* Schema.encodeEffect(stringCodec.codec)("Hello")

      const out = yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.invoke("greet", wire(personWv, greetingWv))),
      )
      const decoded = read<string>(out!)
      expect(decoded).toBe("Hello, Ada (36)!")
    }),
  )

  it.effect("invoke returns an empty tuple for unit-returning methods", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.initialize("Greeter", wire())),
      )
      const out = yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.invoke("ping", wire())),
      )
      expect(out).toBeUndefined()
    }),
  )

  it.effect("preserves state across calls (Counter)", () =>
    Effect.gen(function* () {
      const numberCodec = yield* toWitCodec(Schema.Number)

      // initialize Counter with initial = 10
      const initialWv = yield* Schema.encodeEffect(numberCodec.codec)(10)
      yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.initialize("Counter", wire(initialWv))),
      )

      // add 5 twice
      const fiveWv = yield* Schema.encodeEffect(numberCodec.codec)(5)
      for (let i = 0; i < 2; i++) {
        yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.invoke("add", wire(fiveWv))),
        )
      }

      const out = yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.invoke("getValue", wire())),
      )
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
        kind: "regular",
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
    await expect(
      withCaller(anonymousPrincipal, () => guest.invoke("greet", wire())),
    ).rejects.toThrow(/not initialized/)
  })

  it("initialize twice fails", async () => {
    await withCaller(anonymousPrincipal, () => guest.initialize("Greeter", wire()))
    await expect(
      withCaller(anonymousPrincipal, () => guest.initialize("Greeter", wire())),
    ).rejects.toThrow(/already initialized/)
  })

  it("normalizes escaped method failures without changing declared errors or interruption", async () => {
    defineAgent({
      name: "MethodFailures",
      id: {},
      methods: {
        thrown: method({ input: {}, success: Schema.Void }),
        defect: method({ input: {}, success: Schema.Void }),
        declared: method({ input: {}, success: Schema.Void, error: Schema.String }),
        interrupted: method({ input: {}, success: Schema.Void }),
      },
    }).implement({
      init: () => Effect.void,
      methods: () => ({
        thrown: () => {
          throw new Error("synchronous method error")
        },
        defect: () => Effect.die("method defect"),
        declared: () => Effect.fail("declared error"),
        interrupted: () => Effect.interrupt,
      }),
    })
    await withCaller(anonymousPrincipal, () => guest.initialize("MethodFailures", wire()))
    for (const [methodName, message] of [
      ["thrown", "synchronous method error"],
      ["defect", "method defect"],
    ]) {
      const error = await withCaller(anonymousPrincipal, () =>
        guest.invoke(methodName!, wire()),
      ).then(
        () => {
          throw new Error("method must fail")
        },
        (error: unknown) => error,
      )
      expect(error).toMatchObject({ tag: "custom-error" })
      const payload = (error as { val: { value: Parameters<typeof schemaValueFromWit>[0] } }).val
      expect(schemaValueFromWit(payload.value)).toEqual({
        tag: "string",
        value: expect.stringContaining(message!),
      })
    }
    const result = await withCaller(anonymousPrincipal, () => guest.invoke("declared", wire()))
    expect(schemaValueFromWit(result!)).toEqual({
      tag: "result",
      result: { tag: "err", value: { tag: "string", value: "declared error" } },
    })
    await expect(
      withCaller(anonymousPrincipal, () => guest.invoke("interrupted", wire())),
    ).rejects.not.toMatchObject({
      tag: "custom-error",
    })
  })

  it("keeps input and output codec failures outside the method error channel", async () => {
    let calls = 0
    defineAgent({
      name: "CodecFailures",
      id: {},
      methods: {
        decode: method({ input: { text: Schema.String }, success: Schema.Void }),
        encode: method({ input: {}, success: Schema.String.check(Schema.isPattern(/^valid$/)) }),
      },
    }).implement({
      init: () => Effect.void,
      methods: () => ({
        decode: () =>
          Effect.sync(() => {
            calls++
          }),
        encode: () => Effect.succeed("invalid"),
      }),
    })
    await withCaller(anonymousPrincipal, () => guest.initialize("CodecFailures", wire()))
    for (const methodName of ["decode", "encode"]) {
      await expect(
        withCaller(anonymousPrincipal, () => guest.invoke(methodName, wire())),
      ).rejects.not.toMatchObject({
        tag: "custom-error",
      })
    }
    expect(calls).toBe(0)
  })

  it("does not normalize constructor defects as method failures", async () => {
    defineAgent({ name: "ConstructorDefect", id: {}, methods: {} }).implement({
      init: () => Effect.die("constructor defect"),
      methods: () => ({}),
    })
    await expect(
      withCaller(anonymousPrincipal, () => guest.initialize("ConstructorDefect", wire())),
    ).rejects.toThrow("constructor defect")
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
    }).implement({ init: () => Effect.void, methods: () => ({ ping: () => Effect.void }) })
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

  it.effect("InvocationContext resolves to the initialize-time caller in impl", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        withCaller(userPrincipal("alice"), () => guest.initialize("CallerAgent", wire())),
      )
      const out = yield* Effect.promise(() =>
        // The 'caller' here is irrelevant for `owner`, which captured the
        // initialize-time caller in its closure.
        withCaller(anonymousPrincipal, () => guest.invoke("owner", wire())),
      )
      expect(read<string>(out!)).toBe(userText("alice"))
    }),
  )

  it.effect("InvocationContext resolves to the per-call caller in method handlers", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        withCaller(userPrincipal("alice"), () => guest.initialize("CallerAgent", wire())),
      )

      // First call as Bob: should see Bob, not Alice.
      const out1 = yield* Effect.promise(() =>
        withCaller(userPrincipal("bob"), () => guest.invoke("caller", wire())),
      )
      expect(read<string>(out1!)).toBe(userText("bob"))

      // Second call as anonymous, on the SAME initialized agent: the per-call
      // caller must update, the owner closure must not.
      const out2 = yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.invoke("caller", wire())),
      )
      expect(read<string>(out2!)).toBe("anonymous")

      const ownerOut = yield* Effect.promise(() =>
        withCaller(anonymousPrincipal, () => guest.invoke("owner", wire())),
      )
      expect(read<string>(ownerOut!)).toBe(userText("alice"))
    }),
  )

  it("exposes both roles of the caller", async () => {
    __setCaller(userPrincipal("alice"), userPrincipal("root"))
    await guest.initialize("CallerAgent", wire())
    __setCaller(accountPrincipal(7n), toolPrincipal("fmt"))
    expect(read<string>((await guest.invoke("caller", wire()))!)).toBe(
      'account ("golem","account:7")',
    )
    expect(read<string>((await guest.invoke("original", wire()))!)).toBe(
      'tool ("golem","tool:fmt")',
    )
    expect(read<string>((await guest.invoke("owner", wire()))!)).toBe(userText("alice"))
  })

  it("asks the host for the caller only when a handler reads it", async () => {
    __setCaller(userPrincipal("alice"))
    await guest.initialize("CallerAgent", wire())
    const afterInit = callerReads()
    await guest.invoke("unread", wire())
    expect(callerReads()).toBe(afterInit)
    await guest.invoke("caller", wire())
    expect(callerReads()).toBe(afterInit + 1)
  })

  it("provides the invocation context to read-only handlers", async () => {
    await withCaller(anonymousPrincipal, () => guest.initialize("ReadOnlyCallerAgent", wire()))
    const out = await withCaller(userPrincipal("allowed"), () => guest.invoke("whoami", wire()))
    expect(read<string>(out!)).toBe(userText("allowed"))
    const anonymous = await withCaller(anonymousPrincipal, () => guest.invoke("whoami", wire()))
    expect(read<string>(anonymous!)).toBe("anonymous")
  })

  it.effect("InvocationContext propagates to child fibers forked inside a handler", () =>
    Effect.gen(function* () {
      yield* Effect.promise(() =>
        withCaller(userPrincipal("alice"), () => guest.initialize("CallerAgent", wire())),
      )
      const out = yield* Effect.promise(() =>
        withCaller(userPrincipal("carol"), () => guest.invoke("callerForked", wire())),
      )
      expect(read<string>(out!)).toBe(userText("carol"))
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

        yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.initialize("ConfigAgent", wire())),
        )

        // initialize ran impl which captured greeting at init-time = "hello".
        // The mock was queried once for "greeting".
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // First invoke: greeting still "hello", read twice → 1 host call.
        callLog.length = 0
        const out1 = yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.invoke("currentGreeting", wire())),
        )
        const decoded1 = read<string>(out1!)
        expect(decoded1).toBe("hello/hello")
        expect(callLog.filter((p) => p === "greeting").length).toBe(1)

        // Mutate the mock between invocations; the next invocation must
        // build a fresh shape and observe the updated value.
        callLog.length = 0
        greeting = "hola"
        const out2 = yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.invoke("currentGreeting", wire())),
        )
        const decoded2 = read<string>(out2!)
        expect(decoded2).toBe("hola/hola")

        // initialGreeting captured at init time: still "hello", not "hola".
        const out3 = yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.invoke("initialGreeting", wire())),
        )
        const decoded3 = read<string>(out3!)
        expect(decoded3).toBe("hello")

        // Secret reads resolve the current capability value and are never cached.
        apiKey = "sk-newer-key-xyz789"
        const out4 = yield* Effect.promise(() =>
          withCaller(anonymousPrincipal, () => guest.invoke("keyTail", wire())),
        )
        expect(read<string>(out4!)).toBe("z789")
        expect(callLog.filter((p) => p === "apiKey")).toHaveLength(1)
        expect(revealCount).toBe(1)
      }),
  )

  it("reads self identity afresh within a method and snapshot, without rewriting saved state", async () => {
    const metadata = ApiHostMock.getSelfMetadata()
    const parent = { ...metadata, agentId: { ...metadata.agentId, agentId: "Identity(parent)" } }
    const child = { ...metadata, agentId: { ...metadata.agentId, agentId: "Identity(child)" } }
    const host = vi.spyOn(ApiHostMock, "getSelfMetadata").mockReturnValue(parent)
    try {
      defineAgent({
        name: "FreshSelfIdentity",
        id: {},
        snapshotting: Snapshot.custom({ policy: Snapshot.policy.manual }),
        methods: { read: method({ input: {}, success: Schema.String }) },
      }).implement({
        init: () => SelfAgentId.pipe(Effect.map((id) => ({ saved: id.agentId }))),
        methods: (state) => ({
          read: () =>
            Effect.gen(function* () {
              const before = yield* SelfAgentId
              const after = yield* SelfAgentId
              return JSON.stringify({
                saved: state.saved,
                before: before.agentId,
                after: after.agentId,
              })
            }),
        }),
        snapshot: {
          save: (state) =>
            SelfAgentId.pipe(
              Effect.map((id) =>
                new TextEncoder().encode(
                  JSON.stringify({ saved: state.saved, current: id.agentId }),
                ),
              ),
            ),
          restore: (bytes) =>
            Effect.sync(() => JSON.parse(new TextDecoder().decode(bytes)) as { saved: string }),
        },
      })
      await withCaller(anonymousPrincipal, () => guest.initialize("FreshSelfIdentity", wire()))
      host.mockReturnValueOnce(parent).mockReturnValue(child)
      const result = await withCaller(anonymousPrincipal, () => guest.invoke("read", wire()))
      expect(JSON.parse(read<string>(result!))).toEqual({
        saved: "Identity(parent)",
        before: "Identity(parent)",
        after: "Identity(child)",
      })
      const snapshot = await dispatchSaveSnapshot()
      const envelope = decodeEnvelope(snapshot)
      expect(envelope.kind).toBe("binary")
      if (envelope.kind !== "binary") throw new Error("expected binary snapshot")
      expect(JSON.parse(new TextDecoder().decode(envelope.userPayload))).toEqual({
        saved: "Identity(parent)",
        current: "Identity(child)",
      })
    } finally {
      host.mockRestore()
    }
  })
})
