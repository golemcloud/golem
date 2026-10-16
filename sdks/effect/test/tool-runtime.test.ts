import { beforeEach, describe, expect, it, vi } from "vitest"
import { Context, Effect, Layer, Schema, SchemaGetter, Stream } from "effect"
import { compile } from "../src/WitCodec.js"
import {
  err,
  type HandlerContext,
  resetTools,
  toolDefinition,
  ToolInvokeError,
} from "../src/Tool.js"
import { invokeRegistered } from "../src/internal/tool/runtime.js"

const input = () => {
  const codec = Effect.runSync(compile(Schema.Struct({})))
  return {
    graph: codec.schemaGraph,
    value: Effect.runSync(codec.encode({}) as Effect.Effect<any, any>),
  }
}

const iterator = () => {
  const return_ = vi.fn(async () => ({ done: true as const, value: undefined }))
  return {
    return_,
    value: {
      next: async () => ({ done: true as const, value: undefined }),
      return: return_,
      [Symbol.asyncIterator]() {
        return this
      },
    },
  }
}

const writer = () => ({ write: vi.fn(), finish: vi.fn(), fail: vi.fn() })

describe("registered tool guest runtime", () => {
  beforeEach(resetTools)

  it("uses the registered layer while encoding the output", async () => {
    class Suffix extends Context.Service<Suffix, { readonly value: string }>()("test/Suffix") {}
    const output = Schema.String.pipe(
      Schema.decodeTo(Schema.String, {
        decode: SchemaGetter.transformEffect(Effect.succeed),
        encode: SchemaGetter.transformEffect((value) =>
          Effect.gen(function* () {
            const suffix = yield* Suffix
            return `${value}${suffix.value}`
          }),
        ),
      }),
    )
    toolDefinition("layer-codec")
      .body((body) => body.returns(output))
      .implement(
        { layerCodec: () => Effect.succeed("value") },
        Layer.succeed(Suffix, { value: "!" }),
      )

    const result = await invokeRegistered(
      "layer-codec",
      [],
      input(),
      undefined,
      undefined,
      undefined,
      {},
    )
    const codec = Effect.runSync(compile(Schema.String))
    await expect(Effect.runPromise(codec.decode(result.result!.value))).resolves.toBe("value!")
  })

  it("closes the registered layer scope after output encoding", async () => {
    let finalized = false
    class Scoped extends Context.Service<Scoped, true>()("test/Scoped") {}
    const layer = Layer.effect(
      Scoped,
      Effect.acquireRelease(Effect.succeed(true as const), () =>
        Effect.sync(() => (finalized = true)),
      ),
    )
    toolDefinition("scoped")
      .body((body) => body.returns(Schema.String))
      .implement(
        {
          scoped: () =>
            Effect.gen(function* () {
              yield* Scoped
              return "ok"
            }),
        },
        layer,
      )

    await invokeRegistered("scoped", [], input(), undefined, undefined, undefined, {})
    expect(finalized).toBe(true)
  })

  it("encodes a declared ToolFailure from the typed failure channel", async () => {
    toolDefinition("failure")
      .body((body) => body.error("rejected", Schema.Struct({ reason: Schema.String })))
      .implement({ failure: () => Effect.fail(err("rejected", { reason: "no" })) })

    await expect(
      invokeRegistered("failure", [], input(), undefined, undefined, undefined, {}),
    ).rejects.toMatchObject({
      tag: "custom-error",
      val: { name: "rejected" },
    })
  })

  it("validates required and undeclared stream slots against the command body", async () => {
    const requiredHandler = vi.fn(() => Effect.void)
    toolDefinition("required-streams")
      .body((body) => body.input({ required: true }).output({ required: true }))
      .implement({ requiredStreams: requiredHandler })
    await expect(
      invokeRegistered("required-streams", [], input(), undefined, undefined, undefined, {}),
    ).rejects.toMatchObject({ tag: "invalid-input" })
    expect(requiredHandler).not.toHaveBeenCalled()

    const absentHandler = vi.fn(() => Effect.void)
    toolDefinition("absent-streams")
      .body((body) => body)
      .implement({ absentStreams: absentHandler })
    const supplied = iterator()
    const stdout = writer()
    await expect(
      invokeRegistered(
        "absent-streams",
        [],
        input(),
        supplied.value,
        stdout as never,
        undefined,
        {},
      ),
    ).rejects.toMatchObject({ tag: "invalid-input" })
    expect(absentHandler).not.toHaveBeenCalled()
    expect(supplied.return_).toHaveBeenCalledOnce()
    expect(stdout.finish).toHaveBeenCalledOnce()
  })

  it("closes unused and rejected stdin and completes early-rejected stdout once", async () => {
    toolDefinition("streams")
      .body((body) => body.input())
      .implement({ streams: () => Effect.void })
    const unused = iterator()
    await invokeRegistered("streams", [], input(), unused.value, undefined, undefined, {})
    expect(unused.return_).toHaveBeenCalledOnce()

    const rejected = iterator()
    const stdout = writer()
    await expect(
      invokeRegistered(
        "streams",
        ["missing"],
        input(),
        rejected.value,
        stdout as never,
        undefined,
        {},
      ),
    ).rejects.toMatchObject({ tag: "invalid-command-path" })
    expect(rejected.return_).toHaveBeenCalledOnce()
    expect(stdout.finish).toHaveBeenCalledOnce()
    expect(stdout.fail).not.toHaveBeenCalled()
  })

  it("rejects an empty successful stdin chunk", async () => {
    toolDefinition("empty-stdin-chunk")
      .body((body) => body.input({ required: true }))
      .implement({
        emptyStdinChunk: (_input, context) => Stream.runDrain(context.stdin!),
      })
    const stdin = {
      async *[Symbol.asyncIterator]() {
        yield { tag: "ok" as const, val: new Uint8Array() }
      },
    }

    await expect(
      invokeRegistered("empty-stdin-chunk", [], input(), stdin, undefined, undefined, {}),
    ).rejects.toMatchObject({ tag: "invalid-input" })
  })

  it.each([false, true])(
    "settles both output terminals before returning when one finish fails (declared error: %s)",
    async (declaredError) => {
      toolDefinition("settle-output-terminals")
        .body((body) => body.output().stderr().error("rejected", Schema.String))
        .implement({
          settleOutputTerminals: () =>
            declaredError ? Effect.fail(err("rejected", "no")) : Effect.void,
        })
      let releaseStderr!: () => void
      const stderrGate = new Promise<void>((resolve) => (releaseStderr = resolve))
      let terminalCount = 0
      let terminalStarted!: () => void
      const bothTerminalsStarted = new Promise<void>((resolve) => (terminalStarted = resolve))
      const markTerminalStarted = () => {
        terminalCount += 1
        if (terminalCount === 2) terminalStarted()
      }
      const stdout = {
        write: vi.fn(),
        finish: vi.fn(async () => {
          markTerminalStarted()
          throw new Error("stdout finish failed")
        }),
        fail: vi.fn(),
      }
      const stderr = {
        write: vi.fn(),
        finish: vi.fn(() => {
          markTerminalStarted()
          return stderrGate
        }),
        fail: vi.fn(),
      }
      let settled = false
      const invocation = invokeRegistered(
        "settle-output-terminals",
        [],
        input(),
        undefined,
        stdout as never,
        stderr as never,
        {},
      ).finally(() => (settled = true))
      await bothTerminalsStarted
      expect(stdout.finish).toHaveBeenCalledOnce()
      expect(stderr.finish).toHaveBeenCalledOnce()
      expect(settled).toBe(false)
      releaseStderr()
      if (declaredError) await expect(invocation).rejects.toMatchObject({ tag: "custom-error" })
      else await expect(invocation).rejects.toThrow("stdout finish failed")
      expect(settled).toBe(true)
    },
  )

  it("preserves independent stream terminals, structured outcomes, and stdin cleanup", async () => {
    const terminalWriter = () => {
      const bytes: number[] = []
      let terminal: "ended" | "failed" | undefined
      return {
        bytes,
        get terminal() {
          return terminal
        },
        writer: {
          write: vi.fn(async (chunk: Uint8Array) => bytes.push(...chunk)),
          finish: vi.fn(async () => {
            terminal = "ended"
          }),
          fail: vi.fn(async () => {
            terminal = "failed"
          }),
        },
      }
    }
    const run = async (
      name: string,
      handler: (
        input: Record<string, never>,
        context: HandlerContext,
      ) => Effect.Effect<unknown, unknown, any>,
    ) => {
      toolDefinition(name)
        .body((body) =>
          body.input().output().stderr().returns(Schema.String).error("rejected", Schema.String),
        )
        .implement({
          [name.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase())]: handler,
        } as never)
      const stdin = iterator()
      const stdout = terminalWriter()
      const stderr = terminalWriter()
      const outcome = await invokeRegistered(
        name,
        [],
        input(),
        stdin.value,
        stdout.writer as never,
        stderr.writer as never,
        {},
      ).then(
        (result) => ({ tag: "success" as const, result }),
        (error) => ({ tag: "failure" as const, error }),
      )
      return { outcome, stdin, stdout, stderr }
    }
    const partial = (context: any) =>
      Effect.gen(function* () {
        yield* context.stdout(Stream.make(Uint8Array.of(1), Uint8Array.of(2, 3)))
        yield* context.stderr(Stream.succeed(Uint8Array.of(9)))
      })

    const success = await run("tuple-success", (_input, context) =>
      partial(context).pipe(Effect.as("ok")),
    )
    expect(success.outcome.tag).toBe("success")
    expect(success.stdout.bytes).toEqual([1, 2, 3])
    expect(success.stderr.bytes).toEqual([9])
    expect([success.stdout.terminal, success.stderr.terminal]).toEqual(["ended", "ended"])
    expect(success.stdin.return_).toHaveBeenCalledOnce()

    const declared = await run("tuple-declared", (_input, context) =>
      partial(context).pipe(Effect.andThen(Effect.fail(err("rejected", "no")))),
    )
    expect(declared.outcome).toMatchObject({
      tag: "failure",
      error: { tag: "custom-error" },
    })
    expect(declared.stdout.bytes).toEqual([1, 2, 3])
    expect([declared.stdout.terminal, declared.stderr.terminal]).toEqual(["ended", "ended"])
    expect(declared.stdin.return_).toHaveBeenCalledOnce()

    const explicitFailure = await run("tuple-explicit-failure", (_input, context) =>
      context.stdout!(
        Stream.concat(
          Stream.succeed(Uint8Array.of(4, 5)),
          Stream.fail(
            new ToolInvokeError({ tag: "invalid-result", val: "explicit stdout failure" }),
          ),
        ),
      ).pipe(Effect.ignore, Effect.as("ok")),
    )
    expect(explicitFailure.outcome.tag).toBe("success")
    expect(explicitFailure.stdout.bytes).toEqual([4, 5])
    expect([explicitFailure.stdout.terminal, explicitFailure.stderr.terminal]).toEqual([
      "failed",
      "ended",
    ])

    const exception = await run("tuple-exception", () => Effect.die("boom"))
    expect(exception.outcome.tag).toBe("failure")
    expect([exception.stdout.terminal, exception.stderr.terminal]).toEqual(["failed", "failed"])

    const cancellation = await run("tuple-cancellation", () => Effect.interrupt)
    expect(cancellation.outcome.tag).toBe("failure")
    expect([cancellation.stdout.terminal, cancellation.stderr.terminal]).toEqual([
      "failed",
      "failed",
    ])

    const abandoned = await run("tuple-abandoned-writers", () => Effect.succeed("ok"))
    expect(abandoned.outcome.tag).toBe("success")
    expect(abandoned.stdout.bytes).toEqual([])
    expect([abandoned.stdout.terminal, abandoned.stderr.terminal]).toEqual(["ended", "ended"])
  })
})
