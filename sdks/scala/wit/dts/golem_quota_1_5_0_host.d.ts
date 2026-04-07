/**
 * Host interface for the Golem quota system.
 * Agents use quota-tokens to declare intent to consume a named resource and to
 * reserve / commit actual usage.
 */
declare module 'golem:quota/host@1.5.0' {
  /**
   * Error returned when a reservation cannot be satisfied because the
   * resource's enforcement policy is `reject`.
   */
  export type FailedReservation = {
    /** Estimated wait in nanoseconds until capacity is available (rate-limited resources only). */
    estimatedWaitNanos?: bigint;
  };

  /**
   * A short-lived capability representing a pending or committed resource consumption.
   * Dropping without committing is equivalent to committing zero usage.
   */
  export class Reservation {
    /**
     * Commit actual usage.
     * used < reserved → unused returned to pool.
     * used > reserved → excess deducted as debt.
     */
    commit(used: bigint): void;
  }

  /**
   * An unforgeable capability granting the right to consume a named resource.
   * Dropping the token releases the underlying lease back to the executor pool.
   */
  export class QuotaToken {
    constructor(resourceName: string, expectedUse: bigint);
    /**
     * Reserve `amount` units. Blocks until capacity is available or enforcement fires.
     * @throws FailedReservation when the enforcement policy is `reject`.
     */
    reserve(amount: bigint): Reservation;
    /**
     * Split off a child token with `childExpectedUse` units.
     * Traps if childExpectedUse exceeds the parent's current expectedUse.
     */
    split(childExpectedUse: bigint): QuotaToken;
    /**
     * Merge `other` into this token. `other` is consumed.
     * Traps if the tokens refer to different resources.
     */
    merge(other: QuotaToken): void;
  }
}
