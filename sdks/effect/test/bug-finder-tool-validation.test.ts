import { beforeEach, describe, expect, it, vi } from "vitest"
import { Effect, Fiber, Schema, Stream } from "effect"
import * as ToolSchema from "../src/Schema.js"
import { client, type ToolTransport } from "../src/Tool.js"
import { compile } from "../src/WitCodec.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { c, registeredTools, resetTools, toolDefinition } from "../src/internal/tool/model.js"
import {
  resetMiddlewares,
  toolMiddlewareGuest,
  universal,
} from "../src/internal/tool/middleware.js"

describe("tool metadata WIT validation", () => {
  beforeEach(() => {
    resetTools()
    resetMiddlewares()
  })

  it("declares the default result formatter and selects the first custom formatter", () => {
    toolDefinition("implicit")
      .body((body) => body.returns(Schema.String))
      .implement({ implicit: () => Effect.succeed("ok") })
    toolDefinition("custom")
      .body((body) => body.returns(Schema.String, { formatters: ["table", "json"] }))
      .implement({ custom: () => Effect.succeed("ok") })
    const results = registeredTools().map((tool) => tool.wire.commands.nodes[0].body!.result!)
    expect(results[0].formatters.map((formatter) => formatter.name)).toEqual(["default"])
    expect(results[0].defaultFormatter).toBe("default")
    expect(results[1].defaultFormatter).toBe("table")
    expect(() =>
      toolDefinition("empty")
        .body((body) => body.returns(Schema.String, { formatters: [] }))
        .implement({ empty: () => Effect.succeed("ok") }),
    ).toThrow(/default formatter/)
  })

  it.each([-1, 0.5, 2 ** 32, Number.NaN, Number.POSITIVE_INFINITY])(
    "rejects count-flag maximum %s outside the canonical u32 range",
    (max) => {
      const definition = toolDefinition("invalid-count").body((body) =>
        body.countFlag("verbose", { max }),
      )

      expect(() => definition.implement({ invalidCount: () => undefined as never })).toThrow()
    },
  )

  it.each([0, 0xffffffff])("accepts count-flag boundary maximum %s", (max) => {
    const definition = toolDefinition("valid-count").body((body) =>
      body.countFlag("verbose", { max }),
    )
    expect(() => definition.implement({ validCount: () => undefined as never })).not.toThrow()
  })

  it("encodes canonical WIT argument classes regardless of builder insertion and does not wrap tail items twice", () => {
    const definition = toolDefinition("ordered").body((body) =>
      body
        .flag("force")
        .option("include", Schema.String, { repeatable: { delimited: "," } })
        .tail("files", Schema.String)
        .positional("pattern", Schema.String),
    )
    definition.implement({ ordered: () => Effect.void })

    const wire = registeredTools()[0].wire
    const body = wire.commands.nodes[0].body!
    expect(body.positionals.fixed.map((x) => x.name)).toEqual(["pattern"])
    expect(body.positionals.tail?.name).toBe("files")
    expect(body.options.map((x) => x.long)).toEqual(["include"])
    expect(body.flags.map((x) => x.long)).toEqual(["force"])
    const tailType = wire.schema.typeNodes[body.positionals.tail!.itemType].body
    expect(tailType.tag).toBe("string-type")
    expect(body.options[0].shape.tag).toBe("repeatable-list")
  })

  it("encodes optional scalars, delimited lists, repeatable maps, and defaults asymmetrically", () => {
    const definition = toolDefinition("option-shapes").body((body) =>
      body
        .positional("source", Schema.String, { default: "stdin" })
        .option("color", Schema.String, { optionalScalar: true, default: "auto" })
        .option("include", Schema.String, { repeatable: "delimited", delim: "," })
        .option("define", ToolSchema.Map(Schema.String, Schema.Number), {
          repeatable: "either",
          delim: ",",
          duplicateKeyPolicy: "last-wins",
          default: new Map([["a", 1]]),
        }),
    )
    definition.implement({ optionShapes: () => Effect.void })

    const body = registeredTools()[0].wire.commands.nodes[0].body!
    expect(body.positionals.fixed[0].default_).toBeDefined()
    expect(body.options.map((option) => option.shape.tag)).toEqual([
      "optional-scalar",
      "repeatable-list",
      "repeatable-map",
    ])
    expect(body.options[0].default_).toBeDefined()
    expect(body.options[1].shape).toMatchObject({
      tag: "repeatable-list",
      val: { repetition: { tag: "delimited", val: "," } },
    })
    expect(body.options[2].shape).toMatchObject({
      tag: "repeatable-map",
      val: {
        repetition: { tag: "either", val: "," },
        duplicateKeyPolicy: "last-wins",
      },
    })
    expect(body.options[2].default_).toBeDefined()
  })

  it("publishes globals on their command and validates inherited collisions and constraint references", () => {
    const valid = toolDefinition("global-tool")
      .body((body) => body.globalOption("profile", Schema.String))
      .command("run", (command) =>
        command.body((body) =>
          body.positional("target", Schema.String).constraint(c.requiresAll(c.present("profile"))),
        ),
      )
    valid.implement({ globalTool: () => Effect.void, run: () => Effect.void })
    expect(registeredTools()[0].wire.commands.nodes[0].globals.options[0].long).toBe("profile")

    resetTools()
    const invalid = toolDefinition("collision")
      .body((body) => body.globalFlag("verbose", { short: "v" }))
      .command("run", (command) => command.body((body) => body.flag("verbose")))
    expect(() =>
      invalid.implement({ collision: () => Effect.void, run: () => Effect.void }),
    ).toThrow(/duplicate argument/)
  })

  it("runs a custom client with Effect streams and cancels the transport when interrupted", async () => {
    const seen: number[] = []
    const cancel = vi.fn()
    const transport: ToolTransport = {
      start: (_tool, path, _input, stdin) =>
        Effect.gen(function* () {
          expect(path).toEqual([])
          if (stdin)
            yield* Effect.promise(async () => {
              for await (const item of stdin) if (item.tag === "ok") seen.push(...item.val)
            })
          return {
            result: Effect.never,
            cancel: Effect.sync(cancel),
          }
        }),
    }
    const definition = toolDefinition("streaming").body((body) => body.input())
    const runtime = client(definition, { transport })
    await Effect.runPromise(
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(
          runtime({} as never, { stdin: Stream.make(Uint8Array.of(3, 5, 8)) }),
        )
        yield* Effect.sleep("10 millis")
        yield* Fiber.interrupt(fiber)
      }),
    )
    expect(seen).toEqual([3, 5, 8])
    expect(cancel).toHaveBeenCalledOnce()
  })

  it("pumps live stdin concurrently with result observation and finishes the writer", async () => {
    const chunks: number[][] = []
    let finish!: () => void
    const finished = new Promise<void>((resolve) => {
      finish = resolve
    })
    const cancel = vi.fn()
    const writer = {
      write: async (bytes: Uint8Array) => {
        chunks.push([...bytes])
      },
      finish: vi.fn(async () => {
        finish()
      }),
      fail: vi.fn(async () => {}),
    }
    const source = {}
    const invoke = vi.fn((_path, _value, input) => {
      expect(input).toBe(source)
      return {
        get: async () => {
          await finished
          return {}
        },
        cancel,
      }
    })
    const host = ToolClient.of({
      getAllTools: () => [],
      getTool: () => undefined,
      createStdinFromStream: () => {
        throw new Error("must use concurrent writer pump")
      },
      createStdin: () => [writer, source, { wait: () => new Promise(() => {}) }],
      createStdout: () => {
        throw new Error("no stdout declared")
      },
      rpc: () => ({ asyncInvokeAndAwait: invoke }) as never,
    })
    const definition = toolDefinition("pumped").body((body) => body.input({ required: true }))
    await Effect.runPromise(
      client(definition)(
        {},
        {
          stdin: Stream.make(Uint8Array.of(3, 8), new Uint8Array(), Uint8Array.of(13)),
        },
      ).pipe(Effect.provideService(ToolClient, host)),
    )
    expect(chunks).toEqual([[3, 8], [13]])
    expect(writer.finish).toHaveBeenCalledOnce()
    expect(writer.fail).not.toHaveBeenCalled()
    expect(cancel).toHaveBeenCalledOnce()
  })

  it("drains declared stdout even when no callback is supplied", async () => {
    let released = false
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok", val: Uint8Array.of(1) } as const
            released = true
          })(),
          result: Effect.promise(
            () =>
              new Promise((resolve) => {
                const poll = () => (released ? resolve({ result: undefined }) : setTimeout(poll, 0))
                poll()
              }),
          ),
          cancel: Effect.void,
        }),
    }
    const definition = toolDefinition("backpressured").body((body) => body.output())
    await Effect.runPromise(client(definition, { transport })({}))
    expect(released).toBe(true)
  })

  it("decodes host-shaped remote tool custom errors", async () => {
    const failure = Schema.Struct({ message: Schema.String })
    const definition = toolDefinition("fallible").body((body) => body.error("bad-request", failure))
    const encoded = Effect.runSync(compile(failure))
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          result: Effect.fail({
            tag: "remote-tool-error",
            val: {
              tag: "custom-error",
              val: {
                graph: encoded.schemaGraph,
                value: Effect.runSync(encoded.encode({ message: "no" })),
              },
            },
          }),
          cancel: Effect.void,
        }),
    }
    const exit = await Effect.runPromiseExit(client(definition, { transport })({}))
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") expect(String(exit.cause)).toContain("bad-request")
  })

  it("keeps universal middleware stdin Effect-native and rejects escaped underlying handles", async () => {
    let escaped: Parameters<Parameters<typeof universal>[0]["handler"]>[1] | undefined
    universal({
      name: "audit",
      handler: (invocation, underlying) =>
        Effect.gen(function* () {
          escaped = underlying
          expect(invocation.stdin).toBeDefined()
          expect(yield* Stream.runCollect(invocation.stdin!)).toHaveLength(2)
          return { result: undefined }
        }),
    })
    const metadata = registeredTools()[0]?.wire ?? {
      version: "0.1.0",
      commands: { nodes: [] },
      schema: { root: 0, typeNodes: [] },
    }
    await toolMiddlewareGuest.invokeToolMiddleware(
      "audit",
      "target",
      metadata,
      [],
      { graph: metadata.schema, value: { node: 0 } as never },
      (async function* () {
        yield 1
        yield 2
      })(),
      { tag: "anonymous" },
      { invoke: async () => ({}) } as never,
    )
    const exit = await Effect.runPromiseExit(
      escaped!.invoke([], { graph: metadata.schema, value: { node: 0 } as never }),
    )
    expect(exit._tag).toBe("Failure")
  })

  it("closes middleware-owned stdin and discarded underlying stdout", async () => {
    const stdinReturn = vi.fn(
      async (): Promise<IteratorResult<number>> => ({
        done: true,
        value: undefined,
      }),
    )
    const stdoutReturn = vi.fn(
      async (): Promise<IteratorResult<number>> => ({
        done: true,
        value: undefined,
      }),
    )
    const iterable = (return_: typeof stdinReturn): AsyncIterableIterator<number> => ({
      next: async () => new Promise<IteratorResult<number>>(() => {}),
      return: return_,
      [Symbol.asyncIterator]() {
        return this
      },
    })
    const unit = Effect.runSync(compile(Schema.Void))
    const typedUnit = {
      graph: unit.schemaGraph,
      value: Effect.runSync(unit.encode(undefined)),
    }
    universal({
      name: "cleanup",
      handler: (_invocation, underlying) => Effect.as(underlying.invoke([], typedUnit), {}),
    })
    await toolMiddlewareGuest.invokeToolMiddleware(
      "cleanup",
      "target",
      {
        version: "0.1.0",
        commands: { nodes: [] },
        schema: { root: 0, typeNodes: [], defs: [] },
      },
      [],
      typedUnit,
      iterable(stdinReturn),
      { tag: "anonymous" },
      { invoke: async () => ({ stdout: iterable(stdoutReturn) }) } as never,
    )
    expect(stdinReturn).toHaveBeenCalledOnce()
    expect(stdoutReturn).toHaveBeenCalledOnce()
  })
})
