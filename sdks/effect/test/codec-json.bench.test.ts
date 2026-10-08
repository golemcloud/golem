import { expect, it } from "vitest"
import { Effect, Schema } from "effect"
import { compileJson, compile } from "../src/WitCodec.js"
import { SchemaRef } from "../src/SchemaRef.js"
import { Uint8 } from "../src/WitTypes.js"

for (const size of [100, 10000]) {
  it.skipIf(!process.env.CODEC_BENCH)(
    `canonical JSON ${size} elements`,
    async () => {
      const schema = Schema.Struct({ bytes: Schema.Array(Uint8) })
      const codec = Effect.runSync(compileJson(schema))
      const wire = Effect.runSync(compile(schema))
      const ref = new SchemaRef(wire.schemaGraph)
      const value = { bytes: Array.from({ length: size }, (_, i) => i % 256) }
      const packed = ref.packJson(value)
      const operations = {
        packJson: () => ref.packJson(value),
        unpackJson: () => ref.unpackJson(packed),
        compileJsonDecode: () => Effect.runSync(codec.decode(value)),
        wireDecode: () => Effect.runSync(wire.decode(ref.packJson(value))),
        compileJsonEncode: () => Effect.runPromise(codec.encode(value)),
        wireEncode: async () => ref.unpackJson(await Effect.runPromise(wire.encodeAsync(value))),
      }
      expect(operations.compileJsonDecode()).toEqual(value)
      expect(await operations.compileJsonEncode()).toEqual(value)
      for (const [name, operation] of Object.entries(operations)) {
        for (let i = 0; i < 50; i++) await operation()
        const samples = []
        for (let batch = 0; batch < 7; batch++) {
          const start = performance.now()
          for (let i = 0; i < 100; i++) await operation()
          samples.push((performance.now() - start) * 10)
        }
        console.log(JSON.stringify({ size, name, unit: "µs/op", samples }))
      }
    },
    60000,
  )
}
