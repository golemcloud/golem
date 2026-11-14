import { createHmac, randomUUID } from "node:crypto"
import { Effect } from "effect"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import * as Settlement from "../src/Settlement.ts"
import { closePool, RegistryDatabase } from "./registry.ts"
import {
  baseUrl,
  billingAccountId,
  CookieJar,
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
  webhookSecret
} from "./support.ts"

const MINUTE = 60 * 1000
const DAY = 24 * 60 * MINUTE
const longDate = new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "UTC" })
const longDateTime = new Intl.DateTimeFormat("en", { dateStyle: "long", timeStyle: "short", timeZone: "UTC" })
const fixedPriceId = "price_fixed"
const meteredPriceId = "price_compute"

describe("Polar billing", () => {
  // The registry tables in `public` with the grants of the hosted deployment: the app reads
  // whether an account is deleted.
  const registry = new RegistryDatabase()
  let config: BillingConfigShape
  let pool: pg.Pool
  let app: ReturnType<typeof makeApp>
  let services: ReturnType<typeof makeServices>
  let upstreams: FakeUpstreams

  beforeAll(async () => {
    await registry.create()
    config = registry.configs.web
    pool = queryPool(registry.ownerConfig)
    app = makeApp(config)
    services = makeServices(config)
    vi.stubGlobal("fetch", (input: string | URL | Request, init?: RequestInit) => upstreams.fetch(input, init))
  }, 60_000)

  afterAll(async () => {
    vi.unstubAllGlobals()
    await app.dispose()
    await services.dispose()
    await closePool(pool)
    await registry.drop()
  })

  beforeEach(() => {
    upstreams = new FakeUpstreams()
  })

  afterEach(async () => {
    const schema = config.db.schema
    await pool.query(
      `TRUNCATE "${schema}"."user", "${schema}".polar_subscriptions, "${schema}".polar_orders,
         "${schema}".polar_webhook_deliveries, "${schema}".usage_events, "${schema}".deletion_holds,
         "${schema}".usage_settlements CASCADE`
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
      expect(await page(jar, "/?checkout_id=polar_c_test")).toContain("Polar has your payment")
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
      expect(html).toContain("Off. When the account uses all of its monthly allowance, its agents stop")
      expect(html).toContain("$0.0112 per million GCU")
      expect(html).toContain("$0.045 per GB-hour")
      expect(html).toContain("$0.27 per GB-month")
      expect(html).toContain("$0.24 per GB-month")
      expect(html).toContain(`action="/billing/overage"`)
      expect(html).toContain(`value="allowOverage"`)
      expect(html).toContain("Golem uses a new setting within 5 minutes.")
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
      expect(html).toContain("Paid overage is on. Golem uses the new setting within 5 minutes.")
      expect(html).toContain("On. When the account uses all of its monthly allowance, its agents continue to run.")
      expect(html).toContain(`value="hardLimit"`)
    })

    it("turns paid overage off", async () => {
      const { jar, accountId } = await paidAccount()
      upstreams.usageModes.set(accountId, "allowOverage")

      const response = await postOverage(jar, "hardLimit")

      expect(response.headers.get("location")).toBe("/?overage=off")
      expect(upstreams.usageModes.get(accountId)).toBe("hardLimit")
      expect(await page(jar, "/?overage=off")).toContain("Paid overage is off. Golem uses the new setting within 5 minutes.")
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

      expect(html).toContain("Monthly usage")
      expect(html).toContain("This month (UTC), against the monthly allowance of the plan.")
      expect(html).toContain("2.5M of 100M GCU included · 3%")
      expect(html).toContain("250 of 912.5 GB-hours included · 27%")
      expect(html).toContain(`max="100000000" value="2500000"`)
      expect(html).toContain("Not metered")
      expect(html).toContain("No usage this month")
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
      expect(html).toContain("Polar shows this amount after a delay.")
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

      expect(html).toContain("Golem did not send the usage of this month.")
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
    /** The `subscription.past_due` state of `sub_1` after a payment failed at `failedAt`. */
    const pastDue = (accountId: string, failedAt: Date) =>
      subscription(accountId, { status: "past_due", past_due_at: failedAt.toISOString(), modified_at: "2026-12-04T10:00:00Z" })

    it("moves the account to the paid plan when its subscription is active", async () => {
      const { jar, accountId } = await signedIn()

      const response = await deliver("subscription.active", subscription(accountId))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      const [change] = upstreams.planChanges()
      expect(change!.headers.get("authorization")).toBe(`Bearer ${registryBillingToken}`)
      expect(change!.body).toEqual({ currentRevision: 0, plan: paidPlanId })
      const html = await page(jar)
      expect(html).toContain("<h1>Paid</h1>")
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
      expect(await page(jar)).toContain(`stops on ${longDate.format(end)}`)
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
      expect(await page(jar)).toContain("<h1>Free</h1>")

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

    it("keeps Paid in the past-due grace when the payment of the price fails, then gives Free until the payment", async () => {
      const { jar, accountId } = await signedIn()
      const failedAt = new Date()
      const graceEnd = new Date(failedAt.getTime() + config.pastDueGraceMs)

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId, priceOnly))
      const response = await deliver("subscription.past_due", pastDue(accountId, failedAt))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(await page(jar)).toContain(
        `The last payment for Paid failed. Change the payment method in the Polar customer portal before ${longDateTime.format(graceEnd)} UTC.`
      )

      await sweep(new Date(graceEnd.getTime() - MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)

      await sweep(new Date(graceEnd.getTime() + MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      const html = await page(jar)
      expect(html).toContain("<h1>Free</h1>")
      expect(html).toContain("The last payment for Paid failed")
      expect(html).not.toContain(`action="/billing/checkout"`)

      await deliver("order.paid", order(accountId, { ...priceOnly, ...paidLater }))
      await deliver("subscription.active", subscription(accountId, { modified_at: "2026-12-05T10:00:00Z" }))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
    })

    it("keeps Paid when Polar gets the payment in the past-due grace", async () => {
      const { jar, accountId } = await signedIn()
      const failedAt = new Date()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))
      await deliver("subscription.past_due", pastDue(accountId, failedAt))
      await deliver("order.paid", order(accountId, paidLater))
      await deliver("subscription.updated", subscription(accountId, { modified_at: "2026-12-05T10:00:00Z" }))
      await sweep(new Date(failedAt.getTime() + config.pastDueGraceMs + MINUTE))

      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      expect(upstreams.planChanges()).toHaveLength(1)
      expect(await page(jar)).not.toContain("The last payment for Paid failed")
    })

    it("suspends the account after the past-due grace when an order with overage is not paid, until the order is paid", async () => {
      const { jar, accountId } = await signedIn()
      const failedAt = new Date()

      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))
      await deliver("subscription.past_due", pastDue(accountId, failedAt))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)

      await sweep(new Date(failedAt.getTime() + config.pastDueGraceMs + MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      const html = await page(jar)
      expect(html).toContain("<h1>Suspended</h1>")
      expect(html).toContain("Polar did not get the payment of an invoice for paid overage, so Golem stopped the agents of this account.")
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
      await deliver("subscription.past_due", pastDue(accountId, new Date(Date.now() - config.pastDueGraceMs - MINUTE)))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("gives a trial the free plan, because Polar bills no usage during a trial", async () => {
      const accountId = randomUUID()

      await deliver("subscription.created", subscription(accountId, { status: "trialing" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(upstreams.planChanges()).toEqual([])
    })

    it("leaves a plan that an administrator gave the account, and holds its deletion all the same", async () => {
      const accountId = randomUUID()
      upstreams.planOf(accountId).planId = internalPlanId

      const response = await deliver("subscription.active", subscription(accountId))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(internalPlanId)
      expect(upstreams.planChanges()).toEqual([])
      expect(upstreams.deletionHolds.has(accountId)).toBe(true)
    })

    it("answers 500 when the registry refuses the billing account, so Polar delivers again", async () => {
      // The account exists and is not deleted: the registry hides it because the permission share
      // of the billing account is missing.
      const accountId = await registry.account()
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

    it("answers 200 to a webhook of a deleted account and keeps its subscription", async () => {
      const accountId = await registry.account()
      await registry.deleteAccount(accountId)
      upstreams.deletedAccounts.add(accountId)

      const response = await deliver("subscription.revoked", subscription(accountId, { status: "canceled" }))

      expect(response.status).toBe(200)
      expect(await response.text()).toBe("handled")
      expect(upstreams.planChanges()).toEqual([])
      expect(upstreams.holdChanges()).toEqual([])
      const stored = await pool.query("SELECT status FROM polar_subscriptions WHERE account_id = $1", [accountId])
      expect(stored.rows).toEqual([{ status: "canceled" }])
    })
  })

  describe("deletion hold", () => {
    const subscriptionHold = `Golem Cloud billing: The account has a subscription to Paid. Cancel it in Manage billing at ${baseUrl}. The subscription then runs to the end of the period. Delete the account after the period ends.`
    const canceledHold =
      "Golem Cloud billing: The subscription to Paid is canceled and runs to the end of the period. Delete the account after the period ends."
    const unbilledHold =
      "Golem Cloud billing: The account has paid overage that Polar has not billed yet. Polar bills it on the invoice at the end of the period. Delete the account after that invoice is paid."
    const unsettledHold =
      "Golem Cloud billing: The billing of the paid overage of the account is not final yet. Try again in an hour. If the delete still fails, contact support at contact@golem.cloud."
    const unpaidHold = `Golem Cloud billing: An invoice for paid overage is not paid. Pay it in Manage billing at ${baseUrl}.`
    /** The state of `sub_1` after Polar ended it at its period end, December 4. */
    const endedAtPeriodEnd = { status: "canceled", modified_at: "2026-12-04T10:00:00Z", ended_at: "2026-12-04T10:00:00Z" }

    /** A usage event of the usage push, sent `ago` milliseconds before now, or held then, or not sent. */
    const usageEvent = async (
      accountId: string,
      ago: number,
      options: { readonly held?: boolean; readonly unsent?: boolean } = {}
    ) => {
      await pool.query(
        `INSERT INTO usage_events
           (event_id, account_id, period, dimension, from_micro_units, to_micro_units, created_at, sent_at, held_at)
         VALUES ($1, $2, '2026-11', 'compute', 0, 5, $3, $4, $5)`,
        [
          `${accountId}:2026-11:compute:${randomUUID()}`,
          accountId,
          new Date(Date.now() - ago - MINUTE),
          options.held || options.unsent ? null : new Date(Date.now() - ago),
          options.held ? new Date(Date.now() - ago) : null
        ]
      )
    }

    it("holds the account of a subscription as the billing account until the subscription ends, also after the cancellation", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))

      expect(upstreams.deletionHolds.get(accountId)).toBe(subscriptionHold)
      const [placed] = upstreams.holdChanges()
      expect(placed!.method).toBe("PUT")
      expect(placed!.url.pathname).toBe(`/v1/accounts/${accountId}/deletion-holds/${billingAccountId}`)
      expect(placed!.headers.get("authorization")).toBe(`Bearer ${registryBillingToken}`)

      await deliver(
        "subscription.updated",
        subscription(accountId, { cancel_at_period_end: true, modified_at: "2026-11-05T10:00:00Z" })
      )
      expect(upstreams.deletionHolds.get(accountId)).toBe(canceledHold)

      await deliver("subscription.revoked", subscription(accountId, { status: "canceled", modified_at: "2026-12-04T10:00:00Z" }))
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      expect(upstreams.holdChanges().at(-1)!.method).toBe("DELETE")
    })

    it("places the hold before the move to the paid plan, and removes it after the move to the free plan", async () => {
      const accountId = randomUUID()

      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, { status: "canceled", modified_at: "2026-12-04T10:00:00Z" }))

      const changes = upstreams.requests
        .filter((request) => request.method !== "GET")
        .filter((request) => request.url.pathname.includes("/deletion-holds/") || request.url.pathname.endsWith("/plan"))
        .map((request) => `${request.method} ${request.url.pathname.endsWith("/plan") ? "plan" : "hold"}`)
      expect(changes).toEqual(["PUT hold", "PUT plan", "PUT plan", "DELETE hold"])
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("holds a billable subscription that no reconcile has held yet, at the next sweep", async () => {
      // A subscription from before the app placed holds: no webhook has reconciled it since.
      const accountId = randomUUID()
      await pool.query(
        `INSERT INTO polar_subscriptions
           (subscription_id, account_id, product_id, status, current_period_end, cancel_at_period_end, modified_at,
            metered_price_ids)
         VALUES ($1, $2, $3, 'active', now() + interval '20 days', false, now(), '{}')`,
        [`sub_${accountId}`, accountId, polarProductId]
      )

      await sweep(new Date())
      expect(upstreams.deletionHolds.get(accountId)).toBe(subscriptionHold)
      const recorded = await pool.query("SELECT reason FROM deletion_holds WHERE account_id = $1", [accountId])
      expect(recorded.rows).toEqual([{ reason: subscriptionHold }])

      await sweep(new Date())
      expect(upstreams.holdChanges()).toHaveLength(1)
    })

    it("holds an ended subscription while sent overage waits for its order, and while the order is unpaid", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await usageEvent(accountId, MINUTE)

      await deliver("subscription.revoked", subscription(accountId, endedAtPeriodEnd))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      await deliver("order.created", order(accountId))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)

      await deliver("order.paid", order(accountId, paidLater))
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("keeps the hold while the usage push has not sent an event, also after a newer order", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      // A failed ingestion: the event is older than the last order, and Polar never received it.
      await usageEvent(accountId, MINUTE, { unsent: true })
      await deliver("subscription.revoked", subscription(accountId, endedAtPeriodEnd))

      await deliver("order.paid", order(accountId, { status: "paid", paid: true }))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      // The next push holds the event, because the subscription has ended.
      await pool.query("UPDATE usage_events SET held_at = clock_timestamp() WHERE account_id = $1", [accountId])
      await sweep(new Date())
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("settles an event that looks newer than the last order once Polar created that order after the period end", async () => {
      const accountId = randomUUID()
      const periodEnd = new Date(Date.now() - 20 * MINUTE).toISOString()
      await deliver("subscription.active", subscription(accountId, { current_period_end: periodEnd }))
      await usageEvent(accountId, MINUTE)
      await deliver(
        "subscription.revoked",
        subscription(accountId, {
          status: "canceled",
          current_period_end: periodEnd,
          ended_at: periodEnd,
          modified_at: new Date(Date.now() - 15 * MINUTE).toISOString()
        })
      )
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      // The last order: Polar billed the event on it before the usage push recorded `sent_at`.
      const lastOrderAt = new Date(Date.now() - 10 * MINUTE).toISOString()
      await deliver("order.created", order(accountId, { created_at: lastOrderAt }))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)

      await deliver("order.paid", order(accountId, { ...paidLater, created_at: lastOrderAt }))
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("keeps the hold of an ended subscription until its last order comes, with no time limit", async () => {
      const accountId = randomUUID()
      const periodEnd = new Date(Date.now() - 3 * DAY).toISOString()
      await deliver("subscription.active", subscription(accountId, { current_period_end: periodEnd }))
      await usageEvent(accountId, 3 * DAY + MINUTE)
      await deliver(
        "subscription.revoked",
        subscription(accountId, {
          status: "canceled",
          current_period_end: periodEnd,
          ended_at: periodEnd,
          modified_at: new Date(Date.now() - 2 * DAY).toISOString()
        })
      )

      await sweep(new Date())

      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)
    })

    /** `sub_1` as Polar revokes it before its period end: `ended_at` is `ago` milliseconds before now. */
    const revokedEarly = (accountId: string, ago: number) => {
      const endedAt = new Date(Date.now() - ago).toISOString()
      return subscription(accountId, { status: "canceled", modified_at: endedAt, ended_at: endedAt })
    }

    it("removes the hold when Polar revokes the subscription before its period end, and drops the later overage", async () => {
      // Polar bills paid overage only on the order at the period end, and the product has no meter
      // cycle, so it never bills the event that it received after the newest order.
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await deliver(
        "order.paid",
        order(accountId, { status: "paid", paid: true, created_at: new Date(Date.now() - 10 * MINUTE).toISOString() })
      )
      await usageEvent(accountId, 5 * MINUTE)

      await deliver("subscription.revoked", revokedEarly(accountId, MINUTE))

      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      expect(upstreams.holdChanges().at(-1)!.method).toBe("DELETE")
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("keeps the hold of an early revocation while an order with overage is not paid", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId, { created_at: new Date(Date.now() - 10 * MINUTE).toISOString() }))
      await usageEvent(accountId, 5 * MINUTE)

      await deliver("subscription.revoked", revokedEarly(accountId, MINUTE))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)

      await deliver(
        "order.paid",
        order(accountId, { ...paidLater, created_at: new Date(Date.now() - 10 * MINUTE).toISOString() })
      )
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("keeps the hold of an early revocation while the usage push has not sent or held an event", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await usageEvent(accountId, MINUTE, { unsent: true })

      await deliver("subscription.revoked", revokedEarly(accountId, MINUTE))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      await pool.query("UPDATE usage_events SET held_at = clock_timestamp() WHERE account_id = $1", [accountId])
      await sweep(new Date())
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("waits for the last order of the subscription that ended last, after an early revocation of an older one", async () => {
      // A monthly subscription that Polar revoked keeps its later period end. A daily subscription
      // after it ended at its period end, and its last order has not come.
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", revokedEarly(accountId, 2 * DAY))
      const dailyEnd = new Date(Date.now() - 20 * MINUTE).toISOString()
      const daily = { id: "sub_2", current_period_end: dailyEnd, created_at: new Date(Date.now() - DAY).toISOString() }
      await deliver("subscription.active", subscription(accountId, { ...daily, modified_at: daily.created_at }))
      const firstOrderAt = new Date(Date.now() - DAY).toISOString()
      await deliver(
        "order.paid",
        order(accountId, { id: "order_2", subscription_id: "sub_2", status: "paid", paid: true, created_at: firstOrderAt })
      )
      await usageEvent(accountId, 30 * MINUTE)

      await deliver(
        "subscription.revoked",
        subscription(accountId, {
          ...daily,
          status: "canceled",
          cancel_at_period_end: true,
          ended_at: dailyEnd,
          modified_at: dailyEnd
        })
      )
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      const lastOrderAt = new Date(Date.now() - 10 * MINUTE).toISOString()
      await deliver(
        "order.paid",
        order(accountId, { id: "order_3", subscription_id: "sub_2", status: "paid", paid: true, created_at: lastOrderAt })
      )
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("counts the overage that a billable subscription bills, after an early revocation of an older one", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", revokedEarly(accountId, 2 * DAY))
      const startedAt = new Date(Date.now() - DAY).toISOString()
      await deliver("subscription.active", subscription(accountId, { id: "sub_2", created_at: startedAt, modified_at: startedAt }))
      await usageEvent(accountId, MINUTE)

      await deliver(
        "order.paid",
        order(accountId, { id: "order_2", subscription_id: "sub_2", status: "paid", paid: true, created_at: startedAt })
      )

      expect(upstreams.deletionHolds.get(accountId)).toBe(
        `${subscriptionHold} ${unbilledHold.replace("Golem Cloud billing: ", "")}`
      )
    })

    it("removes the hold after an operator records a usage settlement, and counts a later event again", async () => {
      // The last order of a subscription that ended at its period end never came. An operator
      // checked the account in Polar and recorded a settlement outside the app.
      const accountId = randomUUID()
      const periodEnd = new Date(Date.now() - 20 * MINUTE).toISOString()
      await deliver("subscription.active", subscription(accountId, { current_period_end: periodEnd }))
      await usageEvent(accountId, 10 * MINUTE)
      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", current_period_end: periodEnd, ended_at: periodEnd, modified_at: periodEnd })
      )
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      await pool.query("INSERT INTO usage_settlements (account_id, settled_through) VALUES ($1, $2)", [
        accountId,
        new Date(Date.now() - 5 * MINUTE)
      ])
      await sweep(new Date())
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)

      await usageEvent(accountId, 0)
      await sweep(new Date())
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)
    })

    it("checks a recorded hold again with no time limit, and removes it when the account owes nothing", async () => {
      // A removal that failed long ago: the registry and the record still have the hold, and the
      // account has no subscription and no recent usage event.
      const accountId = randomUUID()
      upstreams.deletionHolds.set(accountId, unpaidHold)
      await pool.query(
        "INSERT INTO deletion_holds (account_id, reason, updated_at) VALUES ($1, $2, now() - interval '30 days')",
        [accountId, unpaidHold]
      )

      await sweep(new Date())

      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      const recorded = await pool.query("SELECT reason FROM deletion_holds WHERE account_id = $1", [accountId])
      expect(recorded.rows).toEqual([])
    })

    it("frees an ended subscription when the usage push holds the overage that Polar would not bill", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      const eventId = `${accountId}:2026-11:compute:5`
      await pool.query(
        `INSERT INTO usage_events (event_id, account_id, period, dimension, from_micro_units, to_micro_units)
         VALUES ($1, $2, '2026-11', 'compute', 0, 5)`,
        [eventId, accountId]
      )
      await deliver("subscription.revoked", subscription(accountId, endedAtPeriodEnd))
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      await pool.query("UPDATE usage_events SET held_at = clock_timestamp() WHERE event_id = $1", [eventId])
      await sweep(new Date())

      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("does not count overage that an order already bills", async () => {
      const accountId = randomUUID()
      await usageEvent(accountId, 2 * MINUTE)
      await usageEvent(accountId, DAY, { held: true })

      await deliver("order.paid", order(accountId, { status: "paid", paid: true }))

      expect(upstreams.holdChanges().map((change) => change.method)).toEqual(["DELETE"])
    })

    it("lists every reason in one hold", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await usageEvent(accountId, MINUTE)

      await deliver("order.created", order(accountId, { created_at: new Date(Date.now() - 10 * MINUTE).toISOString() }))

      const sentence = (hold: string) => hold.replace("Golem Cloud billing: ", "")
      expect(upstreams.deletionHolds.get(accountId)).toBe(
        `${subscriptionHold} ${sentence(unbilledHold)} ${sentence(unpaidHold)}`
      )
    })
  })
})
