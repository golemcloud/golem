import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import { compileMethodSpec, invokeDataValue, method } from "../src/method.js"
import {
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
  type TextReferenceValue,
} from "../src/unstructured.js"

describe("UnstructuredText element", () => {
  it("emits an unstructured-text element schema and round-trips inline text", async () => {
    const echo = method({
      params: { msg: UnstructuredText() },
      success: Schema.String,
    })
    const mc = await Effect.runPromise(compileMethodSpec("echo", echo))
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

    const out = await Effect.runPromise(
      invokeDataValue(mc, handler as any, {
        tag: "tuple",
        val: [{ tag: "unstructured-text", val: inputValue as any }],
      }),
    )
    if (out.tag !== "tuple") throw new Error()
    const elem = out.val[0]!
    if (elem.tag !== "component-model") throw new Error()
    expect(elem.val.nodes.length).toBeGreaterThan(0)
  })

  it("carries restrictions through to the element schema", async () => {
    const m = method({
      params: {
        msg: UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
      },
      success: Schema.Void,
    })
    const mc = await Effect.runPromise(compileMethodSpec("m", m))
    if (mc.inputSchema.tag !== "tuple") throw new Error()
    const schema = mc.inputSchema.val[0]![1]
    expect(schema.tag).toBe("unstructured-text")
    if (schema.tag !== "unstructured-text") throw new Error()
    expect(schema.val.restrictions).toEqual([{ languageCode: "en" }])
  })

  it("rejects a wrong element kind at decode time", async () => {
    const m = method({
      params: { msg: UnstructuredText() },
      success: Schema.Void,
    })
    const mc = await Effect.runPromise(compileMethodSpec("m", m))
    const result = await Effect.runPromiseExit(
      invokeDataValue(mc, () => Effect.void, {
        tag: "tuple",
        val: [
          {
            tag: "component-model" as const,
            val: { nodes: [{ value: { tag: "prim-bool", val: true } }] } as any,
          },
        ],
      }),
    )
    expect(result._tag).toBe("Failure")
  })
})

describe("UnstructuredBinary element", () => {
  it("emits an unstructured-binary element schema with restrictions", async () => {
    const m = method({
      params: {
        blob: UnstructuredBinary({
          restrictions: [{ mimeType: "image/png" }],
        }),
      },
      success: Schema.Void,
    })
    const mc = await Effect.runPromise(compileMethodSpec("m", m))
    if (mc.inputSchema.tag !== "tuple") throw new Error()
    const schema = mc.inputSchema.val[0]![1]
    expect(schema.tag).toBe("unstructured-binary")
    if (schema.tag !== "unstructured-binary") throw new Error()
    expect(schema.val.restrictions).toEqual([{ mimeType: "image/png" }])
  })

  it("round-trips a url-style binary reference", async () => {
    const m = method({
      params: { blob: UnstructuredBinary() },
      success: Schema.String,
    })
    const mc = await Effect.runPromise(compileMethodSpec("m", m))
    const ref: BinaryReferenceValue = {
      _tag: "url",
      val: "https://example.com/x.png",
    }
    const out = await Effect.runPromise(
      invokeDataValue(
        mc,
        (({ blob }: { blob: BinaryReferenceValue }) =>
          Effect.succeed(blob._tag === "url" ? blob.val : "inline")) as any,
        {
          tag: "tuple",
          val: [{ tag: "unstructured-binary", val: ref as any }],
        },
      ),
    )
    if (out.tag !== "tuple") throw new Error()
    const elem = out.val[0]!
    if (elem.tag !== "component-model") throw new Error()
    expect(elem.val).toBeDefined()
  })
})
