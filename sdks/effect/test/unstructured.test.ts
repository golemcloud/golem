import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Schema } from "effect"
import { UnstructuredBinary, UnstructuredText } from "../src/Unstructured.js"

describe("UnstructuredText element", () => {
  it.effect("emits an unstructured-text schema and round-trips inline text", () =>
    Effect.gen(function* () {
      const element = UnstructuredText()
      expect(element.root.metadata.role).toEqual({ tag: "unstructured-text" })
      const value = { _tag: "inline" as const, val: "hello", languageCode: "en" }
      const encoded = yield* Schema.encodeEffect(element.witCodec.codec)(value)
      expect(encoded).toEqual({
        tag: "variant",
        caseIndex: 0,
        payload: { tag: "text", text: "hello", language: "en" },
      })
      expect(yield* Schema.decodeEffect(element.witCodec.codec)(encoded)).toEqual(value)
    }),
  )

  it("carries language restrictions through to the element schema", () => {
    const element = UnstructuredText({ restrictions: [{ languageCode: "en" }] })
    if (element.root.body.tag !== "variant") throw new Error()
    expect(element.root.body.cases[0]!.payload!.body).toEqual({
      tag: "text",
      restrictions: { languages: ["en"] },
    })
  })

  it.effect("accepts absent language metadata and rejects disallowed present metadata", () =>
    Effect.gen(function* () {
      const element = UnstructuredText({ restrictions: [{ languageCode: "en" }] })
      expect(
        element.fromValue({
          tag: "variant",
          caseIndex: 0,
          payload: { tag: "text", text: "hello" },
        }),
      ).toEqual({ _tag: "inline", val: "hello" })
      const exit = yield* Effect.exit(
        Schema.decodeEffect(element.witCodec.codec)({
          tag: "variant",
          caseIndex: 0,
          payload: { tag: "text", text: "hola", language: "es" },
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("rejects a wrong element kind at decode time", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Schema.decodeEffect(UnstructuredText().witCodec.codec)({
          tag: "record",
          fields: [],
        } as any),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )
})

describe("UnstructuredBinary element", () => {
  it("emits restrictions in the binary payload schema", () => {
    const element = UnstructuredBinary({ restrictions: [{ mimeType: "image/png" }] })
    expect(element.root.metadata.role).toEqual({ tag: "unstructured-binary" })
    if (element.root.body.tag !== "variant") throw new Error()
    expect(element.root.body.cases[0]!.payload!.body).toEqual({
      tag: "binary",
      restrictions: { mimeTypes: ["image/png"] },
    })
  })

  it.effect("round-trips a URL-style binary reference", () =>
    Effect.gen(function* () {
      const element = UnstructuredBinary()
      const value = { _tag: "url" as const, val: "https://example.com/x.png" }
      const encoded = yield* Schema.encodeEffect(element.witCodec.codec)(value)
      expect(encoded).toEqual({
        tag: "variant",
        caseIndex: 1,
        payload: { tag: "url", value: value.val },
      })
      expect(yield* Schema.decodeEffect(element.witCodec.codec)(encoded)).toEqual(value)
    }),
  )

  it.effect("round-trips inline binary data with its MIME type", () =>
    Effect.gen(function* () {
      const element = UnstructuredBinary()
      const value = {
        _tag: "inline" as const,
        val: new Uint8Array([0, 127, 255]),
        mimeType: "application/octet-stream",
      }
      const encoded = yield* Schema.encodeEffect(element.witCodec.codec)(value)
      expect(encoded).toEqual({
        tag: "variant",
        caseIndex: 0,
        payload: { tag: "binary", bytes: value.val, mimeType: value.mimeType },
      })
      expect(yield* Schema.decodeEffect(element.witCodec.codec)(encoded)).toEqual(value)
    }),
  )
})
