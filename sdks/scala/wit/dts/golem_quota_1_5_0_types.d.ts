/**
 * Host interface for the Golem quota system.
 * Agents use quota-tokens to declare intent to consume a named resource and to
 * reserve / commit actual usage.
 */
declare module 'golem:quota/types@1.5.0' {
  import * as golemApi150Host from 'golem:api/host@1.5.0';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  export class Reservation {
    /**
     * Commit actual usage, consuming the reservation.
     * If `used` < reserved  — unused capacity is returned to the pool.
     * If `used` > reserved  — the excess is deducted from the token's
     *                          remaining allocation as "debt".
     */
    static commit(this_: Reservation, used: bigint): void;
  }
  export class QuotaToken {
    /**
     * Request a quota capability for the given resource.
     * - `resource-name` : the resource name (as declared in the manifest).
     * - `expected-use`  : expected units per reservation; used to derive
     *                      credit rate and max-credit for fair scheduling.
     */
    constructor(resourceName: string, expectedUse: bigint);
    /**
     * Reserve `amount` units from the local allocation.
     * Blocks internally until capacity is available or the resource's
     * enforcement action fires.  Returns a `reservation` handle that
     * must be committed (or dropped) to release unused capacity.
     * @throws FailedReservation
     */
    reserve(amount: bigint): Reservation;
    /**
     * Split off a child token with `child-expected-use` units from this token.
     * The parent's `expected-use` is reduced by `child-expected-use`.
     * Credits are divided proportionally between parent and child.
     * Traps if `child-expected-use` exceeds the parent's current `expected-use`.
     */
    split(childExpectedUse: bigint): QuotaToken;
    /**
     * Merge `other` back into this token.
     * The two tokens must refer to the same resource (same resource-name
     * and environment).  `other` is consumed.
     * Traps if the tokens refer to different resources.
     */
    merge(other: QuotaToken): void;
    /**
     * Serialize this token to a quota-token-record
     */
    toRecord(): QuotaTokenRecord;
    /**
     * Deserialize a token from this record, restoring all associated state
     */
    static fromRecord(serialized: QuotaTokenRecord): QuotaToken;
  }
  export type EnvironmentId = golemApi150Host.EnvironmentId;
  export type Datetime = wasiClocks023WallClock.Datetime;
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
  /**
   * A serializable represenation of a quota-token, useful when sending across rpc boundaries.
   */
  export type QuotaTokenRecord = {
    environmentId: EnvironmentId;
    resourceName: string;
    expectedUse: bigint;
    lastCredit: bigint;
    lastCreditAt: Datetime;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
