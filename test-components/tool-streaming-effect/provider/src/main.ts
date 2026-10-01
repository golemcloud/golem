import type * as AgentCommon from "golem:agent/common@2.0.0"
import { Effect, Schema, Stream } from "effect"
import { Agents, Quota, Secrets, Tool, WitTypes } from "@golemcloud/effect-golem"

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

const MatrixQuotaToken = WitTypes.QuotaToken({
  resourceName: "matrix-capacity",
})

const MatrixSecret = WitTypes.Secret(Schema.String)
const SecretExchange = Schema.Struct({
  ...ProviderEvidence,
  revealed: Schema.Boolean,
  secret: MatrixSecret,
})

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

const ChunkMStreamEvidence = Schema.Struct({
  bytesRead: WitTypes.Uint64,
  outcome: Schema.String,
})

const ChunkMRejected = Schema.Struct({ reason: Schema.String })

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
  .command("secret", (secret) =>
    secret.command("exchange", (exchange) =>
      exchange.body((body) => body.positional("secret", MatrixSecret).returns(SecretExchange)),
    ),
  )
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
    secret: {
      exchange: ({ secret }, context) =>
        Effect.gen(function* () {
          const owner = yield* getSelfMetadata.pipe(Effect.orDie)
          const revealed = yield* (
            Secrets.reveal(secret, Schema.String) as Effect.Effect<string, unknown>
          ).pipe(Effect.orDie)
          return {
            ...evidence(context.principal, owner.agentId.agentId),
            revealed: revealed === "matrix-secret-value",
            secret,
          }
        }),
    },
    quota: {
      exchange: ({ token }, context) =>
        Effect.gen(function* () {
          const owner = yield* getSelfMetadata.pipe(Effect.orDie)
          return yield* Quota.withReservation(token, 1n, () =>
            Effect.succeed({
              used: 1n,
              value: {
                ...evidence(context.principal, owner.agentId.agentId),
                reserved: true,
                token,
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

Tool.toolDefinition("chunk-m-effect-streaming", { version: "1.0.0" })
  .body((body) =>
    body
      .positional("mode", Schema.String)
      .input({ required: true })
      .output({ required: true })
      .returns(ChunkMStreamEvidence)
      .error("rejected", ChunkMRejected, {
        kind: "runtime-error",
        exitCode: 7,
      }),
  )
  .implement({
    chunkMEffectStreaming: ({ mode }, context) =>
      Effect.gen(function* () {
        if (!context.stdin || !context.stdout) return yield* Effect.die("required streams missing")

        if (mode === "cancel") {
          const pressure = new Uint8Array(1024 * 1024)
          yield* context.stdout(
            Stream.fromIterable([
              new Uint8Array([99, 97, 110, 99, 101, 108, 58]),
              ...Array.from({ length: 32 }, () => pressure),
            ]),
          )
          return { bytesRead: 0n, outcome: "unexpected-cancel-completion" }
        }

        const chunks = Array.from(yield* Stream.runCollect(context.stdin))
        const bytesRead = chunks.reduce((total, chunk) => total + BigInt(chunk.length), 0n)
        yield* context.stdout(Stream.fromIterable([new Uint8Array([0, 127, 128, 255]), ...chunks]))
        if (mode === "declared-error") {
          return yield* Effect.fail(Tool.err("rejected", { reason: "expected" }))
        }
        return { bytesRead, outcome: "success" }
      }),
  })
