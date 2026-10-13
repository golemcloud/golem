import { describe, expect, it, vi } from "vitest"
import { Context, Effect, HashMap, Option, Result, Schema, SchemaGetter } from "effect"
import { compile, toWitCodec } from "../src/WitCodec.js"
import { compileParamBindings } from "../src/Method.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import * as Wire from "../src/internal/schema-model/wit.js"
import * as WitTypes from "../src/WitTypes.js"
import { Uint8 } from "../src/WitTypes.js"

describe("structural wire codecs", () => {
  const relocationCases: Array<readonly [string, Schema.Codec<any, any, never, never>, any]> = [
    ["tuple", Schema.Tuple([Schema.String]), ["tuple-child"]],
    ["fixed-list", WitTypes.FixedList(Schema.String, 2), ["first", "second"]],
    ["map", Schema.ReadonlyMap(Schema.String, Schema.String), new Map([["key", "value"]])],
    ["result-ok", Schema.Result(Schema.String, Schema.String), Result.succeed("success")],
    ["result-err", Schema.Result(Schema.String, Schema.String), Result.fail("failure")],
    [
      "nested",
      Schema.Tuple([Schema.Array(Schema.Option(Schema.String))]),
      [[Option.some("nested-child")]],
    ],
  ]
  for (const [name, schema, items] of relocationCases) {
    for (const async of [false, true]) {
      it(`relocates ${name} after a preceding parameter during ${async ? "async" : "sync"} encoding`, async () => {
        const compiled = Effect.runSync(
          compileParamBindings("relocation", { prefix: Schema.String, items: schema }),
        )
        const value = { prefix: "distinct-prefix", items } as any
        const withoutServices = <A, E>(effect: Effect.Effect<A, E, any>) =>
          effect as Effect.Effect<A, E>
        const ordinary = schemaValueToWit(
          Effect.runSync(withoutServices(Schema.encodeEffect(compiled.codec)(value))),
        )
        const encoded = async
          ? await Effect.runPromise(withoutServices(compiled.encodeAsync(value)))
          : Effect.runSync(withoutServices(compiled.encode(value)))
        expect(encoded).toEqual(ordinary)
        expect(Effect.runSync(withoutServices(compiled.decode(encoded)))).toEqual(value)
      })
    }
  }

  it("directly encodes and decodes option, enum, tuple, result, map and native leaves", async () => {
    const cases: Array<readonly [Schema.Codec<any, any, never, never>, any]> = [
      [Schema.NullOr(Schema.String), null],
      [Schema.NullOr(Schema.String), "some"],
      [Schema.UndefinedOr(Schema.String), undefined],
      [Schema.Option(Schema.String), Option.none()],
      [Schema.Option(Schema.String), Option.some("some")],
      [Schema.Enum({ A: 200, B: 404 }), 404],
      [Schema.Tuple([Schema.String, Schema.Number]), ["first", 42]],
      [
        Schema.Tuple([Schema.String, Schema.optionalKey(Schema.UndefinedOr(Schema.String))]),
        ["first"],
      ],
      [
        Schema.Tuple([Schema.String, Schema.optionalKey(Schema.UndefinedOr(Schema.String))]),
        ["first", "second"],
      ],
      [Schema.Result(Schema.Number, Schema.String), Result.succeed(42)],
      [Schema.Result(Schema.Number, Schema.String), Result.fail("error")],
      [
        Schema.ReadonlyMap(Schema.String, Schema.Number),
        new Map([
          ["a", 42],
          ["b", 7],
        ]),
      ],
      [
        Schema.HashMap(Schema.String, Schema.Number),
        HashMap.fromIterable([
          ["a", 42],
          ["b", 7],
        ]),
      ],
      [WitTypes.Map(Schema.String, Schema.Number), new Map([["a", 42]])],
      [WitTypes.FixedList(Schema.String, 2), ["a", "b"]],
      [WitTypes.Flags(["a", "b"]), [true, false]],
      [WitTypes.Text(), "rich"],
      [WitTypes.Binary(), new Uint8Array([0, 255])],
      [WitTypes.Path(), "/a"],
      [WitTypes.Url(), "https://example.com"],
      [
        Schema.Struct({ highBits: Schema.BigInt, lowBits: Schema.BigInt }).pipe(
          Schema.annotate({ [WitTypes.witSchemaNodeAnnotationKey]: { tag: "uuid" } }),
        ),
        { highBits: 1n, lowBits: 2n },
      ],
      [WitTypes.Datetime, { seconds: -1n, nanoseconds: 999999999 }],
      [WitTypes.Duration, -42n],
      [
        WitTypes.Quantity({ baseUnit: "m", allowedSuffixes: [] }),
        { mantissa: -42n, scale: 2, unit: "m" },
      ],
    ]
    for (const [schema, value] of cases) {
      const compiled = Effect.runSync(compile(schema))
      const model = Effect.runSync(toWitCodec(schema))
      const ordinary = schemaValueToWit(Effect.runSync(Schema.encodeEffect(model.codec)(value)))
      const expected = Effect.runSync(
        Schema.decodeEffect(model.codec)(schemaValueFromWit(ordinary)),
      )
      const encodeSpy = vi.spyOn(Wire, "schemaValueToWit")
      const asyncSpy = vi.spyOn(Wire, "schemaValueToWitAsync")
      const decodeSpy = vi.spyOn(Wire, "schemaValueFromWit")
      try {
        expect(Effect.runSync(compiled.encode(value))).toEqual(ordinary)
        expect(await Effect.runPromise(compiled.encodeAsync(value))).toEqual(ordinary)
        expect(Effect.runSync(compiled.decode(ordinary))).toEqual(expected)
        expect(encodeSpy).not.toHaveBeenCalled()
        expect(asyncSpy).not.toHaveBeenCalled()
        expect(decodeSpy).not.toHaveBeenCalled()
      } finally {
        vi.restoreAllMocks()
      }
    }
  })

  it("rejects invalid native boundaries and malformed composite payloads", () => {
    const duration = Effect.runSync(compile(WitTypes.Duration))
    expect(Effect.runSync(Effect.result(duration.encode(1n << 63n)))._tag).toBe("Failure")
    const datetime = Effect.runSync(compile(WitTypes.Datetime))
    expect(
      Effect.runSync(Effect.result(datetime.encode({ seconds: 0n, nanoseconds: 1000000000 })))._tag,
    ).toBe("Failure")
    const cases = [
      [Schema.Tuple([Schema.String, Schema.Number]), { tag: "tuple-value", val: [] }],
      [WitTypes.FixedList(Schema.String, 2), { tag: "fixed-list-value", val: [] }],
      [WitTypes.Flags(["a", "b"]), { tag: "flags-value", val: [true] }],
      [Schema.Enum({ A: 200 }), { tag: "enum-value", val: 1 }],
      [
        Schema.Result(Schema.Number, Schema.String),
        { tag: "result-value", val: { tag: "ok-value", val: undefined } },
      ],
      [Schema.Option(Schema.String), { tag: "option-value", val: 0 }],
    ] as const
    for (const [schema, node] of cases) {
      const compiled = Effect.runSync(compile(schema))
      expect(
        Effect.runSync(Effect.result(compiled.decode({ valueNodes: [node as any], root: 0 })))._tag,
      ).toBe("Failure")
    }
  })

  it("matches ordinary conversion for missing properties and asymmetric nested values", async () => {
    const schema = Schema.Struct({
      records: Schema.Array(
        Schema.Struct({
          bytes: Schema.Array(Uint8),
          label: Schema.optionalKey(Schema.String),
        }),
      ),
    })
    const compiled = Effect.runSync(compile(schema))
    const model = Effect.runSync(toWitCodec(schema))
    for (const value of [
      { records: [{ bytes: [7, 255] }, { bytes: [31], label: "present" }] },
      { records: [] },
    ]) {
      const ordinary = schemaValueToWit(Effect.runSync(Schema.encodeEffect(model.codec)(value)))
      expect(await Effect.runPromise(compiled.encodeAsync(value))).toEqual(ordinary)
      expect(Effect.runSync(compiled.decode(ordinary))).toEqual(
        Effect.runSync(Schema.decodeEffect(model.codec)(schemaValueFromWit(ordinary))),
      )
    }
    const valid = await Effect.runPromise(compiled.encodeAsync({ records: [] }))
    const malformed = {
      ...valid,
      valueNodes: [...valid.valueNodes, { tag: "u8-value" as const, val: 3 }],
    }
    expect(Effect.runSync(Effect.result(compiled.decode(malformed)))).toMatchObject({
      _tag: "Failure",
    })
  })

  it("does not cache services or rerun stateful transforms between invocations", async () => {
    class Suffix extends Context.Service<Suffix, { readonly value: string }>()("test/WireSuffix") {}
    let decoded = 0
    let encoded = 0
    const label = Schema.String.pipe(
      Schema.decodeTo(Schema.String, {
        decode: SchemaGetter.transformEffect((value) =>
          Effect.map(Suffix, (suffix) => {
            decoded++
            return `${value}${suffix.value}`
          }),
        ),
        encode: SchemaGetter.transform((value) => {
          encoded++
          return value.slice(0, -1)
        }),
      }),
    )
    const compiled = Effect.runSync(compile(Schema.Struct({ labels: Schema.Array(label) })))
    const wire = await Effect.runPromise(compiled.encodeAsync({ labels: ["a!", "b!"] }))
    for (const suffix of ["!", "?"]) {
      expect(
        await Effect.runPromise(
          compiled.decode(wire).pipe(Effect.provideService(Suffix, { value: suffix })),
        ),
      ).toEqual({ labels: [`a${suffix}`, `b${suffix}`] })
    }
    expect(decoded).toBe(4)
    expect(encoded).toBe(2)
  })
})

for (const size of [100, 10000]) {
  it.skipIf(!process.env.CODEC_BENCH)(
    `structural wire profile ${size}`,
    async () => {
      const schema = Schema.Struct({ bytes: Schema.Array(Uint8) })
      const wire = Effect.runSync(compile(schema))
      const model = Effect.runSync(toWitCodec(schema))
      const value = { bytes: Array.from({ length: size }, (_, i) => i % 256) }
      const tree = schemaValueToWit(Effect.runSync(Schema.encodeEffect(model.codec)(value)))
      const operations = {
        ordinaryEncode: () =>
          schemaValueToWit(Effect.runSync(Schema.encodeEffect(model.codec)(value))),
        fusedEncode: () => Effect.runSync(wire.encode(value)),
        ordinaryDecode: () =>
          Effect.runSync(Schema.decodeEffect(model.codec)(schemaValueFromWit(tree))),
        fusedDecode: () => Effect.runSync(wire.decode(tree)),
      }
      expect(operations.fusedEncode()).toEqual(tree)
      expect(operations.fusedDecode()).toEqual(value)
      for (const [name, operation] of Object.entries(operations)) {
        for (let i = 0; i < 50; i++) operation()
        const samples = []
        for (let batch = 0; batch < 7; batch++) {
          const start = performance.now()
          for (let i = 0; i < 100; i++) operation()
          samples.push((performance.now() - start) * 10)
        }
        console.log(
          JSON.stringify({ size, name, unit: "µs/op", nodes: tree.valueNodes.length, samples }),
        )
      }
    },
    60000,
  )
}
