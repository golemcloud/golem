import { describe, expect, it } from "@effect/vitest"
import { Effect, Result, Schema } from "effect"
import type {
  PermissionCard as RawPermissionCard,
  QuotaToken as RawQuotaToken,
  Secret as RawSecret,
} from "golem:core/types@2.0.0"
import * as GolemSchema from "../src/Schema.js"
import { toWitCodec } from "../src/WitCodec.js"
import { field, t, v, type SchemaGraph } from "../src/internal/schema-model/model.js"
import { createGuestPermissionCardHandle } from "../src/internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "../src/internal/schema-model/permissionCardInternal.js"
import { createGuestQuotaTokenHandle } from "../src/internal/schema-model/quotaTokenHandle.js"
import { QUOTA_INTERNAL } from "../src/internal/schema-model/quotaInternal.js"
import { createGuestSecretHandle } from "../src/internal/schema-model/secretHandle.js"
import { SECRET_INTERNAL } from "../src/internal/schema-model/secretInternal.js"

// This is an intentionally asymmetric oracle. The expected graph/value literals are the
// canonical forms independently exercised by the TS SDK's schema markers and sdk.test.ts; they
// are not produced by effect-golem's compiler or inferred from its output.
describe("Effect authoring to TypeScript canonical schema oracle", () => {
  it.effect("compiles and encodes the asymmetric rich all-types corpus", () =>
    Effect.gen(function* () {
      const Choice = GolemSchema.DiscriminatedUnion([
        {
          tag: "name",
          schema: Schema.String.pipe(Schema.check(Schema.isStartsWith("name:"))),
          discriminator: { tag: "prefix", val: "name:" },
        },
        {
          tag: "point",
          schema: Schema.Struct({ kind: Schema.Literal("point"), x: Schema.Number }),
          discriminator: {
            tag: "field-equals",
            val: { fieldName: "kind", literal: "point" },
          },
        },
      ])
      const All = Schema.Struct({
        flags: GolemSchema.Flags(["read", "write"]),
        fixed: GolemSchema.FixedList(Schema.String, 2),
        map: GolemSchema.Map(Schema.String, Schema.Boolean),
        result: Schema.Result(Schema.Number, Schema.String),
        choice: Choice,
        text: GolemSchema.Text({ languages: ["en"], minLength: 1, maxLength: 20 }),
        binary: GolemSchema.Binary({ mimeTypes: ["image/png"], maxBytes: 4 }),
        path: GolemSchema.Path({
          direction: "input",
          kind: "file",
          allowedExtensions: ["txt"],
        }),
        url: GolemSchema.Url({ allowedSchemes: ["https"] }),
        datetime: GolemSchema.Datetime,
        duration: GolemSchema.NanosecondDuration,
        quantity: GolemSchema.Quantity({ baseUnit: "kg", allowedSuffixes: ["kg", "g"] }),
      })
      const expectedGraph: SchemaGraph = {
        defs: new Map(),
        root: t.record([
          field("flags", t.flags(["read", "write"])),
          field("fixed", t.fixedList(t.string(), 2)),
          field("map", t.map(t.string(), t.bool())),
          field("result", t.result(t.f64(), t.string())),
          field(
            "choice",
            t.union([
              {
                tag: "name",
                body: t.string(),
                discriminator: { tag: "prefix", val: "name:" },
                metadata: { aliases: [], examples: [] },
              },
              {
                tag: "point",
                body: t.record([field("kind", t.string()), field("x", t.f64())]),
                discriminator: {
                  tag: "field-equals",
                  val: { fieldName: "kind", literal: "point" },
                },
                metadata: { aliases: [], examples: [] },
              },
            ]),
          ),
          field("text", t.text({ languages: ["en"], minLength: 1, maxLength: 20 })),
          field("binary", t.binary({ mimeTypes: ["image/png"], maxBytes: 4 })),
          field("path", t.path({ direction: "input", kind: "file", allowedExtensions: ["txt"] })),
          field("url", t.url({ allowedSchemes: ["https"] })),
          field("datetime", t.datetime()),
          field("duration", t.duration()),
          field("quantity", t.quantity({ baseUnit: "kg", allowedSuffixes: ["kg", "g"] })),
        ]),
      }
      const input = {
        flags: [true, false] as const,
        fixed: ["a", "b"],
        map: new Map([["enabled", true]]),
        result: Result.succeed(2.5),
        choice: "name:alice",
        text: "hello",
        binary: new Uint8Array([1, 2, 3]),
        path: "notes.txt",
        url: "https://golem.cloud",
        datetime: { seconds: 1_700_000_000n, nanoseconds: 123 },
        duration: 42n,
        quantity: { mantissa: 125n, scale: 1, unit: "kg" },
      }
      const expectedValue = v.record([
        v.flags([true, false]),
        v.fixedList([v.string("a"), v.string("b")]),
        v.map([{ key: v.string("enabled"), value: v.bool(true) }]),
        v.ok(v.f64(2.5)),
        v.union("name", v.string("name:alice")),
        v.text("hello"),
        v.binary(new Uint8Array([1, 2, 3])),
        v.path("notes.txt"),
        v.url("https://golem.cloud"),
        v.datetime({ seconds: 1_700_000_000n, nanoseconds: 123 }),
        v.duration(42n),
        v.quantity({ mantissa: 125n, scale: 1, unit: "kg" }),
      ])

      const compiled = yield* toWitCodec(All)
      expect(compiled.graph).toEqual(expectedGraph)
      expect(yield* Schema.encodeEffect(compiled.codec)(input)).toEqual(expectedValue)
    }),
  )

  it.effect("matches recursive refs and capability ownership values", () =>
    Effect.gen(function* () {
      interface Node {
        readonly label: string
        readonly next: Node | null
      }
      const Node: Schema.Codec<Node, Node> = Schema.Struct({
        label: Schema.String,
        next: Schema.NullOr(Schema.suspend(() => Node)),
      }).pipe(Schema.annotate({ title: "TsOracleNode" }))
      const recursive = yield* toWitCodec(Node)
      expect(recursive.graph.root.body.tag).toBe("ref")
      expect([...recursive.graph.defs.values()].map((definition) => definition.body.body)).toEqual([
        {
          tag: "record",
          fields: [field("label", t.string()), field("next", t.option(recursive.graph.root))],
        },
      ])
      expect(yield* Schema.encodeEffect(recursive.codec)({ label: "root", next: null })).toEqual(
        v.record([v.string("root"), v.option()]),
      )

      const quota = createGuestQuotaTokenHandle(QUOTA_INTERNAL, {} as RawQuotaToken)
      const card = createGuestPermissionCardHandle(
        PERMISSION_CARD_INTERNAL,
        {} as RawPermissionCard,
      )
      const Capabilities = Schema.Struct({
        quota: GolemSchema.QuotaToken({ resourceName: "calls" }),
        card: GolemSchema.PermissionCard({ polymorphic: false }),
        secret: GolemSchema.Secret(Schema.String, { category: "credential" }),
      })
      const secret = createGuestSecretHandle(SECRET_INTERNAL, {} as RawSecret)
      const capabilities = yield* toWitCodec(Capabilities)
      expect(capabilities.graph).toEqual({
        defs: new Map(),
        root: t.record([
          field("quota", t.quotaToken({ resourceName: "calls" })),
          field("card", t.permissionCard({ polymorphic: false })),
          field("secret", t.secret(t.string(), { category: "credential" })),
        ]),
      })
      expect(yield* Schema.encodeEffect(capabilities.codec)({ quota, card, secret })).toEqual(
        v.record([v.quotaToken(quota), v.permissionCard(card), v.secret(secret)]),
      )
    }),
  )
})
