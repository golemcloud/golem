import { createHmac, randomUUID } from "node:crypto"
import { Effect } from "effect"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import * as Settlement from "../src/Settlement.ts"
import {
  baseUrl,
  CookieJar,
  dropSchema,
  FakeUpstreams,
  freePlanId,
  internalPlanId,
  makeApp,
  makeServices,
  paidPlanId,
  polarProductId,
  polarWebToken,
  queryPool,
  registryBillingToken,
  request,
  signInWithGitHub,
  suspendedPlanId,
  testConfig,
  webhookSecret
} from "./support.ts"

const MINUTE = 60 * 1000
const DAY = 24 * 60 * MINUTE
const longDate = new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "UTC" })
const fixedPriceId = "price_fixed"
const meteredPriceId = "price_compute"

describe("Polar billing", () => {
  let config: BillingConfigShape
  let pool: pg.Pool
  let app: ReturnType<typeof makeApp>
  let services: ReturnType<typeof makeServices>
  let upstreams: FakeUpstreams

  beforeAll(() => {
    config = testConfig()
    pool = queryPool(config)
    app = makeApp(config)
    services = makeServices(config)
    vi.stubGlobal("fetch", (input: string | URL | Request, init?: RequestInit) => upstreams.fetch(input, init))
  })

  afterAll(async () => {
    vi.unstubAllGlobals()
    await app.dispose()
    await services.dispose()
    await dropSchema(pool, config.db.schema)
    await pool.end()
  })

  beforeEach(() => {
    upstreams = new FakeUpstreams()
  })

  afterEach(async () => {
    const schema = config.db.schema
    await pool.query(
      `TRUNCATE "${schema}"."user", "${schema}".polar_subscriptions, "${schema}".polar_orders, "${schema}".polar_webhook_deliveries CASCADE`
    )
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
    product: {
      id: polarProductId,
      prices: [
        { id: fixedPriceId, amount_type: "fixed" },
        { id: meteredPriceId, amount_type: "metered_unit" }
      ]
    },
    customer_id: "cus_1",
    customer: { id: "cus_1", external_id: accountId },
    ...overrides
  })

  /** A renewal order of `sub_1`: the price of the next period and, by default, paid overage. */
  const order = (accountId: string, overrides: Record<string, unknown> = {}) => ({
    id: "order_1",
    status: "pending",
    paid: false,
    created_at: new Date().toISOString(),
    modified_at: null,
    billing_reason: "subscription_cycle",
    customer_id: "cus_1",
    customer: { id: "cus_1", external_id: accountId },
    product_id: polarProductId,
    subscription_id: "sub_1",
    items: [
      { product_price_id: fixedPriceId, amount: 2000 },
      { product_price_id: meteredPriceId, amount: 345 }
    ],
    ...overrides
  })

  const priceOnly = { items: [{ product_price_id: fixedPriceId, amount: 2000 }] }
  const paidLater = { status: "paid", paid: true, modified_at: new Date(Date.now() + MINUTE).toISOString() }

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

  /** One run of the sweep of the web app at `now`. */
  const sweep = (now: Date) => services.runPromise(Effect.flatMap(Settlement.make, (settlement) => settlement.sweep(now)))


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

    it("links the stylesheet and the script with a hash of their content", async () => {
      const { jar } = await signedIn()

      const html = await page(jar)
      const stylesheet = /href="(\/assets\/app\.css\?v=[0-9a-f]{12})"/.exec(html)?.[1]
      const script = /src="(\/assets\/app\.js\?v=[0-9a-f]{12})"/.exec(html)?.[1]

      expect(stylesheet).toBeDefined()
      expect(script).toBeDefined()
      const response = await request(app.handler, jar, "GET", stylesheet!)
      expect(response.status).toBe(200)
      expect(response.headers.get("content-type")).toContain("text/css")
    })

    it("lets the forms redirect to Polar", async () => {
      const { jar } = await signedIn()

      const response = await request(app.handler, jar, "GET", "/")

      expect(response.headers.get("content-security-policy")).toContain(
        "form-action 'self' https://github.com https://polar.sh https://*.polar.sh"
      )
    })
  })

  describe("paid overage", () => {
    const paidAccount = async () => {
      const session = await signedIn()
      upstreams.planOf(session.accountId).planId = paidPlanId
      return session
    }

    const postOverage = (jar: CookieJar, mode: string, origin = baseUrl) =>
      request(
        app.handler,
        jar,
        "POST",
        "/billing/overage",
        { origin, "content-type": "application/x-www-form-urlencoded" },
        `mode=${mode}`
      )

    it("shows the switch and the overage prices on the paid plan", async () => {
      const { jar } = await paidAccount()

      const html = await page(jar)

      expect(html).toContain("Paid overage")
      expect(html).toContain("Off. When the account has used its monthly allowance, its agents stop")
      expect(html).toContain("$0.0112 per million GCU")
      expect(html).toContain("$0.045 per GB-hour")
      expect(html).toContain("$0.27 per GB-month")
      expect(html).toContain("$0.24 per GB-month")
      expect(html).toContain(`action="/billing/overage"`)
      expect(html).toContain(`value="allowOverage"`)
      expect(html).toContain("A change takes effect within 5 minutes.")
    })

    it("turns paid overage on with the token of the owner, and tells the delay", async () => {
      const { jar, accountId } = await paidAccount()

      const response = await postOverage(jar, "allowOverage")

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("/?overage=on")
      const [change, ...others] = upstreams.usageModeChanges()
      expect(others).toEqual([])
      expect(change!.url.pathname).toBe(`/v1/accounts/${accountId}/monthly-usage-mode`)
      expect(change!.headers.get("authorization")).toBe(`Bearer ${upstreams.registryAccount.secret}`)
      expect(change!.body).toEqual({ mode: "allowOverage" })
      const html = await page(jar, "/?overage=on")
      expect(html).toContain("Paid overage is on. The change takes effect within 5 minutes.")
      expect(html).toContain("On. When the account has used its monthly allowance, its agents keep running")
      expect(html).toContain(`value="hardLimit"`)
    })

    it("turns paid overage off", async () => {
      const { jar, accountId } = await paidAccount()
      upstreams.usageModes.set(accountId, "allowOverage")

      const response = await postOverage(jar, "hardLimit")

      expect(response.headers.get("location")).toBe("/?overage=off")
      expect(upstreams.usageModes.get(accountId)).toBe("hardLimit")
      expect(await page(jar, "/?overage=off")).toContain("Paid overage is off. The change takes effect within 5 minutes.")
    })

    it("treats a repeated post as done", async () => {
      const { jar, accountId } = await paidAccount()
      upstreams.usageModes.set(accountId, "allowOverage")

      const response = await postOverage(jar, "allowOverage")

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("/?overage=on")
    })

    it("tells the owner when the plan has no paid overage", async () => {
      const { jar } = await signedIn()

      const response = await postOverage(jar, "allowOverage")

      expect(response.headers.get("location")).toBe("/?overage=not-eligible")
      const html = await page(jar, "/?overage=not-eligible")
      expect(html).toContain("The plan of this account has no paid overage.")
      expect(html).not.toContain(`action="/billing/overage"`)
    })

    it("shows no switch on the free plan", async () => {
      const { jar } = await signedIn()

      expect(await page(jar)).not.toContain(`action="/billing/overage"`)
      expect(upstreams.requests.filter((request) => request.url.pathname.endsWith("/limits"))).toEqual([])
    })

    it("refuses an overage form that another site posts", async () => {
      const { jar } = await paidAccount()

      const response = await postOverage(jar, "allowOverage", "https://evil.test")

      expect(response.status).toBe(403)
      expect(upstreams.usageModeChanges()).toEqual([])
    })

    it("shows the usage of the month against the allowance", async () => {
      const { jar } = await paidAccount()

      const html = await page(jar)

      expect(html).toContain("Usage this month (UTC)")
      expect(html).toContain("2.5M of 100M GCU included · 3%")
      expect(html).toContain("250 of 912.5 GB-hours included · 27%")
      expect(html).toContain(`max="100000000" value="2500000"`)
      expect(html).toContain("Not metered")
      expect(html).toContain("No usage reported this month")
    })

    it("shows a used-up allowance and the paid overage above it", async () => {
      const { jar, accountId } = await paidAccount()
      await deliver("subscription.active", subscription(accountId))
      upstreams.usageModes.set(accountId, "allowOverage")
      upstreams.monthly = {
        ...(upstreams.monthly as object),
        computeGcu: {
          metering: "enabled",
          resolvedMonthlyAmount: 100_000_000,
          usage: 101_500_000,
          allowOverageUsage: 1_500_000,
          unit: "GCU"
        }
      }

      const html = await page(jar)

      expect(html).toContain("100M of 100M GCU included · 100%")
      expect(html).toContain(`class="full"`)
      expect(html).toContain("+ 1.5M GCU of paid overage · $0.02")
      expect(html).toContain("Paid overage so far this month: about $0.02 plus tax")
      expect(html).toContain("Polar bills it on the invoice of December 4, 2026, with the next month of the plan.")
    })

    it("shows a small usage with significant digits, not as 0", async () => {
      const { jar } = await paidAccount()
      upstreams.monthly = {
        ...(upstreams.monthly as object),
        memoryGbSeconds: { metering: "enabled", resolvedMonthlyAmount: 3_285_000, usage: 3.6, allowOverageUsage: 0, unit: "GB-seconds" }
      }

      const html = await page(jar)

      expect(html).toContain("0.001 of 912.5 GB-hours included · &lt;1%")
      expect(html).not.toContain("Paid overage so far this month")
    })

    it("shows the switch when the registry reports no usage", async () => {
      const { jar } = await paidAccount()
      upstreams.monthly = undefined

      const html = await page(jar)

      expect(html).not.toContain("Usage this month")
      expect(html).toContain(`action="/billing/overage"`)
    })

    it("refuses an unknown mode", async () => {
      const { jar } = await paidAccount()

      const response = await postOverage(jar, "unlimited")

      expect(response.status).toBe(400)
      expect(upstreams.usageModeChanges()).toEqual([])
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

    it("keeps the paid plan when the subscription is canceled at the end of a later period", async () => {
      const { jar, accountId } = await signedIn()
      const end = new Date(Date.now() + 30 * DAY)

      await deliver("subscription.active", subscription(accountId, { current_period_end: end.toISOString() }))
      await deliver(
        "subscription.canceled",
        subscription(accountId, {
          cancel_at_period_end: true,
          current_period_end: end.toISOString(),
          modified_at: "2026-11-05T10:00:00Z"
        })
      )

      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(await page(jar)).toContain(`ends on ${longDate.format(end)}`)
    })

    it("moves a canceled subscription to the free plan within the settlement margin before its end", async () => {
      const { jar, accountId } = await signedIn()
      const end = new Date(Date.now() + DAY)
      await deliver(
        "subscription.canceled",
        subscription(accountId, { cancel_at_period_end: true, current_period_end: end.toISOString() })
      )

      await sweep(new Date(end.getTime() - Settlement.SETTLEMENT_MARGIN_MS - MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)

      await sweep(new Date(end.getTime() - Settlement.SETTLEMENT_MARGIN_MS + MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(await page(jar)).toContain("You're on Free")

      await sweep(new Date(end.getTime() - MINUTE))
      expect(upstreams.planChanges()).toHaveLength(2)
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

    it("moves the account to Free when the payment of the price alone fails, and back after the payment", async () => {
      const { jar, accountId } = await signedIn()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId, priceOnly))
      const response = await deliver(
        "subscription.past_due",
        subscription(accountId, { status: "past_due", past_due_at: "2026-12-04T10:00:00Z", modified_at: "2026-12-04T10:00:00Z" })
      )

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      const html = await page(jar)
      expect(html).toContain("You're on Free")
      expect(html).toContain("The last payment for Paid failed")
      expect(html).not.toContain(`action="/billing/checkout"`)

      await deliver("order.paid", order(accountId, { ...priceOnly, ...paidLater }))
      await deliver("subscription.active", subscription(accountId, { modified_at: "2026-12-05T10:00:00Z" }))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
    })

    it("suspends the account when an order with overage is not paid, until the order is paid", async () => {
      const { jar, accountId } = await signedIn()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))
      await deliver("subscription.past_due", subscription(accountId, { status: "past_due", past_due_at: "2026-12-04T10:00:00Z", modified_at: "2026-12-04T10:00:00Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      const html = await page(jar)
      expect(html).toContain("Your account is suspended")
      expect(html).toContain("An invoice for paid overage is not paid")
      expect(html).toContain(`action="/billing/portal"`)

      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", past_due_at: "2026-12-04T10:00:00Z", modified_at: "2026-12-25T10:00:00Z" })
      )
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)

      await deliver("order.paid", order(accountId, paidLater))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("keeps a new subscription suspended while an order with overage is not paid", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))
      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", past_due_at: "2026-12-04T10:00:00Z", modified_at: "2026-12-25T10:00:00Z" })
      )
      await deliver("subscription.active", subscription(accountId, { id: "sub_2", modified_at: "2026-12-26T10:00:00Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
    })

    it("keeps the paid plan while Polar charges a renewal with overage", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))

      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(upstreams.planChanges()).toHaveLength(1)
    })

    it("suspends the account when the last order of an ended subscription stays unpaid", async () => {
      const accountId = randomUUID()
      const createdAt = new Date()

      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, { status: "canceled", modified_at: "2026-12-04T10:00:00Z" }))
      await deliver("order.created", order(accountId, { created_at: createdAt.toISOString(), billing_reason: "subscription_cycle" }))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)

      await sweep(new Date(createdAt.getTime() + Settlement.ORDER_PAYMENT_GRACE_MS - MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)

      await sweep(new Date(createdAt.getTime() + Settlement.ORDER_PAYMENT_GRACE_MS + MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
    })

    it("ignores an order of another product", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId, { product_id: randomUUID(), created_at: "2026-11-01T10:00:00Z" }))
      await deliver("subscription.past_due", subscription(accountId, { status: "past_due", past_due_at: "2026-12-04T10:00:00Z", modified_at: "2026-12-04T10:00:00Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("gives a trial the free plan, because Polar bills no usage during a trial", async () => {
      const accountId = randomUUID()

      await deliver("subscription.created", subscription(accountId, { status: "trialing" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(upstreams.planChanges()).toEqual([])
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
