import { Effect, Schema, Stream } from "effect"
import "./concurrent-stream.js"
import {
  defineAgent,
  defineConfig,
  method,
  Quota,
  WitTypes,
} from "@golemcloud/effect-golem"
import { ChunkMEffectStreamingClient } from "chunk-m-effect-streaming-tool-guest-client"
import { client as matrixCore } from "matrix-core-tool-guest-client"
import { client as matrixPermissionIssuer } from "matrix-permission-issuer-tool-guest-client"
import { client as matrixResource } from "matrix-resource-tool-guest-client"

declare module "@golemcloud/effect-golem/BridgeTool" {
  export type AgentStream<T> = Stream.Stream<T, unknown>
}

class MatrixResourceConfig extends defineConfig("EffectToolStreamingCaller.Config", {
  secret: Schema.Redacted(Schema.String),
}) {}

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

const emptyResourceObservation = (): typeof MatrixResourceObservation.Type => ({
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
  typedValues: [],
})

const ChunkMRuntimeObservation = Schema.Struct({
  successStdout: Schema.Array(WitTypes.Uint8),
  successBytesRead: WitTypes.Uint64,
  successOutcome: Schema.String,
  errorStdout: Schema.Array(WitTypes.Uint8),
  errorTerminal: Schema.String,
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
  config: MatrixResourceConfig,
  methods: {
    matrix_core_observation: method({ input: {}, success: MatrixObservation }),
    matrix_resource_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
    matrix_secret_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
    matrix_quota_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
    matrix_permission_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
    matrix_typed_stream_observation: method({
      input: {},
      success: MatrixResourceObservation,
    }),
    chunk_m_runtime_observation: method({
      input: {},
      success: ChunkMRuntimeObservation,
    }),
    chunk_m_cancellation_observation: method({
      input: {},
      success: Schema.String,
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
    matrix_secret_observation: () =>
      Effect.gen(function* () {
        const config = yield* MatrixResourceConfig
        const secret = yield* config.secret.borrow
        const first = yield* matrixResource.secret().exchange(secret)
        const second = yield* matrixResource.secret().exchange(first.secret)
        return {
          ...emptyResourceObservation(),
          secretFirstProvider: first.provider,
          secretSecondProvider: second.provider,
          secretFirstRevealed: first.revealed,
          secretSecondRevealed: second.revealed,
          secretPrincipal: second.principal,
          secretOwnerAgentId: second.ownerAgentId,
        }
      }).pipe(Effect.scoped, Effect.orDie),
    matrix_quota_observation: () =>
      Effect.gen(function* () {
        const original = yield* Quota.acquireQuotaToken("matrix-capacity", 2n)
        const quota = yield* matrixResource.quota().exchange(original)
        const quotaOriginalConsumed = yield* Quota.withReservation(
          original,
          0n,
          () => Effect.succeed({ used: 0n, value: false }),
        ).pipe(
          Effect.as(false),
          Effect.catch(() => Effect.succeed(true)),
        )
        const quotaReturnedUsable = yield* Quota.withReservation(
          quota.token,
          0n,
          () => Effect.succeed({ used: 0n, value: true }),
        ).pipe(Effect.catch(() => Effect.succeed(false)))
        return {
          ...emptyResourceObservation(),
          quotaProvider: quota.provider,
          quotaReserved: quota.reserved,
          quotaReturnedUsable,
          quotaOriginalConsumed,
          quotaPrincipal: quota.principal,
          quotaOwnerAgentId: quota.ownerAgentId,
        }
      }).pipe(Effect.scoped, Effect.orDie),
    matrix_permission_observation: () =>
      Effect.gen(function* () {
        const issued = yield* matrixPermissionIssuer.issue()
        if (issued.issuer !== "rust") {
          return yield* Effect.die(`expected rust permission issuer, got ${issued.issuer}`)
        }
        const permission = yield* matrixResource.permissions().exchange(issued.card)
        if (
          issued.principal !== permission.principal ||
          issued.ownerAgentId !== permission.ownerAgentId
        ) {
          return yield* Effect.die("permission issuer and exchange evidence did not match")
        }
        const permissionOriginalConsumed = yield* matrixResource
          .permissions()
          .exchange(issued.card)
          .pipe(
            Effect.as(false),
            Effect.catch((error) =>
              error.tag === "rpc" ? Effect.succeed(true) : Effect.fail(error),
            ),
          )
        return {
          ...emptyResourceObservation(),
          permissionSupported: true,
          permissionProvider: permission.provider,
          permissionSameIdentity: permissionOriginalConsumed,
          permissionOriginalConsumed,
          permissionPrincipal: permission.principal,
          permissionOwnerAgentId: permission.ownerAgentId,
        }
      }).pipe(Effect.scoped, Effect.orDie),
    matrix_typed_stream_observation: () =>
      Effect.gen(function* () {
        const typed = yield* matrixResource.typed().transform(Stream.fromIterable([2, 5, 9]))
        const typedValues = (yield* (typed as Stream.Stream<number, unknown>).pipe(
          Stream.runCollect,
          Effect.map(Array.from),
        )) as number[]
        return { ...emptyResourceObservation(), typedValues }
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
    chunk_m_runtime_observation: () =>
      Effect.gen(function* () {
        const client = ChunkMEffectStreamingClient.create()
        const successInvocation = yield* client.chunk_m_effect_streaming(
          "success",
          Stream.fromIterable([new Uint8Array([1, 2, 3]), new Uint8Array([250, 251])]),
        )
        const [success, successChunks] = yield* Effect.all(
          [successInvocation.result, successInvocation.stdout!.pipe(Stream.runCollect)],
          { concurrency: "unbounded" },
        )
        const successStdout: number[] = successChunks.flatMap((chunk) => Array.from(chunk))

        const errorInvocation = yield* client.chunk_m_effect_streaming(
          "declared-error",
          Stream.succeed(new Uint8Array([9, 8, 7])),
        )
        const [errorTerminal, errorChunks] = yield* Effect.all(
          [
            errorInvocation.result.pipe(
              Effect.match({
                onSuccess: () => "unexpected-success",
                onFailure: (failure) =>
                  failure.tag === "tool" &&
                  failure.error.tag === "Rejected" &&
                  failure.error.value.reason === "expected"
                    ? "declared:expected"
                    : `unexpected:${String(failure)}`,
              }),
            ),
            errorInvocation.stdout!.pipe(Stream.runCollect),
          ],
          { concurrency: "unbounded" },
        )
        const errorStdout: number[] = errorChunks.flatMap((chunk) => Array.from(chunk))

        return {
          successStdout,
          successBytesRead: success.bytesRead,
          successOutcome: success.outcome,
          errorStdout,
          errorTerminal,
        }
      }).pipe(Effect.scoped, Effect.orDie),
    chunk_m_cancellation_observation: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const client = ChunkMEffectStreamingClient.create()
          const invocation = yield* client.chunk_m_effect_streaming("cancel", Stream.never)
          const pull = yield* Stream.toPull(invocation.stdout!)
          yield* pull
          yield* invocation.cancel
          return "invocation-cancelled"
        }),
      ).pipe(Effect.orDie),
  }),
})
