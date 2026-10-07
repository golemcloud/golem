import { describe, expect, it } from "vitest"
import { Deferred, Effect, Exit, Fiber, Stream } from "effect"
import {
  createToolClientRuntime,
  splitToolRpcError,
  type ToolClientRuntime,
  typedSchemaValueConforms,
} from "../src/BridgeTool.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { ToolTransport, type ToolTransport as ToolTransportShape } from "../src/Tool.js"
import type { TypedSchemaValue } from "../src/Bridge.js"
import { emptyMetadata, schemaType, t, v } from "../src/Bridge.js"
import { typedSchemaValueToWit } from "../src/internal/schema-model/wit.js"

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
    expect(cancellations).toBe(1)
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
    expect(cancellations).toBe(1)
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
            return yield* invocation.result
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
})
