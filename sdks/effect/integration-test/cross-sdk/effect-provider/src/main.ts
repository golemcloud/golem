import { Effect, Ref, Result, Schema, Stream } from "effect"
import {
  AgentStream,
  defineAgent,
  defineConfig,
  method,
  Quota,
  Schema as GolemSchema,
  Snapshot,
  Tool,
  Unstructured,
  WitTypes,
} from "@golemcloud/effect-golem"

class FixtureConfig extends defineConfig("EffectFixture.Config", {
  prefix: Schema.String,
}) {}

const Request = Schema.Struct({
  header: Schema.Struct({ requestId: Schema.String, flags: Schema.Array(Schema.Boolean) }),
  items: Schema.Array(
    Schema.Struct({ sku: Schema.String, quantities: Schema.Array(Schema.Number) }),
  ),
})

const Response = Schema.Struct({
  summary: Schema.String,
  accepted: Schema.Array(Schema.Struct({ sku: Schema.String, total: Schema.Number })),
  audit: Schema.Struct({ requestId: Schema.String, itemCount: Schema.Number }),
})

const FixtureError = Schema.Struct({ code: Schema.String, requestId: Schema.String })
const StreamItem = Schema.Struct({ id: Schema.Number, values: Schema.Array(Schema.Number) })

interface RecursiveNode {
  readonly label: string
  readonly children: ReadonlyArray<RecursiveNode>
}
const RecursiveNode: Schema.Codec<RecursiveNode> = Schema.Struct({
  label: Schema.String,
  children: Schema.Array(Schema.suspend(() => RecursiveNode)),
})

const RichChoice = GolemSchema.DiscriminatedUnion([
  {
    tag: "name",
    schema: Schema.String.pipe(Schema.check(Schema.isStartsWith("name:"))),
    discriminator: { tag: "prefix", val: "name:" },
  },
  {
    tag: "point",
    schema: Schema.Struct({ kind: Schema.Literal("point"), x: Schema.Number }),
    discriminator: { tag: "field-equals", val: { fieldName: "kind", literal: "point" } },
  },
])

const RichCorpus = Schema.Struct({
  u8: WitTypes.Uint8,
  u16: WitTypes.Uint16,
  u32: WitTypes.Uint32,
  u64: WitTypes.Uint64,
  s8: WitTypes.Int8,
  s16: WitTypes.Int16,
  s32: WitTypes.Int32,
  s64: WitTypes.Int64,
  f32: WitTypes.Float32,
  f64: Schema.Number,
  char: WitTypes.Char,
  enumValue: Schema.Literals(["amber", "violet", "cyan"]),
  flags: GolemSchema.Flags(["read", "write", "admin"]),
  variant: Schema.Union([Schema.String, Schema.Number]),
  tuple: Schema.Tuple([Schema.String, WitTypes.Int32, Schema.Boolean]),
  list: Schema.Array(WitTypes.Uint16),
  fixed: GolemSchema.FixedList(Schema.String, 2),
  map: GolemSchema.Map(Schema.String, WitTypes.Int32),
  option: Schema.NullOr(Schema.String),
  result: Schema.Result(WitTypes.Int32, Schema.String),
  path: GolemSchema.Path({ direction: "input", kind: "file", allowedExtensions: ["txt"] }),
  url: GolemSchema.Url({ allowedSchemes: ["https"] }),
  datetime: GolemSchema.Datetime,
  duration: GolemSchema.NanosecondDuration,
  choice: RichChoice,
})

Tool.toolDefinition("effect-cross-streaming")
  .body((body) =>
    body
      .positional("label", Schema.String)
      .input({ required: true })
      .output({ required: true })
      .returns(Schema.String),
  )
  .implement({
    effectCrossStreaming: ({ label }, context) =>
      Effect.gen(function* () {
        if (!context.stdin || !context.stdout) return yield* Effect.die("required streams missing")
        yield* context.stdout(
          Stream.concat(
            Stream.succeed(new TextEncoder().encode(`effect:${label}:`)),
            context.stdin.pipe(
              Stream.decodeText(),
              Stream.map((chunk) => new TextEncoder().encode(chunk.toUpperCase())),
            ),
          ),
        )
        return `effect-ok:${label}`
      }),
  })

defineAgent({
  name: "EffectFixture",
  id: { tenant: Schema.String },
  config: FixtureConfig,
  methods: {
    transform: method({ input: { request: Request }, success: Response, error: FixtureError }),
    transformStream: method({
      input: {
        request: Schema.Struct({
          prefix: Schema.String,
          items: GolemSchema.AgentStream(StreamItem),
        }),
      },
      success: Schema.Struct({ items: GolemSchema.AgentStream(StreamItem) }),
    }),
    reserveForwardedQuota: method({
      input: { token: Quota.QuotaTokenSchema },
      success: Quota.QuotaTokenSchema,
    }),
    echoPermissionCard: method({
      input: { card: WitTypes.PermissionCard({ polymorphic: false }) },
      success: WitTypes.PermissionCard({ polymorphic: false }),
    }),
  },
}).implement(({ tenant }) =>
  Effect.gen(function* () {
    const config = yield* FixtureConfig
    const prefix = yield* config.prefix
    return {
      transform: ({ request }) => {
        if (request.items.length === 0) {
          return Effect.fail({ code: "EMPTY_ITEMS", requestId: request.header.requestId })
        }
        return Effect.succeed({
          summary: `${prefix}:${tenant}:${request.header.flags.length}`,
          accepted: request.items.map((item) => ({
            sku: item.sku,
            total: item.quantities.reduce((left, right) => left + right, 0),
          })),
          audit: { requestId: request.header.requestId, itemCount: request.items.length },
        })
      },
      transformStream: ({ request }) =>
        Effect.map(
          AgentStream.AgentStream.fromEffect(
            request.items.toEffect(String).pipe(
              Stream.map((item) => ({
                id: item.id * 10,
                values: item.values.map((value) => value + request.prefix.length),
              })),
            ),
          ),
          (items) => ({ items }),
        ),
      reserveForwardedQuota: ({ token }) =>
        Quota.withReservation(token, 1n, () => Effect.succeed({ used: 1n, value: token })),
      echoPermissionCard: ({ card }) => Effect.succeed(card),
    }
  }),
)

defineAgent({
  name: "EffectRichFixture",
  id: { tenant: Schema.String },
  methods: {
    transformRichCorpus: method({ input: { corpus: RichCorpus }, success: RichCorpus }),
  },
}).implement(() =>
  Effect.succeed({
    transformRichCorpus: ({ corpus }) =>
      Effect.succeed({
        ...corpus,
        u8: corpus.u8 + 1,
        u64: corpus.u64 - 2n,
        s64: corpus.s64 + 3n,
        char: "λ",
        enumValue: "cyan" as const,
        flags: [corpus.flags[0], !corpus.flags[1], true] as const,
        tuple: [`${corpus.tuple[0]}!`, corpus.tuple[1] - 4, !corpus.tuple[2]] as const,
        list: corpus.list.map((value) => value + 10),
        fixed: [corpus.fixed[1], corpus.fixed[0]],
        map: new Map([...corpus.map].map(([key, value]) => [`${key}!`, value * -1])),
        option: corpus.option === null ? "effect-option" : null,
        result: Result.isSuccess(corpus.result)
          ? Result.fail(`effect:${corpus.result.success}`)
          : Result.succeed(corpus.result.failure.length),
        path: `effect-${corpus.path}`,
        duration: corpus.duration + 9n,
        choice:
          typeof corpus.choice === "string" ? { kind: "point" as const, x: 17 } : "name:effect",
      }),
  }),
)

const SnapshotFixture = defineAgent({
  name: "EffectSnapshotFixture",
  id: { tenant: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ value: Schema.Number }),
    policy: Snapshot.policy.everyN(2),
  }),
  methods: {
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
    value: method({ input: {}, success: Schema.Number }),
  },
})

const snapshotFactory: Parameters<typeof SnapshotFixture.implement>[0] = (_id, snapshotting) =>
  Effect.gen(function* () {
    const state = yield* snapshotting.init({ value: 0 })
    return {
      add: ({ by }) =>
        Ref.updateAndGet(state, ({ value }) => ({ value: value + by })).pipe(
          Effect.map(({ value }) => value),
        ),
      value: () => Ref.get(state).pipe(Effect.map(({ value }) => value)),
    }
  })

SnapshotFixture.implement(snapshotFactory, (_restoration, ...args) => snapshotFactory(...args))

defineAgent({
  name: "EffectSchemaFixture",
  id: { tenant: Schema.String },
  methods: {
    echoText: method({
      input: {
        value: Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
      },
      success: Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
    }),
    echoBinary: method({
      input: {
        value: Unstructured.UnstructuredBinary({
          restrictions: [{ mimeType: "application/octet-stream" }],
        }),
      },
      success: Unstructured.UnstructuredBinary({
        restrictions: [{ mimeType: "application/octet-stream" }],
      }),
    }),
    echoQuantity: method({
      input: {
        value: GolemSchema.Quantity({ baseUnit: "kg", allowedSuffixes: ["kg", "g"] }),
      },
      success: GolemSchema.Quantity({ baseUnit: "kg", allowedSuffixes: ["kg", "g"] }),
    }),
    echoRecursive: method({ input: { value: RecursiveNode }, success: RecursiveNode }),
  },
}).implement(() =>
  Effect.succeed({
    echoText: ({ value }) => Effect.succeed(value),
    echoBinary: ({ value }) => Effect.succeed(value),
    echoQuantity: ({ value }) => Effect.succeed(value),
    echoRecursive: ({ value }) => Effect.succeed(value),
  }),
)
