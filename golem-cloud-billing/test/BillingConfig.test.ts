import { ConfigProvider, Effect, Exit } from "effect"
import { describe, expect, it } from "vitest"
import { pastDueGraceMs } from "../src/BillingConfig.ts"

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
