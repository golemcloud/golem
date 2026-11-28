/**
 * The billable overage of one account and month, calculated as the registry calculates
 * `allowOverageUsage` (`apply_billable_excess` in golem-registry-service/src/repo/account_usage.rs).
 *
 * The registry allocates the monthly amount to the policy revisions in revision order. Only the
 * usage that an `allowOverage` revision received above the monthly amount is billable. Usage under
 * a `hardLimit` revision uses the monthly amount but is never billable, also when enforcement
 * overshoots the amount.
 */

/** Usage in the exact internal units of the registry. */
export interface Usage {
  readonly computeFuel: bigint
  readonly memoryByteNanoseconds: bigint
  readonly durableStorageByteNanoseconds: bigint
  readonly ephemeralStorageByteNanoseconds: bigint
}

export const zeroUsage: Usage = {
  computeFuel: 0n,
  memoryByteNanoseconds: 0n,
  durableStorageByteNanoseconds: 0n,
  ephemeralStorageByteNanoseconds: 0n
}

/** A UTC calendar month, the billing period of the registry. */
export interface Period {
  readonly year: number
  readonly month: number
}

export type MonthlyUsageMode = "hardLimit" | "allowOverage"

/** The usage that one monthly policy revision of an account received in one month. */
export interface MonthlyPolicyUsage {
  readonly revision: bigint
  /** The month in which the registry created the revision. */
  readonly period: Period
  readonly mode: MonthlyUsageMode
  /** The usage of the month when the registry created the revision. */
  readonly baseline: Usage
  /** The monthly amount that the revision resolved. */
  readonly monthlyAmount: Usage
  /** The usage that the revision received in the month. */
  readonly delta: Usage
}

const dimensions = [
  "computeFuel",
  "memoryByteNanoseconds",
  "durableStorageByteNanoseconds",
  "ephemeralStorageByteNanoseconds"
] as const

const max = (a: bigint, b: bigint): bigint => (a > b ? a : b)

/**
 * The change of the billable excess when `delta` adds to `baseline`. It is negative for a refund
 * that takes usage back below a part of the excess.
 */
export const billableExcessDelta = (baseline: bigint, allowance: bigint, delta: bigint): bigint =>
  max(0n, max(0n, baseline + delta) - allowance) - max(0n, baseline - allowance)

const samePeriod = (a: Period, b: Period): boolean => a.year === b.year && a.month === b.month

/** The billable overage of one account in `period`, from the usage of each policy revision. */
export const billableOverage = (revisions: ReadonlyArray<MonthlyPolicyUsage>, period: Period): Usage => {
  const ordered = [...revisions].sort((a, b) => (a.revision < b.revision ? -1 : a.revision > b.revision ? 1 : 0))
  const first = ordered[0]
  const baseline: Record<(typeof dimensions)[number], bigint> = {
    ...(first !== undefined && samePeriod(first.period, period) ? first.baseline : zeroUsage)
  }
  const excess: Record<(typeof dimensions)[number], bigint> = { ...zeroUsage }
  for (const revision of ordered) {
    for (const dimension of dimensions) {
      if (revision.mode === "allowOverage") {
        excess[dimension] += billableExcessDelta(
          baseline[dimension],
          revision.monthlyAmount[dimension],
          revision.delta[dimension]
        )
      }
      baseline[dimension] = max(0n, baseline[dimension] + revision.delta[dimension])
    }
  }
  return {
    computeFuel: max(0n, excess.computeFuel),
    memoryByteNanoseconds: max(0n, excess.memoryByteNanoseconds),
    durableStorageByteNanoseconds: max(0n, excess.durableStorageByteNanoseconds),
    ephemeralStorageByteNanoseconds: max(0n, excess.ephemeralStorageByteNanoseconds)
  }
}

/** `FUEL_PER_GCU` of golem-common. */
export const FUEL_PER_GCU = 1_000_000n
/** `BYTE_NANOSECONDS_PER_GB_SECOND` of golem-common: one GiB for one second. */
export const BYTE_NANOSECONDS_PER_GB_SECOND = 1024n ** 3n * 1_000_000_000n
/** `BYTE_SECONDS_PER_GB_MONTH` of golem-common, in nanoseconds: one GiB for 730 hours. */
export const BYTE_NANOSECONDS_PER_GB_MONTH = 1024n ** 3n * 730n * 3600n * 1_000_000_000n

/** Usage in millionths of the customer units: GCU, GB-seconds and GB-month. */
export interface MicroUnits {
  readonly compute: bigint
  readonly memory: bigint
  readonly durableStorage: bigint
  readonly ephemeralStorage: bigint
}

const MICRO = 1_000_000n

/** Converts usage to millionths of the customer units. The conversion rounds down. */
export const toMicroUnits = (usage: Usage): MicroUnits => ({
  compute: (usage.computeFuel * MICRO) / FUEL_PER_GCU,
  memory: (usage.memoryByteNanoseconds * MICRO) / BYTE_NANOSECONDS_PER_GB_SECOND,
  durableStorage: (usage.durableStorageByteNanoseconds * MICRO) / BYTE_NANOSECONDS_PER_GB_MONTH,
  ephemeralStorage: (usage.ephemeralStorageByteNanoseconds * MICRO) / BYTE_NANOSECONDS_PER_GB_MONTH
})
