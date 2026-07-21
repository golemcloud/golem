import { describe, it, expect } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { compileMethodSpec, invokeDataValue, method } from "../src/Method.js"
import {
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "../src/Unstructured.js"

// The existing host output contract can expose an unstructured element
// directly, but cannot nest it inside the component-model result used for
// typed method errors.
// @ts-expect-error unstructured successes cannot declare typed errors
void method({ params: {}, success: UnstructuredText(), error: Schema.String })

describe("UnstructuredText element", () => {
  it.effect("emits an unstructured-text element schema and round-trips inline text", () =>
    Effect.gen(function* () {
      const echo = method({
        params: { msg: UnstructuredText() },
        success: Schema.String,
      })
      const mc = yield* compileMethodSpec("echo", echo)
      if (mc.inputSchema.tag !== "tuple") throw new Error()
      expect(mc.inputSchema.val.length).toBe(1)
      const [name, schema] = mc.inputSchema.val[0]!
      expect(name).toBe("msg")
      expect(schema.tag).toBe("unstructured-text")

      const inputValue: TextReferenceValue = {
        _tag: "inline",
        val: { data: "hello", textType: { languageCode: "en" } },
      }
      const handler = ({ msg }: { msg: TextReferenceValue }) =>
        Effect.succeed(msg._tag === "inline" ? msg.val.data.toUpperCase() : msg.val)

      const out = yield* invokeDataValue(mc, handler as any, {
        tag: "tuple",
        val: [
          {
            tag: "unstructured-text",
            val: { tag: "inline", val: inputValue.val },
          },
        ],
      }) as Effect.Effect<any, unknown, never>
      if (out.tag !== "tuple") throw new Error()
      const elem = out.val[0]!
      if (elem.tag !== "component-model") throw new Error()
      expect(elem.val.nodes.length).toBeGreaterThan(0)
    }),
  )

  it.effect("emits an unstructured-text success with language metadata", () =>
    Effect.gen(function* () {
      const m = method({
        params: {},
        success: UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
      })
      const mc = yield* compileMethodSpec("m", m)
      expect(mc.outputCodec).toBeNull()
      if (mc.outputSchema.tag !== "tuple") throw new Error()
      const schema = mc.outputSchema.val[0]![1]
      expect(schema).toEqual({
        tag: "unstructured-text",
        val: { restrictions: [{ languageCode: "en" }] },
      })

      const value: TextReferenceValue = {
        _tag: "inline",
        val: { data: "hello", textType: { languageCode: "en" } },
      }
      const out = yield* invokeDataValue(mc, () => Effect.succeed(value), {
        tag: "tuple",
        val: [],
      })
      expect(out).toEqual({
        tag: "tuple",
        val: [
          {
            tag: "unstructured-text",
            val: {
              tag: "inline",
              val: { data: "hello", textType: { languageCode: "en" } },
            },
          },
        ],
      })
    }),
  )

  it.effect("carries restrictions through to the element schema", () =>
    Effect.gen(function* () {
      const m = method({
        params: {
          msg: UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
        },
        success: Schema.Void,
      })
      const mc = yield* compileMethodSpec("m", m)
      if (mc.inputSchema.tag !== "tuple") throw new Error()
      const schema = mc.inputSchema.val[0]![1]
      expect(schema.tag).toBe("unstructured-text")
      if (schema.tag !== "unstructured-text") throw new Error()
      expect(schema.val.restrictions).toEqual([{ languageCode: "en" }])
    }),
  )

  it.effect("rejects a wrong element kind at decode time", () =>
    Effect.gen(function* () {
      const m = method({
        params: { msg: UnstructuredText() },
        success: Schema.Void,
      })
      const mc = yield* compileMethodSpec("m", m)
      const result = yield* Effect.exit(
        invokeDataValue(mc, () => Effect.void, {
          tag: "tuple",
          val: [
            {
              tag: "component-model" as const,
              val: { nodes: [{ value: { tag: "prim-bool", val: true } }] } as any,
            },
          ],
        }) as Effect.Effect<any, unknown, never>,
      )
      expect(result._tag).toBe("Failure")
    }),
  )
})

describe("UnstructuredBinary element", () => {
  it.effect("emits an unstructured-binary element schema with restrictions", () =>
    Effect.gen(function* () {
      const m = method({
        params: {
          blob: UnstructuredBinary({
            restrictions: [{ mimeType: "image/png" }],
          }),
        },
        success: Schema.Void,
      })
      const mc = yield* compileMethodSpec("m", m)
      if (mc.inputSchema.tag !== "tuple") throw new Error()
      const schema = mc.inputSchema.val[0]![1]
      expect(schema.tag).toBe("unstructured-binary")
      if (schema.tag !== "unstructured-binary") throw new Error()
      expect(schema.val.restrictions).toEqual([{ mimeType: "image/png" }])
    }),
  )

  it.effect("round-trips a url-style binary reference", () =>
    Effect.gen(function* () {
      const m = method({
        params: { blob: UnstructuredBinary() },
        success: Schema.String,
      })
      const mc = yield* compileMethodSpec("m", m)
      const ref: BinaryReferenceValue = {
        _tag: "url",
        val: "https://example.com/x.png",
      }
      const encoded = yield* mc.bindings[0]!.element.encode(ref)
      expect(encoded).toEqual({
        tag: "unstructured-binary",
        val: { tag: "url", val: "https://example.com/x.png" },
      })

      const out = yield* invokeDataValue(
        mc,
        (({ blob }: { blob: BinaryReferenceValue }) =>
          Effect.succeed(blob._tag === "url" ? blob.val : "inline")) as any,
        {
          tag: "tuple",
          val: [encoded],
        },
      ) as Effect.Effect<any, unknown, never>
      if (out.tag !== "tuple") throw new Error()
      const elem = out.val[0]!
      if (elem.tag !== "component-model") throw new Error()
      expect(yield* mc.outputElement!.decode(elem)).toBe("https://example.com/x.png")
    }),
  )

  it.effect("emits an unstructured-binary success with its MIME type", () =>
    Effect.gen(function* () {
      const m = method({
        params: {},
        success: UnstructuredBinary({
          restrictions: [{ mimeType: "application/octet-stream" }],
        }),
      })
      const mc = yield* compileMethodSpec("m", m)
      expect(mc.outputCodec).toBeNull()
      if (mc.outputSchema.tag !== "tuple") throw new Error()
      expect(mc.outputSchema.val[0]![1]).toEqual({
        tag: "unstructured-binary",
        val: { restrictions: [{ mimeType: "application/octet-stream" }] },
      })

      const value: BinaryReferenceValue = {
        _tag: "inline",
        val: {
          data: new Uint8Array([0, 127, 255]),
          binaryType: { mimeType: "application/octet-stream" },
        },
      }
      const out = yield* invokeDataValue(mc, () => Effect.succeed(value), {
        tag: "tuple",
        val: [],
      })
      expect(out).toEqual({
        tag: "tuple",
        val: [
          {
            tag: "unstructured-binary",
            val: {
              tag: "inline",
              val: {
                data: new Uint8Array([0, 127, 255]),
                binaryType: { mimeType: "application/octet-stream" },
              },
            },
          },
        ],
      })
    }),
  )
})
