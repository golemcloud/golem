import { describe, expect, it } from "vitest"
import {
  billableExcessDelta,
  billableOverage,
  type MonthlyPolicyUsage,
  toMicroUnits,
  type Usage,
  zeroUsage
} from "../src/BillableUsage.ts"

const period = { year: 2026, month: 11 }

const compute = (computeFuel: bigint): Usage => ({ ...zeroUsage, computeFuel })

const revision = (
  number: bigint,
  mode: MonthlyPolicyUsage["mode"],
  delta: Usage,
  overrides: Partial<MonthlyPolicyUsage> = {}
): MonthlyPolicyUsage => ({
  revision: number,
  period,
  mode,
  baseline: zeroUsage,
  monthlyAmount: compute(2_000_000n),
  delta,
  ...overrides
})

describe("billableExcessDelta", () => {
  // The cases of billable_excess_delta in golem-registry-service/src/repo/model/account_usage.rs.
  it("bills the part of a delta above the allowance, and refunds below it", () => {
    expect(billableExcessDelta(90n, 100n, 20n)).toBe(10n)
    expect(billableExcessDelta(150n, 100n, -100n)).toBe(-50n)
    expect(billableExcessDelta(90n, 100n, -20n)).toBe(0n)
  })
})

describe("billableOverage", () => {
  it("bills only usage above the allowance that an allowOverage revision received", () => {
    // The revisions of test_monthly_usage_attribution_uses_accrual_revision in the registry: a
    // 2 GCU allowance, 1.95 GCU under hardLimit, 0.2 GCU under allowOverage, and 0.05 GCU under
    // hardLimit again. The registry reports 150,000 fuel of billable overage.
    const revisions = [
      revision(0n, "hardLimit", compute(1_950_000n)),
      revision(1n, "allowOverage", compute(200_000n), { baseline: compute(1_900_000n) }),
      revision(2n, "hardLimit", compute(50_000n))
    ]
    expect(billableOverage(revisions, period).computeFuel).toBe(150_000n)
  })

  it("orders the revisions by number", () => {
    const revisions = [
      revision(2n, "hardLimit", compute(50_000n)),
      revision(0n, "hardLimit", compute(1_950_000n)),
      revision(1n, "allowOverage", compute(200_000n))
    ]
    expect(billableOverage(revisions, period).computeFuel).toBe(150_000n)
  })

  it("does not bill usage under hardLimit above the allowance", () => {
    const revisions = [
      revision(0n, "hardLimit", compute(2_500_000n)),
      revision(1n, "allowOverage", compute(100_000n))
    ]
    // The overshoot under hardLimit already used the allowance; the allowOverage revision adds
    // its whole usage above it, and nothing from before.
    expect(billableOverage(revisions, period).computeFuel).toBe(100_000n)
  })

  it("does not bill usage below the allowance", () => {
    const revisions = [revision(0n, "allowOverage", compute(1_999_999n))]
    expect(billableOverage(revisions, period)).toEqual(zeroUsage)
  })

  it("starts from the baseline of the first revision of the same month", () => {
    const revisions = [
      revision(5n, "allowOverage", compute(500_000n), { baseline: compute(1_800_000n) })
    ]
    expect(billableOverage(revisions, period).computeFuel).toBe(300_000n)
  })

  it("ignores the baseline of a first revision from another month", () => {
    const revisions = [
      revision(5n, "allowOverage", compute(500_000n), {
        baseline: compute(1_800_000n),
        period: { year: 2026, month: 10 }
      })
    ]
    expect(billableOverage(revisions, period).computeFuel).toBe(0n)
  })

  it("subtracts refunds and never reports a negative amount", () => {
    const revisions = [
      revision(0n, "allowOverage", compute(2_100_000n)),
      revision(1n, "allowOverage", compute(-300_000n))
    ]
    expect(billableOverage(revisions, period).computeFuel).toBe(0n)
  })

  it("keeps the four dimensions separate", () => {
    const memory = 1024n ** 3n * 1_000_000_000n
    const revisions = [
      revision(
        0n,
        "allowOverage",
        {
          computeFuel: 0n,
          memoryByteNanoseconds: 3n * memory,
          durableStorageByteNanoseconds: 7n,
          ephemeralStorageByteNanoseconds: 0n
        },
        {
          monthlyAmount: {
            computeFuel: 2_000_000n,
            memoryByteNanoseconds: 2n * memory,
            durableStorageByteNanoseconds: 5n,
            ephemeralStorageByteNanoseconds: 0n
          }
        }
      )
    ]
    expect(billableOverage(revisions, period)).toEqual({
      computeFuel: 0n,
      memoryByteNanoseconds: memory,
      durableStorageByteNanoseconds: 2n,
      ephemeralStorageByteNanoseconds: 0n
    })
  })
})

describe("toMicroUnits", () => {
  it("converts to millionths of the customer units, rounding down", () => {
    expect(
      toMicroUnits({
        computeFuel: 1_500_000n,
        memoryByteNanoseconds: 1024n ** 3n * 1_000_000_000n,
        durableStorageByteNanoseconds: 1024n ** 3n * 730n * 3600n * 1_000_000_000n,
        ephemeralStorageByteNanoseconds: 1n
      })
    ).toEqual({ compute: 1_500_000n, memory: 1_000_000n, durableStorage: 1_000_000n, ephemeralStorage: 0n })
  })
})
