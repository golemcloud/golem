import { Effect, Schema, Stream } from "effect"
import { defineAgent, method, WitTypes } from "@golemcloud/effect-golem"
import { client as matrixCore } from "matrix-core-tool-guest-client"
import { client as matrixResource } from "matrix-resource-tool-guest-client"

declare module "@golemcloud/effect-golem/BridgeTool" {
  export type AgentStream<T> = Stream.Stream<T, unknown>
}

const MatrixObservation = Schema.Struct({
  provider: Schema.String,
  command: Schema.String,
  normalizedSource: Schema.String,
  weightedSize: WitTypes.Int64,
  labelSummary: Schema.String,
  principal: Schema.String,
  ownerAgentId: Schema.String,
  errorField: Schema.String,
  errorReason: Schema.String,
  errorRetryable: Schema.Boolean,
})

const MatrixResourceObservation = Schema.Struct({
  secretFirstProvider: Schema.String,
  secretSecondProvider: Schema.String,
  secretFirstRevealed: Schema.Boolean,
  secretSecondRevealed: Schema.Boolean,
  secretPrincipal: Schema.String,
  secretOwnerAgentId: Schema.String,
  quotaProvider: Schema.String,
  quotaReserved: Schema.Boolean,
  quotaReturnedUsable: Schema.Boolean,
  quotaOriginalConsumed: Schema.Boolean,
  quotaPrincipal: Schema.String,
  quotaOwnerAgentId: Schema.String,
  permissionSupported: Schema.Boolean,
  permissionProvider: Schema.String,
  permissionSameIdentity: Schema.Boolean,
  permissionOriginalConsumed: Schema.Boolean,
  permissionPrincipal: Schema.String,
  permissionOwnerAgentId: Schema.String,
  typedValues: Schema.Array(WitTypes.Uint32),
})

const successRequest = {
  source: "matrix.sample",
  dimensions: { width: 3, height: 5 },
  labels: ["north", "east", "south"],
}

const rejectedRequest = {
  source: "reject.me",
  dimensions: { width: 2, height: 11 },
  labels: ["not", "used"],
}

defineAgent({
  name: "EffectToolStreamingCaller",
  id: { name: Schema.String },
  methods: {
    matrix_core_observation: method({ input: {}, success: MatrixObservation }),
    matrix_resource_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    matrix_core_observation: () =>
      Effect.gen(function* () {
        const artifact = matrixCore.artifact()
        const success = yield* artifact.inspect(successRequest, 7n)
        const failure = yield* artifact.inspect(rejectedRequest, 13n).pipe(Effect.flip)
        if (failure.tag !== "tool" || failure.error.tag !== "Rejected") {
          return yield* Effect.die(`expected rejected matrix-core error, got ${failure.tag}`)
        }
        return {
          ...success,
          errorField: failure.error.value.field,
          errorReason: failure.error.value.reason,
          errorRetryable: failure.error.value.retryable,
        }
      }).pipe(Effect.scoped, Effect.orDie),
    matrix_resource_observation: () =>
      Effect.gen(function* () {
        const typed = yield* matrixResource.typed().transform(Stream.fromIterable([2, 5, 9]))
        const typedValues = (yield* (typed as Stream.Stream<number, unknown>).pipe(
          Stream.runCollect,
          Effect.map(Array.from),
        )) as number[]

        return {
          secretFirstProvider: "",
          secretSecondProvider: "",
          secretFirstRevealed: false,
          secretSecondRevealed: false,
          secretPrincipal: "",
          secretOwnerAgentId: "",
          quotaProvider: "",
          quotaReserved: false,
          quotaReturnedUsable: false,
          quotaOriginalConsumed: false,
          quotaPrincipal: "",
          quotaOwnerAgentId: "",
          permissionSupported: false,
          permissionProvider: "",
          permissionSameIdentity: false,
          permissionOriginalConsumed: false,
          permissionPrincipal: "",
          permissionOwnerAgentId: "",
          typedValues,
        }
      }).pipe(Effect.scoped, Effect.orDie),
  }),
})
