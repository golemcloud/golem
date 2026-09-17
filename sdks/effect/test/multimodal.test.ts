import { describe, expect, it } from "@effect/vitest"
import { Context, Effect, Exit, Schema, SchemaGetter } from "effect"
import { multimodal, multimodalTextImage } from "../src/Multimodal.js"
import { UnstructuredBinary, UnstructuredText } from "../src/Unstructured.js"

const Content = multimodal({
  text: UnstructuredText(),
  image: UnstructuredBinary(),
  meta: Schema.Struct({ prompt: Schema.String }),
} as const)

describe("multimodal()", () => {
  it.effect("emits one schema case per member with the expected rich shapes", () =>
    Effect.gen(function* () {
      const compiled = yield* Content.compile()
      expect(compiled.graph.root.metadata.role).toEqual({ tag: "multimodal" })
      expect(compiled.graph.root.body.tag).toBe("list")
      if (compiled.graph.root.body.tag !== "list") throw new Error()
      const variant = compiled.graph.root.body.element.body
      if (variant.tag !== "variant") throw new Error()
      expect(variant.cases.map(({ name }) => name)).toEqual(["text", "image", "meta"])
      expect(variant.cases[0]!.payload!.metadata.role).toEqual({ tag: "unstructured-text" })
      expect(variant.cases[1]!.payload!.metadata.role).toEqual({ tag: "unstructured-binary" })
      expect(variant.cases[2]!.payload!.body.tag).toBe("record")
    }),
  )

  it.effect("decodes and re-encodes a multimodal value", () =>
    Effect.gen(function* () {
      const compiled = yield* Content.compile()
      const value = [
        { _tag: "text" as const, value: { _tag: "inline" as const, val: "hi" } },
        { _tag: "image" as const, value: { _tag: "url" as const, val: "https://x/y.png" } },
        { _tag: "meta" as const, value: { prompt: "p" } },
      ]
      const encoded = yield* Schema.encodeEffect(compiled.codec)(value)
      expect(encoded.tag).toBe("list")
      expect(yield* Schema.decodeEffect(compiled.codec)(encoded)).toEqual(value)
    }),
  )

  it.effect("composes distinct recursive member graphs", () =>
    Effect.gen(function* () {
      interface Node {
        readonly label: string
        readonly children: ReadonlyArray<Node>
      }
      const recursive = (title: string): Schema.Codec<Node, Node> => {
        const node: Schema.Codec<Node, Node> = Schema.Struct({
          label: Schema.String,
          children: Schema.Array(Schema.suspend(() => node)),
        }).pipe(Schema.annotate({ title }))
        return node
      }
      const compiled = yield* multimodal({
        left: recursive("LeftNode"),
        right: recursive("RightNode"),
      }).compile()
      expect(compiled.graph.defs.size).toBe(2)
      expect(new Set(compiled.graph.defs.keys()).size).toBe(2)
    }),
  )

  it.effect("preserves member services through suspended asynchronous validation", () =>
    Effect.gen(function* () {
      class Validation extends Context.Service<Validation, { readonly suffix: string }>()(
        "test/MultimodalValidation",
      ) {}
      const validated = Schema.String.pipe(
        Schema.decodeTo(Schema.String, {
          decode: SchemaGetter.transformOrFail((value) =>
            Effect.gen(function* () {
              yield* Effect.yieldNow
              const validation = yield* Validation
              return `${value}${validation.suffix}`
            }),
          ),
          encode: SchemaGetter.transformOrFail((value) =>
            Effect.gen(function* () {
              yield* Effect.yieldNow
              const validation = yield* Validation
              return value.slice(0, -validation.suffix.length)
            }),
          ),
        }),
      )
      const suspended = Schema.suspend(() => validated)
      const compiled = yield* multimodal({ checked: suspended }).compile()
      const encoded = yield* Schema.encodeEffect(compiled.codec)([
        { _tag: "checked", value: "value!" },
      ]).pipe(Effect.provideService(Validation, { suffix: "!" }))
      const decoded = yield* Schema.decodeEffect(compiled.codec)(encoded).pipe(
        Effect.provideService(Validation, { suffix: "!" }),
      )
      expect(decoded).toEqual([{ _tag: "checked", value: "value!" }])
    }),
  )

  it.effect("rejects malformed multimodal values", () =>
    Effect.gen(function* () {
      const compiled = yield* Content.compile()
      const exit = yield* Effect.exit(
        Schema.decodeEffect(compiled.codec)({
          tag: "list",
          elements: [{ tag: "string", value: "x" }],
        } as any),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("multimodalTextImage builds a two-case multimodal", () =>
    Effect.gen(function* () {
      const compiled = yield* multimodalTextImage().compile()
      if (compiled.graph.root.body.tag !== "list") throw new Error()
      const variant = compiled.graph.root.body.element.body
      if (variant.tag !== "variant") throw new Error()
      expect(variant.cases.map(({ name }) => name)).toEqual(["text", "image"])
    }),
  )
})
