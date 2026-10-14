import { describe, it, expect } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { compile, toWitCodec } from "../src/WitCodec.js"
import { schemaValueToWit } from "../src/internal/schema-model/wit.js"
import {
  BigInt64ArraySchema,
  BigUint64ArraySchema,
  Float32ArraySchema,
  Float64ArraySchema,
  Int8ArraySchema,
  Int16ArraySchema,
  Int32ArraySchema,
  Uint8,
  Uint8ArraySchema,
  Uint16ArraySchema,
  Uint32ArraySchema,
} from "../src/WitTypes.js"

const roundtrip = (s: Schema.Top, v: unknown) =>
  Effect.gen(function* () {
    const wc = yield* toWitCodec(s as any)
    const codec = wc.codec as Schema.Codec<any, any, never, never>
    const sv = yield* Schema.encodeEffect(codec)(v)
    const back = yield* Schema.decodeEffect(codec)(sv)
    return { wc, sv, back }
  })

describe("typed-array schemas → list<primN>", () => {
  const cases: ReadonlyArray<{
    readonly name: string
    readonly schema: Schema.Top
    readonly elemType: string
    readonly elemValue: string
    readonly value: ArrayBufferView
    readonly ctor: new (entries: Iterable<any>) => any
  }> = [
    {
      name: "Uint8ArraySchema",
      schema: Uint8ArraySchema,
      elemType: "u8",
      elemValue: "u8",
      value: new Uint8Array([1, 2, 3]),
      ctor: Uint8Array,
    },
    {
      name: "Int8ArraySchema",
      schema: Int8ArraySchema,
      elemType: "s8",
      elemValue: "s8",
      value: new Int8Array([-1, 0, 1]),
      ctor: Int8Array,
    },
    {
      name: "Uint16ArraySchema",
      schema: Uint16ArraySchema,
      elemType: "u16",
      elemValue: "u16",
      value: new Uint16Array([0, 1000, 65535]),
      ctor: Uint16Array,
    },
    {
      name: "Int16ArraySchema",
      schema: Int16ArraySchema,
      elemType: "s16",
      elemValue: "s16",
      value: new Int16Array([-32000, 0, 32000]),
      ctor: Int16Array,
    },
    {
      name: "Uint32ArraySchema",
      schema: Uint32ArraySchema,
      elemType: "u32",
      elemValue: "u32",
      value: new Uint32Array([0, 4_000_000_000]),
      ctor: Uint32Array,
    },
    {
      name: "Int32ArraySchema",
      schema: Int32ArraySchema,
      elemType: "s32",
      elemValue: "s32",
      value: new Int32Array([-2_000_000_000, 2_000_000_000]),
      ctor: Int32Array,
    },
    {
      name: "Float32ArraySchema",
      schema: Float32ArraySchema,
      elemType: "f32",
      elemValue: "f32",
      value: new Float32Array([1.5, -1.5]),
      ctor: Float32Array,
    },
    {
      name: "Float64ArraySchema",
      schema: Float64ArraySchema,
      elemType: "f64",
      elemValue: "f64",
      value: new Float64Array([Math.PI, -Math.PI]),
      ctor: Float64Array,
    },
    {
      name: "BigInt64ArraySchema",
      schema: BigInt64ArraySchema,
      elemType: "s64",
      elemValue: "s64",
      value: new BigInt64Array([-9_000_000_000_000n, 9_000_000_000_000n]),
      ctor: BigInt64Array,
    },
    {
      name: "BigUint64ArraySchema",
      schema: BigUint64ArraySchema,
      elemType: "u64",
      elemValue: "u64",
      value: new BigUint64Array([0n, 9_000_000_000_000n]),
      ctor: BigUint64Array,
    },
  ]

  for (const c of cases) {
    it.effect(`${c.name} round-trips and emits list<${c.elemType}>`, () =>
      Effect.gen(function* () {
        const r = yield* roundtrip(c.schema, c.value)
        // Top-level WIT type: list<primN>.
        expect(r.wc.graph.root.body.tag).toBe("list")
        const inner = (r.wc.graph.root.body as any).element
        expect(inner?.body.tag).toBe(c.elemType)
        // Decoded back into the right TypedArray subclass.
        expect(r.back).toBeInstanceOf(c.ctor)
        expect(Array.from(r.back as Iterable<unknown>)).toEqual(
          Array.from(c.value as unknown as Iterable<unknown>),
        )
        const wire = yield* compile(c.schema as Schema.Codec<any, any, never, never>)
        const expected = schemaValueToWit(r.sv)
        expect(yield* wire.encode(c.value)).toEqual(expected)
        expect(yield* wire.encodeAsync(c.value)).toEqual(expected)
        const decoded = yield* wire.decode(expected)
        expect(decoded).toBeInstanceOf(c.ctor)
        expect(decoded).toEqual(c.value)
      }),
    )
  }

  for (const length of [0, 1, 255, 256, 10000]) {
    it.effect(`preserves bytes and full-width wire indices at length ${length}`, () =>
      Effect.gen(function* () {
        const schema = Schema.Struct({ prefix: Schema.Array(Uint8), bytes: Uint8ArraySchema })
        const wire = yield* compile(schema)
        const value = {
          prefix: Array.from({ length: 300 }, (_, i) => i % 256),
          bytes: Uint8Array.from({ length }, (_, i) => (i * 37 + 19) % 256),
        }
        const model = yield* toWitCodec(schema)
        const expected = schemaValueToWit(yield* Schema.encodeEffect(model.codec)(value))
        const encoded = yield* wire.encodeAsync(value)
        expect(encoded).toEqual(expected)
        expect(yield* wire.decode(encoded)).toEqual(value)
      }),
    )
  }

  it.effect("rejects wrong array kinds and malformed wire scalars", () =>
    Effect.gen(function* () {
      const wire = yield* compile(Uint8ArraySchema)
      for (const input of [[1, 2], new Int8Array([1, 2]), "abc"]) {
        expect(
          yield* Effect.result(wire.encodeAsync(input as unknown as Uint8Array)),
        ).toMatchObject({
          _tag: "Failure",
        })
      }
      expect(
        yield* Effect.result(
          wire.decode({
            valueNodes: [
              { tag: "u8-value", val: 256 },
              { tag: "list-value", val: [0] },
            ],
            root: 1,
          }),
        ),
      ).toMatchObject({ _tag: "Failure" })
    }),
  )

  it.effect("typed array nested inside a struct round-trips", () =>
    Effect.gen(function* () {
      const S = Schema.Struct({
        tag: Schema.String,
        bytes: Uint8ArraySchema,
      })
      const v = { tag: "x", bytes: new Uint8Array([7, 8, 9]) }
      const r = yield* roundtrip(S, v)
      expect((r.back as any).tag).toBe("x")
      expect(r.back).toMatchObject({ tag: "x" })
      expect(Array.from((r.back as any).bytes as Uint8Array)).toEqual([7, 8, 9])
    }),
  )
})
