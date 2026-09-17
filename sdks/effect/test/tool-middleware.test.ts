import { beforeEach, describe, expect, it, vi } from "vitest"
import { Context, Effect, Exit, Fiber, Layer, Schema, Scope, Stream } from "effect"
import { compile } from "../src/WitCodec.js"
import { err, toolDefinition } from "../src/Tool.js"
import * as WitTypes from "../src/WitTypes.js"
import {
  resetMiddlewares,
  toolMiddlewareGuest,
  typed,
  universal,
  type TypedImplementation,
} from "../src/Middleware.js"
import { byteItems, startMiddleware, type ByteStreamItem } from "./tool-middleware-test-support.js"

const wire = <A>(schema: Schema.Schema<A>, value: A) => {
  const codec = Effect.runSync(compile(schema))
  return {
    graph: codec.schemaGraph,
    value: Effect.runSync(codec.encode(value) as Effect.Effect<any, any>),
  }
}

const metadata = {
  version: "0.1.0",
  commands: { nodes: [] },
  schema: { root: 0, typeNodes: [], defs: [] },
} as never

const tracked = (...values: number[]) => {
  const close = vi.fn(async () => ({ done: true as const, value: undefined }))
  const next = vi.fn(async () =>
    values.length > 0
      ? { done: false as const, value: values.shift()! }
      : { done: true as const, value: undefined },
  )
  const iterator = {
    next,
    return: close,
    [Symbol.asyncIterator]() {
      return this
    },
  }
  return { iterable: { [Symbol.asyncIterator]: vi.fn(() => iterator) }, next, close }
}

const gate = () => {
  let open!: () => void
  const wait = new Promise<void>((resolve) => {
    open = resolve
  })
  return { wait, open }
}

const underlyingResult = (
  result?: ReturnType<typeof wire>,
  stdout?: AsyncIterable<ByteStreamItem>,
) =>
  [
    {
      get: async () => result,
      cancel: vi.fn(),
      [Symbol.dispose]: vi.fn(),
    },
    stdout,
  ] as const

describe("typed tool middleware", () => {
  beforeEach(resetMiddlewares)

  it("exports and decodes author-declared installation parameters", async () => {
    const definition = toolDefinition("configured").body((body) => body.returns(Schema.String))
    typed({
      name: "configured-policy",
      parameters: Schema.Struct({ prefix: Schema.String, retries: Schema.Number }),
      presented: definition,
      handler: {
        configured: (_input, { parameters }) =>
          Effect.succeed(`${parameters.prefix}:${parameters.retries}`),
      },
    })
    const middleware = toolMiddlewareGuest.getToolMiddleware("configured-policy")
    const parameters = wire(Schema.Struct({ prefix: Schema.String, retries: Schema.Number }), {
      prefix: "attempts",
      retries: 3,
    })
    expect(middleware.parameterSchema).toEqual(parameters.graph)

    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "configured-policy",
      "configured",
      metadata,
      parameters,
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    await expect(
      Effect.runPromise(
        Effect.runSync(compile(Schema.String)).decode((await result.completion).result!.value),
      ),
    ).resolves.toBe("attempts:3")
  })

  it("rejects incompatible installation parameter payloads at the guest boundary", async () => {
    universal({
      name: "configured-audit",
      parameters: Schema.Struct({ enabled: Schema.Boolean }),
      handler: () => Effect.succeed({}),
    })
    await expect(
      startMiddleware(
        toolMiddlewareGuest.invokeToolMiddleware,
        "configured-audit",
        "target",
        metadata,
        wire(Schema.Struct({ enabled: Schema.String }), { enabled: "yes" }),
        [],
        wire(Schema.Void, undefined),
        undefined,
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ).completion,
    ).rejects.toMatchObject({ tag: "invalid-input" })
  })

  it("rejects nested capability-bearing installation parameter schemas", () => {
    expect(() =>
      universal({
        name: "stream-parameters",
        parameters: Schema.Struct({
          nested: Schema.Struct({ value: WitTypes.AgentStream(Schema.String) }),
        }),
        handler: () => Effect.succeed({}),
      }),
    ).toThrow(/stream/)
    expect(() =>
      universal({
        name: "secret-parameters",
        parameters: Schema.Struct({
          nested: Schema.Struct({ value: WitTypes.Secret(Schema.String) }),
        }),
        handler: () => Effect.succeed({}),
      }),
    ).toThrow(/secret/)
  })

  it("decodes, transforms, invokes the typed underlying command, and encodes output", async () => {
    const presented = toolDefinition("presented").body((body) =>
      body.positional("message", Schema.String).returns(Schema.String),
    )
    const expected = toolDefinition("expected").body((body) =>
      body.positional("length", Schema.Number).returns(Schema.Number),
    )
    typed({
      name: "adapter",
      parameters: Schema.Struct({}),
      presented,
      expected,
      handler: {
        presented: ({ message }, { underlying }) =>
          underlying({ length: message.length }).pipe(Effect.map((n) => `length=${n}`)),
      },
    })
    const invoke = vi.fn(async (_path, input) => {
      const codec = Effect.runSync(compile(Schema.Struct({ length: Schema.Number })))
      const decoded = await Effect.runPromise(codec.decode(input.value))
      return underlyingResult(wire(Schema.Number, decoded.length * 2))
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "adapter",
      "ignored",
      {
        version: "0.1.0",
        commands: { nodes: [] },
        schema: { root: 0, typeNodes: [], defs: [] },
      },
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({ message: Schema.String }), { message: "abc" }),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke } as never,
    )
    const completion = await result.completion
    expect(invoke).toHaveBeenCalledOnce()
    const output = Effect.runSync(compile(Schema.String))
    await expect(Effect.runPromise(output.decode(completion.result!.value))).resolves.toBe(
      "length=6",
    )
  })

  it("returns lazy service-dependent stdout and keeps forwarded output alive", async () => {
    class Prefix extends Context.Service<Prefix, { readonly value: number }>()("test/Prefix") {}
    const definition = toolDefinition("streaming").body((body) =>
      body.output({ required: true }).returns(Schema.String),
    )
    let closed = 0
    const source: AsyncIterable<number> = {
      [Symbol.asyncIterator]: () => {
        let next = 2
        return {
          next: async () => ({ done: false, value: next++ }),
          return: async () => {
            closed++
            return { done: true, value: undefined }
          },
        }
      },
    }
    typed({
      name: "streaming-policy",
      parameters: Schema.Struct({}),
      presented: definition,
      layer: Layer.succeed(Prefix, { value: 10 }),
      handler: {
        streaming: (_input, { underlying, stdout }) =>
          underlying(
            {},
            {
              stdout: (stream) =>
                stdout!(
                  stream.pipe(
                    Stream.mapEffect((chunk) =>
                      Effect.map(Prefix, ({ value }) => Uint8Array.of(chunk[0]! + value)),
                    ),
                  ),
                ),
            },
          ),
      },
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "streaming-policy",
      "streaming",
      toolMiddlewareGuest.getToolMiddleware("streaming-policy") as never,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => underlyingResult(wire(Schema.String, "ok"), byteItems(source)),
      } as never,
    )
    expect(closed).toBe(0)
    const stdout = result.stdout![Symbol.asyncIterator]()
    await expect(stdout.next()).resolves.toEqual({ done: false, value: 12 })
    await stdout.return?.()
    await result.completion.catch(() => undefined)
    expect(closed).toBe(1)
  })

  it("keeps every source until a fresh final output is closed", async () => {
    const stdin = tracked(1)
    const a = tracked(2)
    universal({
      name: "fresh",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          yield* underlying.invoke([], wire(Schema.Void, undefined))
          return {
            stdout: (async function* () {
              yield 9
            })(),
          }
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "fresh",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      byteItems(stdin.iterable),
      undefined,
      { tag: "anonymous" },
      { invoke: async () => underlyingResult(undefined, byteItems(a.iterable)) } as never,
    )
    expect(stdin.close).not.toHaveBeenCalled()
    expect(a.close).not.toHaveBeenCalled()
    await expect(result.stdout.next()).resolves.toEqual({ done: false, value: 9 })
    await result.stdout![Symbol.asyncIterator]().return?.()
    await result.completion.catch(() => undefined)
    expect(stdin.close).toHaveBeenCalledOnce()
    expect(a.close).toHaveBeenCalledOnce()
  })

  it("keeps both underlying outputs for transformed selection and concatenation", async () => {
    const a = tracked(1)
    const b = tracked(2)
    let call = 0
    universal({
      name: "combine",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          const scope = yield* Scope.make()
          const first = yield* underlying
            .start([], wire(Schema.Void, undefined))
            .pipe(Effect.provideService(Scope.Scope, scope))
          const second = yield* underlying
            .start([], wire(Schema.Void, undefined))
            .pipe(Effect.provideService(Scope.Scope, scope))
          const stream = Stream.concat(
            second.stdout!.pipe(Stream.map((byte) => Uint8Array.of(byte[0]! + 10))),
            first.stdout!.pipe(Stream.map((byte) => Uint8Array.of(byte[0]! + 20))),
          )
          return {
            stdout: (async function* () {
              try {
                for await (const chunk of Stream.toAsyncIterable(stream)) yield* chunk
              } finally {
                await Effect.runPromise(Scope.close(scope, Exit.void))
              }
            })(),
          }
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "combine",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () =>
          underlyingResult(undefined, byteItems(call++ === 0 ? a.iterable : b.iterable)),
      } as never,
    )
    const values: number[] = []
    for await (const byte of result.stdout!) values.push(byte)
    await result.completion
    expect(values).toEqual([12, 21])
    expect(a.close).toHaveBeenCalledOnce()
    expect(b.close).toHaveBeenCalledOnce()
  })

  it("keeps forwarded stdin alive when underlying stdout consumes it lazily", async () => {
    const raw = tracked(19, 43)
    universal({
      name: "lazy-echo",
      parameters: Schema.Struct({}),
      handler: (invocation, underlying) =>
        Effect.gen(function* () {
          const scope = yield* Scope.make()
          const started = yield* underlying
            .start([], invocation.input, invocation.stdin)
            .pipe(Effect.provideService(Scope.Scope, scope))
          return {
            stdout: (async function* () {
              try {
                for await (const chunk of Stream.toAsyncIterable(started.stdout!)) yield* chunk
              } finally {
                await Effect.runPromise(Scope.close(scope, Exit.void))
              }
            })(),
          }
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "lazy-echo",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      byteItems(raw.iterable),
      undefined,
      { tag: "anonymous" },
      {
        invoke: async (_path: unknown, _input: unknown, stdin: AsyncIterable<ByteStreamItem>) =>
          underlyingResult(undefined, stdin),
      } as never,
    )
    expect(raw.next).not.toHaveBeenCalled()
    const output: number[] = []
    for await (const byte of result.stdout!) output.push(byte)
    await result.completion
    expect(output).toEqual([19, 43])
    expect(raw.close).toHaveBeenCalledOnce()
  })

  it("cancels before first pull without pulling and releases raw and layer resources once", async () => {
    const raw = tracked(1)
    const release = vi.fn()
    universal({
      name: "cancel-lazy",
      parameters: Schema.Struct({}),
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: () => Effect.succeed({ stdout: Stream.toAsyncIterable(Stream.make(7)) }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "cancel-lazy",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      byteItems(raw.iterable),
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    await Promise.all([
      result.stdout![Symbol.asyncIterator]().return!(),
      result.stdout![Symbol.asyncIterator]().return!(),
    ])
    await expect(result.completion).rejects.toMatchObject({ tag: "closed" })
    expect(raw.next).not.toHaveBeenCalled()
    expect(raw.close).toHaveBeenCalledOnce()
    expect(release).toHaveBeenCalledOnce()
  })

  it("aborts a blocked Effect pull and preserves registration context and layer lifetime", async () => {
    class Value extends Context.Service<Value, { readonly n: number }>()("middleware/Value") {}
    const release = vi.fn()
    const secondProduced = gate()
    const definition = toolDefinition("blocked").body((body) => body.output({ required: true }))
    typed({
      name: "blocked-layer",
      parameters: Schema.Struct({}),
      presented: definition,
      layer: Layer.effect(
        Value,
        Effect.acquireRelease(Effect.succeed({ n: 4 }), () => Effect.sync(release)),
      ),
      handler: {
        blocked: (_input, { stdout }) =>
          stdout!(
            Stream.fromEffect(Value).pipe(
              Stream.map(({ n }) => Uint8Array.of(n)),
              Stream.concat(
                Stream.fromEffect(
                  Effect.sync(() => {
                    secondProduced.open()
                    return Uint8Array.of(5)
                  }),
                ),
              ),
            ),
          ),
      },
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "blocked-layer",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).resolves.toEqual({ done: false, value: 4 })
    await secondProduced.wait
    await iterator.return?.()
    await expect(result.completion).rejects.toMatchObject({ tag: "closed" })
    expect(release).toHaveBeenCalledOnce()
  })

  it("waits for an asynchronous interruption finalizer before releasing the layer", async () => {
    const pullEntered = gate()
    const finalizerEntered = gate()
    const allowFinalizer = gate()
    const finalizerDone = vi.fn()
    const releaseLayer = vi.fn()
    const definition = toolDefinition("gated").body((body) => body.output({ required: true }))
    typed({
      name: "async-interruption-finalizer",
      parameters: Schema.Struct({}),
      presented: definition,
      layer: Layer.effectDiscard(
        Effect.acquireRelease(Effect.void, () => Effect.sync(releaseLayer)),
      ),
      handler: {
        gated: (_input, { stdout }) =>
          stdout!(
            Stream.succeed(Uint8Array.of(1)).pipe(
              Stream.concat(
                Stream.unwrap(
                  Effect.acquireRelease(Effect.sync(pullEntered.open), () =>
                    Effect.promise(async () => {
                      finalizerEntered.open()
                      await allowFinalizer.wait
                      finalizerDone()
                    }),
                  ).pipe(Effect.map(() => Stream.succeed(Uint8Array.of(2)))),
                ),
              ),
            ),
          ),
      },
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "async-interruption-finalizer",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).resolves.toEqual({ done: false, value: 1 })
    await pullEntered.wait
    const returned = iterator.return!()
    await finalizerEntered.wait
    expect(finalizerDone).not.toHaveBeenCalled()
    expect(releaseLayer).not.toHaveBeenCalled()

    allowFinalizer.open()
    await expect(returned).resolves.toEqual({ done: true, value: undefined })
    await expect(result.completion).rejects.toMatchObject({ tag: "closed" })
    expect(finalizerDone).toHaveBeenCalledOnce()
    expect(releaseLayer).toHaveBeenCalledOnce()
  })

  it("disposes a late observer and stdout without cancelling after its waiter is interrupted", async () => {
    const invokeStarted = gate()
    const resolveInvoke = gate()
    const stdoutClosed = gate()
    const stdout = tracked(1)
    stdout.close.mockImplementationOnce(async () => {
      stdoutClosed.open()
      return { done: true as const, value: undefined }
    })
    const cancel = vi.fn()
    const dispose = vi.fn()
    universal({
      name: "late-underlying-output",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          const waiter = yield* Effect.forkChild(
            Effect.scoped(underlying.start([], wire(Schema.Void, undefined))),
          )
          yield* Effect.promise(() => invokeStarted.wait)
          yield* Fiber.interrupt(waiter)
          return {}
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "late-underlying-output",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => {
          invokeStarted.open()
          await resolveInvoke.wait
          return [
            { get: async () => undefined, cancel, [Symbol.dispose]: dispose },
            byteItems(stdout.iterable),
          ] as const
        },
      } as never,
    )
    await invokeStarted.wait
    resolveInvoke.open()
    await stdoutClosed.wait
    await result.completion
    expect(dispose).toHaveBeenCalledOnce()
    expect(stdout.close).toHaveBeenCalledOnce()
    expect(cancel).not.toHaveBeenCalled()
  })

  it("defers observer disposal after an interrupted get while stdout completes", async () => {
    const getStarted = gate()
    let rejectGet!: (error: unknown) => void
    let pending = false
    const dispose = vi.fn(() => {
      if (pending) throw new Error("resource is borrowed by get")
    })
    universal({
      name: "interrupted-get",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.scoped(
          Effect.gen(function* () {
            const started = yield* underlying.start([], wire(Schema.Void, undefined))
            const waiter = yield* Effect.forkChild(started.get)
            yield* Effect.promise(() => getStarted.wait)
            yield* Fiber.interrupt(waiter)
            return { stdout: tracked(9).iterable }
          }),
        ),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "interrupted-get",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => [
          {
            get: () => {
              pending = true
              getStarted.open()
              return new Promise<undefined>((_resolve, reject) => {
                rejectGet = reject
              }).finally(() => {
                pending = false
              })
            },
            cancel: vi.fn(),
            [Symbol.dispose]: dispose,
          },
          undefined,
        ],
      } as never,
    )

    await expect(result.stdout.next()).resolves.toEqual({ done: false, value: 9 })
    await expect(result.stdout.next()).resolves.toEqual({ done: true, value: undefined })
    await expect(result.completion).resolves.toEqual({ result: undefined })
    expect(dispose).not.toHaveBeenCalled()
    rejectGet({ tag: "resource-exhausted", val: "busy" })
    await vi.waitFor(() => expect(dispose).toHaveBeenCalledOnce())
  })

  it.each([false, true])("disposes an already-settled observer (failure: %s)", async (fails) => {
    const dispose = vi.fn()
    const cancel = vi.fn()
    const failure = { tag: "denied", val: "no access" }
    universal({
      name: "settled-observer",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.scoped(
          Effect.gen(function* () {
            const started = yield* underlying.start([], wire(Schema.Void, undefined))
            const outcome = yield* Effect.exit(started.get)
            expect(Exit.isFailure(outcome)).toBe(fails)
            expect(dispose).not.toHaveBeenCalled()
            return {}
          }),
        ),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "settled-observer",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => [
          {
            get: async () => {
              if (fails) throw failure
              return undefined
            },
            cancel,
            [Symbol.dispose]: dispose,
          },
          undefined,
        ],
      } as never,
    )
    await result.completion
    expect(dispose).toHaveBeenCalledOnce()
    expect(cancel).not.toHaveBeenCalled()
  })

  it("does not call a released raw observer through escaped effects", async () => {
    const get = vi.fn(async () => undefined)
    const cancel = vi.fn()
    const dispose = vi.fn()
    let escapedGet!: Effect.Effect<unknown, unknown>
    let escapedCancel!: Effect.Effect<void>
    universal({
      name: "escaped-observer",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.scoped(
          Effect.gen(function* () {
            const started = yield* underlying.start([], wire(Schema.Void, undefined))
            escapedGet = started.get
            escapedCancel = started.cancel
            return {}
          }),
        ),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "escaped-observer",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: async () => [{ get, cancel, [Symbol.dispose]: dispose }, undefined] } as never,
    )
    await result.completion
    expect(dispose).toHaveBeenCalledOnce()
    expect(Exit.isFailure(await Effect.runPromiseExit(escapedGet))).toBe(true)
    await Effect.runPromise(escapedCancel)
    expect(get).not.toHaveBeenCalled()
    expect(cancel).not.toHaveBeenCalled()
  })

  it("preserves a stdout read error when iterator return also fails", async () => {
    const readFailure = new Error("stdout read failed")
    const returnFailure = new Error("stdout return failed")
    const close = vi.fn(async () => {
      throw returnFailure
    })
    universal({
      name: "stdout-primary-error",
      parameters: Schema.Struct({}),
      handler: () =>
        Effect.succeed({
          stdout: {
            [Symbol.asyncIterator]: () => ({
              next: async () => {
                throw readFailure
              },
              return: close,
            }),
          },
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "stdout-primary-error",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )

    await expect(result.stdout.next()).rejects.toMatchObject({ tag: "failed" })
    await expect(result.completion).rejects.toBe(readFailure)
    expect(close).toHaveBeenCalledOnce()
  })

  it("preserves producer failures rather than turning cleanup into clean EOF", async () => {
    const definition = toolDefinition("failed-output").body((body) =>
      body.output({ required: true }),
    )
    const release = vi.fn()
    typed({
      name: "failed-output",
      parameters: Schema.Struct({}),
      presented: definition,
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: {
        failedOutput: (_input, { stdout }) => stdout!(Stream.die(new Error("producer failed"))),
      },
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "failed-output",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).rejects.toMatchObject({ tag: "failed", val: "producer failed" })
    await expect(result.completion).rejects.toThrow("producer failed")
    expect(release).toHaveBeenCalledOnce()
  })

  it("unblocks an owned source before joining a cancelled universal output", async () => {
    let releaseNext!: () => void
    const pullStarted = gate()
    const close = vi.fn(async () => {
      releaseNext()
      return { done: true as const, value: undefined }
    })
    const source: AsyncIterable<number> = {
      [Symbol.asyncIterator]: () => ({
        next: () =>
          new Promise<IteratorResult<number>>((resolve) => {
            releaseNext = () => resolve({ done: true, value: undefined })
            pullStarted.open()
          }),
        return: close,
      }),
    }
    universal({
      name: "blocked-forward",
      parameters: Schema.Struct({}),
      handler: (_invocation, underlying) =>
        Effect.scoped(
          Effect.gen(function* () {
            const started = yield* underlying.start([], wire(Schema.Void, undefined))
            const iterator = Stream.toAsyncIterable(started.stdout!)[Symbol.asyncIterator]()
            const pending = iterator.next()
            let first = true
            return {
              stdout: {
                [Symbol.asyncIterator]() {
                  return {
                    async next() {
                      if (first) {
                        first = false
                        return { done: false as const, value: 1 }
                      }
                      await pending
                      return { done: true as const, value: undefined }
                    },
                    async return() {
                      await pending
                      await iterator.return?.()
                      return { done: true as const, value: undefined }
                    },
                  }
                },
              },
            }
          }),
        ),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "blocked-forward",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: async () => underlyingResult(undefined, byteItems(source)) } as never,
    )
    await pullStarted.wait
    await result.stdout.return?.()
    await expect(result.completion).rejects.toMatchObject({ tag: "closed" })
    expect(close).toHaveBeenCalledOnce()
  })

  it("closes every source and the layer when handlers and iterator returns throw synchronously", async () => {
    const other = tracked(1)
    const release = vi.fn()
    const throwing: AsyncIterable<number> = {
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: false as const, value: 1 }),
        return: () => {
          throw new Error("return failed")
        },
      }),
    }
    universal({
      name: "throwing-cleanup",
      parameters: Schema.Struct({}),
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          yield* underlying.invoke([], wire(Schema.Void, undefined))
          return { stdout: throwing }
        }),
    })
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "throwing-cleanup",
      "target",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Void, undefined),
      undefined,
      undefined,
      { tag: "anonymous" },
      { invoke: async () => underlyingResult(undefined, byteItems(other.iterable)) } as never,
    )
    await result.stdout.return?.()
    await expect(result.completion).rejects.toMatchObject({ tag: "closed" })
    expect(other.close).toHaveBeenCalledOnce()
    expect(release).toHaveBeenCalledOnce()

    const stdin = tracked(1)
    universal({
      name: "sync-throw",
      parameters: Schema.Struct({}),
      handler: (() => {
        throw new Error("handler failed")
      }) as never,
    })
    await expect(
      startMiddleware(
        toolMiddlewareGuest.invokeToolMiddleware,
        "sync-throw",
        "target",
        metadata,
        wire(Schema.Struct({}), {}),
        [],
        wire(Schema.Void, undefined),
        byteItems(stdin.iterable),
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ).completion,
    ).rejects.toThrow("handler failed")
    expect(stdin.close).toHaveBeenCalledOnce()
  })

  it("enforces required and undeclared stream slots and closes rejected streams", async () => {
    const absent = toolDefinition("absent").body((body) => body.returns(Schema.Void))
    const required = toolDefinition("required").body((body) =>
      body.input({ required: true }).output({ required: true }),
    )
    typed({
      name: "absent-streams",
      parameters: Schema.Struct({}),
      presented: absent,
      handler: { absent: () => Effect.void },
    })
    typed({
      name: "required-streams",
      parameters: Schema.Struct({}),
      presented: required,
      handler: { required: () => Effect.void },
    })
    let closed = 0
    const unexpected = {
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: false as const, value: 1 }),
        return: async () => {
          closed++
          return { done: true as const, value: undefined }
        },
      }),
    }
    const invoke = (name: string, stdin?: AsyncIterable<number>) =>
      startMiddleware(
        toolMiddlewareGuest.invokeToolMiddleware,
        name,
        name,
        {} as never,
        wire(Schema.Struct({}), {}),
        [],
        wire(Schema.Struct({}), {}),
        stdin ? byteItems(stdin) : undefined,
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      )
    await expect(invoke("absent-streams", unexpected).completion).rejects.toMatchObject({
      tag: "invalid-input",
    })
    expect(closed).toBe(1)
    await expect(invoke("required-streams").completion).rejects.toMatchObject({
      tag: "invalid-input",
    })
    const requiredInput = {
      ...unexpected,
      [Symbol.asyncIterator]: unexpected[Symbol.asyncIterator],
    }
    await expect(invoke("required-streams", requiredInput).completion).rejects.toMatchObject({
      tag: "invalid-result",
    })
    expect(closed).toBe(2)
  })

  it("rejects invalid input and encodes declared failures", async () => {
    const failure = Schema.Struct({ reason: Schema.String })
    const presented = toolDefinition("guarded").body((body) =>
      body.positional("value", Schema.String).error("rejected", failure),
    )
    typed({
      name: "guard",
      parameters: Schema.Struct({}),
      presented,
      handler: { guarded: () => Effect.succeed(err("rejected", { reason: "no" })) },
    })
    const metadata = toolMiddlewareGuest.getToolMiddleware("guard")
    await expect(
      startMiddleware(
        toolMiddlewareGuest.invokeToolMiddleware,
        "guard",
        "guarded",
        metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
        wire(Schema.Struct({}), {}),
        [],
        wire(Schema.Struct({ value: Schema.Number }), { value: 1 }),
        undefined,
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ).completion,
    ).rejects.toMatchObject({ tag: "invalid-input" })
    await expect(
      startMiddleware(
        toolMiddlewareGuest.invokeToolMiddleware,
        "guard",
        "guarded",
        metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
        wire(Schema.Struct({}), {}),
        [],
        wire(Schema.Struct({ value: Schema.String }), { value: "blocked" }),
        undefined,
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ).completion,
    ).rejects.toMatchObject({ tag: "custom-error" })
  })

  it("decodes declared underlying failures from observer tool-error", async () => {
    const failure = Schema.Struct({ reason: Schema.String })
    const definition = toolDefinition("fallible").body((body) =>
      body.error("rejected", failure).returns(Schema.String),
    )
    typed({
      name: "decode-tool-error",
      parameters: Schema.Struct({}),
      presented: definition,
      handler: {
        fallible: (_input, { underlying }) =>
          underlying({}).pipe(
            Effect.catchIf(
              (): boolean => true,
              (error) =>
                error._tag === "ToolFailure"
                  ? Effect.succeed(error.value.reason)
                  : Effect.fail(error),
            ),
          ),
      },
    })
    const custom = {
      tag: "custom-error",
      val: { name: "rejected", payload: wire(failure, { reason: "declined" }) },
    } as const
    const metadata = toolMiddlewareGuest.getToolMiddleware("decode-tool-error")
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "decode-tool-error",
      "fallible",
      metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () =>
          underlyingResult(undefined).map((value, index) =>
            index === 0
              ? { ...value, get: async () => Promise.reject({ tag: "tool-error", val: custom }) }
              : value,
          ),
      } as never,
    )
    const completion = await result.completion
    await expect(
      Effect.runPromise(Effect.runSync(compile(Schema.String)).decode(completion.result!.value)),
    ).resolves.toBe("declined")
  })

  it("drains unobserved underlying stdout and exposes nested kebab commands as camel case", async () => {
    const definition = toolDefinition("nested-tool").command("child-command", (command) =>
      command.body((body) => body.returns(Schema.String)),
    )
    let drained = false
    typed({
      name: "nested-adapter",
      parameters: Schema.Struct({}),
      presented: definition,
      handler: {
        nestedTool: {
          childCommand: (_input, { underlying }) => underlying.childCommand({}),
        },
      } satisfies TypedImplementation<typeof definition>,
    })
    const metadata = toolMiddlewareGuest.getToolMiddleware("nested-adapter")
    const result = startMiddleware(
      toolMiddlewareGuest.invokeToolMiddleware,
      "nested-adapter",
      "nested-tool",
      metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
      wire(Schema.Struct({}), {}),
      ["child-command"],
      wire(Schema.Struct({}), {}),
      undefined,
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () =>
          underlyingResult(
            wire(Schema.String, "ok"),
            byteItems(
              (async function* () {
                yield 1
                drained = true
              })(),
            ),
          ),
      } as never,
    )
    const completion = await result.completion
    expect(drained).toBe(true)
    await expect(
      Effect.runPromise(Effect.runSync(compile(Schema.String)).decode(completion.result!.value)),
    ).resolves.toBe("ok")
  })

  it("rejects incomplete implementations during registration", () => {
    const definition = toolDefinition("incomplete").command("required", (command) =>
      command.body((body) => body.output()),
    )
    expect(() =>
      typed({
        name: "incomplete-adapter",
        parameters: Schema.Struct({}),
        presented: definition,
        handler: { incomplete: {} },
      } as never),
    ).toThrow("missing handler 'required'")
    expect(() => toolMiddlewareGuest.getToolMiddleware("incomplete-adapter")).toThrow()
  })

  it("composes recursive command schemas into a graph with resolved refs", () => {
    interface Node {
      readonly value: string
      readonly children: ReadonlyArray<Node>
    }
    const Node: Schema.Schema<Node> = Schema.suspend(() =>
      Schema.Struct({ value: Schema.String, children: Schema.Array(Node) }),
    )
    const definition = toolDefinition("recursive").body((body) =>
      body.positional("root", Node).returns(Node),
    )
    typed({
      name: "recursive-middleware",
      parameters: Schema.Struct({}),
      presented: definition,
      handler: { recursive: (input) => Effect.succeed(input.root) },
    })
    const middleware = toolMiddlewareGuest.getToolMiddleware("recursive-middleware")
    expect(middleware.scope.tag).toBe("monomorphic")
    if (middleware.scope.tag === "monomorphic") {
      expect(middleware.scope.val.presented.schema.defs.length).toBeGreaterThan(0)
    }
  })

  it("starts overlapping calls and pumps their stdout", async () => {
    const definition = toolDefinition("overlap").body((body) =>
      body.output({ required: true }).returns(Schema.String),
    )
    typed({
      name: "overlap",
      parameters: Schema.Struct({}),
      presented: definition,
      handler: {
        overlap: (_input, { underlying, stdout }) =>
          Effect.scoped(
            Effect.gen(function* () {
              const first = yield* underlying.start({})
              const second = yield* underlying.start({})
              yield* stdout!(Stream.concat(first.stdout!, second.stdout!))
              return "ok"
            }),
          ),
      },
    })
    const pulled = [gate(), gate()]
    let calls = 0
    const wrapped = {
      invoke: vi.fn(async () => {
        const index = calls++
        return [
          {
            get: async () => {
              await pulled[index]!.wait
              return wire(Schema.String, String(index))
            },
            cancel: vi.fn(),
          },
          (async function* () {
            yield { tag: "ok" as const, val: Uint8Array.of(index + 1) }
            pulled[index]!.open()
          })(),
        ] as const
      }),
    }
    const bytes: number[] = []
    const writer = {
      write: vi.fn(async (chunk: Uint8Array) => void bytes.push(...chunk)),
      finish: vi.fn(async () => undefined),
      fail: vi.fn(async () => undefined),
    }
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "overlap",
      "overlap",
      metadata,
      wire(Schema.Struct({}), {}),
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      writer,
      { tag: "anonymous" },
      wrapped as never,
    )
    expect(bytes).toEqual([1, 2])
    expect(writer.finish).toHaveBeenCalledOnce()
    await expect(
      Effect.runPromise(Effect.runSync(compile(Schema.String)).decode(result.result!.value)),
    ).resolves.toBe("ok")
  })
})
