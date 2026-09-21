import { describe, expect, it } from "vitest"
import { Effect, Fiber, Stream } from "effect"
import {
  createToolClientRuntime,
  splitToolRpcError,
  typedSchemaValueConforms,
} from "../src/BridgeTool.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { ToolTransport, type ToolTransport as ToolTransportShape } from "../src/Tool.js"
import type { TypedSchemaValue } from "../src/Bridge.js"
import { emptyMetadata, schemaType, t, v } from "../src/Bridge.js"
import { typedSchemaValueToWit } from "../src/internal/schema-model/wit.js"

describe("BridgeTool", () => {
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

  it("requires exact result graphs and values that conform to the expected graph", () => {
    const expected = { defs: new Map(), root: t.u32({ min: { tag: "unsigned", val: 1n } }) }
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
    ).toBe(false)
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
          return { result: Effect.succeed({}), cancel: Effect.void }
        }),
    }
    const program = Effect.scoped(
      Effect.gen(function* () {
        const invocation = yield* createToolClientRuntime("grep").start(
          ["replace"],
          input,
          undefined,
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

  it("returns independently consumable stdout, result, and explicit cancellation", async () => {
    let cancelled = false
    const transport: ToolTransportShape = {
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            yield { tag: "ok" as const, val: Uint8Array.of(1, 2) }
          })(),
          result: Effect.succeed({}),
          cancel: Effect.sync(() => {
            cancelled = true
          }),
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
          )
          expect(invocation.stdout).toBeDefined()
          expect(yield* Stream.runCollect(invocation.stdout!)).toEqual([Uint8Array.of(1, 2)])
          expect(yield* invocation.result).toEqual({ result: undefined })
          expect(cancelled).toBe(false)
          yield* invocation.cancel
        }),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, {} as never),
      ),
    )
    expect(cancelled).toBe(true)
  })

  it("lets a started handle escape the admission scope without cancelling or dropping stdout", async () => {
    let disposed = false
    const transport: ToolTransportShape = {
      start: () =>
        Effect.succeed({
          stdout: {
            [Symbol.asyncIterator]: () => ({
              next: () => new Promise<IteratorResult<never>>(() => undefined),
              return: async () => {
                disposed = true
                return { done: true, value: undefined }
              },
            }),
          },
          result: Effect.succeed({}),
          cancel: Effect.void,
        }),
    }
    const invocation = await Effect.runPromise(
      Effect.scoped(
        createToolClientRuntime("grep").start(
          [],
          { graph: { defs: new Map(), root: t.record([]) }, value: { tag: "record", fields: [] } },
          undefined,
          true,
        ),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, {} as never),
      ),
    )
    expect(disposed).toBe(false)
    const fiber = Effect.runFork(Stream.runDrain(invocation.stdout!))
    await Effect.runPromise(Effect.sleep("1 millis"))
    await Effect.runPromise(Fiber.interrupt(fiber))
    expect(disposed).toBe(true)
  })
})
