import { describe, expect, it, vi } from "vitest"
import { Cause, Effect, Fiber, Schema, Stream } from "effect"
import { command, typedToolkit } from "../src/Ai.js"
import type { ToolTransport } from "../src/Tool.js"
import { compile } from "../src/WitCodec.js"
import { toolDefinition } from "../src/internal/tool/model.js"

describe("typed Effect AI tool adapter", () => {
  it("invokes a selected nested command through Tool.client with stdin and bounded stdout", async () => {
    const resultSchema = Schema.Struct({ lines: Schema.Number })
    const definition = toolDefinition("files")
      .body((body) => body.globalOption("profile", Schema.String))
      .command(
        "count-lines",
        (child) =>
          child.body((body) =>
            body
              .positional("file-name", Schema.String)
              .input({ required: true, mime: ["text/plain"] })
              .output({ mime: ["text/plain"] })
              .returns(resultSchema),
          ),
        { doc: "Count lines in a file" },
      )
    const encodedResult = Effect.runSync(compile(resultSchema))
    const encodedInput = Effect.runSync(
      compile(Schema.Struct({ profile: Schema.NullOr(Schema.String), "file-name": Schema.String })),
    )
    const start = vi.fn<ToolTransport["start"]>((tool, path, input, stdin, stdout) =>
      Effect.gen(function* () {
        expect(tool).toBe("registered-files")
        expect(path).toEqual(["count-lines"])
        expect(stdout).toBe(true)
        expect(yield* encodedInput.decode(input.value)).toEqual({
          profile: "test",
          "file-name": "notes.txt",
        })
        const inputBytes: number[] = []
        if (stdin) {
          yield* Effect.promise(async () => {
            for await (const item of stdin) {
              if (item.tag === "ok") inputBytes.push(...item.val)
            }
          })
        }
        expect(new TextDecoder().decode(Uint8Array.from(inputBytes))).toBe("a\nb\nc")
        return {
          stdout: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("counted") } as const
            yield { tag: "ok", val: new TextEncoder().encode(" lines") } as const
          })(),
          result: Effect.succeed({
            result: {
              graph: encodedResult.schemaGraph,
              value: Effect.runSync(encodedResult.encode({ lines: 3 })),
            },
          }),
          cancel: Effect.void,
        }
      }),
    )

    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command(["count-lines"], { maxStdoutBytes: 8 })], {
        lookupName: "registered-files",
        transport: { start },
      }),
    )
    const handled = await Effect.runPromise(
      toolkit
        .handle("files__count-lines", {
          profile: "test",
          "file-name": "notes.txt",
          _stdin: { data: "a\nb\nc", encoding: "utf8" },
        })
        .pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "success",
      result: { lines: 3 },
      stdout: {
        data: "counted ",
        encoding: "utf8",
        truncated: true,
        totalBytes: 13,
      },
    })
    expect(start).toHaveBeenCalledOnce()
  })

  it("rejects unknown commands and stdout commands without a finite capture limit", () => {
    const definition = toolDefinition("printer").body((body) => body.output())
    expect(() => typedToolkit(definition, [command(["missing"])])).toThrow(/non-callable command/)
    expect(() => typedToolkit(definition, [command()])).toThrow(/requires maxStdoutBytes/)
  })

  it("rejects the unsafe __proto__ model-facing name before toolkit assembly", () => {
    const definition = toolDefinition("safe-name").body((body) => body)
    expect(() => typedToolkit(definition, [{ path: [], options: { name: "__proto__" } }])).toThrow(
      /not supported/,
    )
  })

  it("rejects typed parameter names that collide after TypeScript projection", () => {
    const definition = toolDefinition("collision").body((body) =>
      body.positional("foo-bar", Schema.String).positional("fooBar", Schema.String),
    )

    expect(() => typedToolkit(definition, [command()])).toThrow(
      /parameters 'foo-bar' and 'fooBar' both project to 'fooBar'/,
    )
  })

  it("rejects selected command paths that collide after TypeScript projection", () => {
    const definition = toolDefinition("collision")
      .command("foo-bar", (child) => child.body((body) => body))
      .command("fooBar", (child) => child.body((body) => body))

    expect(() => typedToolkit(definition, [command(["foo-bar"])])).toThrow(
      /path segment 'foo-bar' is ambiguous.*foo-bar, fooBar/,
    )
  })

  it("rejects a command parameter that conflicts with synthetic stdin", () => {
    const definition = toolDefinition("stdin-collision").body((body) =>
      body.positional("_stdin", Schema.String).input({ required: true }),
    )

    expect(() => typedToolkit(definition, [command()])).toThrow(
      /parameter '_stdin' conflicts with stdin/,
    )
  })

  it("returns declared failures with fully drained stdout through the model envelope", async () => {
    const failureSchema = Schema.Struct({ reason: Schema.String })
    const definition = toolDefinition("fallible-printer").body((body) =>
      body.output({ mime: ["application/octet-stream"] }).error("rejected", failureSchema),
    )
    const encodedFailure = Effect.runSync(compile(failureSchema))
    let drained = false
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok", val: Uint8Array.from([0, 1]) } as const
            yield { tag: "ok", val: Uint8Array.from([2, 3]) } as const
            drained = true
          })(),
          result: Effect.fail({
            tag: "custom-error",
            val: {
              name: "rejected",
              payload: {
                graph: encodedFailure.schemaGraph,
                value: Effect.runSync(encodedFailure.encode({ reason: "denied" })),
              },
            },
          }),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 3 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("fallible-printer", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(drained).toBe(true)
    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "error",
      error: { name: "rejected", value: { reason: "denied" } },
      stdout: {
        data: "AAEC",
        encoding: "base64",
        truncated: true,
        totalBytes: 4,
      },
    })
  })

  it("uses canonical JSON for bigint parameters and encoded results", async () => {
    const definition = toolDefinition("counter").body((body) =>
      body.positional("start", Schema.BigInt).returns(Schema.BigInt),
    )
    const encoded = Effect.runSync(compile(Schema.BigInt))
    const input = Effect.runSync(compile(Schema.Struct({ start: Schema.BigInt })))
    const transport: ToolTransport = {
      start: (_tool, _path, value) =>
        Effect.gen(function* () {
          expect(yield* input.decode(value.value)).toEqual({ start: 9007199254740993n })
          return {
            result: Effect.succeed({
              result: {
                graph: encoded.schemaGraph,
                value: yield* encoded.encode(9007199254740994n),
              },
            }),
            cancel: Effect.void,
          }
        }),
    }
    const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))

    const handled = await Effect.runPromise(
      toolkit
        .handle("counter", { start: "9007199254740993" })
        .pipe(Effect.flatMap(Stream.runCollect)),
    )
    const terminal = Array.from(handled).at(-1)

    expect(terminal?.result).toEqual({ status: "success", result: "9007199254740994" })
    expect(terminal?.encodedResult).toEqual({
      status: "success",
      result: "9007199254740994",
    })
  })

  it("preserves absent, null, false, and zero structured results", async () => {
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
      const encoded = testCase.schema ? Effect.runSync(compile(testCase.schema)) : undefined
      const transport: ToolTransport = {
        start: () =>
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
          }),
      }
      const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))
      const handled = await Effect.runPromise(
        toolkit.handle(testCase.name, {}).pipe(Effect.flatMap(Stream.runCollect)),
      )

      expect(Array.from(handled).at(-1)?.encodedResult).toEqual(testCase.expected)
    }
  })

  it("keeps infrastructure failures in the Effect failure channel", async () => {
    const definition = toolDefinition("unavailable-tool").body((body) => body)
    const transport: ToolTransport = {
      start: () => Effect.fail(new Error("transport unavailable")),
    }
    const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))

    const exit = await Effect.runPromiseExit(
      toolkit.handle("unavailable-tool", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      expect(Cause.hasFails(exit.cause)).toBe(true)
      expect(Cause.hasDies(exit.cause)).toBe(false)
    }
  })

  it("rejects an unexpected structured result when none is declared", async () => {
    const definition = toolDefinition("unexpected-result").body((body) => body)
    const encoded = Effect.runSync(compile(Schema.String))
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          result: Effect.succeed({
            result: {
              graph: encoded.schemaGraph,
              value: Effect.runSync(encoded.encode("unexpected")),
            },
          }),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))

    const exit = await Effect.runPromiseExit(
      toolkit.handle("unexpected-result", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") expect(Cause.hasFails(exit.cause)).toBe(true)
  })

  it("keeps a declared failure when stdout also fails and omits incomplete capture", async () => {
    const failureSchema = Schema.Struct({ reason: Schema.String })
    const definition = toolDefinition("failing-output").body((body) =>
      body.output().error("rejected", failureSchema),
    )
    const encodedFailure = Effect.runSync(compile(failureSchema))
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "err", val: { tag: "failed", val: "stream broke" } } as const
          })(),
          result: Effect.fail({
            tag: "custom-error",
            val: {
              name: "rejected",
              payload: {
                graph: encodedFailure.schemaGraph,
                value: Effect.runSync(encodedFailure.encode({ reason: "invalid" })),
              },
            },
          }),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 16 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("failing-output", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(Array.from(handled).at(-1)?.encodedResult).toEqual({
      status: "error",
      error: { name: "rejected", value: { reason: "invalid" } },
    })
  })

  it("distinguishes omitted optional stdin, explicit empty stdin, and missing required stdin", async () => {
    const optional = toolDefinition("optional-input").body((body) =>
      body.input({ required: false }),
    )
    const observed: Array<undefined | number[]> = []
    const transport: ToolTransport = {
      start: (_tool, _path, _input, stdin) =>
        Effect.gen(function* () {
          if (!stdin) observed.push(undefined)
          else {
            const bytes: number[] = []
            yield* Effect.promise(async () => {
              for await (const item of stdin) if (item.tag === "ok") bytes.push(...item.val)
            })
            observed.push(bytes)
          }
          return { result: Effect.succeed({}), cancel: Effect.void }
        }),
    }
    const optionalToolkit = await Effect.runPromise(
      typedToolkit(optional, [command()], { transport }),
    )

    await Effect.runPromise(
      optionalToolkit.handle("optional-input", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )
    await Effect.runPromise(
      optionalToolkit
        .handle("optional-input", { _stdin: { data: "", encoding: "utf8" } })
        .pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(observed).toEqual([undefined, []])

    const required = toolDefinition("required-input").body((body) => body.input({ required: true }))
    const requiredToolkit = await Effect.runPromise(
      typedToolkit(required, [command()], { transport }),
    )
    await expect(
      Effect.runPromise(
        requiredToolkit.handle("required-input", {}).pipe(Effect.flatMap(Stream.runDrain)),
      ),
    ).rejects.toThrow()
  })

  it("preserves omission of optional ordinary arguments through Tool.client", async () => {
    const definition = toolDefinition("optional-argument").body((body) =>
      body.positional("label", Schema.optionalKey(Schema.String)),
    )
    const encodedInput = Effect.runSync(
      compile(Schema.Struct({ label: Schema.optionalKey(Schema.String) })),
    )
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, input) =>
      Effect.gen(function* () {
        expect(yield* encodedInput.decode(input.value)).toEqual({})
        return { result: Effect.succeed({}), cancel: Effect.void }
      }),
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command()], { transport: { start } }),
    )

    await Effect.runPromise(
      toolkit.handle("optional-argument", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(start).toHaveBeenCalledOnce()
  })

  it("injects command-line defaults for omitted options and flags", async () => {
    const definition = toolDefinition("cli-defaults").body((body) =>
      body
        .option("label", Schema.String)
        .option("include", Schema.String, { repeatable: "repeated" })
        .option("labels", Schema.ReadonlyMap(Schema.String, Schema.String), {
          repeatable: "repeated",
        })
        .flag("verbose")
        .tail("rest", Schema.String),
    )
    const encodedInput = Effect.runSync(
      compile(
        Schema.Struct({
          rest: Schema.Array(Schema.String),
          label: Schema.NullOr(Schema.String),
          include: Schema.Array(Schema.String),
          labels: Schema.ReadonlyMap(Schema.String, Schema.String),
          verbose: Schema.Boolean,
        }),
      ),
    )
    const start = vi.fn<ToolTransport["start"]>((_tool, _path, input) =>
      Effect.gen(function* () {
        expect(yield* encodedInput.decode(input.value)).toEqual({
          label: null,
          rest: [],
          include: [],
          labels: new Map(),
          verbose: false,
        })
        return { result: Effect.succeed({}), cancel: Effect.void }
      }),
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command()], { transport: { start } }),
    )

    await Effect.runPromise(
      toolkit.handle("cli-defaults", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(start).toHaveBeenCalledOnce()
  })

  it("handles an optional typed argument named constructor", async () => {
    const observed: unknown[] = []
    const withDefault = toolDefinition("constructor-default").body((body) =>
      body.option("constructor", Schema.String, { default: "safe" }),
    )
    const defaultCodec = Effect.runSync(compile(Schema.Struct({ constructor: Schema.String })))
    const defaultStart: ToolTransport["start"] = (_tool, _path, input) =>
      Effect.gen(function* () {
        observed.push((yield* defaultCodec.decode(input.value)).constructor)
        return { result: Effect.succeed({}), cancel: Effect.void }
      })
    const defaultToolkit = await Effect.runPromise(
      typedToolkit(withDefault, [command()], { transport: { start: defaultStart } }),
    )

    for (const parameters of [{}, { constructor: "explicit" }])
      await Effect.runPromise(
        defaultToolkit
          .handle("constructor-default", parameters)
          .pipe(Effect.flatMap(Stream.runDrain)),
      )

    const optional = toolDefinition("constructor-optional").body((body) =>
      body.option("constructor", Schema.String),
    )
    const optionalCodec = Effect.runSync(
      compile(Schema.Struct({ constructor: Schema.NullOr(Schema.String) })),
    )
    const optionalStart: ToolTransport["start"] = (_tool, _path, input) =>
      Effect.gen(function* () {
        observed.push((yield* optionalCodec.decode(input.value)).constructor)
        return { result: Effect.succeed({}), cancel: Effect.void }
      })
    const optionalToolkit = await Effect.runPromise(
      typedToolkit(optional, [command()], { transport: { start: optionalStart } }),
    )
    await Effect.runPromise(
      optionalToolkit.handle("constructor-optional", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    expect(observed).toEqual(["safe", "explicit", null])
  })

  it("rejects malformed parameter roots without invoking transport", async () => {
    const definition = toolDefinition("object-only").body((body) => body.input({ required: false }))
    const start = vi.fn<ToolTransport["start"]>(() =>
      Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command()], { transport: { start } }),
    )

    for (const malformed of [null, [], false, 0, ""]) {
      await expect(
        Effect.runPromise(
          toolkit.handle("object-only", malformed).pipe(Effect.flatMap(Stream.runDrain)),
        ),
      ).rejects.toThrow()
    }
    expect(start).not.toHaveBeenCalled()
  })

  it("represents declared stdout explicitly when the transport produces no iterable", async () => {
    const definition = toolDefinition("quiet-printer").body((body) =>
      body.output({ mime: ["text/plain"] }),
    )
    const transport: ToolTransport = {
      start: () => Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 16 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("quiet-printer", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "success",
      stdout: { data: "", encoding: "utf8", truncated: false, totalBytes: 0 },
    })
  })

  it("recognizes case-insensitive textual MIME types when encoding captured stdout", async () => {
    const definition = toolDefinition("uppercase-text").body((body) =>
      body.output({ mime: ["TEXT/PLAIN"] }),
    )
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("héllo") } as const
          })(),
          result: Effect.succeed({}),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 16 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("uppercase-text", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "success",
      stdout: { data: "héllo", encoding: "utf8", truncated: false, totalBytes: 6 },
    })
  })

  it("falls back to base64 when truncation splits a textual UTF-8 sequence", async () => {
    const definition = toolDefinition("split-text").body((body) =>
      body.output({ mime: ["text/plain"] }),
    )
    const encoded = new TextEncoder().encode("€x")
    let drained = false
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok", val: encoded.slice(0, 2) } as const
            yield { tag: "ok", val: encoded.slice(2) } as const
            drained = true
          })(),
          result: Effect.succeed({}),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 2 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("split-text", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(drained).toBe(true)
    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "success",
      stdout: { data: "4oI=", encoding: "base64", truncated: true, totalBytes: 4 },
    })
  })

  it("keeps declared binary stdout in base64 even when its bytes are valid UTF-8", async () => {
    const definition = toolDefinition("binary-output").body((body) =>
      body.output({ mime: ["application/octet-stream"] }),
    )
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok", val: new TextEncoder().encode("text") } as const
          })(),
          result: Effect.succeed({}),
          cancel: Effect.void,
        }),
    }
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { maxStdoutBytes: 16 })], { transport }),
    )

    const handled = await Effect.runPromise(
      toolkit.handle("binary-output", {}).pipe(Effect.flatMap(Stream.runCollect)),
    )

    expect(Array.from(handled).at(-1)?.result).toEqual({
      status: "success",
      stdout: { data: "dGV4dA==", encoding: "base64", truncated: false, totalBytes: 4 },
    })
  })

  it("rejects malformed base64 stdin before invoking transport", async () => {
    const definition = toolDefinition("base64-input").body((body) => body.input())
    const start = vi.fn<ToolTransport["start"]>(() =>
      Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command()], { transport: { start } }),
    )

    for (const data of ["%%%", "Zh==", "Zm9="]) {
      const exit = await Effect.runPromiseExit(
        toolkit
          .handle("base64-input", { _stdin: { data, encoding: "base64" } })
          .pipe(Effect.flatMap(Stream.runDrain)),
      )
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        expect(Cause.hasFails(exit.cause)).toBe(true)
        expect(Cause.hasDies(exit.cause)).toBe(false)
      }
    }
    expect(start).not.toHaveBeenCalled()
  })

  it("accepts canonical padded and empty base64 stdin", async () => {
    const definition = toolDefinition("valid-base64").body((body) => body.input())
    const observed: number[][] = []
    const transport: ToolTransport = {
      start: (_tool, _path, _input, stdin) =>
        Effect.gen(function* () {
          const bytes: number[] = []
          if (stdin)
            yield* Effect.promise(async () => {
              for await (const item of stdin) if (item.tag === "ok") bytes.push(...item.val)
            })
          observed.push(bytes)
          return { result: Effect.succeed({}), cancel: Effect.void }
        }),
    }
    const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))

    for (const data of ["Zg==", "Zm8=", ""])
      await Effect.runPromise(
        toolkit
          .handle("valid-base64", { _stdin: { data, encoding: "base64" } })
          .pipe(Effect.flatMap(Stream.runDrain)),
      )

    expect(observed).toEqual([[102], [102, 111], []])
  })

  it("validates finite stdout limits on plain selection objects", () => {
    const definition = toolDefinition("bounded-output").body((body) => body.output())
    for (const maxStdoutBytes of [Infinity, Number.NaN, -1, 1.5])
      expect(() => typedToolkit(definition, [{ path: [], options: { maxStdoutBytes } }])).toThrow(
        /non-negative safe integer/,
      )
  })

  it("cancels the existing tool invocation when AI handling is interrupted", async () => {
    const definition = toolDefinition("interruptible").body((body) => body)
    const cancel = vi.fn()
    const transport: ToolTransport = {
      start: () =>
        Effect.succeed({
          result: Effect.never,
          cancel: Effect.sync(cancel),
        }),
    }
    const toolkit = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))
    const fiber = Effect.runFork(
      toolkit.handle("interruptible", {}).pipe(Effect.flatMap(Stream.runDrain)),
    )

    await Effect.runPromise(Effect.sleep("10 millis"))
    await Effect.runPromise(Fiber.interrupt(fiber))

    expect(cancel).toHaveBeenCalledOnce()
  })
})
