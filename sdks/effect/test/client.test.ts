import { describe, it, expect, beforeEach } from "vitest"
import { Effect, Schema } from "effect"
import { defineAgent } from "../src/agent.js"
import { method } from "../src/method.js"
import { defineConfig } from "../src/config.js"
import {
  __getCancellations,
  __getRecordedRpcCalls,
  __reset,
  __setRpcResponder,
  type RecordedRpcCall,
} from "./mocks/golem-agent-host.js"
import { __resetIdempotency } from "./mocks/golem-api-host.js"

const Counter = defineAgent({
  name: "Counter",
  mode: "durable",
  constructorParams: { initial: Schema.Number },
  methods: {
    getValue: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Void }),
  },
  impl: () =>
    Effect.succeed({
      getValue: () => Effect.succeed(0),
      add: () => Effect.void,
    }),
})

const Worker = defineAgent({
  name: "Worker",
  mode: "ephemeral",
  constructorParams: { jobId: Schema.String },
  methods: {
    run: method({ params: { times: Schema.Number }, success: Schema.String }),
  },
  impl: () =>
    Effect.succeed({
      run: () => Effect.succeed("ok"),
    }),
})

const decodeWv = async <S extends Schema.Top>(s: S, wv: any): Promise<S["Type"]> => {
  const { toWitCodec } = await import("../src/wit-codec.js")
  const codec = await Effect.runPromise(toWitCodec(s))
  return Effect.runPromise(
    Schema.decodeEffect(codec.codec)(wv) as Effect.Effect<S["Type"], unknown, never>,
  )
}

const encodeWv = async <S extends Schema.Top>(s: S, value: S["Type"]): Promise<any> => {
  const { toWitCodec } = await import("../src/wit-codec.js")
  const codec = await Effect.runPromise(toWitCodec(s))
  return Effect.runPromise(
    Schema.encodeEffect(codec.codec)(value) as Effect.Effect<any, unknown, never>,
  )
}

describe("AgentClient (durable)", () => {
  beforeEach(() => {
    __reset()
    __resetIdempotency()
  })

  it("attaches `client` to the defineAgent return value", () => {
    expect(typeof Counter.client.get).toBe("function")
    expect(typeof Counter.client.getPhantom).toBe("function")
    expect(typeof Counter.client.newPhantom).toBe("function")
  })

  it("get(): constructs a WasmRpc with no phantomId and round-trips a method call", async () => {
    // Stub responder to return an encoded number 42 for `getValue`.
    const numWv = await encodeWv(Schema.Number, 42)
    __setRpcResponder(({ methodName }) => {
      if (methodName === "getValue") {
        return {
          tag: "ok",
          val: { tag: "tuple", val: [{ tag: "component-model", val: numWv }] },
        }
      }
      return { tag: "throw", error: new Error("unexpected method") }
    })

    const result = await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 7 })
        return yield* remote.getValue({})
      }) as Effect.Effect<number, unknown, never>,
    )
    expect(result).toBe(42)

    const calls = __getRecordedRpcCalls()
    expect(calls.length).toBe(1)
    const c = calls[0]!
    expect(c.kind).toBe("asyncInvokeAndAwait")
    expect(c.methodName).toBe("getValue")
    expect(c.agentTypeName).toBe("Counter")
    expect(c.phantomId).toBeUndefined()
    // constructor input is a tuple<{ initial: f64 }>
    expect(c.constructorValue.tag).toBe("tuple")
    expect(c.constructorValue.val.length).toBe(1)
    const ctorElem = c.constructorValue.val[0]
    expect(ctorElem.tag).toBe("component-model")
    const decodedCtor = await decodeWv(Schema.Number, ctorElem.val)
    expect(decodedCtor).toBe(7)
  })

  it("get(): encodes named method args positionally", async () => {
    __setRpcResponder(() => ({
      tag: "ok",
      val: { tag: "tuple", val: [] },
    }))
    await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        yield* remote.add({ by: 5 })
      }) as Effect.Effect<void, unknown, never>,
    )
    const c = __getRecordedRpcCalls()[0]!
    expect(c.methodName).toBe("add")
    expect(c.input.tag).toBe("tuple")
    expect(c.input.val.length).toBe(1)
    const decoded = await decodeWv(Schema.Number, c.input.val[0].val)
    expect(decoded).toBe(5)
  })

  it("trigger(): uses fire-and-forget invoke and resolves to void", async () => {
    __setRpcResponder(() => ({ tag: "ok", val: undefined }))
    await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        yield* remote.add.trigger({ by: 9 })
      }) as Effect.Effect<void, unknown, never>,
    )
    const c = __getRecordedRpcCalls()[0]!
    expect(c.kind).toBe("invoke")
  })

  it("schedule(): records scheduledTime and returns a cancel handle", async () => {
    const at = { seconds: 1n, nanoseconds: 0 }
    let cancelHandle: { cancel: () => Effect.Effect<void> } | null = null
    await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        cancelHandle = yield* remote.add.schedule(at, { by: 1 })
      }) as Effect.Effect<void, unknown, never>,
    )
    const c = __getRecordedRpcCalls()[0]!
    expect(c.kind).toBe("schedule")
    expect(c.scheduledTime).toEqual(at)
    expect(cancelHandle).not.toBeNull()
    await Effect.runPromise(cancelHandle!.cancel())
    const cancellations = __getCancellations()
    expect(cancellations.length).toBe(1)
    expect(cancellations[0]).toEqual({ kind: "scheduled", methodName: "add" })
  })

  it("getPhantom(): parses the uuid and forwards it to the WasmRpc constructor", async () => {
    __setRpcResponder(() => ({ tag: "ok", val: { tag: "tuple", val: [] } }))
    const phantomId = "12345678-1234-1234-1234-1234567890ab"
    await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter.client.getPhantom({ initial: 0 }, phantomId)
        yield* remote.add({ by: 1 })
      }) as Effect.Effect<void, unknown, never>,
    )
    const c = __getRecordedRpcCalls()[0]!
    expect(c.phantomId).toBeDefined()
    expect(c.phantomId.highBits).toBe(BigInt("0x1234567812341234"))
    expect(c.phantomId.lowBits).toBe(BigInt("0x12341234567890ab"))
  })

  it("getPhantom(): fails with InvalidUuidError on a malformed string", async () => {
    const result = await Effect.runPromise(
      Effect.result(
        Counter.client.getPhantom({ initial: 0 }, "not-a-uuid") as Effect.Effect<
          unknown,
          any,
          never
        >,
      ),
    )
    expect(result._tag).toBe("Failure")
    if (result._tag !== "Failure") return
    const failure: any = result.failure
    expect(failure._tag).toBe("InvalidUuidError")
    expect(failure.value).toBe("not-a-uuid")
  })

  it("newPhantom(): generates a fresh phantom id and exposes it on the remote handle", async () => {
    __setRpcResponder(() => ({ tag: "ok", val: { tag: "tuple", val: [] } }))
    const remote = await Effect.runPromise(
      Counter.client.newPhantom({ initial: 0 }) as Effect.Effect<
        { phantomId: string; add: any },
        unknown,
        never
      >,
    )
    expect(remote.phantomId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    )
    await Effect.runPromise(remote.add({ by: 1 }) as Effect.Effect<void, unknown, never>)
    const c = __getRecordedRpcCalls()[0]!
    expect(c.phantomId).toBeDefined()
    expect(c.phantomId.lowBits).toBe(1n)
  })

  it("RpcError from the host is surfaced as RpcCallError", async () => {
    __setRpcResponder(() => ({
      tag: "throw",
      error: { tag: "remote-internal-error", val: "boom" },
    }))
    const result = await Effect.runPromise(
      Effect.result(
        Effect.gen(function* () {
          const remote = yield* Counter.client.get({ initial: 0 })
          return yield* remote.getValue({})
        }) as Effect.Effect<unknown, any, never>,
      ),
    )
    expect(result._tag).toBe("Failure")
    if (result._tag !== "Failure") return
    const failure: any = result.failure
    expect(failure._tag).toBe("RpcCallError")
    expect(failure.cause).toEqual({ tag: "remote-internal-error", val: "boom" })
  })
})

describe("AgentClient overrides (config)", () => {
  beforeEach(() => {
    __reset()
    __resetIdempotency()
  })

  class CounterCfg extends defineConfig("CounterClient.Cfg", {
    greeting: Schema.String,
    apiKey: Schema.Redacted(Schema.String),
  }) {}

  const Counter2 = defineAgent({
    name: "Counter2",
    config: CounterCfg,
    constructorParams: { initial: Schema.Number },
    methods: {
      noop: method({ params: {}, success: Schema.Void }),
    },
    impl: () =>
      Effect.succeed({
        noop: () => Effect.void,
      }),
  })

  it("forwards non-secret overrides to the WasmRpc constructor as TypedAgentConfigValue[]", async () => {
    __setRpcResponder(() => ({ tag: "ok", val: { tag: "tuple", val: [] } }))
    await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Counter2.client.get(
          { initial: 0 },
          { overrides: { greeting: "hej" } },
        )
        yield* remote.noop({})
      }) as Effect.Effect<void, unknown, never>,
    )
    const c = __getRecordedRpcCalls()[0]!
    expect(c.agentConfig.length).toBe(1)
    expect((c.agentConfig[0] as { path: Array<string> }).path).toEqual(["greeting"])
  })

  it("rejects secret overrides at runtime via the encodeOverrides guard", async () => {
    const result = await Effect.runPromise(
      Effect.result(
        Counter2.client.get(
          { initial: 0 },
          // The type-level NonSecretOverride strips `apiKey`; we cast
          // to bypass that and verify the runtime guard rejects too.
          { overrides: { apiKey: "leak" } as unknown as never },
        ) as Effect.Effect<unknown, any, never>,
      ),
    )
    expect(result._tag).toBe("Failure")
    if (result._tag !== "Failure") return
    const failure: any = result.failure
    expect(failure._tag).toBe("ConfigError")
    expect(failure.reason._tag).toBe("Unsupported")
  })
})

describe("AgentClient (ephemeral)", () => {
  beforeEach(() => {
    __reset()
    __resetIdempotency()
  })

  it("ephemeral mode hides `get`; only phantom variants are present at runtime", () => {
    // `get` is stripped at the type level; cast for the runtime-only check.
    expect((Worker.client as any).get).toBeUndefined()
    expect(typeof Worker.client.getPhantom).toBe("function")
    expect(typeof Worker.client.newPhantom).toBe("function")
  })

  it("newPhantom on ephemeral can invoke methods", async () => {
    const okWv = await encodeWv(Schema.String, "done")
    __setRpcResponder(({ methodName }) => {
      if (methodName === "run") {
        return {
          tag: "ok",
          val: { tag: "tuple", val: [{ tag: "component-model", val: okWv }] },
        }
      }
      return { tag: "throw", error: new Error("unexpected") }
    })
    const result = await Effect.runPromise(
      Effect.gen(function* () {
        const remote = yield* Worker.client.newPhantom({ jobId: "j-1" })
        const out = yield* remote.run({ times: 3 })
        return { phantomId: remote.phantomId, out }
      }) as Effect.Effect<{ phantomId: string; out: string }, unknown, never>,
    )
    expect(result.out).toBe("done")
    expect(result.phantomId).toMatch(/^[0-9a-f-]{36}$/)

    const calls: ReadonlyArray<RecordedRpcCall> = __getRecordedRpcCalls()
    expect(calls.length).toBe(1)
    expect(calls[0]!.agentTypeName).toBe("Worker")
    const decoded = await decodeWv(Schema.Number, calls[0]!.input.val[0].val)
    expect(decoded).toBe(3)
  })
})
