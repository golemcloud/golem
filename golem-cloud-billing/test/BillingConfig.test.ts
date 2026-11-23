import { ConfigProvider, Effect, Exit } from "effect"
import { describe, expect, it } from "vitest"
import { type OveragePrices, overagePricesConfig, pastDueGraceMs, plansConfig } from "../src/BillingConfig.ts"
import { overagePrices } from "./support.ts"

const MINUTE = 60 * 1000

const read = (env: Record<string, string>) =>
  Effect.runPromiseExit(pastDueGraceMs.parse(ConfigProvider.fromEnvRecord(env)))

describe("GOLEM_CLOUD_BILLING_PAST_DUE_GRACE", () => {
  it("is 7 days when it is not set", async () => {
    expect(await read({})).toEqual(Exit.succeed(7 * 24 * 60 * MINUTE))
  })

  it("reads a duration, so that a test can use a short past-due grace", async () => {
    expect(await read({ GOLEM_CLOUD_BILLING_PAST_DUE_GRACE: "30 minutes" })).toEqual(Exit.succeed(30 * MINUTE))
    expect(await read({ GOLEM_CLOUD_BILLING_PAST_DUE_GRACE: "0 seconds" })).toEqual(Exit.succeed(0))
  })

  it("refuses a value that is not a finite duration", async () => {
    expect(Exit.isFailure(await read({ GOLEM_CLOUD_BILLING_PAST_DUE_GRACE: "7" }))).toBe(true)
    expect(Exit.isFailure(await read({ GOLEM_CLOUD_BILLING_PAST_DUE_GRACE: "Infinity" }))).toBe(true)
  })
})

describe("the plan IDs", () => {
  const free = "6d3b6a42-2d71-4b0e-9b1a-1f4e7b3b2c10"
  const paid = "9a1f3c2e-5b7d-4e8f-a6c4-2d1e0b9f8a73"
  const suspended = "3e8b1d47-0c2a-4f96-b5e1-7a9d2c4f6b08"
  const defaultPlan = "157dc684-00eb-496d-941c-da8fd1d15c63"
  const managed = {
    GOLEM_CLOUD_BILLING_FREE_PLAN_ID: free,
    GOLEM_CLOUD_BILLING_PAID_PLAN_ID: paid,
    GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID: suspended
  }

  const readPlans = (env: Record<string, string>) =>
    Effect.runPromiseExit(plansConfig.parse(ConfigProvider.fromEnvRecord(env)))

  it("has no default plan when GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID is not set", async () => {
    expect(await readPlans(managed)).toEqual(Exit.succeed({ plans: { free, paid, suspended }, defaultPlanId: undefined }))
  })

  it("reads the default plan of the registry", async () => {
    expect(await readPlans({ ...managed, GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID: defaultPlan })).toEqual(
      Exit.succeed({ plans: { free, paid, suspended }, defaultPlanId: defaultPlan })
    )
  })

  it("refuses a default plan that is not a UUID", async () => {
    expect(Exit.isFailure(await readPlans({ ...managed, GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID: "default" }))).toBe(true)
  })

  it("refuses a default plan that is one of the plans that subscriptions switch between", async () => {
    for (const id of [free, paid, suspended]) {
      expect(Exit.isFailure(await readPlans({ ...managed, GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID: id }))).toBe(true)
    }
  })

  it("refuses two plans that subscriptions switch between with the same ID", async () => {
    expect(Exit.isFailure(await readPlans({ ...managed, GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID: free }))).toBe(true)
  })
})

describe("the overage prices", () => {
  const variables = (code: string, prices: OveragePrices): Record<string, string> => ({
    [`GOLEM_CLOUD_BILLING_OVERAGE_${code}_PER_MILLION_GCU`]: prices.perMillionGcu,
    [`GOLEM_CLOUD_BILLING_OVERAGE_${code}_PER_GB_HOUR`]: prices.perGbHour,
    [`GOLEM_CLOUD_BILLING_OVERAGE_${code}_PER_DURABLE_GB_MONTH`]: prices.perDurableGbMonth,
    [`GOLEM_CLOUD_BILLING_OVERAGE_${code}_PER_EPHEMERAL_GB_MONTH`]: prices.perEphemeralGbMonth
  })
  const env = {
    ...variables("USD", overagePrices.usd),
    ...variables("EUR", overagePrices.eur),
    ...variables("GBP", overagePrices.gbp)
  }

  const readPrices = (env: Record<string, string>) =>
    Effect.runPromiseExit(overagePricesConfig.parse(ConfigProvider.fromEnvRecord(env)))

  it("reads the prices of each currency from the variables of its code", async () => {
    expect(await readPrices(env)).toEqual(Exit.succeed(overagePrices))
  })

  it("refuses a currency without its prices", async () => {
    const { GOLEM_CLOUD_BILLING_OVERAGE_GBP_PER_GB_HOUR: _, ...withoutOne } = env
    expect(Exit.isFailure(await readPrices(withoutOne))).toBe(true)
  })

  it("refuses a price that is not a decimal number", async () => {
    for (const value of ["€0.0131", "0,0131", "-0.0131", ".5", ""]) {
      expect(Exit.isFailure(await readPrices({ ...env, GOLEM_CLOUD_BILLING_OVERAGE_EUR_PER_MILLION_GCU: value }))).toBe(true)
    }
  })
})
