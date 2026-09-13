import { Effect, Schema, Stream } from "effect"
import {
  AgentStream,
  defineAgent,
  method,
  Quota,
  Reflection,
  Schema as GolemSchema,
  Tool,
} from "@golemcloud/effect-golem"
import { TsCrossStreamingClient } from "ts-cross-streaming-tool-guest-client"
import { TsPeer as GeneratedTsPeer } from "ts-peer-guest-client"

const StreamItem = Schema.Struct({ id: Schema.Number, values: Schema.Array(Schema.Number) })
const tsTool = Tool.toolDefinition("ts-cross-streaming").body((body) =>
  body
    .positional("label", Schema.String)
    .input({ required: true })
    .output({ required: true })
    .returns(Schema.String),
)
const tsToolClient = Tool.client(tsTool)

const TsPeer = defineAgent({
  name: "TsPeer",
  id: { name: Schema.String },
  methods: {
    echo: method({
      input: { value: Schema.String },
      success: Schema.Struct({ language: Schema.String, value: Schema.String }),
    }),
    nestedStream: method({
      input: { prefix: Schema.String, items: GolemSchema.AgentStream(StreamItem) },
      success: Schema.Struct({ items: GolemSchema.AgentStream(StreamItem) }),
    }),
    markScheduled: method({ input: {}, success: Schema.Void }),
    scheduledCount: method({ input: {}, success: Schema.Number }),
    echoQuota: method({
      input: { token: Quota.QuotaTokenSchema },
      success: Quota.QuotaTokenSchema,
    }),
  },
})

const RustPeer = defineAgent({
  name: "RustPeer",
  id: { name: Schema.String },
  methods: { echo: method({ input: { value: Schema.String }, success: Schema.String }) },
})

defineAgent({
  name: "EffectConsumer",
  id: { name: Schema.String },
  methods: {
    roundTrip: method({ input: { value: Schema.String }, success: Schema.String }),
    reflectedRoundTrip: method({ input: { value: Schema.String }, success: Schema.String }),
    ephemeralRoundTrip: method({ input: { value: Schema.String }, success: Schema.String }),
    nonfiniteReflection: method({ input: {}, success: Schema.String }),
    nestedStreamRoundTrip: method({
      input: {},
      success: Schema.Struct({
        values: Schema.Array(StreamItem),
        closed: Schema.Boolean,
        stoppedEarly: Schema.Boolean,
      }),
    }),
    scheduledMetadata: method({ input: {}, success: Schema.String }),
    callTsTool: method({ input: { payload: Schema.String }, success: Schema.String }),
    toolRoundTrip: method({ input: { payload: Schema.String }, success: Schema.String }),
    quotaThroughTs: method({ input: {}, success: Schema.String }),
  },
}).implement(({ name }) =>
  Effect.succeed({
    roundTrip: ({ value }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const ts = yield* TsPeer.client.get({ name })
          const rust = yield* RustPeer.client.get({ name })
          const tsResult = yield* ts.echo({ value })
          const rustResult = yield* rust.echo({ value })
          return `${tsResult.language}:${tsResult.value}|${rustResult}`
        }),
      ),
    reflectedRoundTrip: ({ value }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const reflected = yield* Reflection.getAgentType("TsPeer")
          if (reflected === undefined || reflected.mode !== "durable") return "missing:TsPeer"
          const echo = reflected.method("echo")
          if (echo === undefined) return "missing:echo"
          if (!echo.input.validateJson({ value }).success) return "invalid:echo-input"
          const client = yield* reflected.client.get({ name })
          const method = yield* client.method("echo")
          const result = yield* method.invoke({ value })
          const output = result.value as { language: string; value: string }
          const byId = yield* Reflection.getAgentTypeByAgentId(result.metadata.agentId)
          const all = yield* Reflection.getAllAgentTypes
          if (byId?.name !== "TsPeer" || !all.some((type) => type.name === "RustPeer")) {
            return "discovery-mismatch"
          }
          return `${reflected.name}:${echo.name}:${output.language}:${output.value}`
        }),
      ),
    ephemeralRoundTrip: ({ value }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const reflected = yield* Reflection.getAgentType("TsEphemeralPeer")
          if (reflected === undefined || reflected.mode !== "ephemeral") return "missing:ephemeral"
          const client = yield* reflected.client.newPhantom({ request: name })
          const echo = yield* client.method("echo")
          const result = yield* echo.invoke({ value })
          return `${result.metadata.agentId}:${String(result.value)}`
        }),
      ),
    nonfiniteReflection: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const reports: string[] = []
          for (const peer of ["TsPeer", "RustPeer"]) {
            const reflected = yield* Reflection.getAgentType(peer)
            if (reflected === undefined || reflected.mode !== "durable") return `missing:${peer}`
            const client = yield* reflected.client.get({ name })
            const method = yield* client.method("nonfinite")
            const native = yield* method.invokeValue({
              root: 0,
              valueNodes: [
                { tag: "record-value", val: [1] },
                { tag: "string-value", val: "nan" },
              ],
            })
            const node = native.value?.valueNodes[native.value.root]
            const nativeNan = node?.tag === "f64-value" && Number.isNaN(node.val)
            const jsonRejected = yield* method.invoke({ kind: "nan" }).pipe(
              Effect.as(false),
              Effect.catch(() => Effect.succeed(true)),
            )
            reports.push(`${peer}:${nativeNan}:${jsonRejected}`)
          }
          return reports.join("|")
        }).pipe(Effect.orDie),
      ),
    nestedStreamRoundTrip: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const peer = yield* GeneratedTsPeer.get(name)
          let pulled = 0
          let readerClosed = false
          const source = Stream.fromIterable([
            { id: 1, values: [9, 12] },
            { id: 2, values: [20] },
            ...Array.from({ length: 2048 }, (_, id) => ({ id: id + 3, values: [99] })),
          ]).pipe(
            Stream.rechunk(1),
            Stream.tap(() => Effect.sync(() => pulled++)),
          )
          const items = yield* AgentStream.AgentStream.fromEffect(source)
          const output = yield* peer.nestedStream("fx", items)
          const values = yield* output.items.toEffect(String).pipe(
            Stream.ensuring(
              Effect.sync(() => {
                readerClosed = true
              }),
            ),
            Stream.take(2),
            Stream.runCollect,
            Effect.map(Array.from),
          )
          return {
            values: values as ReadonlyArray<typeof StreamItem.Type>,
            closed: readerClosed,
            stoppedEarly: pulled < 2050,
          }
        }).pipe(Effect.orDie),
      ),
    scheduledMetadata: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const reflected = yield* Reflection.getAgentType("TsPeer")
          if (reflected === undefined || reflected.mode !== "durable") return "missing:TsPeer"
          const client = yield* reflected.client.get({ name })
          const echo = yield* client.method("echo")
          const invoked = yield* echo.invoke({ value: "metadata" })
          const mark = yield* client.method("markScheduled")
          const scheduled = yield* mark.schedule(
            { seconds: BigInt(Math.floor(Date.now() / 1000) + 2), nanoseconds: 0 },
            {},
          )
          yield* scheduled.cancel
          yield* Effect.sleep("2500 millis")
          const count = yield* (yield* client.method("scheduledCount")).invoke({})
          return `${invoked.metadata.agentId}:${invoked.metadata.idempotencyKey}:${scheduled.metadata.agentId}:${String(count.value)}`
        }),
      ),
    callTsTool: ({ payload }) =>
      Effect.gen(function* () {
        let stdout = ""
        const result = yield* tsToolClient(
          { label: name },
          {
            stdin: Stream.succeed(new TextEncoder().encode(payload)),
            stdout: (stream) =>
              stream.pipe(
                Stream.decodeText(),
                Stream.runForEach((chunk) => Effect.sync(() => (stdout += chunk))),
              ),
          },
        )
        return `${result}|${stdout}`
      }).pipe(Effect.orDie),
    toolRoundTrip: ({ payload }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const input = Stream.make(new TextEncoder().encode(payload))
          const invocation = yield* TsCrossStreamingClient.create().ts_cross_streaming(name, input)
          const [result, stdout] = yield* Effect.all(
            [
              invocation.result,
              invocation.stdout.pipe(
                Stream.decodeText(),
                Stream.runCollect,
                Effect.map((chunks) => Array.from(chunks).join("")),
              ),
            ],
            { concurrency: "unbounded" },
          )
          return `${result}|${stdout}`
        }),
      ).pipe(Effect.orDie),
    quotaThroughTs: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const token = yield* Quota.acquireQuotaToken("cross-sdk-quota", 2n)
          const peer = yield* TsPeer.client.get({ name })
          const returned = yield* peer.echoQuota({ token })
          const value = yield* Quota.withReservation(returned, 1n, () =>
            Effect.succeed({ used: 1n, value: "reserved-after-ts" }),
          )
          const oldRejected = yield* Quota.withReservation(token, 0n, () =>
            Effect.succeed({ used: 0n, value: false }),
          ).pipe(Effect.catch(() => Effect.succeed(true)))
          return `${value}:${oldRejected}`
        }).pipe(Effect.orDie),
      ),
  }),
)
