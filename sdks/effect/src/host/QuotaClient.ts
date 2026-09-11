/**
 * Host service for `golem:quota/types@1.5.0`. Wraps the synchronous
 * (throw-on-failure) host primitives — the `QuotaToken` constructor,
 * `token.reserve`, `Reservation.commit`, `token.split`, and
 * `token.merge` — into an Effect-typed surface so SDK code can reach
 * them via DI rather than importing the WIT specifier directly.
 *
 * The shape mirrors the underlying host calls verbatim (synchronous,
 * `throw` on failure). The Effect-typed wrappers in `src/quota.ts`
 * fold those throws into the standard `FailedReservationError` /
 * `QuotaHostError` typed-failure channels.
 *
 * Note: `golem:quota/types@1.5.0` mixes plain function-style entrypoints
 * (`acquireQuotaToken`, `reserve`, `commit`, `split`, `merge`) with the
 * `QuotaToken` / `Reservation` resource handles. Per the layer-mocks
 * refactor plan §4c, the resources themselves are *not* services — only
 * the constructors / static functions that produce them are. Resource
 * lifetimes (`drop ≡ commit(0)`) are managed via Effect's `Scope` in
 * `src/quota.ts`, not via the service tag.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as QuotaHost from "golem:quota/types@1.5.0"

export interface QuotaClientShape {
  /** Mirrors `new QuotaToken(resourceName, expectedUse)`. */
  readonly acquireQuotaToken: (resourceName: string, expectedUse: bigint) => QuotaHost.QuotaToken
  /** Mirrors `quota-token.reserve(amount)`. */
  readonly reserve: (token: QuotaHost.QuotaToken, amount: bigint) => QuotaHost.Reservation
  /** Mirrors the static `Reservation.commit(reservation, used)`. */
  readonly commit: (reservation: QuotaHost.Reservation, used: bigint) => void
  /** Mirrors `quota-token.split(child-expected-use)`. */
  readonly split: (token: QuotaHost.QuotaToken, childExpectedUse: bigint) => QuotaHost.QuotaToken
  /** Mirrors `quota-token.merge(other)`. */
  readonly merge: (token: QuotaHost.QuotaToken, other: QuotaHost.QuotaToken) => void
}

export class QuotaClient extends Context.Service<QuotaClient, QuotaClientShape>()(
  "effect-golem/host/Quota",
) {}

export const QuotaLive: Layer.Layer<QuotaClient> = Layer.succeed(
  QuotaClient,
  QuotaClient.of({
    acquireQuotaToken: (resourceName, expectedUse) =>
      new QuotaHost.QuotaToken(resourceName, expectedUse),
    reserve: (token, amount) => token.reserve(amount),
    commit: (reservation, used) => QuotaHost.Reservation.commit(reservation, used),
    split: (token, childExpectedUse) => token.split(childExpectedUse),
    merge: (token, other) => token.merge(other),
  }),
)
