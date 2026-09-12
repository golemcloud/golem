import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import * as GolemSchema from "../src/Schema.js"
import { toWitCodec } from "../src/WitCodec.js"

const roundTrip = <S extends Schema.Codec<any, any, never, never>>(schema: S, value: S["Type"]) =>
  Effect.gen(function* () {
    const compiled = yield* toWitCodec(schema)
    const encoded = yield* Schema.encodeEffect(compiled.codec)(value)
    return { compiled, decoded: yield* Schema.decodeEffect(compiled.codec)(encoded) }
  })

describe("Effect schema compiler 1.6", () => {
  it.effect("preserves self-recursive object identity as one definition", () =>
    Effect.gen(function* () {
      interface Tree {
        readonly value: number
        readonly children: ReadonlyArray<Tree>
      }
      const Tree: Schema.Codec<Tree, Tree> = Schema.Struct({
        value: Schema.Number,
        children: Schema.Array(Schema.suspend(() => Tree)),
      }).pipe(Schema.annotate({ title: "Tree" }))

      const value: Tree = { value: 1, children: [{ value: 2, children: [] }] }
      const compiled = yield* toWitCodec(Tree)
      expect(compiled.graph.root.body.tag).toBe("ref")
      expect(compiled.graph.defs.size).toBe(1)
      const encoded = yield* Schema.encodeEffect(compiled.codec)(value)
      const decoded = yield* Schema.decodeEffect(compiled.codec)(encoded)
      expect(decoded).toEqual(value)
    }),
  )

  it.effect("closes mutually recursive schemas", () =>
    Effect.gen(function* () {
      interface A {
        readonly name: string
        readonly b: B | null
      }
      interface B {
        readonly count: number
        readonly a: A | null
      }
      const A: Schema.Codec<A, A> = Schema.Struct({
        name: Schema.String,
        b: Schema.NullOr(Schema.suspend(() => B)),
      })
      const B: Schema.Codec<B, B> = Schema.Struct({
        count: Schema.Number,
        a: Schema.NullOr(Schema.suspend(() => A)),
      })
      const value: A = { name: "root", b: { count: 1, a: null } }
      const { compiled, decoded } = yield* roundTrip(A, value)
      expect(compiled.graph.defs.size).toBeGreaterThan(0)
      expect(decoded).toEqual(value)
    }),
  )

  it.effect("emits rich nodes, maps, fixed lists, and metadata", () =>
    Effect.gen(function* () {
      const Rich = GolemSchema.Struct({
        path: GolemSchema.Path({ kind: "file", allowedExtensions: ["txt"] }),
        quantity: GolemSchema.Quantity({ baseUnit: "kg", allowedSuffixes: ["kg", "g"] }),
        text: GolemSchema.Text({ languages: ["en"] }),
        binary: GolemSchema.Binary({ mimeTypes: ["application/octet-stream"] }),
        url: GolemSchema.Url({ allowedSchemes: ["https"] }),
        datetime: GolemSchema.Datetime,
        duration: GolemSchema.NanosecondDuration,
        fixed: GolemSchema.FixedList(GolemSchema.String, 2),
        map: GolemSchema.Map(GolemSchema.String, GolemSchema.Number),
      }).pipe(GolemSchema.metadata({ doc: "rich record", aliases: ["Rich"] }))
      const value = {
        path: "a.txt",
        quantity: { mantissa: 12n, scale: 1, unit: "kg" },
        text: "hello",
        binary: new Uint8Array([1, 2]),
        url: "https://golem.cloud",
        datetime: { seconds: 1n, nanoseconds: 2 },
        duration: 3n,
        fixed: ["a", "b"],
        map: new Map([["x", 1]]),
      }
      const { compiled, decoded } = yield* roundTrip(Rich, value)
      expect(compiled.graph.root.metadata.doc).toBe("rich record")
      expect(compiled.graph.root.body.tag).toBe("record")
      if (compiled.graph.root.body.tag !== "record") return
      expect(compiled.graph.root.body.fields.map((field) => field.body.body.tag)).toEqual([
        "path",
        "quantity",
        "text",
        "binary",
        "url",
        "datetime",
        "duration",
        "fixed-list",
        "map",
      ])
      expect(decoded).toEqual(value)
    }),
  )
})
