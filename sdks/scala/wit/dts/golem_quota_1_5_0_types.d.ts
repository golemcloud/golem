/**
 * Host interface for the Golem quota system.
 * Agents use quota-tokens to declare intent to consume a named resource and to
 * reserve / commit actual usage.
 */
declare module 'golem:quota/types@1.5.0' {
  export class Reservation {
    static commit(this_: Reservation, used: bigint): void;
  }
  export class QuotaToken {
    constructor(resourceName: string, expectedUse: bigint);
    reserve(amount: bigint): Reservation;
    split(childExpectedUse: bigint): QuotaToken;
    merge(other: QuotaToken): void;
    toRecord(): QuotaTokenRecord;
    static fromRecord(serialized: QuotaTokenRecord): QuotaToken;
  }
  export type FailedReservation = {
    estimatedWaitNanos?: bigint;
  };
  export type QuotaTokenRecord = {
    environmentId: { uuid: string };
    resourceName: string;
    expectedUse: bigint;
    lastCredit: bigint;
    lastCreditAt: { seconds: bigint; nanoseconds: number };
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
