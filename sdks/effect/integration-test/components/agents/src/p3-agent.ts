import { Effect, Ref, Schema, Stream } from "effect"
import { AgentStream, defineAgent, method, Schema as GolemSchema } from "@golemcloud/effect-golem"

const Item = Schema.Struct({
  id: Schema.Number,
  label: Schema.String,
  weights: Schema.Array(Schema.Number),
})

const TransformedItem = Schema.Struct({
  id: Schema.Number,
  summary: Schema.String,
  total: Schema.Number,
})

const StreamTarget = defineAgent({
  name: "EffectStreamTarget",
  mode: "durable",
  id: { name: Schema.String },
  methods: {
    sum: method({
      input: { values: GolemSchema.AgentStream(Schema.Number) },
      success: Schema.Number,
    }),
    doubled: method({
      input: { values: GolemSchema.AgentStream(Schema.Number) },
      success: GolemSchema.AgentStream(Schema.Number),
    }),
    transformNested: method({
      input: {
        request: Schema.Struct({
          prefix: Schema.String,
          nested: Schema.Struct({ values: GolemSchema.AgentStream(Item) }),
        }),
      },
      success: Schema.Struct({
        response: Schema.Struct({ values: GolemSchema.AgentStream(TransformedItem) }),
      }),
    }),
    markScheduled: method({ input: {}, success: Schema.Void }),
    scheduledCount: method({ input: {}, success: Schema.Number }),
  },
})

StreamTarget.implement(() =>
  Effect.gen(function* () {
    const scheduledCount = yield* Ref.make(0)
    return {
      sum: ({ values }) =>
        values.toEffect(String).pipe(
          Stream.runFold(
            () => 0,
            (sum, value) => sum + value,
          ),
        ),
      doubled: ({ values }) =>
        values.toEffect(String).pipe(
          Stream.map((value) => value * 2),
          AgentStream.AgentStream.fromEffect,
        ),
      transformNested: ({ request }) =>
        Effect.gen(function* () {
          const values = yield* request.nested.values.toEffect(String).pipe(
            Stream.map((item) => ({
              id: item.id * 10,
              summary: `${request.prefix}:${item.label}:${item.weights.join("+")}`,
              total: item.weights.reduce((total, weight) => total + weight, 0),
            })),
            AgentStream.AgentStream.fromEffect,
          )
          return { response: { values } }
        }),
      markScheduled: () => Ref.update(scheduledCount, (count) => count + 1),
      scheduledCount: () => Ref.get(scheduledCount),
    }
  }),
)

const EphemeralProbe = defineAgent({
  name: "EffectEphemeralProbe",
  mode: "ephemeral",
  id: { label: Schema.String },
  methods: {
    echo: method({ input: { value: Schema.String }, success: Schema.String }),
  },
})

EphemeralProbe.implement(({ label }) =>
  Effect.succeed({ echo: ({ value }) => Effect.succeed(`${label}:${value}`) }),
)

defineAgent({
  name: "EffectP3Caller",
  id: { name: Schema.String },
  methods: {
    streamRoundtrip: method({ input: {}, success: Schema.Array(Schema.Number) }),
    nestedStreamCancellation: method({
      input: {},
      success: Schema.Struct({
        values: Schema.Array(TransformedItem),
        inputPulls: Schema.Number,
        inputClosed: Schema.Boolean,
        inputStoppedEarly: Schema.Boolean,
      }),
    }),
    ephemeralRoundtrip: method({
      input: {},
      success: Schema.Struct({
        firstAgentId: Schema.String,
        secondAgentId: Schema.String,
        identitiesDiffer: Schema.Boolean,
        idempotencyKeysPresent: Schema.Boolean,
        values: Schema.Array(Schema.String),
      }),
    }),
    cancelledSchedule: method({ input: {}, success: Schema.Number }),
  },
}).implement(({ name }) =>
  Effect.succeed({
    streamRoundtrip: () =>
      Effect.gen(function* () {
        const target = yield* StreamTarget.client.get({ name })
        const input = yield* AgentStream.AgentStream.fromEffect(Stream.fromIterable([1, 2, 3]))
        const output = yield* target.doubled({ values: input })
        return yield* output.toEffect(String).pipe(
          Stream.runCollect,
          Effect.map((values) => Array.from(values)),
        )
      }).pipe(Effect.orDie, Effect.scoped),
    nestedStreamCancellation: () =>
      Effect.gen(function* () {
        const target = yield* StreamTarget.client.get({ name })
        const pulls = yield* Ref.make(0)
        const closed = yield* Ref.make(false)
        const items = [
          { id: 1, label: "alpha", weights: [2, 3] },
          { id: 2, label: "beta", weights: [5, 8, 13] },
          ...Array.from({ length: 4094 }, (_, index) => ({
            id: index + 3,
            label: "unused",
            weights: [21, 34],
          })),
        ]
        const source = Stream.fromIterable(items).pipe(
          Stream.rechunk(1),
          Stream.tap(() => Ref.update(pulls, (count) => count + 1)),
          Stream.ensuring(Ref.set(closed, true)),
        )
        const input = yield* AgentStream.AgentStream.fromEffect(source)
        const output = yield* target.transformNested({
          request: { prefix: "rpc", nested: { values: input } },
        })
        const values = yield* output.response.values.toEffect(String).pipe(
          Stream.take(2),
          Stream.runCollect,
          Effect.map((items) => Array.from(items) as Array<typeof TransformedItem.Type>),
        )
        return {
          values,
          inputPulls: yield* Ref.get(pulls),
          inputClosed: yield* Ref.get(closed),
          inputStoppedEarly: (yield* Ref.get(pulls)) < items.length,
        }
      }).pipe(Effect.orDie, Effect.scoped),
    ephemeralRoundtrip: () =>
      Effect.gen(function* () {
        const first = yield* EphemeralProbe.client.newPhantom({ label: name })
        const second = yield* EphemeralProbe.client.newPhantom({ label: name })
        const firstResult = yield* first.echo({ value: "one" })
        const secondResult = yield* second.echo({ value: "two" })
        return {
          firstAgentId: firstResult.metadata.agentId,
          secondAgentId: secondResult.metadata.agentId,
          identitiesDiffer: firstResult.metadata.agentId !== secondResult.metadata.agentId,
          idempotencyKeysPresent:
            firstResult.metadata.idempotencyKey.length > 0 &&
            secondResult.metadata.idempotencyKey.length > 0,
          values: [firstResult.value, secondResult.value],
        }
      }).pipe(Effect.orDie, Effect.scoped),
    cancelledSchedule: () =>
      Effect.gen(function* () {
        const target = yield* StreamTarget.client.get({ name })
        const scheduled = yield* target.markScheduled.schedule(Date.now() + 1_000, {})
        yield* scheduled.cancel()
        yield* Effect.sleep("1500 millis")
        return yield* target.scheduledCount({})
      }).pipe(Effect.orDie, Effect.scoped),
  }),
)
