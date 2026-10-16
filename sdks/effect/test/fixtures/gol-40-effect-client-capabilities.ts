import type * as CoreTypes from "golem:core/types@2.0.0"
import { Effect, Layer, Schema } from "effect"
import * as BridgeTool from "../../src/BridgeTool.js"
import { compile } from "../../src/WitCodec.js"
import * as Quota from "../../src/Quota.js"
import { ToolTransport, toolDefinition } from "../../src/Tool.js"
import * as WitTypes from "../../src/WitTypes.js"
import { QuotaClient } from "../../src/host/QuotaClient.js"
import { ToolClient } from "../../src/host/ToolClient.js"
import { schemaGraphFromWit, schemaValueFromWit } from "../../src/internal/schema-model/wit.js"
import { loopbackTransport, noHost } from "./gol-40-effect-conformance.js"

type RawQuota = CoreTypes.QuotaToken & { readonly resourceName: string }

export const quotaEvents: string[] = []

export const quotaLayer = Layer.succeed(
  QuotaClient,
  QuotaClient.of({
    newToken: (resourceName) => {
      quotaEvents.push(`acquire:${resourceName}`)
      return { resourceName } as RawQuota
    },
    reserve: (token, amount) => {
      quotaEvents.push(`reserve:${(token as RawQuota).resourceName}:${amount}`)
      return { token, amount } as never
    },
    commit: (_reservation, used) => quotaEvents.push(`commit:${used}`),
    split: () => {
      throw new Error("unexpected split")
    },
    merge: () => {
      throw new Error("unexpected merge")
    },
  }),
)

const MatrixQuotaHandle = WitTypes.QuotaToken({ resourceName: "matrix-capacity" })
const Input = Schema.Struct({ token: MatrixQuotaHandle })
const Output = Schema.Struct({ reserved: Schema.Boolean, token: MatrixQuotaHandle })

export const installQuotaExchange = () =>
  toolDefinition("gol40-effect-quota")
    .body((body) => body.positional("token", MatrixQuotaHandle).returns(Output))
    .implement(
      {
        gol40EffectQuota: ({ token }) =>
          Quota.withReservation(token, 1n, () =>
            Effect.succeed({
              used: 1n,
              value: { reserved: true, token },
            }),
          ).pipe(Effect.orDie),
      },
      quotaLayer,
    )

export const exchangeQuota = (token: Quota.QuotaToken) =>
  Effect.gen(function* () {
    const inputCodec = yield* compile(Input)
    const outputCodec = yield* compile(Output)
    const wireInput = yield* inputCodec.encode({ token })
    const started = yield* BridgeTool.createToolClientRuntime("gol40-effect-quota").start(
      [],
      {
        graph: schemaGraphFromWit(inputCodec.schemaGraph),
        value: schemaValueFromWit(wireInput),
      },
      undefined,
      false,
      false,
    )
    const terminal = yield* started.result
    if (!terminal.result) return yield* Effect.die("quota exchange returned no result")
    const output = yield* outputCodec.decode(terminal.result.value)
    return output
  }).pipe(
    Effect.provideService(ToolTransport, loopbackTransport({ tag: "anonymous" })),
    Effect.provideService(ToolClient, noHost),
    Effect.provide(quotaLayer),
    Effect.scoped,
  )
