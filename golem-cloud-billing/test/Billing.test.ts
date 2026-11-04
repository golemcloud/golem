import { createHmac, randomUUID } from "node:crypto"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import {
  baseUrl,
  CookieJar,
  dropSchema,
  FakeUpstreams,
  freePlanId,
  internalPlanId,
  makeApp,
  paidPlanId,
  polarProductId,
  polarWebToken,
  queryPool,
  registryBillingToken,
  request,
  signInWithGitHub,
  testConfig,
  webhookSecret
} from "./support.ts"

describe("Polar billing", () => {
  let config: BillingConfigShape
  let pool: pg.Pool
  let app: ReturnType<typeof makeApp>
  let upstreams: FakeUpstreams

  beforeAll(() => {
    config = testConfig()
    pool = queryPool(config)
    app = makeApp(config)
    vi.stubGlobal("fetch", (input: string | URL | Request, init?: RequestInit) => upstreams.fetch(input, init))
  })

  afterAll(async () => {
    vi.unstubAllGlobals()
    await app.dispose()
    await dropSchema(pool, config.db.schema)
    await pool.end()
  })

  beforeEach(() => {
    upstreams = new FakeUpstreams()
  })

  afterEach(async () => {
    const schema = config.db.schema
    await pool.query(`TRUNCATE "${schema}"."user", "${schema}".polar_subscriptions, "${schema}".polar_webhook_deliveries CASCADE`)
  })

  const signedIn = async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)
    return { jar, accountId: upstreams.registryAccount.accountId }
  }

  const subscription = (accountId: string, overrides: Record<string, unknown> = {}) => ({
    id: "sub_1",
    status: "active",
    created_at: "2026-11-04T10:00:00Z",
    modified_at: "2026-11-04T10:00:00Z",
    current_period_start: "2026-11-04T10:00:00Z",
    current_period_end: "2026-12-04T10:00:00Z",
    cancel_at_period_end: false,
    product_id: polarProductId,
    customer_id: "cus_1",
    customer: { id: "cus_1", external_id: accountId },
    ...overrides
  })

  /** Delivers a webhook as Polar does: Standard Webhooks headers, signed with the secret. */
  const deliver = (
    type: string,
    data: object,
    options: { readonly id?: string; readonly secret?: string } = {}
  ): Promise<Response> => {
    const body = JSON.stringify({ type, timestamp: new Date().toISOString(), api_version: "2026-10", data })
    const id = options.id ?? `msg_${randomUUID()}`
    const timestamp = Math.floor(Date.now() / 1000)
    const key = Buffer.from((options.secret ?? webhookSecret).slice("whsec_".length), "base64")
    const signature = createHmac("sha256", key).update(`${id}.${timestamp}.${body}`).digest("base64")
    return app.handler(
      new Request(new URL("/api/auth/polar/webhooks", baseUrl), {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "webhook-id": id,
          "webhook-timestamp": String(timestamp),
          "webhook-signature": `v1,${signature}`
        },
        body
      })
    )
  }

  const page = async (jar: CookieJar, path = "/") => (await request(app.handler, jar, "GET", path)).text()

  describe("checkout and customer portal", () => {
    it("starts a checkout of the paid product for the Golem account", async () => {
      const { jar, accountId } = await signedIn()

      const response = await request(app.handler, jar, "POST", "/billing/checkout", {
        origin: baseUrl,
        "x-forwarded-for": "203.0.113.7, 10.0.0.1"
      })

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("https://sandbox.polar.sh/checkout/polar_c_test")
      const [checkout, ...others] = upstreams.polarRequests("/v1/checkouts/")
      expect(others).toEqual([])
      expect(checkout!.headers.get("authorization")).toBe(`Bearer ${polarWebToken}`)
      expect(checkout!.body).toEqual({
        products: [polarProductId],
        external_customer_id: accountId,
        customer_email: "octocat@example.com",
        customer_name: "Octo Cat",
        customer_ip_address: "203.0.113.7",
        success_url: `${baseUrl}/?checkout_id={CHECKOUT_ID}`,
        return_url: `${baseUrl}/`,
        allow_discount_codes: true
      })
    })

    it("refuses a checkout form that another site posts", async () => {
      const { jar } = await signedIn()

      const response = await request(app.handler, jar, "POST", "/billing/checkout", { origin: "https://evil.test" })

      expect(response.status).toBe(403)
      expect(upstreams.polarRequests("/v1/checkouts/")).toEqual([])
    })

    it("sends a signed-out browser back to the sign-in page", async () => {
      const response = await request(app.handler, new CookieJar(), "POST", "/billing/checkout", { origin: baseUrl })

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("/")
      expect(upstreams.polarRequests("/v1/checkouts/")).toEqual([])
    })

    it("opens the customer portal of the Golem account", async () => {
      const { jar, accountId } = await signedIn()

      const response = await request(app.handler, jar, "POST", "/billing/portal", { origin: baseUrl })

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe(
        "https://sandbox.polar.sh/golem/portal?customer_session_token=polar_cst_test"
      )
      expect(upstreams.polarRequests("/v1/customer-sessions/")[0]!.body).toEqual({
        external_customer_id: accountId,
        return_url: `${baseUrl}/`
      })
    })

    it("shows the upgrade form on the free plan and the activation notice after a payment", async () => {
      const { jar } = await signedIn()

      expect(await page(jar)).toContain(`action="/billing/checkout"`)
      expect(await page(jar, "/?checkout_id=polar_c_test")).toContain("Activating Paid")
    })

    it("lets the forms redirect to Polar", async () => {
      const { jar } = await signedIn()

      const response = await request(app.handler, jar, "GET", "/")

      expect(response.headers.get("content-security-policy")).toContain(
        "form-action 'self' https://github.com https://polar.sh https://*.polar.sh"
      )
    })
  })

  describe("webhooks", () => {
    it("moves the account to the paid plan when its subscription is active", async () => {
      const { jar, accountId } = await signedIn()

      const response = await deliver("subscription.active", subscription(accountId))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      const [change] = upstreams.planChanges()
      expect(change!.headers.get("authorization")).toBe(`Bearer ${registryBillingToken}`)
      expect(change!.body).toEqual({ currentRevision: 0, plan: paidPlanId })
      const html = await page(jar)
      expect(html).toContain("You're on Paid")
      expect(html).toContain("renews on December 4, 2026")
      expect(html).toContain(`action="/billing/portal"`)
    })

    it("rejects a delivery with a wrong signature", async () => {
      const accountId = randomUUID()
      const otherSecret = `whsec_${Buffer.from("another secret of 32 bytes!!!!!!").toString("base64")}`

      const response = await deliver("subscription.active", subscription(accountId), { secret: otherSecret })

      expect(response.status).toBe(403)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("handles each webhook-id once", async () => {
      const accountId = randomUUID()

      const first = await deliver("subscription.active", subscription(accountId), { id: "msg_1" })
      upstreams.planOf(accountId).planId = freePlanId
      const second = await deliver("subscription.active", subscription(accountId), { id: "msg_1" })

      expect([first.status, second.status]).toEqual([200, 200])
      expect(await second.text()).toBe("already handled")
      expect(upstreams.planChanges()).toHaveLength(1)
    })

    it("keeps the paid plan until the period ends when the subscription is canceled", async () => {
      const { jar, accountId } = await signedIn()

      await deliver("subscription.active", subscription(accountId))
      await deliver(
        "subscription.canceled",
        subscription(accountId, { cancel_at_period_end: true, modified_at: "2026-11-05T10:00:00Z" })
      )

      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(await page(jar)).toContain("ends on December 4, 2026")
    })

    it("moves the account back to the free plan when the subscription is revoked", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, { status: "canceled", modified_at: "2026-12-04T10:00:00Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("does not undo a newer state with an older delivery", async () => {
      const accountId = randomUUID()

      await deliver("subscription.revoked", subscription(accountId, { status: "canceled", modified_at: "2026-11-04T11:00:00Z" }))
      await deliver("subscription.active", subscription(accountId, { modified_at: "2026-11-04T10:00:00Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("keeps the plan when a payment fails, and shows it on the page", async () => {
      const { jar, accountId } = await signedIn()

      await deliver("subscription.active", subscription(accountId))
      const response = await deliver(
        "subscription.past_due",
        subscription(accountId, { status: "past_due", modified_at: "2026-12-04T10:00:00Z" })
      )

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(await page(jar)).toContain("The last payment failed")
    })

    it("leaves a plan that an administrator gave the account", async () => {
      const accountId = randomUUID()
      upstreams.planOf(accountId).planId = internalPlanId

      const response = await deliver("subscription.active", subscription(accountId))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(internalPlanId)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("answers 500 when the registry refuses the billing account, so Polar delivers again", async () => {
      const accountId = randomUUID()
      upstreams.billingShare = false

      const failed = await deliver("subscription.active", subscription(accountId), { id: "msg_retry" })
      upstreams.billingShare = true
      const retried = await deliver("subscription.active", subscription(accountId), { id: "msg_retry" })

      expect(failed.status).toBe(500)
      expect(retried.status).toBe(200)
      expect(await retried.text()).toBe("handled")
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
    })

    it("reads the account again when the plan change conflicts", async () => {
      const accountId = randomUUID()
      upstreams.planConflicts = 1

      const response = await deliver("subscription.active", subscription(accountId))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(upstreams.planChanges()).toHaveLength(2)
    })

    it("ignores a subscription of another product", async () => {
      const accountId = randomUUID()

      const response = await deliver("subscription.active", subscription(accountId, { product_id: randomUUID() }))

      expect(response.status).toBe(200)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("ignores a subscription of a Polar customer without a Golem account", async () => {
      const response = await deliver(
        "subscription.active",
        subscription("unused", { customer: { id: "cus_2", external_id: null } })
      )

      expect(response.status).toBe(200)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("accepts an event type that the app does not handle", async () => {
      const response = await deliver("order.paid", { id: "order_1" })

      expect(response.status).toBe(200)
      expect(upstreams.planChanges()).toEqual([])
    })
  })
})
