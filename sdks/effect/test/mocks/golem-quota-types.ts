/**
 * Runtime mock for `golem:quota/types@1.5.0` used by the test suite.
 *
 * The real host classes are provided by the WASM runtime; this mock
 * implements the full surface (`new QuotaToken(...)`, `reserve`,
 * `Reservation.commit`, `split`, `merge`, `toRecord`/`fromRecord`) so
 * the Effect wrappers can be exercised in node-vitest.
 */

export type EnvironmentId = { uuid: { highBits: bigint; lowBits: bigint } }
export type Datetime = { seconds: bigint; nanoseconds: number }

export type FailedReservation = {
  estimatedWaitNanos?: bigint
}

export type QuotaTokenRecord = {
  environmentId: EnvironmentId
  resourceName: string
  expectedUse: bigint
  lastCredit: bigint
  lastCreditAt: Datetime
}

const ZERO_ENV: EnvironmentId = { uuid: { highBits: 0n, lowBits: 0n } }
const ZERO_TS: Datetime = { seconds: 0n, nanoseconds: 0 }

const FROM_RECORD_BRAND: unique symbol = Symbol("from-record")

/**
 * Test event log — appended to whenever the mock observes a host call.
 * Tests use this to assert the exact sequence of `reserve` / `commit` /
 * `split` / `merge` interactions without resorting to spies on the SDK.
 */
export type Event =
  | { tag: "construct"; resourceName: string; expectedUse: bigint }
  | { tag: "reserve"; resourceName: string; amount: bigint }
  | { tag: "commit"; resourceName: string; used: bigint; reservedAmount: bigint }
  | { tag: "split"; resourceName: string; childExpectedUse: bigint }
  | { tag: "merge"; resourceName: string; otherResource: string }

export const events: Array<Event> = []

let nextReservationId = 1
type ReserveBehavior =
  | { tag: "ok" }
  | { tag: "fail"; failure: FailedReservation }
  | { tag: "throw"; error: unknown }
let reserveBehavior: ReserveBehavior = { tag: "ok" }

let splitBehavior: { tag: "ok" } | { tag: "throw"; error: unknown } = { tag: "ok" }
let mergeBehavior: { tag: "ok" } | { tag: "throw"; error: unknown } = { tag: "ok" }
let commitBehavior: { tag: "ok" } | { tag: "throw"; error: unknown } = { tag: "ok" }

/** Reset the mock to the default "everything succeeds" state. */
export const __reset = (): void => {
  events.length = 0
  nextReservationId = 1
  reserveBehavior = { tag: "ok" }
  splitBehavior = { tag: "ok" }
  mergeBehavior = { tag: "ok" }
  commitBehavior = { tag: "ok" }
}

/** Make the next `reserve` (and all subsequent ones) throw a `failed-reservation`. */
export const __setReserveFails = (failure: FailedReservation = {}): void => {
  reserveBehavior = { tag: "fail", failure }
}

/** Make the next `reserve` throw an arbitrary host error (NOT a failed-reservation). */
export const __setReserveThrows = (error: unknown): void => {
  reserveBehavior = { tag: "throw", error }
}

/** Make `split` throw on its next call (simulating a WIT trap). */
export const __setSplitThrows = (error: unknown): void => {
  splitBehavior = { tag: "throw", error }
}

/** Make `merge` throw on its next call (simulating a WIT trap). */
export const __setMergeThrows = (error: unknown): void => {
  mergeBehavior = { tag: "throw", error }
}

/** Make `Reservation.commit` throw on its next call. */
export const __setCommitThrows = (error: unknown): void => {
  commitBehavior = { tag: "throw", error }
}

export class Reservation {
  /** @internal — surfaced for test assertions, not part of the real WIT shape. */
  readonly id: number
  /** @internal — the amount originally reserved, used for event-log clarity. */
  readonly reservedAmount: bigint
  /** @internal — back-reference for resource-name event tagging. */
  readonly token: QuotaToken
  /** @internal — flipped after commit so a duplicate raises like the host would. */
  consumed = false

  constructor(token: QuotaToken, reservedAmount: bigint) {
    this.id = nextReservationId++
    this.reservedAmount = reservedAmount
    this.token = token
  }

  static commit(this_: Reservation, used: bigint): void {
    if (this_.consumed) {
      throw new Error(`mock: reservation ${this_.id} already consumed`)
    }
    this_.consumed = true
    if (commitBehavior.tag === "throw") {
      const e = commitBehavior.error
      commitBehavior = { tag: "ok" }
      throw e
    }
    events.push({
      tag: "commit",
      resourceName: this_.token.resourceName,
      used,
      reservedAmount: this_.reservedAmount,
    })
  }
}

export class QuotaToken {
  readonly resourceName: string
  expectedUse: bigint
  readonly environmentId: EnvironmentId
  lastCredit: bigint
  lastCreditAt: Datetime

  constructor(resourceName: string, expectedUse: bigint)
  constructor(record: QuotaTokenRecord, _internal: typeof FROM_RECORD_BRAND)
  constructor(a: string | QuotaTokenRecord, b?: bigint | typeof FROM_RECORD_BRAND) {
    if (typeof a === "string" && typeof b === "bigint") {
      this.resourceName = a
      this.expectedUse = b
      this.environmentId = ZERO_ENV
      this.lastCredit = 0n
      this.lastCreditAt = ZERO_TS
      events.push({ tag: "construct", resourceName: a, expectedUse: b })
    } else if (typeof a === "object" && b === FROM_RECORD_BRAND) {
      this.resourceName = a.resourceName
      this.expectedUse = a.expectedUse
      this.environmentId = a.environmentId
      this.lastCredit = a.lastCredit
      this.lastCreditAt = a.lastCreditAt
    } else {
      throw new Error("mock: invalid QuotaToken constructor args")
    }
  }

  reserve(amount: bigint): Reservation {
    if (reserveBehavior.tag === "fail") {
      // The WIT binding throws the FailedReservation record directly.
      throw reserveBehavior.failure as unknown
    }
    if (reserveBehavior.tag === "throw") {
      throw reserveBehavior.error
    }
    events.push({ tag: "reserve", resourceName: this.resourceName, amount })
    return new Reservation(this, amount)
  }

  split(childExpectedUse: bigint): QuotaToken {
    if (splitBehavior.tag === "throw") {
      const e = splitBehavior.error
      splitBehavior = { tag: "ok" }
      throw e
    }
    if (childExpectedUse > this.expectedUse) {
      throw new Error("mock: child-expected-use exceeds parent expected-use")
    }
    events.push({ tag: "split", resourceName: this.resourceName, childExpectedUse })
    this.expectedUse -= childExpectedUse
    return new QuotaToken(this.resourceName, childExpectedUse)
  }

  merge(other: QuotaToken): void {
    if (mergeBehavior.tag === "throw") {
      const e = mergeBehavior.error
      mergeBehavior = { tag: "ok" }
      throw e
    }
    if (other.resourceName !== this.resourceName) {
      throw new Error("mock: cannot merge tokens of different resources")
    }
    events.push({
      tag: "merge",
      resourceName: this.resourceName,
      otherResource: other.resourceName,
    })
    this.expectedUse += other.expectedUse
    this.lastCredit += other.lastCredit
  }

  toRecord(): QuotaTokenRecord {
    return {
      environmentId: this.environmentId,
      resourceName: this.resourceName,
      expectedUse: this.expectedUse,
      lastCredit: this.lastCredit,
      lastCreditAt: this.lastCreditAt,
    }
  }

  static fromRecord(serialized: QuotaTokenRecord): QuotaToken {
    return new QuotaToken(serialized, FROM_RECORD_BRAND)
  }
}
