import { it } from "vitest"
import { Effect, Schema } from "effect"
import { compileMethodSpec, invokeMethod, method } from "../src/Method.js"

// These benchmark schemas require no services; the method boundary erases that type.
const runSync = <A, E>(effect: Effect.Effect<A, E, any>) =>
  Effect.runSync(effect as Effect.Effect<A, E>)
const runPromise = <A, E>(effect: Effect.Effect<A, E, any>) =>
  Effect.runPromise(effect as Effect.Effect<A, E>)

for (const size of [1, 64]) {
  const item = Schema.Struct({ name: Schema.String, count: Schema.Number })
  const compiled = Effect.runSync(
    compileMethodSpec(
      "echo",
      method({ input: { items: Schema.Array(item) }, success: Schema.Array(item) }),
    ),
  )
  const value = Array.from({ length: size }, (_, i) => ({ name: `item-${i}`, count: i + 7 }))
  const input = runSync(compiled.inputCodec.encode({ items: value }))
  const operations = {
    decode: () => runSync(compiled.inputCodec.decode(input)),
    encode: () => runPromise(compiled.encodeOutput!(value)),
    invoke: () => runPromise(invokeMethod(compiled, ({ items }) => Effect.succeed(items), input)),
  }
  for (const [name, operation] of Object.entries(operations)) {
    it.skipIf(!process.env.CODEC_BENCH)(
      `${size} records: ${name}`,
      async () => {
        for (let i = 0; i < 1000; i++) await operation()
        const samples = []
        for (let batch = 0; batch < 7; batch++) {
          const start = performance.now()
          for (let i = 0; i < 2000; i++) await operation()
          samples.push(((performance.now() - start) * 1000) / 2000)
        }
        console.log(JSON.stringify({ size, name, unit: "µs/op", samples }))
      },
      60000,
    )
  }
}
