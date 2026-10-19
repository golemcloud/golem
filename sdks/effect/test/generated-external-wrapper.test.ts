import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"
import { Effect, Stream } from "effect"
import ts from "typescript"

// Exercise the generator's runtime source, not a hand-maintained copy of it.
const generator = readFileSync(
  new URL("../../../cli/golem-cli/src/bridge_gen/effect/effect_external.rs", import.meta.url),
  "utf8",
)
const runtime = generator.split('const RUNTIME: &str = r#"')[1]!.split('"#;')[0]!
const javascript = ts.transpileModule(
  runtime
    .replace('import { Effect, Stream } from "effect"', "")
    .replaceAll("export const", "const"),
  {
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ES2020 },
  },
).outputText
const { fromEffectValue, toEffectValue } = new Function(
  "Effect",
  "Stream",
  `${javascript}\nreturn { fromEffectValue, toEffectValue }`,
)(Effect, Stream) as {
  fromEffectValue: (value: any, schema: any, graph: any, names?: any) => Effect.Effect<any, unknown>
  toEffectValue: (value: any, schema: any, graph: any, names?: any) => any
}

describe("generated Effect external wrapper runtime", () => {
  it("converts nested renamed fields, options, maps and stream elements", async () => {
    const scalar = { kind: "u8" }
    const list = { kind: "list", value: { element: scalar } }
    const record = {
      kind: "record",
      value: {
        fields: [
          { name: "raw-field", body: list },
          { name: "stream", body: { kind: "stream", value: { inner: list } } },
          { name: "optional", body: { kind: "option", value: { inner: list } } },
          { name: "map", body: { kind: "map", value: { key: scalar, value: list } } },
        ],
      },
    }
    const graph = { root: record }
    const names = {
      '["raw-field","stream","optional","map"]': ["rawField", "stream", "optional", "map"],
    }
    const input = {
      rawField: [7, 255],
      stream: Stream.make([3, 31]),
      optional: undefined,
      map: new Map([[9, [31, 7]]]),
    }
    const transport = await Effect.runPromise(fromEffectValue(input, record, graph, names))
    expect(transport.rawField).toEqual([7, 255])
    expect(transport.rawField).not.toBe(input.rawField)
    const output = toEffectValue(transport, record, graph, names)
    expect(await Effect.runPromise(Stream.runCollect(output.stream))).toEqual([[3, 31]])
    expect(output.optional).toBeUndefined()
    expect(output.map).toEqual(input.map)
  })

  it("closes the transport iterator when a decoded stream is interrupted", async () => {
    let closed = 0
    const input = {
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: false, value: 23 }),
        return: async () => {
          closed++
          return { done: true, value: undefined }
        },
      }),
    }
    const schema = { kind: "stream", value: { inner: { kind: "u8" } } }
    const stream = toEffectValue(input, schema, { root: schema })
    expect(await Effect.runPromise(Stream.runCollect(Stream.take(stream, 1)))).toEqual([23])
    expect(closed).toBe(1)
  })

  for (const size of [100, 10000]) {
    it.skipIf(!process.env.CODEC_BENCH)(
      `scalar-list facade ${size}`,
      async () => {
        const schema = { kind: "list", value: { element: { kind: "u8" } } }
        const value = Array.from({ length: size }, (_, i) => i % 256)
        const graph = { root: schema }
        const operations = {
          fromEffectValue: () => Effect.runPromise(fromEffectValue(value, schema, graph)),
          toEffectValue: () => toEffectValue(value, schema, graph),
        }
        for (const [name, operation] of Object.entries(operations)) {
          expect(await operation()).toEqual(value)
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
})
