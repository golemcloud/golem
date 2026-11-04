import { Effect, Layer } from "effect"
import { afterAll, beforeAll, describe, expect, it } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import { BYTE_NANOSECONDS_PER_GB_SECOND, type Period, type Usage, zeroUsage } from "../src/BillableUsage.ts"
import * as Database from "../src/Database.ts"
import * as Metering from "../src/Metering.ts"
import { RegistryDatabase } from "./registry.ts"

const november = { year: 2026, month: 11 }
const october = { year: 2026, month: 10 }

describe("Metering", () => {
  const registry = new RegistryDatabase()

  const billableOverage = (period: Period) =>
    Effect.runPromise(
      Effect.gen(function* () {
        const metering = yield* Metering.Metering
        return yield* metering.billableOverage(period)
      }).pipe(
        Effect.provide(
          Metering.layer.pipe(
            Layer.provide(Database.layer),
            Layer.provide(Layer.succeed(BillingConfig, registry.config))
          )
        )
      )
    )

  beforeAll(() => registry.create(), 60_000)
  afterAll(() => registry.drop())

  it("reads the billable overage of the registry scenario through the metering tables only", async () => {
    // test_monthly_usage_attribution_uses_accrual_revision of the registry: the registry reports
    // 150,000 fuel of billable overage. The fractional remainders reach the allowOverage
    // revision, which resolved no memory or storage amount, so they are billable too.
    const accountId = await registry.account()
    const resolved = { computeFuel: 2_000_000n }
    await registry.revision(accountId, 0, "hard_limit", { resolved, period: november })
    await registry.revision(accountId, 1, "allow_overage", {
      baseline: { computeFuel: 1_900_000n },
      resolved,
      period: november
    })
    await registry.revision(accountId, 2, "hard_limit", {
      baseline: { computeFuel: 2_050_000n },
      resolved,
      period: november
    })
    const usage = (revision: number, dimensions: Parameters<typeof registry.attribution>[2]) =>
      registry.attribution(accountId, revision, dimensions, { period: november })
    await usage(0, { computeFuel: 1_900_000n })
    await usage(0, { computeFuel: 50_000n })
    await usage(1, { computeFuel: 100_000n })
    await usage(2, { computeFuel: 50_000n })
    await usage(1, { computeFuel: 40_000n })
    await usage(1, { computeFuel: 60_000n })
    await usage(1, { memoryRemainder: 11n, durableRemainder: 22n, ephemeralRemainder: 33n })

    const overage = await billableOverage(november)
    expect(overage.get(accountId)).toEqual<Usage>({
      computeFuel: 150_000n,
      memoryByteNanoseconds: 11n,
      durableStorageByteNanoseconds: 22n,
      ephemeralStorageByteNanoseconds: 33n
    })
  })

  it("combines whole units and remainders exactly", async () => {
    const accountId = await registry.account()
    await registry.revision(accountId, 0, "allow_overage", {
      resolved: { memoryGbSeconds: 1n, durableByteSeconds: 10n, ephemeralByteSeconds: 10n },
      period: november
    })
    await registry.attribution(
      accountId,
      0,
      {
        memoryGbSeconds: 2n,
        memoryRemainder: 5n,
        durableByteSeconds: 12n,
        durableRemainder: 500_000_000n,
        ephemeralByteSeconds: 9n,
        ephemeralRemainder: 999_999_999n
      },
      { period: november }
    )

    const overage = await billableOverage(november)
    expect(overage.get(accountId)).toEqual<Usage>({
      computeFuel: 0n,
      memoryByteNanoseconds: BYTE_NANOSECONDS_PER_GB_SECOND + 5n,
      durableStorageByteNanoseconds: 2_500_000_000n,
      ephemeralStorageByteNanoseconds: 0n
    })
  })

  it("does not count usage that a disabled meter reported", async () => {
    const accountId = await registry.account()
    await registry.revision(accountId, 0, "allow_overage", { period: november })
    await registry.attribution(
      accountId,
      0,
      { computeFuel: 7n, memoryGbSeconds: 1n, durableByteSeconds: 3n, ephemeralByteSeconds: 4n },
      { period: november, metering: { compute: false, memory: false, filesystem: false } }
    )
    await registry.attribution(accountId, 0, { computeFuel: 5n }, { period: november })

    const overage = await billableOverage(november)
    expect(overage.get(accountId)).toEqual<Usage>({ ...zeroUsage, computeFuel: 5n })
  })

  it("reads only the requested month, separately for each account", async () => {
    const first = await registry.account()
    const second = await registry.account()
    await registry.revision(first, 0, "allow_overage", { period: november })
    await registry.revision(second, 0, "allow_overage", { resolved: { computeFuel: 100n }, period: november })
    await registry.attribution(first, 0, { computeFuel: 10n }, { period: november })
    await registry.attribution(first, 0, { computeFuel: 1_000n }, { period: october })
    await registry.attribution(second, 0, { computeFuel: 150n }, { period: november })

    const inNovember = await billableOverage(november)
    expect(inNovember.get(first)?.computeFuel).toBe(10n)
    expect(inNovember.get(second)?.computeFuel).toBe(50n)

    const inOctober = await billableOverage(october)
    // The revision belongs to November, so its baseline does not apply to October.
    expect(inOctober.get(first)?.computeFuel).toBe(1_000n)
    expect(inOctober.has(second)).toBe(false)
  })
})
