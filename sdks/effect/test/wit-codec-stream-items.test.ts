import { Context, Effect, Fiber, Option, Schema, SchemaGetter, SchemaIssue, Stream } from "effect"
import { describe, expect, it, vi } from "vitest"
import { compile } from "../src/WitCodec.js"
import * as WitTypes from "../src/WitTypes.js"

const transport = vi.hoisted(() => ({
  wrap: vi.fn(async (source: AsyncIterable<unknown>) => ({ source })),
  unwrap: vi.fn(async (stream: { source: AsyncIterable<unknown> }) => stream.source),
}))
vi.mock("golem:core/types@2.0.0", () => ({ SchemaValueStream: transport }))

const roundTrip = async <S extends Schema.Top>(schema: S, values: readonly S["Type"][]) => {
  const codec = await Effect.runPromise(compile(WitTypes.AgentStream(schema)))
  const wire = (await Effect.runPromise(
    codec.encodeAsync(Stream.fromIterable(values)) as any,
  )) as import("golem:core/types@2.0.0").SchemaValueTree
  return Effect.runPromise(codec.decode(wire) as any) as Promise<Stream.Stream<S["Type"], unknown>>
}

describe("stream item schemas", () => {
  it("validates literal and refinement items on each pull", async () => {
    const literal = await roundTrip(Schema.Literal("yes"), ["no" as "yes"])
    await expect(Effect.runPromise(Stream.runCollect(literal))).rejects.toThrow(/yes/)

    const positive = Schema.Number.pipe(
      Schema.check(Schema.makeFilter((n) => n > 0 || "must be positive")),
    )
    const refined = await roundTrip(positive, [-1])
    await expect(Effect.runPromise(Stream.runCollect(refined))).rejects.toThrow(/must be positive/)
  })

  it("runs asymmetric item transformations in both directions", async () => {
    const stream = await roundTrip(Schema.NumberFromString, [42])
    expect([...(await Effect.runPromise(Stream.runCollect(stream)))]).toEqual([42])
  })

  it("retains full item codecs for recursively nested streams", async () => {
    const codec = await Effect.runPromise(
      compile(Schema.Struct({ items: WitTypes.AgentStream(Schema.NumberFromString) })),
    )
    const wire = await Effect.runPromise(codec.encodeAsync({ items: Stream.make(7) }))
    const decoded = await Effect.runPromise(codec.decode(wire))
    expect([...(await Effect.runPromise(Stream.runCollect(decoded.items)))]).toEqual([7])
  })

  it("compiles recursion through stream elements without eagerly compiling forever", async () => {
    interface Node {
      readonly label: string
      readonly children: Stream.Stream<Node, unknown>
    }
    const node: Schema.Codec<Node> = Schema.suspend(() =>
      Schema.Struct({ label: Schema.String, children: WitTypes.AgentStream(node) }),
    )
    const codec = await Effect.runPromise(compile(node))
    const wire = await Effect.runPromise(
      codec.encodeAsync({
        label: "parent",
        children: Stream.make({ label: "child", children: Stream.empty }),
      }),
    )
    const decoded = await Effect.runPromise(codec.decode(wire))
    const children = await Effect.runPromise(Stream.runCollect(decoded.children))
    expect(children[0]!.label).toBe("child")
    expect([...(await Effect.runPromise(Stream.runCollect(children[0]!.children)))]).toEqual([])
  })

  it("captures services used by suspended item validation", async () => {
    class Expected extends Context.Service<Expected, { readonly value: string }>()(
      "test/StreamExpected",
    ) {}
    const encoded = Schema.String
    const serviceful = encoded.pipe(
      Schema.decodeTo(Schema.String, {
        decode: SchemaGetter.transformOrFail((value) =>
          Effect.gen(function* () {
            yield* Effect.sleep("1 millis")
            const expected = yield* Expected
            return value === expected.value
              ? value
              : yield* Effect.fail(
                  new SchemaIssue.InvalidValue(Option.none(), { message: "wrong" }),
                )
          }),
        ),
        encode: SchemaGetter.transform((value) => value),
      }),
    )
    const codec = await Effect.runPromise(compile(WitTypes.AgentStream(serviceful)))
    const context = Context.make(Expected, { value: "accepted" })
    const wire = await Effect.runPromise(
      codec.encodeAsync(Stream.make("accepted")).pipe(Effect.provide(context)),
    )
    const stream = await Effect.runPromise(codec.decode(wire).pipe(Effect.provide(context)))
    expect([...(await Effect.runPromise(Stream.runCollect(stream)))]).toEqual(["accepted"])
  })

  it.each(["decoder", "encoder"])(
    "interrupts an in-flight item %s when stream consumption is interrupted",
    async (side) => {
      let decoding!: () => void
      const decodingStarted = new Promise<void>((resolve) => (decoding = resolve))
      let finalized = 0
      const waiting = SchemaGetter.transformOrFail<string, string>(() =>
        Effect.acquireUseRelease(
          Effect.sync(decoding),
          () => Effect.never,
          () => Effect.sync(() => finalized++),
        ),
      )
      const schema = Schema.String.pipe(
        Schema.decodeTo(Schema.String, {
          decode: side === "decoder" ? waiting : SchemaGetter.transform((value) => value),
          encode: side === "encoder" ? waiting : SchemaGetter.transform((value) => value),
        }),
      )
      const stream = await roundTrip(schema, ["value"])
      const consumer = Effect.runFork(Stream.runDrain(stream))
      await decodingStarted

      await Effect.runPromise(Fiber.interrupt(consumer))

      expect(finalized).toBe(1)
    },
  )

  it("rolls back capability items when later item validation fails", async () => {
    const rawSecret = {}
    const item = Schema.Tuple([WitTypes.QuotaToken(), Schema.Literal("valid")])
    const codec = await Effect.runPromise(compile(WitTypes.AgentStream(item)))
    const secretNode = { tag: "quota-token-handle" as const, val: rawSecret }
    const tree = {
      valueNodes: [
        secretNode,
        { tag: "string-value" as const, val: "invalid" },
        { tag: "tuple-value" as const, val: [0, 1] },
      ],
      root: 2,
    }
    const source = {
      async *[Symbol.asyncIterator]() {
        yield tree
      },
    }
    const outer = {
      valueNodes: [{ tag: "stream-value" as const, val: { source } }],
      root: 0,
    }
    const stream = (await Effect.runPromise(codec.decode(outer as any) as any)) as Stream.Stream<
      unknown,
      unknown
    >
    await expect(Effect.runPromise(Stream.runCollect(stream))).rejects.toThrow(/valid/)
    expect(secretNode.val).toBe(rawSecret)
  })
})
