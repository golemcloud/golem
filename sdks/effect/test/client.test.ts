import { describe, expect, it } from "@effect/vitest"
import { Cause, DateTime, Effect, Exit, Fiber, Layer, Result, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { defineAgent } from "../src/Agent.js"
import { AgentStream } from "../src/AgentStream.js"
import { defineConfig } from "../src/Config.js"
import { DurabilityModeClient } from "../src/host/DurabilityModeClient.js"
import { RpcClient, RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"
import { compileMethodSpec, invokeMethod, method } from "../src/Method.js"
import {
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "../src/Unstructured.js"
import { compile } from "../src/WitCodec.js"
import * as WitTypes from "../src/WitTypes.js"

const metadata = (suffix: string): AgentHost.InvocationMetadata => ({
  agentId: `agent-${suffix}`,
  idempotencyKey: `key-${suffix}`,
})

interface Call {
  readonly kind: "await" | "trigger" | "schedule"
  readonly agentType: string
  readonly method: string
  readonly constructor: CoreTypes.SchemaValueTree
  readonly input: CoreTypes.SchemaValueTree
  readonly phantom: CoreTypes.Uuid | undefined
  readonly config: ReadonlyArray<AgentCommon.TypedAgentConfigValue>
  readonly scheduledTime?: AgentHost.Datetime
}

const makeRuntime = (
  options: {
    readonly response?: CoreTypes.SchemaValueTree | undefined
    readonly error?: unknown
    readonly pending?: boolean
  } = {},
) => {
  const calls: Call[] = []
  const lifecycle = {
    connectionOpen: 0,
    connectionDrop: 0,
    futureCancel: 0,
    futureDrop: 0,
    tokenCancel: 0,
    tokenDrop: 0,
  }
  let resolvePending: ((value: CoreTypes.SchemaValueTree | undefined) => void) | undefined
  const connect = (
    agentType: string,
    constructor: CoreTypes.SchemaValueTree,
    phantom: CoreTypes.Uuid | undefined,
    config: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ): RpcConnection => {
    lifecycle.connectionOpen++
    const record = (
      kind: Call["kind"],
      methodName: string,
      input: CoreTypes.SchemaValueTree,
      scheduledTime?: AgentHost.Datetime,
    ) =>
      calls.push({
        kind,
        agentType,
        method: methodName,
        constructor,
        input,
        phantom,
        config,
        scheduledTime,
      })
    return {
      invokeAndAwait: () => ({ metadata: metadata("sync"), result: options.response }),
      invoke: (methodName, input) => {
        record("trigger", methodName, input)
        if (options.error !== undefined) throw options.error
        return metadata("trigger")
      },
      asyncInvokeAndAwait: (methodName, input) => {
        record("await", methodName, input)
        return {
          metadata: metadata("await"),
          get: () => {
            if (options.error !== undefined) return Promise.reject(options.error)
            if (!options.pending) return Promise.resolve(options.response)
            return new Promise((resolve) => {
              resolvePending = resolve
            })
          },
          cancel: () => lifecycle.futureCancel++,
          drop: () => lifecycle.futureDrop++,
        }
      },
      scheduleInvocation: () => ({ metadata: metadata("scheduled") }),
      scheduleCancelableInvocation: (time, methodName, input) => {
        record("schedule", methodName, input, time)
        return {
          metadata: metadata("scheduled"),
          token: {
            cancel: () => lifecycle.tokenCancel++,
            drop: () => lifecycle.tokenDrop++,
          },
        }
      },
      drop: () => lifecycle.connectionDrop++,
    }
  }
  const rpc = Layer.succeed(
    RpcClient,
    RpcClient.of({
      connect: (agentType, constructor, phantom, config) =>
        Effect.succeed(connect(agentType, constructor, phantom, config)),
    }),
  )
  const durability = Layer.succeed(
    DurabilityModeClient,
    DurabilityModeClient.of({
      getIdempotenceMode: () => true,
      setIdempotenceMode: () => undefined,
      markBeginOperation: () => 0n,
      markEndOperation: () => undefined,
      oplogCommit: () => undefined,
      generateIdempotencyKey: () => ({ highBits: 1n, lowBits: 2n }),
    }),
  )
  return {
    layer: Layer.mergeAll(rpc, durability),
    calls,
    lifecycle,
    resolve: (v?: CoreTypes.SchemaValueTree) => resolvePending?.(v),
  }
}

const Counter = defineAgent({
  name: "Counter",
  mode: "durable",
  id: { initial: Schema.Number },
  methods: {
    value: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Void }),
  },
})

const Worker = defineAgent({
  name: "Worker",
  mode: "ephemeral",
  id: { job: Schema.String },
  methods: { run: method({ input: { times: Schema.Number }, success: Schema.String }) },
})

const RawOutputs = defineAgent({
  name: "RawOutputs",
  id: {},
  methods: {
    text: method({ input: {}, success: UnstructuredText() }),
    binary: method({ input: {}, success: UnstructuredBinary() }),
  },
})

const encode = <S extends Schema.Top>(schema: S, value: S["Type"]) =>
  Effect.gen(function* () {
    const codec = yield* compile(schema)
    return yield* codec.encode(value)
  })

describe("Client 1.6 durable lifecycle", () => {
  it.effect("rejects nested input streams before invoking or consuming sibling streams", () =>
    Effect.gen(function* () {
      const StreamingInput = defineAgent({
        name: "StreamingInput",
        id: {},
        methods: {
          consume: method({
            input: {
              nested: Schema.Struct({
                streams: Schema.Struct({
                  first: WitTypes.AgentStream(Schema.Number),
                  sibling: WitTypes.AgentStream(Schema.Number),
                }),
              }),
            },
            success: Schema.String,
          }),
        },
      })
      const runtime = makeRuntime()
      const first = AgentStream.from([11])
      const sibling = AgentStream.from([22])
      const scheduledFirst = AgentStream.from([33])
      const scheduledSibling = AgentStream.from([44])

      yield* Effect.gen(function* () {
        const remote = yield* StreamingInput.client.get({})
        const triggered = yield* remote.consume
          .trigger({ nested: { streams: { first, sibling } } })
          .pipe(Effect.result)
        const scheduled = yield* remote.consume
          .schedule(
            { seconds: 1n, nanoseconds: 0 },
            { nested: { streams: { first: scheduledFirst, sibling: scheduledSibling } } },
          )
          .pipe(Effect.result)
        expect(triggered).toMatchObject({
          _tag: "Failure",
          failure: { _tag: "RemoteResponseError" },
        })
        expect(scheduled).toMatchObject({
          _tag: "Failure",
          failure: { _tag: "RemoteResponseError" },
        })
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))

      expect(runtime.calls).toHaveLength(0)
      expect(yield* Effect.promise(() => first.next())).toEqual({ done: false, value: 11 })
      expect(yield* Effect.promise(() => sibling.next())).toEqual({ done: false, value: 22 })
      expect(yield* Effect.promise(() => scheduledFirst.next())).toEqual({
        done: false,
        value: 33,
      })
      expect(yield* Effect.promise(() => scheduledSibling.next())).toEqual({
        done: false,
        value: 44,
      })
    }),
  )

  it.effect(
    "rejects recursive output streams for trigger and schedule before host invocation",
    () =>
      Effect.gen(function* () {
        interface Node {
          readonly label: string
          readonly children: AgentStream<Node>
        }
        const Node: Schema.Codec<Node> = Schema.suspend(() =>
          Schema.Struct({ label: Schema.String, children: WitTypes.AgentStream(Node) }),
        )
        const StreamingOutput = defineAgent({
          name: "StreamingOutput",
          id: {},
          methods: {
            produce: method({ input: { label: Schema.String }, success: Node }),
          },
        })
        const runtime = makeRuntime()

        yield* Effect.gen(function* () {
          const remote = yield* StreamingOutput.client.get({})
          expect(
            yield* remote.produce.trigger({ label: "trigger" }).pipe(Effect.result),
          ).toMatchObject({ _tag: "Failure", failure: { _tag: "RemoteResponseError" } })
          expect(
            yield* remote.produce
              .schedule({ seconds: 1n, nanoseconds: 0 }, { label: "schedule" })
              .pipe(Effect.result),
          ).toMatchObject({ _tag: "Failure", failure: { _tag: "RemoteResponseError" } })
        }).pipe(Effect.scoped, Effect.provide(runtime.layer))

        expect(runtime.calls).toHaveLength(0)
      }),
  )

  it.effect("encodes schema trees and returns value-only awaited results", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ response: yield* encode(Schema.Number, 42) })
      const value = yield* Counter.client.get({ initial: 7 }).pipe(
        Effect.flatMap((remote) => remote.value({})),
        Effect.scoped,
        Effect.provide(runtime.layer),
      )
      expect(value).toBe(42)
      expect(runtime.calls[0]!.kind).toBe("await")
      expect(runtime.calls[0]!.agentType).toBe("Counter")
      expect(runtime.calls[0]!.constructor.valueNodes.length).toBeGreaterThan(0)
      expect(runtime.lifecycle).toMatchObject({
        connectionOpen: 1,
        connectionDrop: 1,
        futureCancel: 1,
        futureDrop: 1,
      })
    }),
  )

  it.effect("trigger returns void while schedule returns an idempotent cancel handle", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime()
      yield* Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        expect(yield* remote.add.trigger({ by: 1 })).toBeUndefined()
        const scheduled = yield* remote.add.schedule({ seconds: 1n, nanoseconds: 2 }, { by: 2 })
        expect("metadata" in scheduled).toBe(false)
        yield* scheduled.cancel()
        yield* scheduled.cancel()
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.calls.map((call) => call.kind)).toEqual(["trigger", "schedule"])
      expect(runtime.lifecycle).toMatchObject({ tokenCancel: 1, tokenDrop: 1 })
    }),
  )

  it.effect("converts Date, DateTime, and epoch milliseconds for scheduling", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime()
      const epoch = 1_700_000_000_123
      yield* Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        for (const at of [
          new Date(epoch),
          DateTime.makeZonedUnsafe(epoch, { timeZone: "Pacific/Auckland" }),
          epoch,
        ])
          yield* remote.add.schedule(at, { by: 1 })
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.calls.map((call) => call.scheduledTime)).toEqual([
        { seconds: 1_700_000_000n, nanoseconds: 123_000_000 },
        { seconds: 1_700_000_000n, nanoseconds: 123_000_000 },
        { seconds: 1_700_000_000n, nanoseconds: 123_000_000 },
      ])
    }),
  )

  it.effect("drops an accepted schedule token without cancelling its work", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime()
      yield* Counter.client.get({ initial: 0 }).pipe(
        Effect.flatMap((remote) => remote.add.schedule({ seconds: 1n, nanoseconds: 0 }, { by: 1 })),
        Effect.scoped,
        Effect.provide(runtime.layer),
      )
      expect(runtime.lifecycle).toMatchObject({
        connectionDrop: 1,
        tokenCancel: 0,
        tokenDrop: 1,
      })
    }),
  )

  it.effect("supports normal, addressed phantom, and freshly generated phantom identities", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime()
      yield* Effect.gen(function* () {
        yield* Counter.client.get({ initial: 0 })
        yield* Counter.client.getPhantom({ initial: 0 }, "12345678-1234-1234-1234-1234567890ab")
        const generated = yield* Counter.client.newPhantom({ initial: 0 })
        expect(generated.phantomId).toBe("00000000-0000-0001-0000-000000000002")
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.calls).toHaveLength(0)

      const invalid = yield* Counter.client
        .getPhantom({ initial: 0 }, "bad")
        .pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
      expect(invalid._tag).toBe("Failure")
      if (invalid._tag === "Failure")
        expect(invalid.failure).toMatchObject({ _tag: "InvalidUuidError", value: "bad" })
    }),
  )

  it.effect("decodes unstructured text and binary outputs", () =>
    Effect.gen(function* () {
      const text: TextReferenceValue = {
        _tag: "inline",
        val: "hello",
        languageCode: "en",
      }
      const binary: BinaryReferenceValue = {
        _tag: "inline",
        val: new Uint8Array([1, 2, 3]),
        mimeType: "application/octet-stream",
      }
      for (const [methodName, expected] of [
        ["text", text],
        ["binary", binary],
      ] as const) {
        const codec = yield* compileMethodSpec(methodName, RawOutputs.methods[methodName] as any)
        const input = yield* codec.inputCodec.encodeAsync({})
        const response = yield* invokeMethod(
          codec,
          () => Effect.succeed(expected),
          input,
        ) as Effect.Effect<CoreTypes.SchemaValueTree | undefined, unknown, never>
        const runtime = makeRuntime({ response })
        const actual = yield* RawOutputs.client.get({}).pipe(
          Effect.flatMap((remote) => (remote[methodName] as any)({})),
          Effect.scoped,
          Effect.provide(runtime.layer),
        ) as Effect.Effect<unknown, unknown, never>
        expect(actual).toEqual(expected)
      }
    }),
  )

  it.live("interrupts and drops an in-flight future", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ pending: true })
      const exit = yield* Effect.gen(function* () {
        const remote = yield* Counter.client.get({ initial: 0 })
        const fiber = yield* Effect.forkChild(remote.value({}))
        yield* Effect.sleep("1 millis")
        yield* Fiber.interrupt(fiber)
        return yield* Fiber.await(fiber)
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true)
      expect(runtime.lifecycle).toMatchObject({ connectionDrop: 1, futureCancel: 1, futureDrop: 1 })
      runtime.resolve()
    }),
  )

  it.live("race interruption and late completion clean up exactly once", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ pending: true })
      const result = yield* Counter.client.get({ initial: 0 }).pipe(
        Effect.flatMap((remote) =>
          Effect.raceFirst(
            remote.value({}),
            Effect.sleep("5 millis").pipe(Effect.andThen(Effect.fail("timeout" as const))),
          ),
        ),
        Effect.scoped,
        Effect.provide(runtime.layer),
        Effect.result,
      )
      expect(result).toEqual(Result.fail("timeout"))
      expect(runtime.lifecycle).toMatchObject({ futureCancel: 1, futureDrop: 1 })
      runtime.resolve(yield* encode(Schema.Number, 7))
      yield* Effect.sleep("1 millis")
      expect(runtime.lifecycle).toMatchObject({ futureCancel: 1, futureDrop: 1 })
    }),
  )

  it.effect("preserves typed RPC errors and does not misclassify arbitrary tagged values", () =>
    Effect.gen(function* () {
      for (const error of [
        { tag: "denied" as const, val: "no" },
        Object.assign(new Error("wrapped"), {
          payload: { tag: "remote-internal-error" as const, val: "boom" },
        }),
        Object.assign(new Error("caused"), {
          cause: { tag: "not-found" as const, val: "gone" },
        }),
        Object.assign(new Error("agent"), {
          payload: {
            tag: "remote-agent-error" as const,
            val: { tag: "custom-error", val: { value: { root: 0, valueNodes: [] } } },
          },
        }),
        new Error("opaque host failure"),
        { tag: "user-tag", val: "not-rpc" },
      ]) {
        const runtime = makeRuntime({ error })
        const result = yield* Counter.client.get({ initial: 0 }).pipe(
          Effect.flatMap((remote) => remote.value({})),
          Effect.scoped,
          Effect.provide(runtime.layer),
          Effect.result,
        )
        expect(result._tag).toBe("Failure")
        if (result._tag === "Failure") {
          const failure = result.failure as any
          expect(failure._tag).toBe("RpcCallError")
          const nested = error instanceof Error ? ((error as any).payload ?? error.cause) : error
          const expectedTag = [
            "denied",
            "not-found",
            "remote-internal-error",
            "remote-agent-error",
          ].includes(nested?.tag)
            ? nested.tag
            : "protocol-error"
          expect(failure.cause.tag).toBe(expectedTag)
          if (expectedTag === "remote-agent-error") expect(failure.cause.val).toEqual(nested.val)
          expect(runtime.lifecycle).toMatchObject({ futureCancel: 1, futureDrop: 1 })
        }
      }
    }),
  )

  it.effect("unwraps user-declared result success and failure", () =>
    Effect.gen(function* () {
      const NotFound = Schema.Struct({ _tag: Schema.Literal("NotFound"), id: Schema.String })
      const Lookup = defineAgent({
        name: "Lookup",
        id: {},
        methods: {
          find: method({ input: { id: Schema.String }, success: Schema.Number, error: NotFound }),
        },
      })
      const okRuntime = makeRuntime({
        response: yield* encode(Schema.Result(Schema.Number, NotFound), Result.succeed(9)),
      })
      expect(
        yield* Lookup.client.get({}).pipe(
          Effect.flatMap((remote) => remote.find({ id: "x" })),
          Effect.scoped,
          Effect.provide(okRuntime.layer),
        ),
      ).toBe(9)
      const failRuntime = makeRuntime({
        response: yield* encode(
          Schema.Result(Schema.Number, NotFound),
          Result.fail({ _tag: "NotFound" as const, id: "x" }) as any,
        ),
      })
      const result = yield* Lookup.client.get({}).pipe(
        Effect.flatMap((remote) => remote.find({ id: "x" })),
        Effect.scoped,
        Effect.provide(failRuntime.layer),
        Effect.result,
      )
      expect(result._tag).toBe("Failure")
      if (result._tag === "Failure") expect(result.failure).toEqual({ _tag: "NotFound", id: "x" })
    }),
  )

  it.effect("handles unit typed success and rejects a missing typed result", () =>
    Effect.gen(function* () {
      const Failure = Schema.Struct({ _tag: Schema.Literal("Failure"), reason: Schema.String })
      const Commands = defineAgent({
        name: "Commands",
        id: {},
        methods: { run: method({ input: {}, success: Schema.Void, error: Failure }) },
      })
      const ok = makeRuntime({
        response: yield* encode(Schema.Result(Schema.Struct({}), Failure), Result.succeed({})),
      })
      expect(
        yield* Commands.client.get({}).pipe(
          Effect.flatMap((remote) => remote.run({})),
          Effect.scoped,
          Effect.provide(ok.layer),
        ),
      ).toBeUndefined()

      const malformed = makeRuntime({ response: undefined })
      const result = yield* Commands.client.get({}).pipe(
        Effect.flatMap((remote) => remote.run({})),
        Effect.scoped,
        Effect.provide(malformed.layer),
        Effect.result,
      )
      expect(result._tag).toBe("Failure")
      if (result._tag === "Failure")
        expect(result.failure).toMatchObject({ _tag: "RemoteResponseError" })
    }),
  )
})

describe("Client 1.6 config and ephemeral receipts", () => {
  it.effect("encodes non-secret config and rejects a secret override before connecting", () =>
    Effect.gen(function* () {
      class ClientConfig extends defineConfig("ClientConfig", {
        greeting: Schema.String,
        secret: Schema.Redacted(Schema.String),
      }) {}
      const Configured = defineAgent({
        name: "Configured",
        id: {},
        config: ClientConfig,
        methods: { ping: method({ input: {}, success: Schema.Void }) },
      })
      const runtime = makeRuntime()
      yield* Configured.client
        .get({}, { overrides: { greeting: "hello" } })
        .pipe(Effect.scoped, Effect.provide(runtime.layer))
      const remote = yield* Configured.client
        .get({}, { overrides: { secret: "leak" } as never })
        .pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
      expect(remote._tag).toBe("Failure")
      expect(runtime.calls).toHaveLength(0)
      expect(runtime.lifecycle.connectionOpen).toBe(1)
    }),
  )

  it.effect("uses one ephemeral client and returns final identity on every operation", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ response: yield* encode(Schema.String, "done") })
      yield* Effect.gen(function* () {
        expect((Worker.client as any).get).toBeUndefined()
        expect((Worker.client as any).getPhantom).toBeUndefined()
        const remote = yield* Worker.client.newPhantom({ job: "j" })
        const awaited = yield* remote.run({ times: 1 })
        expect(awaited).toEqual({ metadata: metadata("await"), value: "done" })
        expect(yield* remote.run.trigger({ times: 2 })).toEqual(metadata("trigger"))
        const receipt = yield* remote.run.schedule({ seconds: 1n, nanoseconds: 0 }, { times: 3 })
        expect(receipt.metadata).toEqual(metadata("scheduled"))
        yield* receipt.cancel()
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.calls.every((call) => call.phantom === undefined)).toBe(true)
    }),
  )

  it.effect("wraps constructor failures as typed RPC errors", () =>
    Effect.gen(function* () {
      const layer = Layer.succeed(
        RpcClient,
        RpcClient.of({
          connect: () =>
            Effect.fail(
              new RpcHostError({ tag: "not-found", val: "missing" }, "WasmRpc.constructor"),
            ),
        }),
      )
      const result = yield* Counter.client
        .get({ initial: 0 })
        .pipe(Effect.scoped, Effect.provide(layer), Effect.result)
      expect(result._tag).toBe("Failure")
      if (result._tag === "Failure")
        expect(result.failure).toEqual({
          _tag: "RpcCallError",
          cause: { tag: "not-found", val: "missing" },
        })
    }),
  )
})
