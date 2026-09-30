import { describe, expect, it, vi } from "vitest"
import { Cause, Effect, Schema, Stream } from "effect"
import { command, reflectedCommand, reflectedToolkit } from "../src/Ai.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { compileDefinition, toolDefinition } from "../src/internal/tool/model.js"
import { ToolTransport } from "../src/Tool.js"
import { ToolType } from "../src/ToolReflection.js"
import { compile } from "../src/WitCodec.js"
import type { JsonValue } from "../src/SchemaRef.js"
import { t, v } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"

const registration = (definition: ReturnType<typeof toolDefinition>) => ({
  lookupName: definition.name,
  definition: compileDefinition(definition).wire,
  implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
})

const host = ToolClient.of({
  getAllTools: vi.fn(),
  getTool: vi.fn(),
  createStdin: vi.fn() as never,
  createStdinFromStream: vi.fn() as never,
  createOutput: vi.fn() as never,
  rpc: vi.fn() as never,
  createRpc: vi.fn() as never,
})

describe("reflected Effect AI tool adapter", () => {
  it("invokes a selected reflected command through startJson with stdin and bounded stdout", async () => {
    const definition = toolDefinition("reflected-files").body((body) =>
      body
        .option("label", Schema.String)
        .input({ required: true, mime: ["text/plain"] })
        .output({ mime: ["text/plain"] })
        .stderr({ mime: ["text/plain"] })
        .returns(Schema.BigInt),
    )
    const reflected = new ToolType(registration(definition))
    const encodedResult = Effect.runSync(compile(Schema.BigInt))
    const encodedInput = Effect.runSync(
      compile(Schema.Struct({ label: Schema.optionalKey(Schema.String) })),
    )
    let stderrDrained = false
    let completeStderr!: () => void
    const stderrDone = new Promise<void>((resolve) => (completeStderr = resolve))
    const start = vi.fn<ToolTransport["start"]>((tool, path, input, stdin, stdout, stderr) =>
      Effect.gen(function* () {
        expect(tool).toBe("reflected-files")
        expect(path).toEqual([])
        expect(stdout).toBe(true)
        expect(stderr).toBe(true)
        expect(yield* encodedInput.decode(input.value)).toEqual({})
        const bytes: number[] = []
        if (stdin)
          yield* Effect.promise(async () => {
            for await (const item of stdin) if (item.tag === "ok") bytes.push(...item.val)
          })
        expect(new TextDecoder().decode(Uint8Array.from(bytes))).toBe("hello")
        const resultValue = yield* encodedResult.encode(9007199254740993n)
        return {
          stdout: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("printed output") } as const
          })(),
          stderr: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("diagnostic") } as const
            stderrDrained = true
            completeStderr()
          })(),
          result: Effect.promise(async () => {
            await stderrDone
            return {
              result: {
                graph: encodedResult.schemaGraph,
                value: resultValue,
              },
            }
          }),
          cancel: Effect.void,
        }
      }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command([], { maxStdoutBytes: 7, maxStderrBytes: 5 })]).pipe(
        Effect.provideService(ToolClient, host),
      ),
    )

    const handled = await Effect.runPromise(
      toolkit
        .handle("reflected-files", {
          _stdin: { data: "hello", encoding: "utf8" },
        })
        .pipe(
          Effect.flatMap(Stream.runCollect),
          Effect.provideService(ToolTransport, { start }),
          Effect.provideService(ToolClient, host),
        ),
    )

    expect(Array.from(handled).at(-1)?.encodedResult).toEqual({
      status: "success",
      result: "9007199254740993",
      stdout: { data: "printed", encoding: "utf8", truncated: true, totalBytes: 14 },
      stderr: { data: "diagn", encoding: "utf8", truncated: true, totalBytes: 10 },
    })
    expect(stderrDrained).toBe(true)
    expect(start).toHaveBeenCalledOnce()
  })

  it("drains stderr-only output before observing a gated reflected result", async () => {
    const definition = toolDefinition("reflected-diagnostics").body((body) =>
      body.stderr({ mime: ["text/plain"] }),
    )
    const reflected = new ToolType(registration(definition))
    let drained = false
    let releaseResult!: () => void
    const outputDone = new Promise<void>((resolve) => (releaseResult = resolve))
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, _input, _stdin, stdout, stderr) => {
      expect(stdout).toBe(false)
      expect(stderr).toBe(true)
      return Effect.succeed({
        stderr: (async function* () {
          yield { tag: "ok", val: new TextEncoder().encode("warning") } as const
          drained = true
          releaseResult()
        })(),
        result: Effect.promise(async () => {
          await outputDone
          return {}
        }),
        cancel: Effect.void,
      })
    })
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command([], { maxStderrBytes: 7 })]).pipe(
        Effect.provideService(ToolClient, host),
      ),
    )
    const handled = await Effect.runPromise(
      toolkit
        .handle("reflected-diagnostics", {})
        .pipe(
          Effect.flatMap(Stream.runCollect),
          Effect.provideService(ToolTransport, { start }),
          Effect.provideService(ToolClient, host),
        ),
    )
    expect(drained).toBe(true)
    expect(Array.from(handled).at(-1)?.encodedResult).toEqual({
      status: "success",
      stderr: { data: "warning", encoding: "utf8", truncated: false, totalBytes: 7 },
    })
  })

  it("accepts direct reflected commands and rejects duplicate model names", () => {
    const definition = toolDefinition("direct-reflection").body((body) => body)
    const reflected = new ToolType(registration(definition)).client.command([])

    expect(() =>
      reflectedToolkit([
        reflectedCommand(reflected, { name: "same" }),
        reflectedCommand(reflected, { name: "same" }),
      ]),
    ).toThrow(/duplicate AI tool name/)
  })

  it("rejects the unsafe __proto__ model-facing name before toolkit assembly", () => {
    const definition = toolDefinition("safe-reflection").body((body) => body)
    const reflected = new ToolType(registration(definition)).client.command([])

    expect(() =>
      reflectedToolkit([{ command: reflected, options: { name: "__proto__" } }]),
    ).toThrow(/not supported/)
  })

  it("injects reflected defaults and maps declared failures to model-visible envelopes", async () => {
    const failure = Schema.Struct({ reason: Schema.String })
    const definition = toolDefinition("reflected-failure").body((body) =>
      body
        .option("mode", Schema.String, { default: "safe" })
        .flag("verbose")
        .output({ mime: ["text/plain"] })
        .stderr({ mime: ["text/plain"] })
        .error("rejected", failure),
    )
    const reflected = new ToolType(registration(definition))
    const encodedInput = Effect.runSync(
      compile(Schema.Struct({ mode: Schema.String, verbose: Schema.Boolean })),
    )
    const encodedFailure = Effect.runSync(compile(failure))
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, input) =>
      Effect.gen(function* () {
        expect(yield* encodedInput.decode(input.value)).toEqual({ mode: "safe", verbose: false })
        return {
          stdout: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("partial-output") } as const
          })(),
          stderr: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("partial-diagnostic") } as const
          })(),
          result: Effect.fail({
            tag: "remote-tool-error",
            val: {
              tag: "custom-error",
              val: {
                name: "rejected",
                payload: {
                  graph: encodedFailure.schemaGraph,
                  value: yield* encodedFailure.encode({ reason: "denied" }),
                },
              },
            },
          }),
          cancel: Effect.void,
        }
      }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command([], { maxStdoutBytes: 7, maxStderrBytes: 8 })]).pipe(
        Effect.provideService(ToolClient, host),
      ),
    )

    const handled = await Effect.runPromise(
      toolkit
        .handle("reflected-failure", {})
        .pipe(
          Effect.flatMap(Stream.runCollect),
          Effect.provideService(ToolTransport, { start }),
          Effect.provideService(ToolClient, host),
        ),
    )

    expect(Array.from(handled).at(-1)?.encodedResult).toEqual({
      status: "error",
      error: { name: "rejected", value: { reason: "denied" } },
      stdout: { data: "partial", encoding: "utf8", truncated: true, totalBytes: 14 },
      stderr: { data: "partial-", encoding: "utf8", truncated: true, totalBytes: 18 },
    })
  })

  it("preserves absent, null, false, and zero reflected results", async () => {
    const cases = [
      { name: "absent", expected: { status: "success" }, schema: undefined, value: undefined },
      {
        name: "null-result",
        expected: { status: "success", result: null },
        schema: Schema.NullOr(Schema.String),
        value: null,
      },
      {
        name: "false-result",
        expected: { status: "success", result: false },
        schema: Schema.Boolean,
        value: false,
      },
      {
        name: "zero-result",
        expected: { status: "success", result: 0 },
        schema: Schema.Number,
        value: 0,
      },
    ] as const

    for (const testCase of cases) {
      const definition = toolDefinition(testCase.name).body((body) =>
        testCase.schema ? body.returns(testCase.schema) : body,
      )
      const reflected = new ToolType(registration(definition))
      const encoded = testCase.schema ? Effect.runSync(compile(testCase.schema)) : undefined
      const start: ToolTransport["start"] = () =>
        Effect.succeed({
          result: Effect.succeed(
            encoded
              ? {
                  result: {
                    graph: encoded.schemaGraph,
                    value: Effect.runSync(encoded.encode(testCase.value as never)),
                  },
                }
              : {},
          ),
          cancel: Effect.void,
        })
      const toolkit = await Effect.runPromise(
        reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
      )
      const handled = await Effect.runPromise(
        toolkit
          .handle(testCase.name, {})
          .pipe(
            Effect.flatMap(Stream.runCollect),
            Effect.provideService(ToolTransport, { start }),
            Effect.provideService(ToolClient, host),
          ),
      )

      expect(Array.from(handled).at(-1)?.encodedResult).toEqual(testCase.expected)
    }
  })

  it("keeps reflected infrastructure failures in the Effect failure channel", async () => {
    const definition = toolDefinition("unavailable-reflection").body((body) => body)
    const reflected = new ToolType(registration(definition))
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )
    const start: ToolTransport["start"] = () => Effect.fail(new Error("transport unavailable"))

    const exit = await Effect.runPromiseExit(
      toolkit
        .handle("unavailable-reflection", {})
        .pipe(
          Effect.flatMap(Stream.runDrain),
          Effect.provideService(ToolTransport, { start }),
          Effect.provideService(ToolClient, host),
        ),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      expect(Cause.hasFails(exit.cause)).toBe(true)
      expect(Cause.hasDies(exit.cause)).toBe(false)
    }
  })

  it("validates payload-less declared errors before exposing them to the model", async () => {
    const definition = toolDefinition("payload-less-error").body((body) =>
      body.error("rejected", Schema.String),
    )
    const registered = registration(definition)
    const node = registered.definition.commands.nodes[0]
    const reflected = new ToolType({
      ...registered,
      definition: {
        ...registered.definition,
        commands: {
          nodes: [
            {
              ...node,
              body: {
                ...node.body!,
                errors: node.body!.errors.map((error) => ({ ...error, payload: undefined })),
              },
            },
          ],
        },
      },
    })
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )
    const emptyTupleValue = schemaValueToWit(v.tuple([]))
    const startWithGraph =
      (root: ReturnType<typeof t.tuple>): ToolTransport["start"] =>
      () =>
        Effect.succeed({
          result: Effect.fail({
            tag: "remote-tool-error",
            val: {
              tag: "custom-error",
              val: {
                name: "rejected",
                payload: {
                  graph: schemaGraphToWit({ defs: new Map(), root }),
                  value: emptyTupleValue,
                },
              },
            },
          }),
          cancel: Effect.void,
        })

    const valid = await Effect.runPromise(
      toolkit
        .handle("payload-less-error", {})
        .pipe(
          Effect.flatMap(Stream.runCollect),
          Effect.provideService(ToolTransport, { start: startWithGraph(t.tuple([])) }),
          Effect.provideService(ToolClient, host),
        ),
    )
    expect(Array.from(valid).at(-1)?.encodedResult).toEqual({
      status: "error",
      error: { name: "rejected" },
    })

    const malformed = await Effect.runPromiseExit(
      toolkit
        .handle("payload-less-error", {})
        .pipe(
          Effect.flatMap(Stream.runDrain),
          Effect.provideService(ToolTransport, { start: startWithGraph(t.string()) }),
          Effect.provideService(ToolClient, host),
        ),
    )
    expect(malformed._tag).toBe("Failure")
    if (malformed._tag === "Failure") expect(Cause.hasFails(malformed.cause)).toBe(true)
  })

  it("rejects malformed reflected stdin before invoking transport", async () => {
    const definition = toolDefinition("validated-stdin").body((body) =>
      body.input({ required: false }),
    )
    const reflected = new ToolType(registration(definition))
    const start = vi.fn<ToolTransport["start"]>(() =>
      Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )

    for (const stdin of [
      { data: "YQ==", encoding: "bogus" },
      { encoding: "utf8" },
      { data: 1, encoding: "utf8" },
      { data: "Zh==", encoding: "base64" },
      null,
    ]) {
      const exit = await Effect.runPromiseExit(
        toolkit
          .handle("validated-stdin", { _stdin: stdin })
          .pipe(
            Effect.flatMap(Stream.runDrain),
            Effect.provideService(ToolTransport, { start }),
            Effect.provideService(ToolClient, host),
          ),
      )
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        expect(Cause.hasFails(exit.cause)).toBe(true)
        expect(Cause.hasDies(exit.cause)).toBe(false)
      }
    }
    expect(start).not.toHaveBeenCalled()
  })

  it("injects empty reflected defaults for omitted repeatable lists and maps", async () => {
    const definition = toolDefinition("repeatable-defaults").body((body) =>
      body
        .option("include", Schema.String, { repeatable: "repeated" })
        .option("labels", Schema.ReadonlyMap(Schema.String, Schema.String), {
          repeatable: "repeated",
        }),
    )
    const reflected = new ToolType(registration(definition))
    const commandDefinition = reflected.client.command([])
    expect(commandDefinition.arguments.map((argument) => argument.defaultJson)).toEqual([[], []])
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, input) =>
      Effect.sync(() => {
        expect(commandDefinition.inputSchema?.unpackJson(input.value)).toEqual({
          include: [],
          labels: [],
        })
        return { result: Effect.succeed({}), cancel: Effect.void }
      }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )

    await Effect.runPromise(
      toolkit
        .handle("repeatable-defaults", {})
        .pipe(
          Effect.flatMap(Stream.runDrain),
          Effect.provideService(ToolTransport, { start }),
          Effect.provideService(ToolClient, host),
        ),
    )
    expect(start).toHaveBeenCalledOnce()
  })

  it("deeply freezes reflected canonical defaults", () => {
    const definition = toolDefinition("immutable-defaults").body((body) =>
      body
        .option("config", Schema.Struct({ nested: Schema.Array(Schema.String) }), {
          default: { nested: ["original"] },
        })
        .tail("rest", Schema.String),
    )
    const reflected = new ToolType(registration(definition)).client.command([])
    const defaults = new Map(
      reflected.arguments.map((argument) => [argument.name, argument.defaultJson]),
    )
    const config = defaults.get("config")
    const tail = defaults.get("rest")

    expect(config).toEqual({ nested: ["original"] })
    expect(tail).toEqual([])
    expect(Object.isFrozen(config)).toBe(true)
    expect(Object.isFrozen((config as { nested: string[] }).nested)).toBe(true)
    expect(Object.isFrozen(tail)).toBe(true)
    expect(() => (config as { nested: string[] }).nested.push("changed")).toThrow()
    expect(() => (tail as string[]).push("changed")).toThrow()
  })

  it("keeps native non-JSON defaults reflectable but rejects them for AI exposure", () => {
    const definition = toolDefinition("native-default").body((body) =>
      body.option("number", Schema.Number, { default: Number.NaN }),
    )

    const reflected = new ToolType(registration(definition))
    const argument = reflected.client.command([]).arguments[0]
    expect(argument.default).toBeDefined()
    expect(argument.defaultJson).toBeUndefined()
    expect(() => reflectedToolkit(reflected, [command()])).toThrow(/has no default/)
  })

  it("injects a default for an argument named constructor without replacing an explicit value", async () => {
    const definition = toolDefinition("constructor-default").body((body) =>
      body.option("constructor", Schema.String, { default: "safe" }),
    )
    const reflected = new ToolType(registration(definition))
    const commandDefinition = reflected.client.command([])
    const observed: JsonValue[] = []
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, input) =>
      Effect.sync(() => {
        observed.push(commandDefinition.inputSchema!.unpackJson(input.value))
        return { result: Effect.succeed({}), cancel: Effect.void }
      }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )

    for (const parameters of [{}, { constructor: "explicit" }])
      await Effect.runPromise(
        toolkit
          .handle("constructor-default", parameters)
          .pipe(
            Effect.flatMap(Stream.runDrain),
            Effect.provideService(ToolTransport, { start }),
            Effect.provideService(ToolClient, host),
          ),
      )

    expect(observed).toEqual([{ constructor: "safe" }, { constructor: "explicit" }])
  })

  it("validates reflected stdout limits on plain selection objects", () => {
    const definition = toolDefinition("reflected-limit").body((body) => body.output())
    const reflected = new ToolType(registration(definition))
    for (const maxStdoutBytes of [Infinity, Number.NaN, -1, 1.5]) {
      expect(() =>
        reflectedToolkit(reflected, [{ path: [], options: { maxStdoutBytes } }]),
      ).toThrow(/non-negative safe integer/)
      expect(() =>
        reflectedToolkit([{ command: reflected.client.command([]), options: { maxStdoutBytes } }]),
      ).toThrow(/non-negative safe integer/)
    }
    const diagnostics = new ToolType(
      registration(toolDefinition("reflected-stderr-limit").body((body) => body.stderr())),
    )
    expect(() => reflectedToolkit(diagnostics, [command()])).toThrow(/requires maxStderrBytes/)
  })
})
