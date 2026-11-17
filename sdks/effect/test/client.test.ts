import { describe, expect, it } from "@effect/vitest"
import { Cause, DateTime, Effect, Exit, Fiber, Layer, Option, Result, Schema, Stream } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { AgentId } from "../src/AgentId.js"
import { defineAgent } from "../src/Agent.js"
import { defineAgentClient } from "../src/index.js"
import * as Client from "../src/Client.js"
import { defineConfig } from "../src/Config.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { RpcHostError, type RpcConnection } from "../src/host/RpcClient.js"
import { compileMethodSpec, invokeMethod, method } from "../src/Method.js"
import { PrincipalSchema } from "../src/Principal.js"
import {
  Option as UnstructuredOption,
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "../src/Unstructured.js"
import { compile } from "../src/WitCodec.js"
import * as WitTypes from "../src/WitTypes.js"
import {
  agentIdFor,
  hostAgentIdFor,
  hostInvocationMetadata,
  sdkInvocationMetadata,
} from "./invocation-metadata.js"
import { rpcLayer } from "./rpc-fake.js"

const metadata = (suffix: string): AgentHost.InvocationMetadata =>
  hostInvocationMetadata(`agent-${suffix}`)
const expectedMetadata = (suffix: string) => sdkInvocationMetadata(`agent-${suffix}`)

/** How the host opened a client. */
type Target =
  | { readonly kind: "upsert"; readonly constructor: CoreTypes.SchemaValueTree }
  | { readonly kind: "getById"; readonly agentId: CoreTypes.AgentId }
  | {
      readonly kind: "create"
      readonly constructor: CoreTypes.SchemaValueTree
      readonly options: AgentHost.CreateOptions
    }

interface Open {
  readonly agentType: string
  readonly target: Target
}

interface Call {
  readonly kind: "await" | "trigger" | "schedule"
  readonly agentType: string
  readonly method: string
  readonly target: Target
  readonly input: CoreTypes.SchemaValueTree
  readonly scheduledTime?: AgentHost.Datetime
}

const constructorOf = (target: Target | undefined) =>
  target !== undefined && target.kind !== "getById" ? target.constructor : undefined

/** The agent ID that `create` issues in the fake host. */
const createdAgentId = hostAgentIdFor("created")
/** The agent ID of a client of `upsert` in the fake host. */
const upsertedAgentId = hostAgentIdFor("upserted")

const makeRuntime = (
  options: {
    readonly response?: CoreTypes.SchemaValueTree | undefined
    readonly error?: unknown
    readonly pending?: boolean
    readonly rejected?: boolean
    /** Fails every client open with this host `rpc-error`. */
    readonly openError?: AgentHost.RpcError
    /** The deployed agent type that `getAgentTypeByAgentId` reports. */
    readonly registered?: { readonly typeName: string; readonly mode?: AgentCommon.AgentMode }
  } = {},
) => {
  const calls: Call[] = []
  const opens: Open[] = []
  const lifecycle = {
    connectionOpen: 0,
    connectionDrop: 0,
    futureCancel: 0,
    futureDrop: 0,
    tokenCancel: 0,
    tokenDrop: 0,
  }
  const awaitedMetadata = metadata("await")
  let resolvePending: ((value: CoreTypes.SchemaValueTree | undefined) => void) | undefined
  const connect = (agentType: string, target: Target): RpcConnection => {
    lifecycle.connectionOpen++
    opens.push({ agentType, target })
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
        target,
        input,
        scheduledTime,
      })
    return {
      agentId: () =>
        Promise.resolve(
          target.kind === "getById"
            ? target.agentId
            : target.kind === "create"
              ? createdAgentId
              : upsertedAgentId,
        ),
      invokeAndAwait: () => ({ metadata: metadata("sync"), result: options.response }),
      invoke: (methodName, input) => {
        record("trigger", methodName, input)
        if (options.error !== undefined) throw options.error
        return metadata("trigger")
      },
      asyncInvokeAndAwait: (methodName, input) => {
        record("await", methodName, input)
        return {
          metadata: options.rejected ? undefined : awaitedMetadata,
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
  const open = <A>(agentType: string, target: Target, value: (c: RpcConnection) => A) =>
    options.openError === undefined
      ? Effect.succeed(value(connect(agentType, target)))
      : Effect.fail(new RpcHostError(options.openError, `WasmRpc.${target.kind}`))
  const rpc = rpcLayer({
    upsert: (agentType, constructor) =>
      open(agentType, { kind: "upsert", constructor }, (connection) => connection),
    getById: (agentType, agentId) =>
      open(agentType, { kind: "getById", agentId }, (connection) => connection),
    create: (agentType, constructor, createOptions) =>
      open(agentType, { kind: "create", constructor, options: createOptions }, (connection) => ({
        connection,
        agentId: createdAgentId,
      })),
  })
  const host = Layer.succeed(AgentHostClient, {
    getAgentTypeByAgentId: () =>
      options.registered === undefined
        ? undefined
        : {
            agentType: {
              typeName: options.registered.typeName,
              mode: options.registered.mode ?? "durable",
            },
          },
  } as never)
  return {
    layer: Layer.mergeAll(rpc, host),
    calls,
    opens,
    lifecycle,
    awaitedMetadata,
    hasPending: () => resolvePending !== undefined,
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
  it.effect(
    "binds the returned self ID, rejects another agent type, and releases the resource",
    () =>
      Effect.gen(function* () {
        for (const typeName of ["Counter", "Other", "host-refusal"]) {
          let drops = 0
          let invoked = false
          const selfId = hostAgentIdFor("self")
          const layer = Layer.merge(
            rpcLayer({
              bindSelf: () =>
                typeName === "host-refusal"
                  ? Effect.fail(
                      new RpcHostError(
                        { tag: "denied", val: "ephemeral owner" },
                        "WasmRpc.bindSelf",
                      ),
                    )
                  : Effect.succeed([
                      selfId,
                      {
                        agentId: () => Promise.resolve(selfId),
                        invokeAndAwait: () => {
                          throw new Error("unexpected awaited invocation")
                        },
                        asyncInvokeAndAwait: () => {
                          throw new Error("unexpected async invocation")
                        },
                        scheduleInvocation: () => {
                          throw new Error("unexpected scheduled invocation")
                        },
                        scheduleCancelableInvocation: () => {
                          throw new Error("unexpected cancelable invocation")
                        },
                        invoke: (name: string) => {
                          expect(name).toBe("add")
                          invoked = true
                          return metadata("self")
                        },
                        drop: () => {
                          drops++
                        },
                      } satisfies RpcConnection,
                    ] as const),
            }),
            Layer.succeed(AgentHostClient, {
              getAgentTypeByAgentId: (id: CoreTypes.AgentId) => {
                expect(id).toEqual(selfId)
                return { agentType: { typeName, mode: "durable" } }
              },
            } as never),
          )
          const result = yield* Counter.client.bindSelf().pipe(
            Effect.flatMap((client) => Effect.zip(client.add.trigger({ by: 7 }), client.agentId)),
            Effect.scoped,
            Effect.provide(layer),
            Effect.result,
          )
          expect(invoked).toBe(typeName === "Counter")
          if (typeName === "Counter") {
            expect(result).toMatchObject({
              _tag: "Success",
              success: [undefined, agentIdFor("self")],
            })
            expect(drops).toBe(1)
          } else {
            expect(result).toMatchObject({
              _tag: "Failure",
              failure: {
                _tag: "RpcCallError",
                cause: { tag: typeName === "Other" ? "remote-agent-error" : "denied" },
              },
            })
            if (typeName === "Other")
              expect(result).toMatchObject({ failure: { cause: { val: { tag: "invalid-type" } } } })
            expect(drops).toBe(typeName === "Other" ? 1 : 0)
          }
        }
      }),
  )
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
      const first = Stream.make(11)
      const sibling = Stream.make(22)
      const scheduledFirst = Stream.make(33)
      const scheduledSibling = Stream.make(44)

      yield* Effect.gen(function* () {
        const remote = yield* StreamingInput.client.upsert({})
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
      expect([...(yield* Stream.runCollect(first))]).toEqual([11])
      expect([...(yield* Stream.runCollect(sibling))]).toEqual([22])
      expect([...(yield* Stream.runCollect(scheduledFirst))]).toEqual([33])
      expect([...(yield* Stream.runCollect(scheduledSibling))]).toEqual([44])
    }),
  )

  it.effect(
    "rejects recursive output streams for trigger and schedule before host invocation",
    () =>
      Effect.gen(function* () {
        interface Node {
          readonly label: string
          readonly children: Stream.Stream<Node, unknown>
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
          const remote = yield* StreamingOutput.client.upsert({})
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
      const value = yield* Counter.client.upsert({ initial: 7 }).pipe(
        Effect.flatMap((remote) => remote.value({})),
        Effect.scoped,
        Effect.provide(runtime.layer),
      )
      expect(value).toBe(42)
      expect(runtime.calls[0]!.kind).toBe("await")
      expect(runtime.calls[0]!.agentType).toBe("Counter")
      expect(runtime.opens[0]!.target.kind).toBe("upsert")
      expect(constructorOf(runtime.calls[0]!.target)!.valueNodes.length).toBeGreaterThan(0)
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
        const remote = yield* Counter.client.upsert({ initial: 0 })
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
        const remote = yield* Counter.client.upsert({ initial: 0 })
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
      yield* Counter.client.upsert({ initial: 0 }).pipe(
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

  it.effect("maps upsert, create and getById to the host functions of the same name", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime()
      const id = agentIdFor("existing")
      const ids = yield* Effect.gen(function* () {
        const upserted = yield* Counter.client.upsert({ initial: 1 })
        const created = yield* Counter.client.create({ initial: 2 })
        const keyed = yield* Counter.client.create({ initial: 3 }, { keyed: true })
        const found = yield* Counter.client.getById(id)
        return [
          yield* upserted.agentId,
          yield* created.agentId,
          yield* keyed.agentId,
          yield* found.agentId,
        ]
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.opens.map(({ agentType, target }) => [agentType, target.kind])).toEqual([
        ["Counter", "upsert"],
        ["Counter", "create"],
        ["Counter", "create"],
        ["Counter", "getById"],
      ])
      const [, create, keyedCreate, byId] = runtime.opens.map(({ target }) => target)
      expect(create).toMatchObject({ options: { keyed: false, agentConfig: [], env: [] } })
      expect(keyedCreate).toMatchObject({ options: { keyed: true, agentConfig: [], env: [] } })
      expect(byId).toEqual({ kind: "getById", agentId: hostAgentIdFor("existing") })
      for (const [index, value] of [1, 2, 3].entries()) {
        const tree = constructorOf(runtime.opens[index]!.target)!
        expect(tree.valueNodes).toContainEqual({ tag: "f64-value", val: value })
      }
      expect(ids).toEqual([
        agentIdFor("upserted"),
        agentIdFor("created"),
        agentIdFor("created"),
        id,
      ])
      expect(String(ids[3])).toBe(id.uuid)
      expect(runtime.lifecycle).toMatchObject({ connectionOpen: 4, connectionDrop: 4 })
      expect(runtime.calls).toHaveLength(0)

      const invalid = yield* Counter.client
        .getById({ uuid: "bad" } as AgentId)
        .pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
      expect(invalid).toMatchObject({
        _tag: "Failure",
        failure: { _tag: "InvalidUuidError", value: "bad" },
      })
      expect(runtime.lifecycle.connectionOpen).toBe(4)
    }),
  )

  it.effect("reports a typed not-found for an unknown ID and conflict for a current key", () =>
    Effect.gen(function* () {
      for (const [cause, open] of [
        [{ tag: "not-found", val: "no agent of type Counter has the ID" }, "getById"],
        [{ tag: "conflict", val: "the key Counter(3) is current" }, "keyed create"],
      ] as const) {
        const runtime = makeRuntime({ openError: cause })
        const result = yield* (
          open === "getById"
            ? Counter.client.getById(agentIdFor("missing"))
            : Counter.client.create({ initial: 3 }, { keyed: true })
        ).pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
        expect(result).toEqual(Result.fail({ _tag: "RpcCallError", cause }))
        expect(runtime.lifecycle.connectionOpen).toBe(0)
      }
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
        const actual = yield* RawOutputs.client.upsert({}).pipe(
          Effect.flatMap((remote) => (remote[methodName] as any)({})),
          Effect.scoped,
          Effect.provide(runtime.layer),
        ) as Effect.Effect<unknown, unknown, never>
        expect(actual).toEqual(expected)
      }
    }),
  )

  it.effect("decodes unstructured outputs combined with typed errors and options", () =>
    Effect.gen(function* () {
      const Pending = Schema.Struct({ _tag: Schema.Literal("Pending"), retryAfter: Schema.Number })
      const RawResults = defineAgent({
        name: "RawResults",
        id: {},
        methods: {
          binary: method({ input: {}, success: UnstructuredBinary(), error: Pending }),
          text: method({ input: {}, success: UnstructuredText(), error: Pending }),
          maybe: method({ input: {}, success: UnstructuredOption(UnstructuredBinary()) }),
          lookup: method({
            input: {},
            success: UnstructuredOption(UnstructuredBinary()),
            error: Pending,
          }),
        },
      })
      const bytes: BinaryReferenceValue = {
        _tag: "inline",
        val: new Uint8Array([9, 8, 7]),
        mimeType: "application/octet-stream",
      }
      const url: BinaryReferenceValue = { _tag: "url", val: "https://example.com/blob" }
      const text: TextReferenceValue = { _tag: "inline", val: "ready" }
      const pending = { _tag: "Pending" as const, retryAfter: 3 }
      const cases: ReadonlyArray<
        readonly [keyof typeof RawResults.methods, Effect.Effect<unknown, unknown>]
      > = [
        ["binary", Effect.succeed(bytes)],
        ["binary", Effect.succeed(url)],
        ["binary", Effect.fail(pending)],
        ["text", Effect.succeed(text)],
        ["text", Effect.fail(pending)],
        ["maybe", Effect.succeed(Option.none())],
        ["maybe", Effect.succeed(Option.some(bytes))],
        ["lookup", Effect.succeed(Option.none())],
        ["lookup", Effect.succeed(Option.some(bytes))],
        ["lookup", Effect.succeed(Option.some(url))],
        ["lookup", Effect.fail(pending)],
      ]
      for (const [methodName, handler] of cases) {
        const codec = yield* compileMethodSpec(methodName, RawResults.methods[methodName] as any)
        const input = yield* codec.inputCodec.encodeAsync({})
        const response = yield* invokeMethod(codec, () => handler as any, input) as Effect.Effect<
          CoreTypes.SchemaValueTree | undefined,
          unknown,
          never
        >
        const runtime = makeRuntime({ response })
        const actual = yield* RawResults.client.upsert({}).pipe(
          Effect.flatMap((remote) => (remote[methodName] as any)({})),
          Effect.scoped,
          Effect.provide(runtime.layer),
          Effect.result,
        ) as Effect.Effect<Result.Result<unknown, unknown>, never, never>
        expect(actual).toEqual(yield* Effect.result(handler))
      }
    }),
  )

  it.live("interrupts and drops an in-flight future", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ pending: true })
      const exit = yield* Effect.gen(function* () {
        const remote = yield* Counter.client.upsert({ initial: 0 })
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
      const result = yield* Counter.client.upsert({ initial: 0 }).pipe(
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

  it.effect("snapshots invocation metadata before awaiting completion", () =>
    Effect.gen(function* () {
      const response = yield* encode(Schema.String, "done")
      const runtime = makeRuntime({ pending: true })
      const result = yield* Effect.gen(function* () {
        const remote = yield* Worker.client.upsert({ job: "snapshot" })
        const fiber = yield* Effect.forkChild(remote.run({ times: 1 }))
        while (!runtime.hasPending()) yield* Effect.yieldNow
        runtime.awaitedMetadata.agentId = hostAgentIdFor("mutated-after-start")
        runtime.resolve(response)
        return yield* Fiber.join(fiber)
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(result.metadata).toEqual(expectedMetadata("await"))
      expect(Object.isFrozen(result.metadata)).toBe(true)
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
        const result = yield* Counter.client.upsert({ initial: 0 }).pipe(
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

  it.effect("fails a rejected invocation with the error from its future", () =>
    Effect.gen(function* () {
      const response = yield* encode(Schema.String, "done")
      const failureOf = (options: Parameters<typeof makeRuntime>[0]) => {
        const runtime = makeRuntime({ ...options, rejected: true })
        return Worker.client.upsert({ job: "rejected" }).pipe(
          Effect.flatMap((remote) => remote.run({ times: 1 })),
          Effect.scoped,
          Effect.provide(runtime.layer),
          Effect.flip,
          Effect.map((failure) => ({ failure: failure as any, lifecycle: runtime.lifecycle })),
        )
      }
      const denied = yield* failureOf({ error: { tag: "denied", val: "no permission" } })
      expect(denied.failure).toEqual({
        _tag: "RpcCallError",
        cause: { tag: "denied", val: "no permission" },
      })
      expect(denied.lifecycle).toMatchObject({ futureCancel: 1, futureDrop: 1 })
      const completed = yield* failureOf({ response })
      expect(completed.failure._tag).toBe("RpcCallError")
      expect(completed.failure.cause.tag).toBe("protocol-error")
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
        yield* Lookup.client.upsert({}).pipe(
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
      const result = yield* Lookup.client.upsert({}).pipe(
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
        yield* Commands.client.upsert({}).pipe(
          Effect.flatMap((remote) => remote.run({})),
          Effect.scoped,
          Effect.provide(ok.layer),
        ),
      ).toBeUndefined()

      const malformed = makeRuntime({ response: undefined })
      const result = yield* Commands.client.upsert({}).pipe(
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
  it.effect("passes config and env overrides to create and rejects a secret override", () =>
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
      const caller = defineAgentClient({
        name: "Configured",
        id: {},
        config: ClientConfig,
        methods: Configured.methods,
      })
      const runtime = makeRuntime()
      yield* Configured.client
        .create({}, { config: { greeting: "hello" }, env: { MODE: "test", LEVEL: "debug" } })
        .pipe(
          Effect.flatMap((client) => client.ping({})),
          Effect.scoped,
          Effect.provide(runtime.layer),
        )
      yield* caller.client.create({}, { keyed: true, config: { greeting: "keyed" } }).pipe(
        Effect.flatMap((client) => client.ping({})),
        Effect.scoped,
        Effect.provide(runtime.layer),
      )
      const [plain, keyed] = runtime.opens.map(({ target }) => target)
      if (plain?.kind !== "create" || keyed?.kind !== "create") throw new Error("expected create")
      expect(plain.options.keyed).toBe(false)
      expect(plain.options.env).toEqual([
        ["MODE", "test"],
        ["LEVEL", "debug"],
      ])
      expect(plain.options.agentConfig.map((entry) => entry.path)).toEqual([["greeting"]])
      expect(keyed.options.keyed).toBe(true)
      expect(keyed.options.env).toEqual([])
      expect(keyed.options.agentConfig.map((entry) => entry.path)).toEqual([["greeting"]])
      expect(keyed.options.agentConfig[0]!.value).not.toEqual(plain.options.agentConfig[0]!.value)

      const rejected = yield* caller.client
        .create({}, { config: { secret: "leak" } as never })
        .pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
      expect(rejected._tag).toBe("Failure")
      const unconfigured = yield* Counter.client
        .create({ initial: 0 }, { config: { greeting: "x" } as never })
        .pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
      expect(unconfigured).toMatchObject({ _tag: "Failure", failure: { _tag: "ConfigError" } })
      expect(runtime.calls).toHaveLength(2)
      expect(runtime.lifecycle.connectionOpen).toBe(2)
    }),
  )

  it.effect("uses one ephemeral client and returns final identity on every operation", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({ response: yield* encode(Schema.String, "done") })
      yield* Effect.gen(function* () {
        expect((Worker.client as any).create).toBeUndefined()
        expect((Worker.client as any).getById).toBeUndefined()
        expect((Worker.client as any).bindSelf).toBeUndefined()
        const remote = yield* Worker.client.upsert({ job: "j" })
        expect("agentId" in remote).toBe(false)
        const awaited = yield* remote.run({ times: 1 })
        expect(awaited).toEqual({ metadata: expectedMetadata("await"), value: "done" })
        expect(yield* remote.run.trigger({ times: 2 })).toEqual(expectedMetadata("trigger"))
        const receipt = yield* remote.run.schedule({ seconds: 1n, nanoseconds: 0 }, { times: 3 })
        expect(receipt.metadata).toEqual(expectedMetadata("scheduled"))
        yield* receipt.cancel()
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.opens.map(({ target }) => target.kind)).toEqual(["upsert"])
    }),
  )

  it.effect("binds method-only and full durable clients to an agent ID", () =>
    Effect.gen(function* () {
      const runtime = makeRuntime({
        response: yield* encode(Schema.Number, 42),
        registered: { typeName: "Counter" },
      })
      const id = agentIdFor("counter")
      yield* Effect.gen(function* () {
        const complete = defineAgentClient({
          name: "Counter",
          id: Counter.id,
          methods: Counter.methods,
        })
        expect(yield* (yield* Client.bind(id, complete)).value({})).toBe(42)
        expect(yield* (yield* complete.client.getById(id)).value({})).toBe(42)
        expect(yield* (yield* Client.bind(id, Counter)).value({})).toBe(42)

        const methodOnly = defineAgentClient({ methods: Counter.methods })
        expect("client" in methodOnly).toBe(false)
        const bound = yield* Client.bind(id, methodOnly)
        expect(yield* bound.value({})).toBe(42)
        expect(yield* bound.agentId).toEqual(id)
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.opens).toEqual(
        Array.from({ length: 4 }, () => ({
          agentType: "Counter",
          target: { kind: "getById", agentId: hostAgentIdFor("counter") },
        })),
      )
      expect(runtime.lifecycle.connectionDrop).toBe(4)
    }),
  )

  it.effect("omits host-injected principal fields from caller-owned identities and factories", () =>
    Effect.gen(function* () {
      const PrincipalTarget = defineAgent({
        name: "PrincipalTarget",
        id: { name: Schema.String, principal: PrincipalSchema },
        methods: {
          value: method({ input: { principal: PrincipalSchema }, success: Schema.String }),
          mixed: method({
            input: { before: Schema.String, principal: PrincipalSchema, after: Schema.Number },
            success: Schema.String,
          }),
        },
      })
      const runtime = makeRuntime({ response: yield* encode(Schema.String, "ok") })
      yield* Effect.gen(function* () {
        const full = yield* Client.bind(agentIdFor("principal"), PrincipalTarget)
        expect(yield* full.value({})).toBe("ok")
        const input = { before: "first", after: 7 }
        expect(yield* full.mixed(input)).toBe("ok")
        yield* full.mixed.trigger(input)
        yield* full.mixed.schedule({ seconds: 1n, nanoseconds: 0 }, input)
        yield* full.value.trigger({})
        yield* full.value.schedule({ seconds: 1n, nanoseconds: 0 }, {})
        const codec = yield* compileMethodSpec("mixed", PrincipalTarget.methods.mixed)
        expect(codec.inputSchema.val.map((field) => [field.name, field.source.tag])).toEqual([
          ["before", "user-supplied"],
          ["principal", "auto-injected"],
          ["after", "user-supplied"],
        ])
        for (const call of runtime.calls) {
          expect(yield* codec.inputCodec.decode(call.input).pipe(Effect.option)).toEqual(
            call.method === "mixed" ? Option.some(input) : Option.none(),
          )
          if (call.method === "value") expect(call.input.valueNodes).toHaveLength(1)
        }
        const factory = yield* PrincipalTarget.client.upsert({ name: "main" })
        expect(yield* factory.value({})).toBe("ok")
      }).pipe(Effect.scoped, Effect.provide(runtime.layer))
      expect(runtime.lifecycle.connectionOpen).toBe(2)
      const upserted = constructorOf(runtime.opens[1]?.target)!
      expect(upserted.valueNodes.filter((node) => node.tag === "record-value")).toHaveLength(1)
      expect(upserted.valueNodes).toHaveLength(2)
    }),
  )

  it.effect("rejects ephemeral and undeployed ID binding before opening RPC", () =>
    Effect.gen(function* () {
      const ephemeralClient = defineAgentClient({
        name: "Worker",
        mode: "ephemeral",
        id: Worker.id,
        methods: Worker.methods,
      })
      expect("upsert" in ephemeralClient.client).toBe(true)
      expect("getById" in ephemeralClient.client).toBe(false)
      const methodOnly = defineAgentClient({ methods: Counter.methods })
      for (const [binding, registered, failure] of [
        [ephemeralClient, undefined, { _tag: "ClientBindingError" }],
        [Worker, undefined, { _tag: "ClientBindingError" }],
        [methodOnly, { typeName: "Worker", mode: "ephemeral" }, { _tag: "ClientBindingError" }],
        [methodOnly, undefined, { _tag: "RpcCallError", cause: { tag: "not-found" } }],
      ] as const) {
        const runtime = makeRuntime({ registered })
        const result = yield* Client.bind(
          agentIdFor("worker"),
          binding as Client.AgentIdBinding<unknown>,
        ).pipe(Effect.scoped, Effect.provide(runtime.layer), Effect.result)
        expect(result).toMatchObject({ _tag: "Failure", failure })
        expect(runtime.lifecycle.connectionOpen).toBe(0)
      }
    }),
  )

  it("rejects incomplete client definitions without registering or opening RPC", () => {
    for (const partial of [
      { name: "Counter", methods: Counter.methods },
      { id: Counter.id, methods: Counter.methods },
      { mode: "durable", methods: Counter.methods },
      { config: undefined, methods: Counter.methods },
    ])
      expect(() => defineAgentClient(partial as never)).toThrow(TypeError)
  })

  it.effect("wraps constructor failures as typed RPC errors", () =>
    Effect.gen(function* () {
      const layer = rpcLayer({
        upsert: () =>
          Effect.fail(new RpcHostError({ tag: "not-found", val: "missing" }, "WasmRpc.upsert")),
      })
      const result = yield* Counter.client
        .upsert({ initial: 0 })
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
