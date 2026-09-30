import type * as AgentCommon from "golem:agent/common@2.0.0"
import { Effect, Schema } from "effect"
import { Agents, Tool, WitTypes } from "@golemcloud/effect-golem"

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
