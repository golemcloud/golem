import { Effect, Schema, Stream } from "effect"
import { vi } from "vitest"
import { compile } from "../../src/WitCodec.js"
import { err, toolDefinition, type HandlerContext } from "../../src/Tool.js"

export const emptyInput = () => {
  const codec = Effect.runSync(compile(Schema.Struct({})))
  return {
    graph: codec.schemaGraph,
    value: Effect.runSync(codec.encode({}) as Effect.Effect<any, any>),
  }
}

export const trackedBytes = (...bytes: number[]) => {
  const returned = vi.fn(async () => ({ done: true as const, value: undefined }))
  const iterator = {
    next: vi.fn(async () =>
      bytes.length > 0
        ? {
            done: false as const,
            value: { tag: "ok" as const, val: Uint8Array.of(bytes.shift()!) },
          }
        : { done: true as const, value: undefined },
    ),
    return: returned,
    [Symbol.asyncIterator]() {
      return this
    },
  }
  return { iterable: { [Symbol.asyncIterator]: () => iterator }, returned }
}

export const lifecycleWriter = (finishError?: Error) => {
  const events: string[] = []
  return {
    events,
    writer: {
      write: vi.fn(async (bytes: Uint8Array) => events.push(`write:${[...bytes].join(",")}`)),
      finish: vi.fn(async () => {
        events.push("finish")
        if (finishError) throw finishError
      }),
      fail: vi.fn(async () => events.push("fail")),
    },
  }
}

export const lifecycleDefinition = (scenario: "success" | "declared" | "trap") =>
  toolDefinition(`gol40-lifecycle-${scenario}`)
    .body((body) =>
      body.input().output().stderr().returns(Schema.String).error("rejected", Schema.String),
    )
    .implement({
      [`gol40Lifecycle${scenario[0]!.toUpperCase()}${scenario.slice(1)}`]: (
        _input: Record<string, never>,
        context: HandlerContext,
      ) =>
        Effect.gen(function* () {
          yield* Stream.runDrain(context.stdin!)
          yield* context.stdout!(Stream.succeed(Uint8Array.of(1)))
          yield* context.stderr!(Stream.succeed(Uint8Array.of(2)))
          if (scenario === "declared") return yield* Effect.fail(err("rejected", "no"))
          if (scenario === "trap") return yield* Effect.die(new Error("provider trap"))
          return "ok"
        }),
    } as never)
