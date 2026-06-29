/**
 * Host interface for the Golem quota system.
 * Agents use quota-tokens to declare intent to consume a named resource and to
 * reserve / commit actual usage.
 * The `quota-token` capability itself is defined in `golem:core/types` so that
 * it can travel inside a `schema-value-tree` as an opaque, unforgeable handle.
 * This interface only exposes the operations that act on such a handle.
 */
declare module 'golem:quota/types@1.5.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  /**
   * Request a quota capability for the given resource.
   * - `resource-name` : the resource name (as declared in the manifest).
   * - `expected-use`  : expected units per reservation; used to derive
   *                      credit rate and max-credit for fair scheduling.
   */
  export function newToken(resourceName: string, expectedUse: bigint): QuotaToken;
  /**
   * Reserve `amount` units from the token's local allocation.
   * Blocks internally until capacity is available or the resource's
   * enforcement action fires.  Returns a `reservation` handle that
   * must be committed (or dropped) to release unused capacity.
   * @throws FailedReservation
   */
  export function reserve(token: QuotaToken, amount: bigint): Reservation;
  /**
   * Split off a child token with `child-expected-use` units from `token`.
   * The parent's `expected-use` is reduced by `child-expected-use`.
   * Credits are divided proportionally between parent and child.
   * Traps if `child-expected-use` exceeds the parent's current `expected-use`.
   */
  export function split(token: QuotaToken, childExpectedUse: bigint): QuotaToken;
  /**
   * Merge `other` back into `token`.
   * The two tokens must refer to the same resource (same resource-name
   * and environment).  `other` is consumed.
   * Traps if the tokens refer to different resources.
   */
  export function merge(token: QuotaToken, other: QuotaToken): void;
  export class Reservation {
    /**
     * Commit actual usage, consuming the reservation.
     * If `used` < reserved  — unused capacity is returned to the pool.
     * If `used` > reserved  — the excess is deducted from the token's
     *                          remaining allocation as "debt".
     */
    static commit(this_: Reservation, used: bigint): void;
  }
  export type QuotaToken = golemCore200Types.QuotaToken;
  /**
   * Error returned when a reservation cannot be satisfied because the
   * resource's enforcement policy is `reject`.
   * The inner value is an optional estimate in nanoseconds of how long
   * the caller would need to wait for capacity (only available for
   * rate-limited resources).
   */
  export type FailedReservation = {
    estimatedWaitNanos?: bigint;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
