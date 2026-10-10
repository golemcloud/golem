import { describe, expect, it } from "vitest"
import { Context, Effect, Schema, SchemaGetter } from "effect"
import { compile, toWitCodec } from "../src/WitCodec.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { Uint8 } from "../src/WitTypes.js"

describe("structural wire codecs", () => {
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
