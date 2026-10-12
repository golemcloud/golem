import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Option, Result, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { v, type SchemaValue } from "../src/internal/schema-model/model.js"
import { schemaValueToWit } from "../src/internal/schema-model/wit.js"
import {
  compileMethodSpec,
  invokeMethod,
  method,
  type MethodSpec,
  type MethodSuccess,
  type MethodSuccessType,
} from "../src/Method.js"
import { SchemaRef } from "../src/SchemaRef.js"
import {
  Option as UnstructuredOption,
  UnstructuredBinary,
  UnstructuredText,
  type BinaryReferenceValue,
} from "../src/Unstructured.js"
import { decodeFromWire } from "../src/WitCodec.js"

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

const Pending = Schema.Struct({ _tag: Schema.Literal("Pending"), retryAfter: Schema.Number })

const nodeAt = (graph: CoreTypes.SchemaGraph, index: number) => graph.typeNodes[index]!

const outputRoot = (compiled: { readonly outputSchema: AgentCommon.OutputSchema }): number => {
  if (compiled.outputSchema.tag !== "single") throw new Error("expected a single output")
  return compiled.outputSchema.val
}

const roundTrip = <S extends MethodSuccess>(
  spec: MethodSpec<Record<never, never>, S, typeof Pending>,
  handler: () => Effect.Effect<MethodSuccessType<S>, typeof Pending.Type>,
) =>
  Effect.gen(function* () {
    const compiled = yield* compileMethodSpec("download", spec)
    const input = yield* compiled.inputCodec.encode({})
    const tree = (yield* invokeMethod(compiled, handler, input))!
    const ref = new SchemaRef(compiled.schemaGraph, outputRoot(compiled))
    expect(ref.validateValue(tree)).toMatchObject({ success: true })
    const decoded = yield* decodeFromWire(compiled.outputCodec!.codec, tree)
    return { decoded: decoded as Result.Result<unknown, unknown>, json: ref.unpackJson(tree) }
  })

describe("Unstructured method success with a typed error", () => {
  it.effect("compiles result<unstructured, error> and keeps the role on the ok node", () =>
    Effect.gen(function* () {
      const compiled = yield* compileMethodSpec(
        "download",
        method({
          input: {},
          success: UnstructuredBinary({ restrictions: [{ mimeType: "image/png" }] }),
          error: Pending,
        }),
      )
      expect(compiled.errorWrapped).toBe(true)
      expect(compiled.successVoid).toBe(false)
      const root = nodeAt(compiled.schemaGraph, outputRoot(compiled))
      if (root.body.tag !== "result-type") throw new Error(`unexpected ${root.body.tag}`)
      expect(root.metadata.role).toBeUndefined()
      const ok = nodeAt(compiled.schemaGraph, root.body.val.ok!)
      expect(ok.metadata.role).toEqual({ tag: "unstructured-binary" })
      expect(ok.body.tag).toBe("variant-type")
      const err = nodeAt(compiled.schemaGraph, root.body.val.err!)
      expect(err.body.tag).toBe("record-type")
    }),
  )

  it.effect("compiles option<unstructured> and result<option<unstructured>, error>", () =>
    Effect.gen(function* () {
      const optional = yield* compileMethodSpec(
        "maybe",
        method({ input: {}, success: UnstructuredOption(UnstructuredText()) }),
      )
      expect(optional.errorWrapped).toBe(false)
      const optionalRoot = nodeAt(optional.schemaGraph, outputRoot(optional))
      if (optionalRoot.body.tag !== "option-type") throw new Error(optionalRoot.body.tag)
      expect(nodeAt(optional.schemaGraph, optionalRoot.body.val).metadata.role).toEqual({
        tag: "unstructured-text",
      })

      const lookup = yield* compileMethodSpec(
        "lookup",
        method({ input: {}, success: UnstructuredOption(UnstructuredBinary()), error: Pending }),
      )
      const root = nodeAt(lookup.schemaGraph, outputRoot(lookup))
      if (root.body.tag !== "result-type") throw new Error(root.body.tag)
      const ok = nodeAt(lookup.schemaGraph, root.body.val.ok!)
      if (ok.body.tag !== "option-type") throw new Error(ok.body.tag)
      expect(ok.metadata.role).toBeUndefined()
      expect(nodeAt(lookup.schemaGraph, ok.body.val).metadata.role).toEqual({
        tag: "unstructured-binary",
      })
      expect(nodeAt(lookup.schemaGraph, root.body.val.err!).body.tag).toBe("record-type")
    }),
  )

  it.effect("encodes inline, url, and typed-error results that decode and reflect", () =>
    Effect.gen(function* () {
      const spec = method({ input: {}, success: UnstructuredBinary(), error: Pending })
      const inline: BinaryReferenceValue = {
        _tag: "inline",
        val: new Uint8Array([137, 80, 78, 71]),
        mimeType: "image/png",
      }
      const ok = yield* roundTrip(spec, () => Effect.succeed(inline))
      expect(ok.decoded).toEqual(Result.succeed(inline))

      const url: BinaryReferenceValue = { _tag: "url", val: "https://example.com/a.png" }
      const linked = yield* roundTrip(spec, () => Effect.succeed(url))
      expect(linked.decoded).toEqual(Result.succeed(url))

      const failed = yield* roundTrip(spec, () =>
        Effect.fail({ _tag: "Pending" as const, retryAfter: 5 }),
      )
      expect(failed.decoded).toEqual(Result.fail({ _tag: "Pending", retryAfter: 5 }))
      expect(failed.json).toEqual({ err: { _tag: "Pending", retryAfter: 5 } })

      const text = yield* roundTrip(
        method({ input: {}, success: UnstructuredText(), error: Pending }),
        () => Effect.succeed({ _tag: "inline" as const, val: "ready", languageCode: "en" }),
      )
      expect(text.decoded).toEqual(
        Result.succeed({ _tag: "inline", val: "ready", languageCode: "en" }),
      )
    }),
  )

  it.effect("encodes none, some, and typed-error results for an optional element", () =>
    Effect.gen(function* () {
      const spec = method({
        input: {},
        success: UnstructuredOption(UnstructuredBinary()),
        error: Pending,
      })
      const none = yield* roundTrip(spec, () => Effect.succeed(Option.none()))
      expect(none.decoded).toEqual(Result.succeed(Option.none()))
      expect(none.json).toEqual({ ok: null })

      const bytes: BinaryReferenceValue = { _tag: "inline", val: new Uint8Array([1, 2, 3]) }
      const some = yield* roundTrip(spec, () => Effect.succeed(Option.some(bytes)))
      expect(some.decoded).toEqual(Result.succeed(Option.some(bytes)))

      const url: BinaryReferenceValue = { _tag: "url", val: "https://example.com/blob" }
      const linked = yield* roundTrip(spec, () => Effect.succeed(Option.some(url)))
      expect(linked.decoded).toEqual(Result.succeed(Option.some(url)))

      const failed = yield* roundTrip(spec, () =>
        Effect.fail({ _tag: "Pending" as const, retryAfter: 1 }),
      )
      expect(failed.decoded).toEqual(Result.fail({ _tag: "Pending", retryAfter: 1 }))
    }),
  )

  it.effect("rejects element values that violate the element restrictions on decode", () =>
    Effect.gen(function* () {
      const compiled = yield* compileMethodSpec(
        "download",
        method({
          input: {},
          success: UnstructuredBinary({ restrictions: [{ mimeType: "image/png" }] }),
          error: Pending,
        }),
      )
      const tree = schemaValueToWit(
        v.ok(v.variant(0, { tag: "binary", bytes: new Uint8Array([0]), mimeType: "text/html" })),
      )
      const codec = compiled.outputCodec!.codec as Schema.Codec<unknown, SchemaValue>
      const exit = yield* Effect.exit(decodeFromWire(codec, tree))
      expect(Exit.isFailure(exit)).toBe(true)
      const notAResult = schemaValueToWit(v.option(undefined))
      expect(Exit.isFailure(yield* Effect.exit(decodeFromWire(codec, notAResult)))).toBe(true)
    }),
  )
})
