import { describe, it, expect } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { compileMethodSpec, invokeDataValue, method } from "../src/Method.js"
import { multimodal, multimodalTextImage } from "../src/Multimodal.js"
import {
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "../src/Unstructured.js"

const Meta = Schema.Struct({ prompt: Schema.String })
const Content = multimodal({
  text: UnstructuredText(),
  image: UnstructuredBinary(),
  meta: Meta,
} as const)

describe("multimodal()", () => {
  it.effect("emits a multimodal DataSchema with one entry per case", () =>
    Effect.gen(function* () {
      const m = method({
        params: { content: Content },
        success: Schema.String,
      })
      const mc = yield* compileMethodSpec("send", m)
      expect(mc.inputSchema.tag).toBe("multimodal")
      if (mc.inputSchema.tag !== "multimodal") throw new Error()
      expect(mc.inputSchema.val.map(([k]) => k)).toEqual(["text", "image", "meta"])
      expect(mc.inputSchema.val[0]![1].tag).toBe("unstructured-text")
      expect(mc.inputSchema.val[1]![1].tag).toBe("unstructured-binary")
      expect(mc.inputSchema.val[2]![1].tag).toBe("component-model")
    }),
  )

  it.effect("decodes a multimodal DataValue and re-encodes the result", () =>
    Effect.gen(function* () {
      const m = method({
        params: { content: Content },
        success: Schema.Number,
      })
      const mc = yield* compileMethodSpec("count", m)

      type Item =
        | { _tag: "text"; value: TextReferenceValue }
        | { _tag: "image"; value: BinaryReferenceValue }
        | { _tag: "meta"; value: { prompt: string } }

      const handler = ({ content }: { content: ReadonlyArray<Item> }) =>
        Effect.succeed(content.length)

      // Build the input via the multimodal compiled encoder so we don't
      // hand-roll the WitValue for the meta case.
      const compiled = yield* Content.compile()
      const dv = yield* compiled.encode([
        {
          _tag: "text",
          value: { _tag: "inline", val: { data: "hi" } } as TextReferenceValue,
        },
        {
          _tag: "image",
          value: { _tag: "url", val: "https://x/y.png" } as BinaryReferenceValue,
        },
        { _tag: "meta", value: { prompt: "p" } },
      ] as any)
      expect(dv.tag).toBe("multimodal")

      const out = yield* invokeDataValue(mc, handler as any, dv) as Effect.Effect<
        any,
        unknown,
        never
      >
      if (out.tag !== "tuple") throw new Error()
      const elem = out.val[0]!
      if (elem.tag !== "component-model") throw new Error()
      // Decode the success value back through the outputElement codec.
      const decoded = yield* mc.outputElement!.decode(elem)
      expect(decoded).toBe(3)
    }),
  )

  it.effect("rejects non-sole multimodal parameters", () =>
    Effect.gen(function* () {
      const m = method({
        params: { content: Content, extra: Schema.String },
        success: Schema.Void,
      })
      const exit = yield* Effect.exit(compileMethodSpec("bad", m))
      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("multimodalTextImage builds a two-case multimodal", () =>
    Effect.gen(function* () {
      const C = multimodalTextImage()
      const compiled = yield* C.compile()
      expect(compiled.dataSchema.tag).toBe("multimodal")
      if (compiled.dataSchema.tag !== "multimodal") throw new Error()
      expect(compiled.dataSchema.val.map(([k]) => k)).toEqual(["text", "image"])
    }),
  )
})
