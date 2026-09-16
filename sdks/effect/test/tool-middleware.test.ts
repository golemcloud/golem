import { beforeEach, describe, expect, it, vi } from "vitest"
import { Context, Effect, Fiber, Layer, Schema, Stream } from "effect"
import { compile } from "../src/WitCodec.js"
import { err, toolDefinition } from "../src/Tool.js"
import {
  resetMiddlewares,
  toolMiddlewareGuest,
  typed,
  universal,
  type TypedImplementation,
} from "../src/Middleware.js"

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

describe("typed tool middleware", () => {
  beforeEach(resetMiddlewares)

  it("decodes, transforms, invokes the typed underlying command, and encodes output", async () => {
    const presented = toolDefinition("presented").body((body) =>
      body.positional("message", Schema.String).returns(Schema.String),
    )
    const expected = toolDefinition("expected").body((body) =>
      body.positional("length", Schema.Number).returns(Schema.Number),
    )
    typed({
      name: "adapter",
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
      return { result: wire(Schema.Number, decoded.length * 2) }
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "adapter",
      "ignored",
      {
        version: "0.1.0",
        commands: { nodes: [] },
        schema: { root: 0, typeNodes: [], defs: [] },
      },
      [],
      wire(Schema.Struct({ message: Schema.String }), { message: "abc" }),
      undefined,
      { tag: "anonymous" },
      { invoke } as never,
    )
    expect(invoke).toHaveBeenCalledOnce()
    const output = Effect.runSync(compile(Schema.String))
    await expect(Effect.runPromise(output.decode(result.result!.value))).resolves.toBe("length=6")
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
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "streaming-policy",
      "streaming",
      toolMiddlewareGuest.getToolMiddleware("streaming-policy") as never,
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      { tag: "anonymous" },
      { invoke: async () => ({ result: wire(Schema.String, "ok"), stdout: source }) } as never,
    )
    expect(closed).toBe(0)
    const stdout = result.stdout![Symbol.asyncIterator]()
    await expect(stdout.next()).resolves.toEqual({ done: false, value: 12 })
    await stdout.return?.()
    expect(closed).toBe(1)
  })

  it("keeps every source until a fresh final output is closed", async () => {
    const stdin = tracked(1)
    const a = tracked(2)
    universal({
      name: "fresh",
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
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "fresh",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      stdin.iterable,
      { tag: "anonymous" },
      { invoke: async () => ({ stdout: a.iterable }) } as never,
    )
    expect(stdin.close).not.toHaveBeenCalled()
    expect(a.close).not.toHaveBeenCalled()
    await result.stdout![Symbol.asyncIterator]().return?.()
    expect(stdin.close).toHaveBeenCalledOnce()
    expect(a.close).toHaveBeenCalledOnce()
  })

  it("keeps both underlying outputs for transformed selection and concatenation", async () => {
    const a = tracked(1)
    const b = tracked(2)
    let call = 0
    universal({
      name: "combine",
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          const first = yield* underlying.invoke([], wire(Schema.Void, undefined))
          const second = yield* underlying.invoke([], wire(Schema.Void, undefined))
          return {
            stdout: (async function* () {
              for await (const byte of second.stdout!) yield byte + 10
              for await (const byte of first.stdout!) yield byte + 20
            })(),
          }
        }),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "combine",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      undefined,
      { tag: "anonymous" },
      { invoke: async () => ({ stdout: call++ === 0 ? a.iterable : b.iterable }) } as never,
    )
    const values: number[] = []
    for await (const byte of result.stdout!) values.push(byte)
    expect(values).toEqual([12, 21])
    expect(a.close).toHaveBeenCalledOnce()
    expect(b.close).toHaveBeenCalledOnce()
  })

  it("keeps forwarded stdin alive when underlying stdout consumes it lazily", async () => {
    const raw = tracked(19, 43)
    universal({
      name: "lazy-echo",
      handler: (invocation, underlying) =>
        underlying.invoke([], invocation.input, invocation.stdin),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "lazy-echo",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      raw.iterable,
      { tag: "anonymous" },
      {
        invoke: async (_path: unknown, _input: unknown, stdin: AsyncIterable<number>) => ({
          stdout: stdin,
        }),
      } as never,
    )
    expect(raw.next).not.toHaveBeenCalled()
    const output: number[] = []
    for await (const byte of result.stdout!) output.push(byte)
    expect(output).toEqual([19, 43])
    expect(raw.close).toHaveBeenCalledOnce()
  })

  it("cancels before first pull without pulling and releases raw and layer resources once", async () => {
    const raw = tracked(1)
    const release = vi.fn()
    universal({
      name: "cancel-lazy",
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: () => Effect.succeed({ stdout: Stream.toAsyncIterable(Stream.make(7)) }),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "cancel-lazy",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      raw.iterable,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    await Promise.all([
      result.stdout![Symbol.asyncIterator]().return!(),
      result.stdout![Symbol.asyncIterator]().return!(),
    ])
    expect(raw.next).not.toHaveBeenCalled()
    expect(raw.close).toHaveBeenCalledOnce()
    expect(release).toHaveBeenCalledOnce()
  })

  it("aborts a blocked Effect pull and preserves registration context and layer lifetime", async () => {
    class Value extends Context.Service<Value, { readonly n: number }>()("middleware/Value") {}
    const release = vi.fn()
    const definition = toolDefinition("blocked").body((body) => body.output({ required: true }))
    typed({
      name: "blocked-layer",
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
              Stream.concat(Stream.never),
            ),
          ),
      },
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "blocked-layer",
      "target",
      metadata,
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).resolves.toEqual({ done: false, value: 4 })
    const blocked = iterator.next()
    await iterator.return?.()
    await expect(blocked).resolves.toEqual({ done: true, value: undefined })
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
      presented: definition,
      layer: Layer.effectDiscard(
        Effect.acquireRelease(Effect.void, () => Effect.sync(releaseLayer)),
      ),
      handler: {
        gated: (_input, { stdout }) =>
          stdout!(
            Stream.succeed(Uint8Array.of(1)).pipe(
              Stream.concat(
                Stream.fromEffect(
                  Effect.sync(pullEntered.open).pipe(
                    Effect.andThen(Effect.never),
                    Effect.ensuring(
                      Effect.promise(async () => {
                        finalizerEntered.open()
                        await allowFinalizer.wait
                        finalizerDone()
                      }),
                    ),
                  ),
                ),
              ),
            ),
          ),
      },
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "async-interruption-finalizer",
      "target",
      metadata,
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).resolves.toEqual({ done: false, value: 1 })
    const pending = iterator.next()
    await pullEntered.wait
    const returned = iterator.return!()
    await finalizerEntered.wait
    expect(finalizerDone).not.toHaveBeenCalled()
    expect(releaseLayer).not.toHaveBeenCalled()

    allowFinalizer.open()
    await expect(returned).resolves.toEqual({ done: true, value: undefined })
    await expect(pending).resolves.toEqual({ done: true, value: undefined })
    expect(finalizerDone).toHaveBeenCalledOnce()
    expect(releaseLayer).toHaveBeenCalledOnce()
  })

  it("closes stdout exactly once when wrapped invoke resolves after its waiter is interrupted", async () => {
    const invokeStarted = gate()
    const resolveInvoke = gate()
    const stdoutClosed = gate()
    const stdout = tracked(1)
    stdout.close.mockImplementationOnce(async () => {
      stdoutClosed.open()
      return { done: true as const, value: undefined }
    })
    universal({
      name: "late-underlying-output",
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          const waiter = yield* Effect.forkChild(
            underlying.invoke([], wire(Schema.Void, undefined)),
          )
          yield* Effect.promise(() => invokeStarted.wait)
          yield* Fiber.interrupt(waiter)
          return {}
        }),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "late-underlying-output",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => {
          invokeStarted.open()
          await resolveInvoke.wait
          return { stdout: stdout.iterable }
        },
      } as never,
    )
    expect(result.stdout).toBeUndefined()
    expect(stdout.close).not.toHaveBeenCalled()

    resolveInvoke.open()
    await stdoutClosed.wait
    expect(stdout.close).toHaveBeenCalledOnce()
  })

  it("preserves a stdout read error when iterator return also fails", async () => {
    const readFailure = new Error("stdout read failed")
    const returnFailure = new Error("stdout return failed")
    const close = vi.fn(async () => {
      throw returnFailure
    })
    universal({
      name: "stdout-primary-error",
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
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "stdout-primary-error",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )

    await expect(result.stdout![Symbol.asyncIterator]().next()).rejects.toBe(readFailure)
    expect(close).toHaveBeenCalledOnce()
  })

  it("preserves producer failures rather than turning cleanup into clean EOF", async () => {
    const definition = toolDefinition("failed-output").body((body) =>
      body.output({ required: true }),
    )
    const release = vi.fn()
    typed({
      name: "failed-output",
      presented: definition,
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: {
        failedOutput: (_input, { stdout }) => stdout!(Stream.die(new Error("producer failed"))),
      },
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "failed-output",
      "target",
      metadata,
      [],
      wire(Schema.Struct({}), {}),
      undefined,
      { tag: "anonymous" },
      { invoke: vi.fn() } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    await expect(iterator.next()).rejects.toThrow("producer failed")
    await expect(iterator.next()).resolves.toEqual({ done: true, value: undefined })
    expect(release).toHaveBeenCalledOnce()
  })

  it("unblocks an owned source before joining a cancelled universal output", async () => {
    let releaseNext!: () => void
    const close = vi.fn(async () => {
      releaseNext()
      return { done: true as const, value: undefined }
    })
    const source: AsyncIterable<number> = {
      [Symbol.asyncIterator]: () => ({
        next: () =>
          new Promise<IteratorResult<number>>(
            (resolve) => (releaseNext = () => resolve({ done: true, value: undefined })),
          ),
        return: close,
      }),
    }
    universal({
      name: "blocked-forward",
      handler: (_invocation, underlying) => underlying.invoke([], wire(Schema.Void, undefined)),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "blocked-forward",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      undefined,
      { tag: "anonymous" },
      { invoke: async () => ({ stdout: source }) } as never,
    )
    const iterator = result.stdout![Symbol.asyncIterator]()
    const blocked = iterator.next()
    await iterator.return?.()
    await expect(blocked).resolves.toEqual({ done: true, value: undefined })
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
      layer: Layer.effectDiscard(Effect.acquireRelease(Effect.void, () => Effect.sync(release))),
      handler: (_invocation, underlying) =>
        Effect.gen(function* () {
          yield* underlying.invoke([], wire(Schema.Void, undefined))
          return { stdout: throwing }
        }),
    })
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "throwing-cleanup",
      "target",
      metadata,
      [],
      wire(Schema.Void, undefined),
      undefined,
      { tag: "anonymous" },
      { invoke: async () => ({ stdout: other.iterable }) } as never,
    )
    await expect(result.stdout![Symbol.asyncIterator]().return?.()).rejects.toThrow("return failed")
    expect(other.close).toHaveBeenCalledOnce()
    expect(release).toHaveBeenCalledOnce()

    const stdin = tracked(1)
    universal({
      name: "sync-throw",
      handler: (() => {
        throw new Error("handler failed")
      }) as never,
    })
    await expect(
      toolMiddlewareGuest.invokeToolMiddleware(
        "sync-throw",
        "target",
        metadata,
        [],
        wire(Schema.Void, undefined),
        stdin.iterable,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ),
    ).rejects.toThrow("handler failed")
    expect(stdin.close).toHaveBeenCalledOnce()
  })

  it("enforces required and undeclared stream slots and closes rejected streams", async () => {
    const absent = toolDefinition("absent").body((body) => body.returns(Schema.Void))
    const required = toolDefinition("required").body((body) =>
      body.input({ required: true }).output({ required: true }),
    )
    typed({ name: "absent-streams", presented: absent, handler: { absent: () => Effect.void } })
    typed({
      name: "required-streams",
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
      toolMiddlewareGuest.invokeToolMiddleware(
        name,
        name,
        {} as never,
        [],
        wire(Schema.Struct({}), {}),
        stdin,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      )
    await expect(invoke("absent-streams", unexpected)).rejects.toMatchObject({
      tag: "invalid-input",
    })
    expect(closed).toBe(1)
    await expect(invoke("required-streams")).rejects.toMatchObject({ tag: "invalid-input" })
    const requiredInput = {
      ...unexpected,
      [Symbol.asyncIterator]: unexpected[Symbol.asyncIterator],
    }
    await expect(invoke("required-streams", requiredInput)).rejects.toMatchObject({
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
      presented,
      handler: { guarded: () => Effect.succeed(err("rejected", { reason: "no" })) },
    })
    const metadata = toolMiddlewareGuest.getToolMiddleware("guard")
    await expect(
      toolMiddlewareGuest.invokeToolMiddleware(
        "guard",
        "guarded",
        metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
        [],
        wire(Schema.Struct({ value: Schema.Number }), { value: 1 }),
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ),
    ).rejects.toMatchObject({ tag: "invalid-input" })
    await expect(
      toolMiddlewareGuest.invokeToolMiddleware(
        "guard",
        "guarded",
        metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
        [],
        wire(Schema.Struct({ value: Schema.String }), { value: "blocked" }),
        undefined,
        { tag: "anonymous" },
        { invoke: vi.fn() } as never,
      ),
    ).rejects.toMatchObject({ tag: "custom-error" })
  })

  it.each(["custom-error", "remote-tool-error"] as const)(
    "decodes declared underlying failures from %s",
    async (carrier) => {
      const failure = Schema.Struct({ reason: Schema.String })
      const definition = toolDefinition("fallible").body((body) =>
        body.error("rejected", failure).returns(Schema.String),
      )
      typed({
        name: `decode-${carrier}`,
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
      const custom = { tag: "custom-error", val: wire(failure, { reason: "declined" }) } as const
      const metadata = toolMiddlewareGuest.getToolMiddleware(`decode-${carrier}`)
      const result = await toolMiddlewareGuest.invokeToolMiddleware(
        `decode-${carrier}`,
        "fallible",
        metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
        [],
        wire(Schema.Struct({}), {}),
        undefined,
        { tag: "anonymous" },
        {
          invoke: async () => {
            throw carrier === "custom-error" ? custom : { tag: "remote-tool-error", val: custom }
          },
        } as never,
      )
      await expect(
        Effect.runPromise(Effect.runSync(compile(Schema.String)).decode(result.result!.value)),
      ).resolves.toBe("declined")
    },
  )

  it("drains unobserved underlying stdout and exposes nested kebab commands as camel case", async () => {
    const definition = toolDefinition("nested-tool").command("child-command", (command) =>
      command.body((body) => body.returns(Schema.String)),
    )
    let drained = false
    typed({
      name: "nested-adapter",
      presented: definition,
      handler: {
        nestedTool: {
          childCommand: (_input, { underlying }) => underlying.childCommand({}),
        },
      } satisfies TypedImplementation<typeof definition>,
    })
    const metadata = toolMiddlewareGuest.getToolMiddleware("nested-adapter")
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "nested-adapter",
      "nested-tool",
      metadata.scope.tag === "monomorphic" ? metadata.scope.val.presented : ({} as never),
      ["child-command"],
      wire(Schema.Struct({}), {}),
      undefined,
      { tag: "anonymous" },
      {
        invoke: async () => ({
          result: wire(Schema.String, "ok"),
          stdout: (async function* () {
            yield 1
            drained = true
          })(),
        }),
      } as never,
    )
    expect(drained).toBe(true)
    await expect(
      Effect.runPromise(Effect.runSync(compile(Schema.String)).decode(result.result!.value)),
    ).resolves.toBe("ok")
  })

  it("rejects incomplete implementations during registration", () => {
    const definition = toolDefinition("incomplete").command("required", (command) =>
      command.body((body) => body.output()),
    )
    expect(() =>
      typed({
        name: "incomplete-adapter",
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
      presented: definition,
      handler: { recursive: (input) => Effect.succeed(input.root) },
    })
    const middleware = toolMiddlewareGuest.getToolMiddleware("recursive-middleware")
    expect(middleware.scope.tag).toBe("monomorphic")
    if (middleware.scope.tag === "monomorphic") {
      expect(middleware.scope.val.presented.schema.defs.length).toBeGreaterThan(0)
    }
  })
})
