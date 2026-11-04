import { Effect, Layer, Redacted } from "effect"
import { FetchHttpClient } from "effect/http"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import { BillingConfig } from "../src/BillingConfig.ts"
import * as Database from "../src/Database.ts"
import * as Metering from "../src/Metering.ts"
import * as Polar from "../src/Polar.ts"
import * as UsagePush from "../src/UsagePush.ts"
import { RegistryDatabase } from "./registry.ts"

const polarUrl = "https://polar.test"
const polarToken = "polar-test-token"
const november = { year: 2026, month: 11 }
const midNovember = new Date("2026-11-15T00:15:00Z")
const firstOfDecember = new Date("2026-12-01T00:15:00Z")
const GIB_SECOND = 1n

interface IngestedEvent {
  readonly name: string
  readonly external_customer_id: string
  readonly external_id: string
  readonly metadata: { readonly units: number; readonly micro_units: number; readonly period: string }
}

/** Polar's `POST /v1/events/ingest`. It skips an event whose `external_id` it has already. */
class FakePolar {
  readonly requests: Array<{ readonly authorization: string | null; readonly events: Array<IngestedEvent> }> = []
  readonly stored = new Map<string, IngestedEvent>()
  failures = 0

  readonly fetch = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init)
    if (request.method !== "POST" || request.url !== `${polarUrl}/v1/events/ingest`) {
      return new Response(`No fake for ${request.method} ${request.url}`, { status: 599 })
    }
    const { events } = (await request.json()) as { events: Array<IngestedEvent> }
    this.requests.push({ authorization: request.headers.get("authorization"), events })
    // Polar refuses the whole request when a numeric metadata value is outside ±(2^53 - 1).
    // JSON.parse rounds a larger integer to at least 2^53, so the check still sees it.
    const outOfRange = events.some((event) =>
      Object.values(event.metadata).some((value) => typeof value === "number" && Math.abs(value) > Number.MAX_SAFE_INTEGER)
    )
    if (outOfRange) {
      return Response.json({ detail: "metadata value out of range" }, { status: 422 })
    }
    if (this.failures > 0) {
      this.failures--
      return Response.json({ detail: "unavailable" }, { status: 503 })
    }
    let inserted = 0
    for (const event of events) {
      if (!this.stored.has(event.external_id)) {
        this.stored.set(event.external_id, event)
        inserted++
      }
    }
    return Response.json({ inserted, duplicates: events.length - inserted })
  }

  /** The sum of the units that Polar stored for one account and event name. */
  units(accountId: string, name: string): number {
    return [...this.stored.values()]
      .filter((event) => event.external_customer_id === accountId && event.name === name)
      .reduce((sum, event) => sum + event.metadata.units, 0)
  }
}

describe("UsagePush", () => {
  const registry = new RegistryDatabase()
  let polar: FakePolar

  const push = (now: Date) =>
    Effect.runPromise(
      UsagePush.pushUsage(now).pipe(
        Effect.provide(
          Layer.mergeAll(Metering.layer, Polar.layer).pipe(
            Layer.provideMerge(Database.layer),
            Layer.provide(FetchHttpClient.layer),
            Layer.provide(Layer.succeed(BillingConfig, registry.config)),
            Layer.provide(
              Layer.succeed(Polar.PolarConfig, { apiUrl: new URL(polarUrl), accessToken: Redacted.make(polarToken) })
            )
          )
        )
      )
    )

  /** An account in paid overage with a monthly amount of 1 GCU and 1 GB-second. */
  const overageAccount = async () => {
    const accountId = await registry.account()
    await registry.revision(accountId, 0, "allow_overage", {
      resolved: { computeFuel: 1_000_000n, memoryGbSeconds: GIB_SECOND },
      period: november
    })
    return accountId
  }

  beforeAll(() => registry.create(), 60_000)
  afterAll(() => registry.drop())
  beforeEach(() => {
    polar = new FakePolar()
    vi.stubGlobal("fetch", (input: string | URL | Request, init?: RequestInit) => polar.fetch(input, init))
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("pushes the billable overage of each dimension once", async () => {
    const accountId = await overageAccount()
    await registry.attribution(accountId, 0, { computeFuel: 1_500_000n, memoryGbSeconds: 3n }, { period: november })

    await push(midNovember)

    expect(polar.requests.every((request) => request.authorization === `Bearer ${polarToken}`)).toBe(true)
    expect([...polar.stored.values()].filter((event) => event.external_customer_id === accountId)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          name: "golem.compute",
          metadata: { units: 0.5, micro_units: 500_000, period: "2026-11" }
        }),
        expect.objectContaining({
          name: "golem.memory",
          metadata: { units: 2, micro_units: 2_000_000, period: "2026-11" }
        })
      ])
    )
    expect(polar.units(accountId, "golem.durable_storage")).toBe(0)

    const before = polar.requests.length
    await push(midNovember)
    expect(polar.requests.slice(before).flatMap((request) => request.events)).toEqual([])
  })

  it("pushes only the increase since the last push", async () => {
    const accountId = await overageAccount()
    await registry.attribution(accountId, 0, { computeFuel: 1_200_000n }, { period: november })
    await push(midNovember)
    await registry.attribution(accountId, 0, { computeFuel: 250_000n }, { period: november })
    await push(midNovember)

    const events = [...polar.stored.values()].filter((event) => event.external_customer_id === accountId)
    expect(events.map((event) => event.metadata.units)).toEqual([0.2, 0.25])
    expect(new Set(events.map((event) => event.external_id)).size).toBe(2)
  })

  it("splits an increase above the numeric limit of Polar, and sends the events behind it", async () => {
    const large = await overageAccount()
    const ordinary = await overageAccount()
    const limit = BigInt(Number.MAX_SAFE_INTEGER)
    await registry.attribution(large, 0, { computeFuel: 1_000_000n + limit + 2n }, { period: november })
    await registry.attribution(ordinary, 0, { computeFuel: 1_500_000n }, { period: november })

    await push(midNovember)

    const events = [...polar.stored.values()].filter((event) => event.external_customer_id === large)
    expect(events.map((event) => [event.external_id, event.metadata.micro_units])).toEqual([
      [`${large}:2026-11:compute:${limit}`, Number.MAX_SAFE_INTEGER],
      [`${large}:2026-11:compute:${limit + 2n}`, 2]
    ])
    expect(polar.units(ordinary, "golem.compute")).toBe(0.5)
  })

  it("sends an unsent event again with the same ID after Polar fails", async () => {
    const accountId = await overageAccount()
    await registry.attribution(accountId, 0, { computeFuel: 1_300_000n }, { period: november })
    polar.failures = 1
    await expect(push(midNovember)).rejects.toThrow()

    await registry.attribution(accountId, 0, { computeFuel: 100_000n }, { period: november })
    await push(midNovember)

    const sent = polar.requests.flatMap((request) => request.events).filter((e) => e.external_customer_id === accountId)
    // The failed event and its retry have the same ID; the new usage is a separate event.
    expect(sent.map((event) => event.metadata.units)).toEqual([0.3, 0.3, 0.1])
    expect(sent[0]?.external_id).toBe(sent[1]?.external_id)
    expect(polar.units(accountId, "golem.compute")).toBeCloseTo(0.4, 9)
  })

  it("pushes the rest of the previous month after the month changes", async () => {
    const accountId = await overageAccount()
    await registry.attribution(accountId, 0, { computeFuel: 1_100_000n }, { period: november })
    await push(midNovember)
    // Usage of November that the registry receives after the last push of November.
    await registry.attribution(accountId, 0, { computeFuel: 400_000n }, { period: november })
    await push(firstOfDecember)

    const events = [...polar.stored.values()].filter((event) => event.external_customer_id === accountId)
    expect(events.map((event) => [event.metadata.period, event.metadata.units])).toEqual([
      ["2026-11", 0.1],
      ["2026-11", 0.4]
    ])
  })

  it("sends nothing when the billable overage goes down", async () => {
    const accountId = await overageAccount()
    await registry.attribution(accountId, 0, { computeFuel: 1_500_000n }, { period: november })
    await push(midNovember)
    await registry.attribution(accountId, 0, { computeFuel: -200_000n }, { period: november })
    await push(midNovember)

    expect(polar.units(accountId, "golem.compute")).toBe(0.5)
  })

  it("sends nothing for usage within the monthly amount or under hardLimit", async () => {
    const within = await overageAccount()
    await registry.attribution(within, 0, { computeFuel: 900_000n }, { period: november })
    const capped = await registry.account()
    await registry.revision(capped, 0, "hard_limit", { resolved: { computeFuel: 1_000_000n }, period: november })
    await registry.attribution(capped, 0, { computeFuel: 1_200_000n }, { period: november })

    await push(midNovember)

    const accounts = new Set([...polar.stored.values()].map((event) => event.external_customer_id))
    expect(accounts.has(within)).toBe(false)
    expect(accounts.has(capped)).toBe(false)
  })
})
