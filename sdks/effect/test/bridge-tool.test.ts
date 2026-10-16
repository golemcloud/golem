import { describe, expect, it, vi } from "vitest"
import type * as Common from "golem:tool/common@0.1.0"
import { SchemaValueStream } from "golem:core/types@2.0.0"
import { Cause, Deferred, Effect, Exit, Fiber, Option, Scheduler, Schema, Stream } from "effect"
import * as ToolSchema from "../src/Schema.js"
import { compile } from "../src/WitCodec.js"
import {
  createToolClientRuntime,
  decodeTypedSchemaValue,
  startedToolInvocation,
  splitToolRpcError,
  type ToolClientRuntime,
  typedSchemaValueConforms,
} from "../src/BridgeTool.js"
import { ToolClient } from "../src/host/ToolClient.js"
import {
  client,
  compileDefinition,
  toolDefinition,
  ToolTransport,
  type ToolTransport as ToolTransportShape,
} from "../src/Tool.js"
import { ToolType } from "../src/ToolReflection.js"
import type { TypedSchemaValue } from "../src/Bridge.js"
import { emptyMetadata, schemaType, t, v } from "../src/Bridge.js"
import { typedSchemaValueToWit } from "../src/internal/schema-model/wit.js"

const emptyInput: TypedSchemaValue = {
  graph: { defs: new Map(), root: t.record([]) },
  value: { tag: "record", fields: [] },
}

const hostFor = (future: unknown, outputs: AsyncIterable<unknown>[] = []) =>
  ToolClient.of({
    rpc: () => ({ asyncInvokeAndAwait: () => future }),
    createRpc: () => ({ asyncInvokeAndAwait: () => future }),
    createOutput: () => [{}, outputs.shift()],
  } as never)

const checkToolClientRuntimeRequirements = (
  runtime: ToolClientRuntime,
  input: TypedSchemaValue,
) => {
  const started = runtime.start([], input, undefined, false, false)
  // @ts-expect-error invocation startup acquires a resource in Scope
  const scopeFree: Effect.Effect<unknown, unknown, ToolClient> = started
  const scoped: Effect.Effect<unknown, unknown, ToolClient> = Effect.scoped(started)
  void scopeFree
  void scoped
}
void checkToolClientRuntimeRequirements

describe("BridgeTool", () => {
  it.each(["result", "custom", "transport-custom", "rejected"])(
    "reclaims decoded %s capabilities registered by the host observer",
    async (mode) => {
      const disposes = Array.from({ length: 4 }, () =>
        vi.fn(() => {
          if (mode === "rejected") throw new Error("cleanup failed")
        }),
      )
      const next = vi.fn()
      const close = vi.fn(async () => {
        if (mode === "rejected") throw new Error("close failed")
        return { done: true as const, value: undefined }
      })
      const stream = await SchemaValueStream.wrap({
        [Symbol.asyncIterator]: () => ({ next, return: close }),
      })
      Object.assign(stream, { [Symbol.dispose]: disposes[3] })
      const payload: Common.TypedSchemaValue = {
        graph: Effect.runSync(
          compile(
            Schema.Struct({
              secret: ToolSchema.Secret(Schema.String),
              quota: ToolSchema.QuotaToken(),
              permission: ToolSchema.PermissionCard,
              stream: ToolSchema.AgentStream(Schema.Number),
            }),
          ),
        ).schemaGraph,
        value: {
          root: 4,
          valueNodes: [
            { tag: "secret-value", val: { [Symbol.dispose]: disposes[0] } as never },
            { tag: "quota-token-handle", val: { [Symbol.dispose]: disposes[1] } as never },
            { tag: "permission-card-handle", val: { [Symbol.dispose]: disposes[2] } as never },
            { tag: "stream-value", val: stream },
            { tag: "record-value", val: [0, 1, 2, 3] },
          ],
        },
      }
      await Effect.runPromise(
        Effect.gen(function* () {
          const decoded = yield* Deferred.make<void>()
          const output = yield* Deferred.make<void>()
          const future = {
            get: async () => {
              if (mode === "custom")
                throw {
                  tag: "remote-tool-error",
                  val: {
                    tag: "custom-error",
                    val: { name: "failure", payload },
                  },
                }
              return { result: payload }
            },
            cancel: () => Effect.runSync(Deferred.succeed(output, undefined)),
            [Symbol.dispose]: vi.fn(),
          }
          const call = Effect.scoped(
            Effect.gen(function* () {
              const low = yield* createToolClientRuntime("probe").start(
                [],
                emptyInput,
                undefined,
                true,
                false,
              )
              return yield* startedToolInvocation(
                low.stdout,
                low.stderr,
                low.result.pipe(
                  Effect.flatMap((terminal) =>
                    Effect.try({
                      try: () =>
                        decodeTypedSchemaValue(terminal.result!, (value) => {
                          if (mode === "rejected") throw new Error("decoder rejected")
                          return value
                        }),
                      catch: () => ({ tag: "tool" as const, error: "decoder rejected" }),
                    }),
                  ),
                  Effect.mapError((error) =>
                    error.tag === "rpc"
                      ? splitToolRpcError(error.error, (_name, value) => value)
                      : error,
                  ),
                  Effect.onExit(() => Deferred.succeed(decoded, undefined)),
                ),
                low.cancel,
              ).collect
            }),
          ).pipe(
            Effect.provideService(
              ToolClient,
              hostFor(future, [
                {
                  [Symbol.asyncIterator]: () => ({
                    next: async () => {
                      if (mode === "rejected") return { done: true as const, value: undefined }
                      await Effect.runPromise(Deferred.await(output))
                      return { done: true as const, value: undefined }
                    },
                  }),
                },
              ]),
            ),
          )
          const injected =
            mode === "transport-custom"
              ? call.pipe(
                  Effect.provideService(
                    ToolTransport,
                    ToolTransport.of({
                      start: () =>
                        Effect.succeed({
                          result: Effect.fail({
                            tag: "remote-tool-error",
                            val: { tag: "custom-error", val: { name: "failure", payload } },
                          }),
                          stdout: {
                            [Symbol.asyncIterator]: () => ({
                              next: async () => {
                                await Effect.runPromise(Deferred.await(output))
                                return { done: true as const, value: undefined }
                              },
                            }),
                          },
                          cancel: Deferred.succeed(output, undefined).pipe(Effect.asVoid),
                          detach: Effect.void,
                        }),
                    }),
                  ),
                )
              : call
          const fiber = yield* Effect.forkChild(injected)
          yield* Deferred.await(decoded)
          if (mode === "rejected") {
            const collected = yield* Fiber.join(fiber)
            expect(collected.result).toMatchObject({
              failure: { tag: "tool", error: "decoder rejected" },
            })
            return
          }
          disposes.forEach((dispose) => expect(dispose).not.toHaveBeenCalled())
          yield* Fiber.interrupt(fiber)
        }),
      )
      await vi.waitFor(() => disposes.forEach((dispose) => expect(dispose).toHaveBeenCalledOnce()))
      expect(close).toHaveBeenCalledOnce()
      expect(next).not.toHaveBeenCalled()
    },
  )

  it.each([
    "value",
    "json",
    "collect",
    "success",
    "decoded",
    "decoded-success",
    "rejected",
    "custom",
  ])("retains internally collected capabilities until public delivery: %s", async (mode) => {
    const dispose = vi.fn(() => {
      if (mode !== "success" && mode !== "decoded-success") throw new Error("disposer failed")
    })
    const payload: Common.TypedSchemaValue = {
      graph: Effect.runSync(compile(ToolSchema.Secret(Schema.String))).schemaGraph,
      value: {
        root: 0,
        valueNodes: [{ tag: "secret-value", val: { [Symbol.dispose]: dispose } as never }],
      },
    }
    const cancel = vi.fn()
    const definition = toolDefinition("probe").body((body) =>
      body.returns(ToolSchema.Secret(Schema.String)).output(),
    )
    const command = new ToolType({
      lookupName: "probe",
      definition: compileDefinition(definition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 0n } },
    }).client.command([])
    await Effect.runPromise(
      Effect.gen(function* () {
        const delivered = yield* Deferred.make<void>()
        const output = yield* Deferred.make<void>()
        const transport = ToolTransport.of({
          start: () =>
            Effect.succeed({
              result:
                mode === "custom"
                  ? Effect.fail({
                      tag: "remote-tool-error",
                      val: { tag: "custom-error", val: { name: "failure", payload } },
                    })
                  : Effect.succeed({ result: payload }),
              stdout: {
                [Symbol.asyncIterator]: () => ({
                  next: async () => {
                    await Effect.runPromise(Deferred.await(output))
                    return { done: true as const, value: undefined }
                  },
                }),
              },
              cancel: Effect.sync(() => {
                cancel()
                Effect.runSync(Deferred.succeed(output, undefined))
              }),
              detach: Effect.void,
            }),
        })
        const call: Effect.Effect<unknown, unknown, ToolClient> =
          mode === "value"
            ? command.invokeValue({ root: 0, valueNodes: [{ tag: "record-value", val: [] }] })
            : mode === "json"
              ? command.invokeJson({})
              : Effect.scoped(
                  Effect.gen(function* () {
                    const low = yield* createToolClientRuntime("probe").start(
                      [],
                      emptyInput,
                      undefined,
                      true,
                      false,
                    )
                    return yield* startedToolInvocation(
                      low.stdout,
                      low.stderr,
                      low.result.pipe(
                        Effect.flatMap((terminal) =>
                          Effect.try({
                            try: () => {
                              if (mode === "decoded" || mode === "decoded-success")
                                return decodeTypedSchemaValue(terminal.result!, (value) => value)
                              if (mode === "rejected")
                                return decodeTypedSchemaValue(terminal.result!, () => {
                                  throw new Error("rejected")
                                })
                              return terminal
                            },
                            catch: () => ({ tag: "tool" as const, error: "rejected" }),
                          }),
                        ),
                        Effect.mapError((error) =>
                          "tag" in error && error.tag === "rpc"
                            ? splitToolRpcError(error.error, (_name, value) => value)
                            : error,
                        ),
                        Effect.onExit(() => Deferred.succeed(delivered, undefined)),
                      ),
                      low.cancel,
                    ).collect
                  }),
                )
        const fiber = yield* Effect.forkChild(
          call.pipe(
            Effect.provideService(ToolTransport, transport),
            Effect.provideService(ToolClient, hostFor({})),
          ),
        )
        if (mode !== "value" && mode !== "json") yield* Deferred.await(delivered)
        else {
          // Drain runnable work; the only remaining work is the blocked output.
          for (let i = 0; i < 20; i++) yield* Effect.yieldNow
        }
        if (mode === "rejected") expect(dispose).toHaveBeenCalledOnce()
        else expect(dispose).not.toHaveBeenCalled()
        if (mode === "success" || mode === "decoded-success") {
          yield* Deferred.succeed(output, undefined)
          yield* Fiber.join(fiber)
        } else {
          yield* Fiber.interrupt(fiber)
          const exit = yield* Fiber.await(fiber)
          expect(Exit.hasInterrupts(exit)).toBe(true)
        }
      }),
    )
    const successful = mode === "success" || mode === "decoded-success"
    expect(dispose).toHaveBeenCalledTimes(successful ? 0 : 1)
    expect(cancel).toHaveBeenCalledTimes(successful ? 0 : 1)
  })

  it("isolates generated result detach when the transport shares a cancel Effect", async () => {
    let index = 0
    const transport = ToolTransport.of({
      start: () => {
        const id = index++
        let detached = false
        return Effect.succeed({
          result: Effect.suspend(() =>
            detached
              ? Effect.fail({ tag: "protocol-error" as const, val: "detached" })
              : Effect.succeed({
                  result: typedSchemaValueToWit({
                    graph: { defs: new Map(), root: t.string() },
                    value: v.string(`${id}`),
                  }),
                }),
          ),
          cancel: Effect.void,
          detach: Effect.sync(() => {
            detached = true
          }),
        })
      },
    })
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const runtime = createToolClientRuntime("probe")
          const first = yield* runtime.start([], emptyInput, undefined, false, false)
          const second = yield* runtime.start([], emptyInput, undefined, false, false)
          const generated = startedToolInvocation(
            first.stdout,
            first.stderr,
            first.result.pipe(Effect.map(({ result }) => result)),
            first.cancel,
          )
          yield* generated.detach
          expect(Exit.isFailure(yield* Effect.exit(generated.result))).toBe(true)
          expect((yield* second.result).result?.value).toEqual({
            root: 0,
            valueNodes: [{ tag: "string-value", val: "1" }],
          })
        }),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, {} as never),
      ),
    )
  })

  it("passes the declared custom-error name and payload to generated decoders", () => {
    const payload = typedSchemaValueToWit({
      graph: { defs: new Map(), root: t.string() },
      value: v.string("bad"),
    })
    expect(
      splitToolRpcError(
        {
          tag: "remote-tool-error",
          val: { tag: "custom-error", val: { name: "second", payload } },
        },
        (name, decoded) => ({ name, value: decoded.value }),
      ),
    ).toMatchObject({ tag: "tool", error: { name: "second" } })
  })

  it("requires equivalent result graphs and values that conform to the expected graph", () => {
    const expected = { defs: new Map(), root: t.u32({ min: { tag: "unsigned", val: 1n } }) }
    const referenced = {
      defs: new Map([
        [
          "example.Number",
          {
            name: "DisplayNumber",
            body: t.u32({ min: { tag: "unsigned", val: 1n } }),
          },
        ],
      ]),
      root: schemaType({ tag: "ref", id: "example.Number" }),
    }
    expect(
      typedSchemaValueConforms(expected, {
        graph: expected,
        value: { tag: "u32", value: 1 },
      }),
    ).toBe(true)
    expect(
      typedSchemaValueConforms(expected, {
        graph: { defs: new Map(), root: t.u32({ min: { tag: "unsigned", val: 2n } }) },
        value: { tag: "u32", value: 2 },
      }),
    ).toBe(false)
    expect(
      typedSchemaValueConforms(expected, {
        graph: referenced,
        value: { tag: "u32", value: 1 },
      }),
    ).toBe(true)
    expect(
      typedSchemaValueConforms(expected, {
        graph: {
          defs: new Map(),
          root: schemaType(
            { tag: "u32", restrictions: { min: { tag: "unsigned", val: 1n } } },
            {
              ...emptyMetadata(),
              doc: "unexpected",
            },
          ),
        },
        value: { tag: "u32", value: 1 },
      }),
    ).toBe(true)
    expect(
      typedSchemaValueConforms(expected, {
        graph: expected,
        value: { tag: "string", value: "1" },
      }),
    ).toBe(false)
  })

  it("starts an asynchronous contextual transport without leaving Effect", async () => {
    const input: TypedSchemaValue = {
      graph: { defs: new Map(), root: t.record([]) },
      value: { tag: "record", fields: [] },
    }
    let observed: readonly unknown[] | undefined
    const transport: ToolTransportShape = {
      start: (tool, path, wireInput, stdin, stdout) =>
        Effect.promise(async () => {
          await Promise.resolve()
          observed = [tool, path, wireInput, stdin, stdout]
          return { result: Effect.succeed({}), cancel: Effect.void, detach: Effect.void }
        }),
    }
    const program = Effect.scoped(
      Effect.gen(function* () {
        const invocation = yield* createToolClientRuntime("grep").start(
          ["replace"],
          input,
          undefined,
          false,
          false,
        )
        return yield* invocation.result
      }),
    ).pipe(
      Effect.provideService(ToolTransport, transport),
      Effect.provideService(ToolClient, {} as never),
    )
    expect(await Effect.runPromise(program)).toEqual({ result: undefined })
    expect(observed?.[0]).toBe("grep")
    expect(observed?.[1]).toEqual(["replace"])
    expect((observed?.[2] as { graph: { typeNodes: unknown[] } }).graph.typeNodes).toHaveLength(1)
    expect(observed?.[4]).toBe(false)
  })

  it("keeps an invocation alive until its owning scope closes", async () => {
    let cancellations = 0
    const transport: ToolTransportShape = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok" as const, val: Uint8Array.of(1, 2) }
          })(),
          result: Effect.succeed({}),
          cancel: Effect.sync(() => {
            cancellations += 1
          }),
          detach: Effect.void,
        }),
    }
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const invocation = yield* createToolClientRuntime("grep").start(
            [],
            {
              graph: { defs: new Map(), root: t.record([]) },
              value: { tag: "record", fields: [] },
            },
            undefined,
            true,
            false,
          )
          expect(invocation.stdout).toBeDefined()
          expect(yield* Stream.runCollect(invocation.stdout!)).toEqual([Uint8Array.of(1, 2)])
          expect(yield* invocation.result).toEqual({ result: undefined })
          expect(cancellations).toBe(0)
        }),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, {} as never),
      ),
    )
    expect(cancellations).toBe(0)
  })

  it("finalizes an invocation after its result fails", async () => {
    let cancellations = 0
    const transport: ToolTransportShape = {
      start: () =>
        Effect.succeed({
          result: Effect.fail("failed"),
          cancel: Effect.sync(() => {
            cancellations += 1
          }),
          detach: Effect.void,
        }),
    }
    const exit = await Effect.runPromiseExit(
      Effect.scoped(
        Effect.gen(function* () {
          const invocation = yield* createToolClientRuntime("grep").start(
            [],
            {
              graph: { defs: new Map(), root: t.record([]) },
              value: { tag: "record", fields: [] },
            },
            undefined,
            false,
            false,
          )
          return yield* invocation.result
        }),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, {} as never),
      ),
    )
    expect(Exit.isFailure(exit)).toBe(true)
    expect(cancellations).toBe(0)
  })

  it("finalizes an acquired invocation when its pending result is interrupted", async () => {
    let cancellations = 0
    await Effect.runPromise(
      Effect.gen(function* () {
        const resultStarted = yield* Deferred.make<void>()
        const finalized = yield* Deferred.make<void>()
        const transport: ToolTransportShape = {
          start: () =>
            Effect.succeed({
              result: Deferred.succeed(resultStarted, undefined).pipe(Effect.andThen(Effect.never)),
              cancel: Effect.sync(() => {
                cancellations += 1
              }).pipe(Effect.andThen(Deferred.succeed(finalized, undefined)), Effect.asVoid),
              detach: Effect.void,
            }),
        }
        const program = Effect.scoped(
          Effect.gen(function* () {
            const invocation = yield* createToolClientRuntime("grep").start(
              [],
              {
                graph: { defs: new Map(), root: t.record([]) },
                value: { tag: "record", fields: [] },
              },
              undefined,
              false,
              false,
            )
            return yield* startedToolInvocation(
              invocation.stdout,
              invocation.stderr,
              invocation.result,
              invocation.cancel,
            ).collect
          }),
        ).pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, {} as never),
        )
        const fiber = yield* Effect.forkChild(program)
        yield* Deferred.await(resultStarted)
        expect(cancellations).toBe(0)
        yield* Fiber.interrupt(fiber)
        yield* Deferred.await(finalized)
        expect(cancellations).toBe(1)
      }),
    )
  })

  it("exposes detach through the generated mapped-result seam without closing either output", async () => {
    const future = { get: vi.fn(async () => ({})), cancel: vi.fn(), [Symbol.dispose]: vi.fn() }
    const closes = [vi.fn(), vi.fn()]
    const outputs = closes.map((close, index) => ({
      async *[Symbol.asyncIterator]() {
        try {
          yield { tag: "ok", val: Uint8Array.of(index + 1) }
          yield { tag: "ok", val: Uint8Array.of(index + 10) }
        } finally {
          close()
        }
      },
    }))
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const low = yield* createToolClientRuntime("probe").start(
            [],
            emptyInput,
            undefined,
            true,
            true,
          )
          const generated = startedToolInvocation(
            low.stdout,
            low.stderr,
            low.result.pipe(Effect.map(() => "decoded")),
            low.cancel,
          )
          yield* generated.detach
          yield* generated.detach
          yield* generated.cancel
          expect(Exit.isFailure(yield* Effect.exit(generated.result))).toBe(true)
          expect(yield* Stream.runCollect(Stream.take(generated.stdout!, 1))).toEqual([
            Uint8Array.of(1),
          ])
          expect(closes[0]).toHaveBeenCalledOnce()
          expect(closes[1]).not.toHaveBeenCalled()
          expect(yield* Stream.runCollect(generated.stderr!)).toEqual([
            Uint8Array.of(2),
            Uint8Array.of(11),
          ])
        }),
      ).pipe(Effect.provideService(ToolClient, hostFor(future, outputs))),
    )
    expect(future.get).not.toHaveBeenCalled()
    expect(future.cancel).not.toHaveBeenCalled()
    expect(future[Symbol.dispose]).toHaveBeenCalledOnce()
    expect(closes[1]).toHaveBeenCalledOnce()
  })

  it.each(["result", "declared-error", "interrupted"])(
    "detaches during get and disposes late %s capabilities",
    async (kind) => {
      let resolve!: (result: Common.InvocationResult) => void
      let reject!: (cause: unknown) => void
      let observed!: () => void
      let disposed!: () => void
      const observing = new Promise<void>((r) => {
        observed = r
      })
      const disposal = new Promise<void>((r) => {
        disposed = r
      })
      const pending = new Promise<Common.InvocationResult>((r, j) => {
        resolve = r
        reject = j
      })
      const capabilityDispose = vi.fn()
      const payload = {
        graph: {} as never,
        value: {
          root: 0,
          valueNodes: [
            { tag: "secret-value" as const, val: { [Symbol.dispose]: capabilityDispose } as never },
          ],
        },
      }
      const future = {
        get: vi.fn(() => {
          observed()
          return pending
        }),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(disposed),
      }
      await Effect.runPromise(
        Effect.scoped(
          Effect.gen(function* () {
            const low = yield* createToolClientRuntime("probe").start(
              [],
              emptyInput,
              undefined,
              false,
              false,
            )
            const started = startedToolInvocation(low.stdout, low.stderr, low.result, low.cancel)
            const waiter = yield* Effect.forkChild(started.result)
            yield* Effect.promise(() => observing)
            expect(Exit.isFailure(yield* Effect.exit(started.result))).toBe(true)
            if (kind === "interrupted") yield* Fiber.interrupt(waiter)
            else yield* started.detach
            expect(Exit.isFailure(yield* Fiber.await(waiter))).toBe(true)
            expect(future[Symbol.dispose]).not.toHaveBeenCalled()
            expect(future.cancel).not.toHaveBeenCalled()
            yield* started.cancel
            expect(future.cancel).toHaveBeenCalledOnce()
            if (kind !== "declared-error") resolve({ result: payload })
            else
              reject({
                tag: "remote-tool-error",
                val: { tag: "custom-error", val: { name: "rejected", payload } },
              })
            yield* Effect.promise(() => disposal)
            yield* started.cancel
            expect(capabilityDispose).toHaveBeenCalledOnce()
            expect(future.cancel).toHaveBeenCalledOnce()
            expect(future.get).toHaveBeenCalledOnce()
          }),
        ).pipe(Effect.provideService(ToolClient, hostFor(future))),
      )
      expect(future[Symbol.dispose]).toHaveBeenCalledOnce()
    },
  )

  it("observes a result once, retaining explicit cancellation until detach", async () => {
    const future = { get: vi.fn(async () => ({})), cancel: vi.fn(), [Symbol.dispose]: vi.fn() }
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const started = yield* createToolClientRuntime("probe").start(
            [],
            emptyInput,
            undefined,
            false,
            false,
          )
          expect(yield* started.result).toEqual({ result: undefined })
          expect(Exit.isFailure(yield* Effect.exit(started.result))).toBe(true)
          yield* started.cancel
          yield* started.detach
          yield* started.cancel
        }),
      ).pipe(Effect.provideService(ToolClient, hostFor(future))),
    )
    expect(future.get).toHaveBeenCalledOnce()
    expect(future.cancel).toHaveBeenCalledOnce()
    expect(future[Symbol.dispose]).toHaveBeenCalledOnce()
  })

  it.each(["success", "declared"])(
    "releases every unclaimed %s payload when detach races settlement",
    async (kind) => {
      for (let ticks = 0; ticks < 6; ticks++) {
        let resolve!: (value: Common.InvocationResult) => void
        let reject!: (cause: unknown) => void
        let observing!: () => void
        let disposal!: () => void
        const observed = new Promise<void>((r) => {
          observing = r
        })
        const disposed = new Promise<void>((r) => {
          disposal = r
        })
        const pending = new Promise<Common.InvocationResult>((r, j) => {
          resolve = r
          reject = j
        })
        const release = vi.fn()
        let finishClose!: () => void
        let beginClose!: () => void
        const closing = new Promise<void>((r) => {
          beginClose = r
        })
        const closeGate = new Promise<void>((r) => {
          finishClose = r
        })
        const raw = await SchemaValueStream.wrap({
          [Symbol.asyncIterator]: () => ({
            next: async () => ({ done: true as const, value: undefined }),
            return: async () => {
              beginClose()
              await closeGate
              return { done: true as const, value: undefined }
            },
          }),
        })
        Object.assign(raw, { [Symbol.dispose]: release })
        const payload: Common.TypedSchemaValue = {
          graph: {} as never,
          value: {
            root: 0,
            valueNodes: [{ tag: "stream-value", val: raw }],
          },
        }
        const future = {
          get: () => {
            observing()
            return pending
          },
          cancel: vi.fn(),
          [Symbol.dispose]: disposal,
        }
        await Effect.runPromise(
          Effect.scoped(
            Effect.gen(function* () {
              const started = yield* createToolClientRuntime("probe").start(
                [],
                emptyInput,
                undefined,
                false,
                false,
              )
              const waiter = yield* Effect.forkChild(Effect.exit(started.result))
              yield* Effect.promise(() => observed)
              if (kind === "success") resolve({ result: payload })
              else
                reject({
                  tag: "remote-tool-error",
                  val: { tag: "custom-error", val: { name: "rejected", payload } },
                })
              yield* Effect.promise(async () => {
                for (let step = 0; step < ticks; step++) await Promise.resolve()
              })
              const detacher = yield* Effect.forkChild(started.detach)
              const exit = yield* Fiber.join(waiter)
              const failure = Exit.isFailure(exit)
                ? Cause.findErrorOption(exit.cause)
                : Option.none()
              const delivered =
                Exit.isSuccess(exit) ||
                (Option.isSome(failure) &&
                  failure.value.tag === "rpc" &&
                  failure.value.error.tag === "remote-tool-error")
              if (!delivered) yield* Effect.promise(() => closing)
              finishClose()
              yield* Fiber.join(detacher)
              yield* Effect.promise(() => disposed)
              expect(release).toHaveBeenCalledTimes(delivered ? 0 : 1)
              expect(future.cancel).not.toHaveBeenCalled()
            }),
          ).pipe(Effect.provideService(ToolClient, hostFor(future))),
        )
      }
    },
  )

  it.each(["generated", "typed", "reflected"])(
    "cancels %s whole calls before joining output cleanup",
    async (mode) => {
      const events: string[] = []
      let getStarted!: () => void
      let readStarted!: () => void
      let resolveResult!: (value: Common.InvocationResult) => void
      let resolveRead!: (value: IteratorResult<unknown>) => void
      const getting = new Promise<void>((r) => {
        getStarted = r
      })
      const reading = new Promise<void>((r) => {
        readStarted = r
      })
      const result = new Promise<Common.InvocationResult>((r) => {
        resolveResult = r
      })
      const next = new Promise<IteratorResult<unknown>>((r) => {
        resolveRead = r
      })
      let cancellation!: () => void
      const cancelled = new Promise<void>((r) => {
        cancellation = r
      })
      let disposal!: () => void
      const disposed = new Promise<void>((r) => {
        disposal = r
      })
      const future = {
        get: () => {
          getStarted()
          return result
        },
        cancel: () => {
          events.push("cancel")
          cancellation()
          resolveResult({})
          resolveRead({ done: true, value: undefined })
        },
        [Symbol.dispose]: () => {
          events.push("dispose")
          disposal()
        },
      }
      const output = {
        [Symbol.asyncIterator]: () => ({
          next: () => {
            readStarted()
            return next
          },
          return: async () => {
            await cancelled
            events.push("close")
            return { done: true as const, value: undefined }
          },
        }),
      }
      const definition = toolDefinition("probe").body((body) => body.output())
      const call: Effect.Effect<unknown, unknown, ToolClient> =
        mode === "typed"
          ? client(definition)({})
          : mode === "reflected"
            ? new ToolType({
                lookupName: "probe",
                definition: compileDefinition(definition).wire,
                implementedBy: { uuid: { highBits: 0n, lowBits: 0n } },
              }).client
                .command([])
                .invokeJson({})
            : Effect.scoped(
                Effect.gen(function* () {
                  const low = yield* createToolClientRuntime("probe").start(
                    [],
                    emptyInput,
                    undefined,
                    true,
                    false,
                  )
                  return yield* startedToolInvocation(
                    low.stdout,
                    low.stderr,
                    low.result,
                    low.cancel,
                  ).collect
                }),
              )
      await Effect.runPromise(
        Effect.gen(function* () {
          const fiber = yield* Effect.forkChild(
            call.pipe(Effect.provideService(ToolClient, hostFor(future, [output]))),
          )
          yield* Effect.promise(() => Promise.all([getting, reading]))
          yield* Fiber.interrupt(fiber)
          yield* Effect.promise(() => disposed)
        }),
      )
      expect(events[0]).toBe("cancel")
      expect(events).toContain("close")
      expect(events).toContain("dispose")
      expect(events.filter((event) => event === "cancel")).toHaveLength(1)
    },
  )

  it.each(["typed", "reflected"])(
    "cancels %s whole-call interruption during admission",
    async (mode) => {
      const cancel = vi.fn()
      const detach = vi.fn()
      await Effect.runPromise(
        Effect.gen(function* () {
          const accepted = yield* Deferred.make<void>()
          const release = yield* Deferred.make<void>()
          const interrupting = yield* Deferred.make<void>()
          const transport: ToolTransportShape = {
            start: () =>
              Effect.gen(function* () {
                yield* Deferred.succeed(accepted, undefined)
                yield* Deferred.await(release)
                return {
                  result: Effect.never,
                  cancel: Effect.sync(cancel),
                  detach: Effect.sync(detach),
                }
              }),
          }
          const definition = toolDefinition("startup").body((body) => body)
          const call: Effect.Effect<unknown, unknown, ToolClient> =
            mode === "typed"
              ? client(definition, { transport })({})
              : new ToolType({
                  lookupName: "startup",
                  definition: compileDefinition(definition).wire,
                  implementedBy: { uuid: { highBits: 0n, lowBits: 0n } },
                }).client
                  .command([])
                  .invokeJson({})
                  .pipe(Effect.provideService(ToolTransport, transport))
          const fiber = yield* Effect.forkChild(
            call.pipe(Effect.provideService(ToolClient, {} as never)),
          )
          yield* Deferred.await(accepted)
          const interrupter = yield* Effect.forkChild(
            Deferred.succeed(interrupting, undefined).pipe(Effect.andThen(Fiber.interrupt(fiber))),
          )
          yield* Deferred.await(interrupting)
          yield* Effect.yieldNow
          yield* Deferred.succeed(release, undefined)
          yield* Fiber.join(interrupter)
        }).pipe(Effect.provideService(Scheduler.MaxOpsBeforeYield, 3)),
      )
      expect(cancel).toHaveBeenCalledOnce()
      expect(detach).toHaveBeenCalledOnce()
    },
  )
})
