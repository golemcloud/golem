import { Context, Effect, Option, Schema, SchemaGetter, SchemaIssue } from "effect"
import { describe, expect, it, vi } from "vitest"
import { AgentStream } from "../src/AgentStream.js"
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
    codec.encodeAsync(AgentStream.from(values)) as any,
  )) as import("golem:core/types@2.0.0").SchemaValueTree
  return Effect.runPromise(codec.decode(wire) as any) as Promise<AgentStream<S["Type"]>>
}

describe("stream item schemas", () => {
  it("validates literal and refinement items on each pull", async () => {
    const literal = await roundTrip(Schema.Literal("yes"), ["no" as "yes"])
    await expect(literal.next()).rejects.toThrow(/yes/)

    const positive = Schema.Number.pipe(
      Schema.check(Schema.makeFilter((n) => n > 0 || "must be positive")),
    )
    const refined = await roundTrip(positive, [-1])
    await expect(refined.next()).rejects.toThrow(/must be positive/)
  })

  it("runs asymmetric item transformations in both directions", async () => {
    const stream = await roundTrip(Schema.NumberFromString, [42])
    await expect(stream.next()).resolves.toEqual({ done: false, value: 42 })
  })

  it("retains full item codecs for recursively nested streams", async () => {
    const codec = await Effect.runPromise(
      compile(Schema.Struct({ items: WitTypes.AgentStream(Schema.NumberFromString) })),
    )
    const wire = await Effect.runPromise(codec.encodeAsync({ items: AgentStream.from([7]) }))
    const decoded = await Effect.runPromise(codec.decode(wire))
    await expect(decoded.items.next()).resolves.toEqual({ done: false, value: 7 })
  })

  it("compiles recursion through stream elements without eagerly compiling forever", async () => {
    interface Node {
      readonly label: string
      readonly children: AgentStream<Node>
    }
    const node: Schema.Codec<Node> = Schema.suspend(() =>
      Schema.Struct({ label: Schema.String, children: WitTypes.AgentStream(node) }),
    )
    const codec = await Effect.runPromise(compile(node))
    const wire = await Effect.runPromise(
      codec.encodeAsync({
        label: "parent",
        children: AgentStream.from([{ label: "child", children: AgentStream.from([]) }]),
      }),
    )
    const decoded = await Effect.runPromise(codec.decode(wire))
    const child = await decoded.children.next()
    expect(child.done).toBe(false)
    expect(child.value.label).toBe("child")
    await expect(child.value.children.next()).resolves.toEqual({ done: true, value: undefined })
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
      codec.encodeAsync(AgentStream.from(["accepted"])).pipe(Effect.provide(context)),
    )
    const stream = await Effect.runPromise(codec.decode(wire).pipe(Effect.provide(context)))
    await expect(stream.next()).resolves.toEqual({ done: false, value: "accepted" })
  })

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
    const stream = (await Effect.runPromise(
      codec.decode(outer as any) as any,
    )) as AgentStream<unknown>
    await expect(stream.next()).rejects.toThrow(/valid/)
    expect(secretNode.val).toBe(rawSecret)
  })
})
