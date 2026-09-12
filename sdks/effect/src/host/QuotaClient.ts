import { Context, Layer } from "effect"
import * as QuotaHost from "golem:quota/types@1.5.0"
import type { QuotaToken } from "golem:core/types@2.0.0"

export interface QuotaClientShape {
  readonly newToken: (resourceName: string, expectedUse: bigint) => QuotaToken
  readonly reserve: (token: QuotaToken, amount: bigint) => QuotaHost.Reservation
  readonly commit: (reservation: QuotaHost.Reservation, used: bigint) => void
  readonly split: (token: QuotaToken, childExpectedUse: bigint) => QuotaToken
  readonly merge: (token: QuotaToken, other: QuotaToken) => void
}

export class QuotaClient extends Context.Service<QuotaClient, QuotaClientShape>()(
  "effect-golem/host/Quota",
) {}

export const QuotaLive: Layer.Layer<QuotaClient> = Layer.succeed(
  QuotaClient,
  QuotaClient.of({
    newToken: QuotaHost.newToken,
    reserve: QuotaHost.reserve,
    commit: QuotaHost.Reservation.commit,
    split: QuotaHost.split,
    merge: QuotaHost.merge,
  }),
)
