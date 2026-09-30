import type * as AgentCommon from "golem:agent/common@2.0.0"
import { Effect, Schema, Stream } from "effect"
import { Agents, Bridge, Quota, Tool, WitTypes } from "@golemcloud/effect-golem"

const MatrixRequest = Schema.Struct({
  source: Schema.String,
  dimensions: Schema.Struct({
    width: WitTypes.Uint32,
    height: WitTypes.Uint32,
  }),
  labels: Schema.Array(Schema.String),
})

const MatrixResult = Schema.Struct({
  provider: Schema.String,
  command: Schema.String,
  normalizedSource: Schema.String,
  weightedSize: WitTypes.Int64,
  labelSummary: Schema.String,
  principal: Schema.String,
  ownerAgentId: Schema.String,
})

const Rejected = Schema.Struct({
  field: Schema.String,
  reason: Schema.String,
  retryable: Schema.Boolean,
})

const ProviderEvidence = {
  provider: Schema.String,
  principal: Schema.String,
  ownerAgentId: Schema.String,
}

const MatrixQuotaToken = WitTypes.QuotaToken({ resourceName: "matrix-capacity" })

const QuotaExchange = Schema.Struct({
  ...ProviderEvidence,
  reserved: Schema.Boolean,
  token: MatrixQuotaToken,
})

const PermissionCard = WitTypes.PermissionCard({ polymorphic: false })
const PermissionExchange = Schema.Struct({
  ...ProviderEvidence,
  card: PermissionCard,
})

const getSelfMetadata = Agents.getSelfMetadata as Effect.Effect<
  Agents.AgentMetadata,
  Agents.AgentsHostError
>

const principalLabel = (principal: unknown): string => {
  const value = principal as AgentCommon.Principal
  switch (value.tag) {
    case "oidc":
      return `oidc:${value.val.sub}`
    case "agent":
      return "agent"
    case "golem-user":
      return "golem-user"
    case "anonymous":
      return "anonymous"
  }
}

const evidence = (principal: unknown, ownerAgentId: string) => ({
  provider: "effect",
  principal: principalLabel(principal),
  ownerAgentId,
})

Tool.toolDefinition("matrix-core", { version: "1.0.0" })
  .command("artifact", (artifact) =>
    artifact.command("inspect", (inspect) =>
      inspect.body((body) =>
        body
          .positional("request", MatrixRequest)
          .positional("multiplier", WitTypes.Int64)
          .returns(MatrixResult)
          .error("rejected", Rejected, { kind: "usage-error", exitCode: 2 }),
      ),
    ),
  )
  .implement({
    artifact: {
      inspect: ({ request, multiplier }, context) =>
        request.source === "reject.me"
          ? Effect.fail(
              Tool.err("rejected", {
                field: "request.source",
                reason: "unsupported source",
                retryable: false,
              }),
            )
          : Effect.gen(function* () {
              const metadata = yield* getSelfMetadata.pipe(Effect.orDie)
              return {
                provider: "effect",
                command: "artifact/inspect",
                normalizedSource: request.source.toUpperCase(),
                weightedSize:
                  BigInt(request.dimensions.width) *
                    BigInt(request.dimensions.height) *
                    multiplier +
                  BigInt(request.labels.length),
                labelSummary: [...request.labels].reverse().join("|"),
                principal: principalLabel(context.principal),
                ownerAgentId: metadata.agentId.agentId,
              }
            }),
    },
  })

Tool.toolDefinition("matrix-resource", { version: "1.0.0" })
  .command("quota", (quota) =>
    quota.command("exchange", (exchange) =>
      exchange.body((body) => body.positional("token", MatrixQuotaToken).returns(QuotaExchange)),
    ),
  )
  .command("permissions", (permissions) =>
    permissions.command("exchange", (exchange) =>
      exchange.body((body) => body.positional("card", PermissionCard).returns(PermissionExchange)),
    ),
  )
  .command("typed", (typed) =>
    typed.command("transform", (transform) =>
      transform.body((body) =>
        body
          .positional("input", WitTypes.AgentStream(WitTypes.Uint32))
          .returns(WitTypes.AgentStream(WitTypes.Uint32)),
      ),
    ),
  )
  .implement({
    quota: {
      exchange: ({ token }, context) =>
        Effect.gen(function* () {
          const owner = yield* getSelfMetadata.pipe(Effect.orDie)
          const effectToken = Bridge.quotaTokenFromSchemaValue({
            tag: "quota-token",
            handle: token,
          })
          const returnedToken = Bridge.quotaTokenToSchemaValue(effectToken)
          if (returnedToken.tag !== "quota-token") {
            return yield* Effect.die("quota token bridge returned a non-quota value")
          }
          return yield* Quota.withReservation(effectToken, 1n, () =>
            Effect.succeed({
              used: 1n,
              value: {
                ...evidence(context.principal, owner.agentId.agentId),
                reserved: true,
                token: returnedToken.handle,
              },
            }),
          ).pipe(Effect.orDie) as Effect.Effect<typeof QuotaExchange.Type, never>
        }),
    },
    permissions: {
      exchange: ({ card }, context) =>
        Effect.gen(function* () {
          const owner = yield* getSelfMetadata.pipe(Effect.orDie)
          return {
            ...evidence(context.principal, owner.agentId.agentId),
            card,
          }
        }),
    },
    typed: {
      transform: ({ input }) => Effect.succeed(input.pipe(Stream.map((value) => value * 3 + 1))),
    },
  })
