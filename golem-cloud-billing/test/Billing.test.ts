import { createHmac, randomUUID } from "node:crypto"
import { Effect } from "effect"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import * as Cleanup from "../src/Cleanup.ts"
import { type Currency, currencies } from "../src/Currency.ts"
import { webhookBodyLimit } from "../src/Http.ts"
import * as Settlement from "../src/Settlement.ts"
import * as Subscriptions from "../src/Subscriptions.ts"
import { closePool, RegistryDatabase } from "./registry.ts"
import {
  baseUrl,
  billingAccountId,
  cleanupLayer,
  CookieJar,
  FakeUpstreams,
  freePlanId,
  internalPlanId,
  makeApp,
  makeServices,
  overagePrices,
  paidPlanId,
  polarApiUrl,
  polarCleanupToken,
  polarProductId,
  polarSettlementProductId,
  polarWebToken,
  queryPool,
  registryBillingToken,
  registryDefaultPlanId,
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

/** The fixed and the metered price of the paid product in the currency; USD has the IDs above. */
const priceIdsOf = (currency: Currency) =>
  currency === "usd"
    ? { fixed: fixedPriceId, metered: meteredPriceId }
    : { fixed: `${fixedPriceId}_${currency}`, metered: `${meteredPriceId}_${currency}` }

const pricesOf = (currency: Currency) => [
  { id: priceIdsOf(currency).fixed, amount_type: "fixed", price_currency: currency },
  { id: priceIdsOf(currency).metered, amount_type: "metered_unit", price_currency: currency }
]

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
         "${schema}".polar_checkouts, "${schema}".polar_webhook_deliveries, "${schema}".usage_events,
         "${schema}".deletion_holds, "${schema}".deleted_account_cleanups, "${schema}".usage_settlements,
         "${schema}".overage_settlements CASCADE`
    )
  })

  const signedIn = async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)
    return { jar, accountId: upstreams.registryAccount.accountId }
  }

  /**
   * `sub_1` as Polar sends it, in USD unless `overrides` give another currency. The subscription
   * lists its prices, in its currency, and its product lists the prices of every currency.
   */
  const subscription = (accountId: string, overrides: Record<string, unknown> = {}) => {
    const currency = (overrides["currency"] as Currency | undefined) ?? "usd"
    return {
      id: "sub_1",
      status: "active",
      currency,
      created_at: "2026-11-04T10:00:00Z",
      modified_at: "2026-11-04T10:00:00Z",
      current_period_start: "2026-11-04T10:00:00Z",
      current_period_end: "2026-12-04T10:00:00Z",
      cancel_at_period_end: false,
      product_id: polarProductId,
      product: { id: polarProductId, prices: currencies.flatMap(pricesOf) },
      prices: pricesOf(currency),
      customer_id: "cus_1",
      customer: { id: "cus_1", external_id: accountId },
      ...overrides
    }
  }

  /** A renewal order of `sub_1`: the price of the next period and, by default, paid overage. */
  /** An order as Polar sends it: no discount and no tax unless `overrides` give them. */
  const order = (accountId: string, overrides: Record<string, unknown> = {}) => {
    const currency = (overrides["currency"] as Currency | undefined) ?? "usd"
    const items = (overrides["items"] as ReadonlyArray<{ readonly amount: number }> | undefined) ?? [
      { product_price_id: priceIdsOf(currency).fixed, amount: 2000, tax_amount: 0, proration: false },
      { product_price_id: priceIdsOf(currency).metered, amount: 345, tax_amount: 0, proration: false }
    ]
    const subtotal = items.reduce((sum, item) => sum + item.amount, 0)
    return {
      id: "order_1",
      status: "pending",
      currency,
      paid: false,
      created_at: new Date().toISOString(),
      modified_at: null,
      billing_reason: "subscription_cycle",
      customer_id: "cus_1",
      customer: { id: "cus_1", external_id: accountId },
      product_id: polarProductId,
      subscription_id: "sub_1",
      subtotal_amount: subtotal,
      discount_amount: 0,
      net_amount: subtotal,
      tax_amount: 0,
      total_amount: subtotal,
      applied_balance_amount: 0,
      due_amount: subtotal,
      items,
      ...overrides
    }
  }

  const priceOnly = { items: [{ product_price_id: fixedPriceId, amount: 2000, tax_amount: 0, proration: false }] }
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
        // The client sent the first address; the gateway appended the second, which it saw.
        "x-forwarded-for": "203.0.113.7, 198.51.100.7"
      })

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("https://sandbox.polar.sh/checkout/polar_c_1")
      const [checkout, ...others] = upstreams.polarRequests("/v1/checkouts/")
      expect(others).toEqual([])
      expect(checkout!.headers.get("authorization")).toBe(`Bearer ${polarWebToken}`)
      expect(checkout!.body).toEqual({
        products: [polarProductId],
        external_customer_id: accountId,
        customer_email: "octocat@example.com",
        customer_name: "Octo Cat",
        customer_ip_address: "198.51.100.7",
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

    const postCheckout = (jar: CookieJar) => request(app.handler, jar, "POST", "/billing/checkout", { origin: baseUrl })

    /** The route sends a refused checkout back to the page, and Polar gets no request. */
    const expectRefused = (response: Response) => {
      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("/")
      expect(upstreams.polarRequests("/v1/checkouts/")).toEqual([])
    }

    it.each([
      ["the paid plan", paidPlanId],
      ["the suspended plan", suspendedPlanId],
      ["a custom plan", internalPlanId]
    ])("refuses a checkout on %s, and the page does not offer one", async (_, planId) => {
      const { jar, accountId } = await signedIn()
      upstreams.planOf(accountId).planId = planId

      expectRefused(await postCheckout(jar))
      expect(await page(jar)).not.toContain(`action="/billing/checkout"`)
    })

    it("offers Paid to an account on the default plan of the registry, and starts its checkout", async () => {
      const { jar, accountId } = await signedIn()
      upstreams.planOf(accountId).planId = registryDefaultPlanId

      const body = await page(jar)
      expect(body).toContain("<h1>Default</h1>")
      expect(body).toContain("When the subscription ends, your plan changes to Free, not back to Default.")
      expect(body).toContain(`action="/billing/checkout"`)
      const response = await postCheckout(jar)
      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("https://sandbox.polar.sh/checkout/polar_c_1")
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
    })

    it("refuses a checkout on the free plan while the subscription is past due", async () => {
      const { jar, accountId } = await signedIn()
      const failedAt = new Date(Date.now() - config.pastDueGraceMs - MINUTE)
      await deliver(
        "subscription.past_due",
        subscription(accountId, { status: "past_due", past_due_at: failedAt.toISOString() })
      )
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)

      expectRefused(await postCheckout(jar))
      expect(await page(jar)).not.toContain(`action="/billing/checkout"`)
    })

    it("offers the customer portal, not a checkout, on the default plan while the subscription is past due", async () => {
      // The first reconcile comes after the past-due grace, so `planFor` gives the free plan, and the
      // account stays on the default plan.
      const { jar, accountId } = await signedIn()
      upstreams.planOf(accountId).planId = registryDefaultPlanId
      const failedAt = new Date(Date.now() - config.pastDueGraceMs - MINUTE)
      await deliver(
        "subscription.past_due",
        subscription(accountId, { status: "past_due", past_due_at: failedAt.toISOString() })
      )
      expect(upstreams.planOf(accountId).planId).toBe(registryDefaultPlanId)

      expectRefused(await postCheckout(jar))
      const html = await page(jar)
      expect(html).toContain("<h1>Default</h1>")
      expect(html).not.toContain(`action="/billing/checkout"`)
      expect(html).toContain(`action="/billing/portal"`)
      expect(html).toContain("The last payment for Paid failed.")
    })

    it("refuses a checkout when the registry does not send the account", async () => {
      const { jar } = await signedIn()
      upstreams.accountReadStatus = 503

      expectRefused(await postCheckout(jar))
      expect(await page(jar)).toContain("Golem did not send the plan of the account.")
    })

    it("refuses a checkout when the database cannot read the subscription", async () => {
      const { jar } = await signedIn()
      const schema = config.db.schema
      await pool.query(`ALTER TABLE "${schema}".polar_subscriptions RENAME TO polar_subscriptions_hidden`)
      try {
        expectRefused(await postCheckout(jar))
      } finally {
        await pool.query(`ALTER TABLE "${schema}".polar_subscriptions_hidden RENAME TO polar_subscriptions`)
      }
    })

    it("refuses a checkout when the database cannot read the session", async () => {
      const { jar } = await signedIn()
      const schema = config.db.schema
      await pool.query(`ALTER TABLE "${schema}"."session" RENAME TO session_hidden`)
      try {
        expectRefused(await postCheckout(jar))
      } finally {
        await pool.query(`ALTER TABLE "${schema}".session_hidden RENAME TO "session"`)
      }
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

    it("shows the switch and the overage prices of the currency of the subscription on the paid plan", async () => {
      const { jar, accountId } = await paidAccount()
      await deliver("subscription.active", subscription(accountId))

      const html = await page(jar)

      expect(html).toContain("Paid overage")
      expect(html).toContain("Off. When the account uses all of its monthly allowance, its agents stop")
      expect(html).toContain("$0.0112 per million GCU")
      expect(html).toContain("$0.045 per GB-hour")
      expect(html).toContain("$0.27 per GB-month")
      expect(html).toContain("$0.24 per GB-month")
      expect(html).toContain("Prices in USD, plus tax.")
      expect(html).toContain(`action="/billing/overage"`)
      expect(html).toContain(`value="allowOverage"`)
      expect(html).toContain("Golem uses a new setting within 5 minutes.")
    })

    it("shows the switch and the usage, without prices or costs, when the page cannot read the subscription", async () => {
      const { jar, accountId } = await paidAccount()
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

      expect(html).toContain("This page cannot read the subscription, so it does not show the overage prices.")
      expect(html).toContain("Polar bills the usage above the allowance at the overage prices of the subscription.")
      expect(html).toContain("+ 1.5M GCU of paid overage</dd>")
      expect(html).not.toContain("per million GCU")
      expect(html).not.toContain("Paid overage so far this month")
      expect(html).toContain(`value="hardLimit"`)
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
      expect(html).toContain(
        "This month, against the monthly allowance of the plan. The monthly allowance starts again on the 1st of each month (UTC)."
      )
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

  describe("form posts", () => {
    const routes = ["/sign-in/github", "/sign-out", "/billing/checkout", "/billing/portal", "/billing/overage"]
    const origins: ReadonlyArray<readonly [string, Record<string, string>]> = [
      ["without Origin", {}],
      ["with Origin null", { origin: "null" }],
      ["from another site", { origin: "https://evil.test" }]
    ]

    it.each(routes.flatMap((route) => origins.map(([name, headers]) => [route, name, headers] as const)))(
      "refuses a post to %s %s",
      async (route, _, headers) => {
        const { jar } = await signedIn()
        const before = upstreams.requests.length

        const response = await request(
          app.handler,
          jar,
          "POST",
          route,
          { "content-type": "application/x-www-form-urlencoded", ...headers },
          "mode=allowOverage"
        )

        expect(response.status).toBe(403)
        expect(upstreams.requests.slice(before)).toEqual([])
      }
    )
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

    it("rejects a delivery without the signature headers before it reads the body", async () => {
      let read = false
      // With a high-water mark of 0, the stream calls `pull` only when a reader asks for data.
      const body = new ReadableStream<Uint8Array>(
        {
          pull: (controller) => {
            read = true
            controller.enqueue(new TextEncoder().encode("{}"))
            controller.close()
          }
        },
        { highWaterMark: 0 }
      )

      const response = await app.handler(
        new Request(new URL("/api/auth/polar/webhooks", baseUrl), {
          method: "POST",
          headers: { "content-type": "application/json" },
          body,
          duplex: "half"
        } as RequestInit)
      )

      expect(response.status).toBe(403)
      expect(read).toBe(false)
    })

    it("answers 413 to a signed delivery whose body is above the limit", async () => {
      const accountId = randomUUID()
      const padding = "x".repeat(Number(webhookBodyLimit))

      const response = await deliver("subscription.active", subscription(accountId, { metadata: { padding } }))

      expect(response.status).toBe(413)
      expect(upstreams.planChanges()).toEqual([])
      expect(upstreams.holdChanges()).toEqual([])
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

    // Polar sends timestamps with microseconds; a JavaScript Date keeps milliseconds only.
    it("does not undo a newer state with an older delivery of the same millisecond", async () => {
      const accountId = randomUUID()

      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", modified_at: "2026-11-04T11:00:00.123900Z" })
      )
      await deliver("subscription.active", subscription(accountId, { modified_at: "2026-11-04T11:00:00.123100Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      const stored = await pool.query("SELECT status FROM polar_subscriptions WHERE account_id = $1", [accountId])
      expect(stored.rows).toEqual([{ status: "canceled" }])
    })

    it("does not make a paid order unpaid with an older delivery of the same millisecond", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      const createdAt = new Date(Date.now() - 2 * Settlement.ORDER_PAYMENT_GRACE_MS).toISOString()

      await deliver(
        "order.paid",
        order(accountId, { status: "paid", paid: true, created_at: createdAt, modified_at: "2026-11-04T11:00:00.123900Z" })
      )
      await deliver("order.updated", order(accountId, { created_at: createdAt, modified_at: "2026-11-04T11:00:00.123100Z" }))

      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      const stored = await pool.query("SELECT status FROM polar_orders WHERE account_id = $1", [accountId])
      expect(stored.rows).toEqual([{ status: "paid" }])
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
      expect(html).toContain("An invoice for paid overage is not paid, so Golem stopped the agents of this account.")
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

    it("moves an account on the default plan to Paid with its subscription, and to Free after the end", async () => {
      const accountId = randomUUID()
      upstreams.planOf(accountId).planId = registryDefaultPlanId

      await deliver("subscription.active", subscription(accountId))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)

      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", modified_at: "2026-11-04T11:00:00Z" })
      )
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("leaves an account on the default plan while its subscription does not make it paid", async () => {
      const accountId = randomUUID()
      upstreams.planOf(accountId).planId = registryDefaultPlanId

      const response = await deliver("subscription.created", subscription(accountId, { status: "trialing" }))

      expect(response.status).toBe(200)
      expect(upstreams.planOf(accountId).planId).toBe(registryDefaultPlanId)
      expect(upstreams.planChanges()).toEqual([])
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
      "Golem Cloud billing: The billing of the paid overage of the account is not final yet. Try again in an hour. If the delete still fails, contact Golem Cloud at sales@golem.cloud."
    const unpaidHold = `Golem Cloud billing: An invoice for paid overage is not paid. Pay it on the billing page at ${baseUrl}.`
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
            metered_price_ids, currency)
         VALUES ($1, $2, $3, 'active', now() + interval '20 days', false, now(), '{}', 'usd')`,
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

    it("removes the hold at the next sweep after an operator records a usage settlement for old usage", async () => {
      // The usage events are older than a day and the hold is new, so only the settlement makes the
      // sweep check the account again.
      const accountId = randomUUID()
      const periodEnd = new Date(Date.now() - 3 * DAY).toISOString()
      await deliver("subscription.active", subscription(accountId, { current_period_end: periodEnd }))
      await usageEvent(accountId, 3 * DAY + MINUTE)
      await deliver(
        "subscription.revoked",
        subscription(accountId, { status: "canceled", current_period_end: periodEnd, ended_at: periodEnd, modified_at: periodEnd })
      )
      await sweep(new Date())
      expect(upstreams.deletionHolds.get(accountId)).toBe(unsettledHold)

      await pool.query("INSERT INTO usage_settlements (account_id, settled_through) VALUES ($1, clock_timestamp())", [
        accountId
      ])
      await sweep(new Date())

      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
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

  describe("checkout hold", () => {
    const subscriptionHold = `Golem Cloud billing: The account has a subscription to Paid. Cancel it in Manage billing at ${baseUrl}. The subscription then runs to the end of the period. Delete the account after the period ends.`
    const paidCheckoutHold =
      "Golem Cloud billing: Polar has a payment for Paid, and Golem Cloud billing has not recorded its subscription yet. Try again in an hour. If the delete still fails, contact Golem Cloud at sales@golem.cloud."
    /**
     * The hold of an open checkout that expires at `expiresAt`: the job of Polar expires it every 15
     * minutes, and the sweep reads it every 5.
     */
    const openCheckoutHold = (expiresAt: Date) =>
      `Golem Cloud billing: A checkout for Paid from the billing page is open. Delete the account after ${longDateTime.format(new Date(expiresAt.getTime() + 30 * MINUTE))} UTC.`
    const lateCheckoutHold =
      "Golem Cloud billing: A checkout for Paid from the billing page is past its expiry, and Polar has not closed it yet. Try again in an hour. If the delete still fails, contact Golem Cloud at sales@golem.cloud."

    /** One run of the cleanup CronJob, with the database role and the Polar token of the cleanup. */
    const cleanup = () =>
      Effect.runPromise(
        Cleanup.cleanupDeletedAccounts.pipe(
          Effect.provide(cleanupLayer(registry.configs.cleanup, polarApiUrl, polarCleanupToken))
        )
      )

    /** A signed-in person whose Golem account is a row of the registry tables, which the cleanup reads. */
    const signedInToRegistryAccount = async () => {
      const accountId = await registry.account()
      upstreams.registryAccounts.set(upstreams.github.accessToken, { accountId, secret: `golem_${accountId}` })
      const { jar } = await signedIn()
      return { jar, accountId }
    }

    const postCheckout = (jar: CookieJar) => request(app.handler, jar, "POST", "/billing/checkout", { origin: baseUrl })
    const expiryOf = (checkoutId: string) => upstreams.checkouts.get(checkoutId)!.expiresAt
    /** Whether `promise` settles within `ms` milliseconds. */
    const settlesWithin = (promise: Promise<unknown>, ms: number) =>
      Promise.race([promise.then(() => true), new Promise<boolean>((resolve) => setTimeout(() => resolve(false), ms))])
    const at = (time: Date, offset: number) => new Date(time.getTime() + offset)

    it("holds the account before it sends the checkout URL, until Polar expires the checkout", async () => {
      const { jar, accountId } = await signedInToRegistryAccount()

      const response = await postCheckout(jar)

      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe(upstreams.checkoutUrl("polar_c_1"))
      const expiresAt = expiryOf("polar_c_1")
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiresAt))
      const steps = upstreams.requests
        .filter((request) => request.method !== "GET")
        .map((request) => (request.url.pathname.includes("/deletion-holds/") ? "hold" : request.url.pathname))
      expect(steps.slice(-2)).toEqual(["/v1/checkouts/", "hold"])

      // Polar can take the payment until the expiry, so the sweep does not read the checkout before.
      await sweep(at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS - MINUTE))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toEqual([])
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiresAt))

      // The job of Polar has not expired the checkout yet.
      await sweep(at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(1)
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiresAt))

      upstreams.checkouts.get("polar_c_1")!.status = "expired"
      const read = at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS)
      await sweep(at(read, Settlement.CHECKOUT_READ_INTERVAL_MS - MINUTE))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(1)
      await sweep(at(read, Settlement.CHECKOUT_READ_INTERVAL_MS))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(2)
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      await sweep(at(read, 60 * MINUTE))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(2)
    })

    it("keeps the hold of a checkout that Polar still reports open after its expiry, until the subscription of a late payment comes", async () => {
      // The person paid just before the expiry. Polar answers the read from a replica that lags,
      // so the sweep reads an `open` checkout.
      const { jar, accountId } = await signedInToRegistryAccount()
      await postCheckout(jar)
      const expiresAt = expiryOf("polar_c_1")

      await sweep(at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS))
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiresAt))

      // After the time that the hold names, the text names a contact.
      await sweep(at(expiresAt, 30 * MINUTE))
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1").length).toBeGreaterThan(1)
      expect(upstreams.deletionHolds.get(accountId)).toBe(lateCheckoutHold)

      await deliver("subscription.active", subscription(accountId, { checkout_id: "polar_c_1" }))
      expect(upstreams.deletionHolds.get(accountId)).toBe(subscriptionHold)
    })

    it("keeps the hold of a paid checkout after its expiry until the subscription webhook comes", async () => {
      const { jar, accountId } = await signedInToRegistryAccount()
      await postCheckout(jar)
      const expiresAt = expiryOf("polar_c_1")
      upstreams.checkouts.get("polar_c_1")!.status = "succeeded"

      await sweep(at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS))
      expect(upstreams.deletionHolds.get(accountId)).toBe(paidCheckoutHold)

      // The webhook is late by hours; the sweep does not read the paid checkout again.
      await sweep(at(expiresAt, 3 * 60 * MINUTE))
      expect(upstreams.deletionHolds.get(accountId)).toBe(paidCheckoutHold)
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(1)

      await deliver("subscription.active", subscription(accountId, { checkout_id: "polar_c_1" }))
      expect(upstreams.deletionHolds.get(accountId)).toBe(subscriptionHold)
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
    })

    it("cancels the subscription of a checkout that an account deleted by an administrator starts, and frees the sign-in", async () => {
      const { jar, accountId } = await signedInToRegistryAccount()
      await postCheckout(jar)
      expect(upstreams.deletionHolds.has(accountId)).toBe(true)
      // An administrator removes the hold, and the registry deletes the account.
      upstreams.deletionHolds.delete(accountId)
      await registry.deleteAccount(accountId)
      upstreams.deletedAccounts.add(accountId)

      expect(await cleanup()).toEqual({ deletedCustomers: 0, unsettled: 1, canceledSubscriptions: 0, deletedUsers: 0 })

      // The person pays the open checkout: Polar creates the customer and the subscription.
      upstreams.polarCustomers.add(accountId)
      expect((await deliver("subscription.active", subscription(accountId, { checkout_id: "polar_c_1" }))).status).toBe(200)

      expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, canceledSubscriptions: 1, deletedUsers: 1 })
      expect(upstreams.polarCustomers.has(accountId)).toBe(false)
      expect(await cleanup()).toEqual({ deletedCustomers: 0, unsettled: 0, canceledSubscriptions: 0, deletedUsers: 0 })
      expect(upstreams.requests.filter((request) => request.method === "DELETE" && request.url.pathname.startsWith("/v1/customers/"))).toHaveLength(1)

      // The same GitHub identity signs in to a new account.
      const replacement = await registry.account()
      upstreams.registryAccounts.set(upstreams.github.accessToken, { accountId: replacement, secret: `golem_${replacement}` })
      const callback = await signInWithGitHub(app.handler, new CookieJar())
      expect(new URL(callback.headers.get("location")!, baseUrl).searchParams.get("error")).toBeNull()
    })

    it("cancels a subscription that the app records while the cleanup deletes the Polar customer", async () => {
      const { accountId } = await signedInToRegistryAccount()
      upstreams.polarCustomers.add(accountId)
      await registry.deleteAccount(accountId)
      upstreams.deletedAccounts.add(accountId)
      const deleting = Promise.withResolvers<void>()
      const deleted = Promise.withResolvers<void>()
      upstreams.onCustomerDelete = async () => {
        deleting.resolve()
        await deleted.promise
      }

      const run = cleanup()
      await deleting.promise
      // Polar has a new customer of the account, and sends its subscription.
      upstreams.polarCustomers.add(accountId)
      const delivery = deliver("subscription.active", subscription(accountId))

      // The webhook waits for the lock of the account until the cleanup recorded the delete.
      expect(await settlesWithin(delivery, 200)).toBe(false)
      deleted.resolve()
      expect(await run).toEqual({ deletedCustomers: 1, unsettled: 0, canceledSubscriptions: 0, deletedUsers: 1 })
      expect((await delivery).status).toBe(200)

      upstreams.onCustomerDelete = undefined
      expect(await cleanup()).toEqual({ deletedCustomers: 1, unsettled: 0, canceledSubscriptions: 1, deletedUsers: 0 })
      expect(upstreams.polarCustomers.has(accountId)).toBe(false)
    })

    it("serves five clicks of one account at once with a pool of five connections", async () => {
      const { jar } = await signedIn()
      const creating = Promise.withResolvers<void>()
      const created = Promise.withResolvers<void>()
      upstreams.onCheckoutCreate = async () => {
        creating.resolve()
        await created.promise
      }

      const first = postCheckout(jar)
      await creating.promise
      const others = Array.from({ length: 4 }, () => postCheckout(jar))
      // The other clicks wait for the lock of the account while the first click holds it.
      await new Promise((resolve) => setTimeout(resolve, 200))
      created.resolve()

      const responses = await Promise.all([first, ...others])
      expect(responses.map((response) => response.headers.get("location"))).toEqual(
        Array.from({ length: 5 }, () => upstreams.checkoutUrl("polar_c_1"))
      )
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
    })

    it("sends a second click back to the open checkout", async () => {
      const { jar } = await signedIn()
      await postCheckout(jar)

      const second = await postCheckout(jar)

      expect(second.headers.get("location")).toBe(upstreams.checkoutUrl("polar_c_1"))
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(1)
    })

    /** Makes the checkout take payments for 30 minutes more. */
    const expireSoon = async (checkoutId: string) => {
      const expiresAt = new Date(Date.now() + 30 * MINUTE)
      upstreams.checkouts.get(checkoutId)!.expiresAt = expiresAt
      await pool.query("UPDATE polar_checkouts SET expires_at = $2 WHERE checkout_id = $1", [checkoutId, expiresAt])
    }

    it("starts a new checkout when the open one takes payments for less than an hour more, and holds the account until the later expiry", async () => {
      const { jar, accountId } = await signedIn()
      await postCheckout(jar)
      await expireSoon("polar_c_1")

      const second = await postCheckout(jar)

      expect(second.headers.get("location")).toBe(upstreams.checkoutUrl("polar_c_2"))
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiryOf("polar_c_2")))
    })

    it.each(["confirmed", "succeeded"] as const)(
      "sends each later click to the payment notice while Polar has the payment of a %s checkout",
      async (status) => {
        const { jar, accountId } = await signedIn()
        await postCheckout(jar)
        upstreams.checkouts.get("polar_c_1")!.status = status

        const second = await postCheckout(jar)
        const third = await postCheckout(jar)

        expect(second.headers.get("location")).toBe(`${baseUrl}/?checkout_id=polar_c_1`)
        expect(third.headers.get("location")).toBe(`${baseUrl}/?checkout_id=polar_c_1`)
        expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
        expect(upstreams.deletionHolds.get(accountId)).toBe(paidCheckoutHold)
      }
    )

    it("sends a click to the payment notice when the person paid the checkout in its last hour", async () => {
      const { jar } = await signedIn()
      await postCheckout(jar)
      await expireSoon("polar_c_1")
      upstreams.checkouts.get("polar_c_1")!.status = "confirmed"

      const second = await postCheckout(jar)

      expect(second.headers.get("location")).toBe(`${baseUrl}/?checkout_id=polar_c_1`)
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
    })

    it("sends a click back to the checkout when its payment failed and Polar opened it again", async () => {
      const { jar, accountId } = await signedIn()
      await postCheckout(jar)
      upstreams.checkouts.get("polar_c_1")!.status = "confirmed"
      expect((await postCheckout(jar)).headers.get("location")).toBe(`${baseUrl}/?checkout_id=polar_c_1`)
      upstreams.checkouts.get("polar_c_1")!.status = "open"

      const third = await postCheckout(jar)

      expect(third.headers.get("location")).toBe(upstreams.checkoutUrl("polar_c_1"))
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiryOf("polar_c_1")))
    })

    it("reads a checkout in the sweep under the lock of the account, so a click meanwhile sees the payment", async () => {
      const { jar } = await signedIn()
      await postCheckout(jar)
      await expireSoon("polar_c_1")
      const reading = Promise.withResolvers<void>()
      const read = Promise.withResolvers<void>()
      let reads = 0
      upstreams.onCheckoutRead = async () => {
        reads++
        if (reads > 1) {
          // A replica of Polar that lags.
          return "open"
        }
        reading.resolve()
        await read.promise
        return "succeeded"
      }

      const swept = sweep(at(expiryOf("polar_c_1"), Settlement.CHECKOUT_READ_DELAY_MS))
      await reading.promise
      const click = postCheckout(jar)

      // The click waits for the lock of the account until the sweep recorded the payment.
      expect(await settlesWithin(click, 200)).toBe(false)
      read.resolve()
      await swept
      expect((await click).headers.get("location")).toBe(`${baseUrl}/?checkout_id=polar_c_1`)
      expect(upstreams.polarRequests("/v1/checkouts/")).toHaveLength(1)
    })

    it("keeps the status of a checkout that the sweep cannot read, and reads it again after 5 minutes", async () => {
      const { jar, accountId } = await signedIn()
      await postCheckout(jar)
      const expiresAt = expiryOf("polar_c_1")
      upstreams.onCheckoutRead = () => Promise.reject(new Error("Polar is unavailable"))

      const read = at(expiresAt, Settlement.CHECKOUT_READ_DELAY_MS)
      await sweep(read)
      await sweep(at(read, Settlement.CHECKOUT_READ_INTERVAL_MS - MINUTE))

      expect(upstreams.polarRequests("/v1/checkouts/polar_c_1")).toHaveLength(1)
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiresAt))
      upstreams.onCheckoutRead = undefined
      upstreams.checkouts.get("polar_c_1")!.status = "expired"
      await sweep(at(read, Settlement.CHECKOUT_READ_INTERVAL_MS))
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
    })

    it("keeps a final status of a checkout when a later read from Polar reports an older one", async () => {
      const { jar, accountId } = await signedIn()
      await postCheckout(jar)
      upstreams.checkouts.get("polar_c_1")!.status = "succeeded"
      await postCheckout(jar)

      await services.runPromise(
        Effect.flatMap(Subscriptions.Subscriptions, (subscriptions) =>
          subscriptions.recordCheckoutStatus("polar_c_1", "confirmed", new Date())
        )
      )

      const { rows } = await pool.query("SELECT status FROM polar_checkouts WHERE checkout_id = 'polar_c_1'")
      expect(rows).toEqual([{ status: "succeeded" }])
      expect(upstreams.deletionHolds.get(accountId)).toBe(paidCheckoutHold)
    })

    it("sends no checkout URL when the hold cannot be placed, and the sweep places the hold", async () => {
      const { jar, accountId } = await signedIn()
      upstreams.billingShare = false

      const response = await postCheckout(jar)

      expect(response.status).toBe(500)
      expect(response.headers.get("location")).toBeNull()
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)

      upstreams.billingShare = true
      await sweep(new Date())
      expect(upstreams.deletionHolds.get(accountId)).toBe(openCheckoutHold(expiryOf("polar_c_1")))
    })
  })

  describe("settlement of unpaid paid overage", () => {
    const unpaidHold = `Golem Cloud billing: An invoice for paid overage is not paid. Pay it on the billing page at ${baseUrl}.`
    const ended = { status: "canceled", modified_at: "2026-12-04T10:00:00Z", ended_at: "2026-12-04T10:00:00Z" }
    /** The last order of `sub_1` after a cancellation: created after the period end, unpaid for two hours. */
    const lastOrder = { created_at: new Date(Date.now() - 120 * MINUTE).toISOString() }

    /** An order of the settlement product, paid through the checkout of `POST /billing/settle`. */
    const settlementOrder = (accountId: string, overrides: Record<string, unknown> = {}) => ({
      id: "order_settlement_1",
      status: "paid",
      paid: true,
      created_at: new Date().toISOString(),
      modified_at: null,
      billing_reason: "purchase",
      currency: "usd",
      customer_id: "cus_1",
      customer: { id: "cus_1", external_id: accountId },
      product_id: polarSettlementProductId,
      subscription_id: null,
      subtotal_amount: 345,
      discount_amount: 0,
      net_amount: 345,
      tax_amount: 0,
      items: [{ product_price_id: "price_ad_hoc", amount: 345, tax_amount: 0 }],
      metadata: { golem_settled_order_ids: "order_1" },
      ...overrides
    })

    /** An account whose subscription ended with an unpaid last order of $3.45 of paid overage. */
    const unpaidAfterEnd = async (accountId: string) => {
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      await deliver("order.created", order(accountId, lastOrder))
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)
    }

    const postSettle = (jar: CookieJar) =>
      request(app.handler, jar, "POST", "/billing/settle", {
        origin: baseUrl,
        "x-forwarded-for": "203.0.113.7, 198.51.100.7"
      })

    const settledOrders = async () =>
      (
        await pool.query(
          "SELECT order_id, settlement_order_id, currency, amount::int AS amount FROM overage_settlements ORDER BY settled_at"
        )
      ).rows

    it("lets the person settle the unpaid last order of an ended subscription, which frees the account", async () => {
      const { jar, accountId } = await signedIn()
      await unpaidAfterEnd(accountId)

      const html = await page(jar)
      expect(html).toContain("The subscription of the invoice has ended, and Polar does not try the payment again.")
      expect(html).toContain(`action="/billing/settle"`)
      expect(html).toContain("Settle $3.45 here. This amount includes the tax of the invoice.")
      expect(html).toContain("When the account owes no paid overage, your plan is Free")
      expect(html).not.toContain("Pay the invoice")

      const response = await postSettle(jar)
      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("https://sandbox.polar.sh/checkout/polar_c_1")
      const [checkout, ...others] = upstreams.polarRequests("/v1/checkouts/")
      expect(others).toEqual([])
      expect(checkout!.headers.get("authorization")).toBe(`Bearer ${polarWebToken}`)
      expect(checkout!.body).toEqual({
        products: [polarSettlementProductId],
        prices: {
          [polarSettlementProductId]: [{ amount_type: "fixed", price_amount: 345, price_currency: "usd", tax_behavior: "inclusive" }]
        },
        currency: "usd",
        external_customer_id: accountId,
        customer_email: "octocat@example.com",
        customer_name: "Octo Cat",
        customer_ip_address: "198.51.100.7",
        success_url: `${baseUrl}/?settlement_orders=order_1`,
        return_url: `${baseUrl}/`,
        allow_discount_codes: false,
        metadata: { golem_settled_order_ids: "order_1" }
      })
      const returned = await page(jar, "/?settlement_orders=order_1")
      expect(returned).toContain("Golem records the payment within one minute")
      expect(returned).not.toContain("Settle $")

      await deliver("order.created", settlementOrder(accountId))
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      expect(await settledOrders()).toEqual([
        { order_id: "order_1", settlement_order_id: "order_settlement_1", currency: "usd", amount: 345 }
      ])
      const reloaded = await page(jar, "/?settlement_orders=order_1")
      expect(reloaded).not.toContain("Polar has your payment")
      expect(reloaded).toContain("<h1>Free</h1>")

      // Polar delivers the paid order again in `order.updated` and `order.paid`.
      await deliver("order.updated", settlementOrder(accountId))
      await deliver("order.paid", settlementOrder(accountId))
      expect(await settledOrders()).toHaveLength(1)
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(await page(jar)).not.toContain("Settle $")
    })

    it("keeps an account that subscribed again suspended until it settles, and then gives it Paid", async () => {
      const { jar, accountId } = await signedIn()
      await unpaidAfterEnd(accountId)
      await deliver(
        "subscription.active",
        subscription(accountId, {
          id: "sub_2",
          created_at: "2026-12-05T10:00:00Z",
          modified_at: "2026-12-05T10:00:00Z",
          current_period_start: "2026-12-05T10:00:00Z",
          current_period_end: "2027-01-05T10:00:00Z"
        })
      )
      // A new subscription does not pay the last order of the ended one.
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)

      const html = await page(jar)
      expect(html).toContain("Settle $3.45")
      expect(html).toContain("When the account owes no paid overage, your plan is Paid")

      await deliver("order.created", settlementOrder(accountId))
      expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
    })

    it("settles the oldest orders first, and after their webhook the returned page offers the rest", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      const orderIds = Array.from({ length: 14 }, (_, index) => `order_${String(index + 1).padStart(2, "0")}`)
      for (const [index, id] of orderIds.entries()) {
        await deliver("order.created", order(accountId, { id, created_at: new Date(Date.now() - (200 - index) * MINUTE).toISOString() }))
      }
      expect(await page(jar)).toContain("Settle $44.85")

      const batch = orderIds.slice(0, 13)
      await postSettle(jar)
      const [checkout] = upstreams.polarRequests("/v1/checkouts/")
      expect(checkout!.body).toMatchObject({
        prices: {
          [polarSettlementProductId]: [{ amount_type: "fixed", price_amount: 4485, price_currency: "usd", tax_behavior: "inclusive" }]
        },
        success_url: `${baseUrl}/?settlement_orders=${batch.join("%2C")}`,
        metadata: { golem_settled_order_ids: batch.join(",") }
      })
      const returnedPath = `/?settlement_orders=${batch.join("%2C")}`
      expect(await page(jar, returnedPath)).toContain("Polar has your payment")

      await deliver(
        "order.paid",
        settlementOrder(accountId, { subtotal_amount: 4485, net_amount: 4485, metadata: { golem_settled_order_ids: batch.join(",") } })
      )

      const returned = await page(jar, returnedPath)
      expect(returned).not.toContain("Polar has your payment")
      expect(returned).toContain("Settle $3.45")
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
    })

    /**
     * The last order of `sub_1` with tax: $20.00 for the price and $3.45 of paid overage. Polar
     * computes the tax for the whole order, and the items have none.
     */
    const taxedLastOrder = (taxOnTop: boolean) => ({
      ...lastOrder,
      subtotal_amount: 2345,
      discount_amount: 0,
      // Polar adds the tax on top, or takes it out of the subtotal.
      net_amount: taxOnTop ? 2345 : 2094,
      tax_amount: taxOnTop ? 281 : 251,
      total_amount: taxOnTop ? 2626 : 2345,
      due_amount: taxOnTop ? 2626 : 2345
    })

    it("charges the paid overage with the tax that the invoice added on top, and records a payment that includes the tax", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      await deliver("order.created", order(accountId, taxedLastOrder(true)))
      expect(await page(jar)).toContain("Settle $3.86")

      await postSettle(jar)
      const [checkout] = upstreams.polarRequests("/v1/checkouts/")
      expect(checkout!.body).toMatchObject({
        prices: {
          [polarSettlementProductId]: [{ amount_type: "fixed", price_amount: 386, price_currency: "usd", tax_behavior: "inclusive" }]
        }
      })

      // Polar takes the tax out of the inclusive price, so the net amount is below the price.
      await deliver("order.paid", settlementOrder(accountId, { subtotal_amount: 386, net_amount: 345, tax_amount: 41, total_amount: 386 }))

      expect(await settledOrders()).toEqual([
        { order_id: "order_1", settlement_order_id: "order_settlement_1", currency: "usd", amount: 386 }
      ])
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
    })

    it("charges the paid overage of an invoice with inclusive tax as the invoice did", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      await deliver("order.created", order(accountId, taxedLastOrder(false)))

      expect(await page(jar)).toContain("Settle $3.45")
    })

    it("charges the paid overage of a discounted invoice after the discount, with the tax on what is left", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      // The last order after a cancellation bills only the paid overage: $10.00 with a coupon of
      // 50%, and 12% tax on top of the $5.00 after the discount.
      await deliver(
        "order.created",
        order(accountId, {
          ...lastOrder,
          items: [{ product_price_id: meteredPriceId, amount: 1000, tax_amount: 0, proration: false }],
          subtotal_amount: 1000,
          discount_amount: 500,
          net_amount: 500,
          tax_amount: 60,
          total_amount: 560,
          due_amount: 560
        })
      )

      expect(await page(jar)).toContain("Settle $5.60")
    })

    it("takes the credit that a pending order used off the settlement, and charges the total of a void order", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.active", subscription(accountId))
      await deliver("subscription.revoked", subscription(accountId, ended))
      // $10.00 of credit from the wallet of the customer, for example after a downgrade, pays part
      // of the order. It stays used while the order is pending.
      const withCredit = { ...taxedLastOrder(true), applied_balance_amount: -1000, due_amount: 1626 }
      await deliver("order.created", order(accountId, withCredit))
      expect(await page(jar)).toContain("Settle $2.39")

      // A void gives the credit back to the wallet, so the person owes the whole order again.
      await deliver("order.updated", order(accountId, { ...withCredit, status: "void", modified_at: new Date().toISOString() }))
      expect(await page(jar)).toContain("Settle $3.86")
    })

    it("keeps the debt when Polar voids the unpaid order of a revoked subscription", async () => {
      const accountId = randomUUID()
      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId, { created_at: new Date(Date.now() - 10 * MINUTE).toISOString() }))
      await deliver(
        "subscription.revoked",
        subscription(accountId, { ...ended, past_due_at: "2026-11-13T10:00:00Z", modified_at: "2026-12-04T10:00:00Z" })
      )
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)

      await deliver("order.updated", order(accountId, { status: "void", modified_at: new Date().toISOString() }))

      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)
    })

    it("offers no settlement while Polar collects the order, and the route refuses one", async () => {
      const { jar, accountId } = await signedIn()
      const failedAt = new Date()
      await deliver("subscription.active", subscription(accountId))
      await deliver("order.created", order(accountId))
      await deliver(
        "subscription.past_due",
        subscription(accountId, { status: "past_due", past_due_at: failedAt.toISOString(), modified_at: "2026-12-04T10:00:00Z" })
      )
      await sweep(new Date(failedAt.getTime() + config.pastDueGraceMs + MINUTE))
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)

      const html = await page(jar)
      expect(html).toContain("Pay the invoice")
      expect(html).not.toContain("Settle $")
      const response = await postSettle(jar)
      expect(response.status).toBe(303)
      expect(response.headers.get("location")).toBe("/")
      expect(upstreams.polarRequests("/v1/checkouts/")).toEqual([])
    })

    it("refuses a settlement on the free plan, from another site, and without a sign-in", async () => {
      const { jar } = await signedIn()

      const free = await postSettle(jar)
      expect(free.headers.get("location")).toBe("/")
      const crossSite = await request(app.handler, jar, "POST", "/billing/settle", { origin: "https://evil.test" })
      expect(crossSite.status).toBe(403)
      const signedOut = await postSettle(new CookieJar())
      expect(signedOut.headers.get("location")).toBe("/")
      expect(upstreams.polarRequests("/v1/checkouts/")).toEqual([])
    })

    it("settles nothing for a payment below the paid overage or for an order that the account does not owe", async () => {
      const accountId = randomUUID()
      await unpaidAfterEnd(accountId)

      await deliver("order.paid", settlementOrder(accountId, { subtotal_amount: 344, net_amount: 344 }))
      await deliver("order.paid", settlementOrder(accountId, { id: "order_settlement_2", metadata: { golem_settled_order_ids: "order_9" } }))
      await deliver("order.paid", settlementOrder(randomUUID(), { id: "order_settlement_3" }))
      await deliver("order.created", settlementOrder(accountId, { id: "order_settlement_4", status: "pending", paid: false }))

      expect(await settledOrders()).toEqual([])
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      expect(upstreams.deletionHolds.get(accountId)).toBe(unpaidHold)
    })

    it("keeps the account free when Polar charges the original order after its settlement", async () => {
      const accountId = randomUUID()
      await unpaidAfterEnd(accountId)
      await deliver("order.paid", settlementOrder(accountId))

      await deliver("order.paid", order(accountId, { ...lastOrder, ...paidLater }))

      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(upstreams.deletionHolds.has(accountId)).toBe(false)
      expect(await settledOrders()).toHaveLength(1)
    })

    it.each([
      ["eur", "€"],
      ["gbp", "£"]
    ] as const)(
      "settles an unpaid last order in %s in its currency, with the price in USD that Polar requires and does not charge",
      async (currency, symbol) => {
        const { jar, accountId } = await signedIn()
        await deliver("subscription.active", subscription(accountId, { currency }))
        await deliver("subscription.revoked", subscription(accountId, { currency, ...ended }))
        await deliver("order.created", order(accountId, { currency, ...lastOrder }))
        expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)

        expect(await page(jar)).toContain(`Settle ${symbol}3.45 here. This amount includes the tax of the invoice.`)
        const response = await postSettle(jar)
        expect(response.status).toBe(303)
        const [checkout] = upstreams.polarRequests("/v1/checkouts/")
        expect(checkout!.body).toMatchObject({
          products: [polarSettlementProductId],
          prices: {
            [polarSettlementProductId]: [
              { amount_type: "fixed", price_amount: 345, price_currency: "usd", tax_behavior: "inclusive" },
              { amount_type: "fixed", price_amount: 345, price_currency: currency, tax_behavior: "inclusive" }
            ]
          },
          currency,
          customer_ip_address: "198.51.100.7",
          metadata: { golem_settled_order_ids: "order_1" }
        })

        await deliver("order.paid", settlementOrder(accountId, { currency }))
        expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
        expect(upstreams.deletionHolds.has(accountId)).toBe(false)
        expect(await settledOrders()).toEqual([
          { order_id: "order_1", settlement_order_id: "order_settlement_1", currency, amount: 345 }
        ])
      }
    )

    it("settles the orders of each currency in a settlement of their own, the oldest first", async () => {
      const { jar, accountId } = await signedIn()
      await deliver("subscription.revoked", subscription(accountId, { currency: "gbp", ...ended }))
      await deliver(
        "subscription.revoked",
        subscription(accountId, {
          id: "sub_2",
          current_period_end: "2027-01-04T10:00:00Z",
          ...ended,
          modified_at: "2027-01-04T10:00:00Z"
        })
      )
      await deliver(
        "order.created",
        order(accountId, { currency: "gbp", created_at: new Date(Date.now() - 180 * MINUTE).toISOString() })
      )
      await deliver(
        "order.created",
        order(accountId, {
          id: "order_2",
          subscription_id: "sub_2",
          ...lastOrder,
          items: [{ product_price_id: meteredPriceId, amount: 500, tax_amount: 0, proration: false }]
        })
      )
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      expect(await page(jar)).toContain("Settle £3.45")

      await deliver("order.paid", settlementOrder(accountId, { currency: "gbp" }))
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
      expect(await page(jar)).toContain("Settle $5.00")

      await postSettle(jar)
      const [checkout] = upstreams.polarRequests("/v1/checkouts/")
      expect(checkout!.body).toMatchObject({ currency: "usd", metadata: { golem_settled_order_ids: "order_2" } })
      await deliver(
        "order.paid",
        settlementOrder(accountId, {
          id: "order_settlement_2",
          subtotal_amount: 500,
          net_amount: 500,
          metadata: { golem_settled_order_ids: "order_2" }
        })
      )
      expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      expect(await settledOrders()).toEqual([
        { order_id: "order_1", settlement_order_id: "order_settlement_1", currency: "gbp", amount: 345 },
        { order_id: "order_2", settlement_order_id: "order_settlement_2", currency: "usd", amount: 500 }
      ])
    })

    it("settles nothing with a settlement order in another currency than its orders", async () => {
      const accountId = randomUUID()
      await unpaidAfterEnd(accountId)

      await deliver("order.paid", settlementOrder(accountId, { currency: "gbp" }))

      expect(await settledOrders()).toEqual([])
      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
    })
  })

  describe("currencies", () => {
    describe.each([
      ["eur", "€", 2400],
      ["gbp", "£", 1900]
    ] as const)("%s", (currency, symbol, proPrice) => {
      const prices = overagePrices[currency]
      const { fixed, metered } = priceIdsOf(currency)
      /** A renewal of `sub_1` in the currency: the price of Pro and 3.45 of paid overage, tax included. */
      const renewal = (accountId: string, overrides: Record<string, unknown> = {}) =>
        order(accountId, {
          currency,
          items: [
            { product_price_id: fixed, amount: proPrice, tax_amount: 0, proration: false },
            { product_price_id: metered, amount: 345, tax_amount: 0, proration: false }
          ],
          ...overrides
        })
      const pastDue = (accountId: string, failedAt: Date) =>
        subscription(accountId, {
          currency,
          status: "past_due",
          past_due_at: failedAt.toISOString(),
          modified_at: "2026-12-04T10:00:00Z"
        })

      it("lets Polar choose the currency of the checkout, records the currency of the subscription, and shows its prices with the tax included", async () => {
        const { jar, accountId } = await signedIn()

        const checkout = await request(app.handler, jar, "POST", "/billing/checkout", {
          origin: baseUrl,
          "x-forwarded-for": "203.0.113.7, 198.51.100.7"
        })
        expect(checkout.status).toBe(303)
        const [created] = upstreams.polarRequests("/v1/checkouts/")
        expect(created!.body).toMatchObject({ products: [polarProductId], customer_ip_address: "198.51.100.7" })
        expect(created!.body).not.toHaveProperty("currency")
        expect(created!.body).not.toHaveProperty("prices")

        await deliver("subscription.active", subscription(accountId, { currency, checkout_id: "polar_c_1" }))
        expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
        const stored = await pool.query(
          "SELECT currency, metered_price_ids FROM polar_subscriptions WHERE account_id = $1",
          [accountId]
        )
        // The metered price of the subscription first, then the other ones of the product.
        const otherMetered = currencies.map((other) => priceIdsOf(other).metered).filter((id) => id !== metered)
        expect(stored.rows).toEqual([{ currency, metered_price_ids: [metered, ...otherMetered] }])

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
        expect(html).toContain(`${symbol}${prices.perMillionGcu} per million GCU`)
        expect(html).toContain(`${symbol}${prices.perGbHour} per GB-hour`)
        expect(html).toContain(`${symbol}${prices.perDurableGbMonth} per GB-month`)
        expect(html).toContain(`${symbol}${prices.perEphemeralGbMonth} per GB-month`)
        expect(html).toContain(`Prices in ${currency.toUpperCase()}, including tax.`)
        expect(html).toContain(`+ 1.5M GCU of paid overage · ${symbol}0.02`)
        expect(html).toContain(`Paid overage so far this month: about ${symbol}0.02 including tax, at the prices below.`)
        expect(html).not.toContain("$")
      })

      it("suspends the account when a renewal with paid overage is not paid after the past-due grace, until the payment", async () => {
        const { jar, accountId } = await signedIn()
        const failedAt = new Date()

        await deliver("subscription.active", subscription(accountId, { currency }))
        await deliver("order.created", renewal(accountId))
        await deliver("subscription.past_due", pastDue(accountId, failedAt))
        expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)

        await sweep(new Date(failedAt.getTime() + config.pastDueGraceMs + MINUTE))
        expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
        expect(await page(jar)).toContain("Pay the invoice")
        const stored = await pool.query("SELECT currency FROM polar_orders WHERE account_id = $1", [accountId])
        expect(stored.rows).toEqual([{ currency }])

        await deliver("order.paid", renewal(accountId, paidLater))
        await deliver("subscription.active", subscription(accountId, { currency, modified_at: "2026-12-05T10:00:00Z" }))
        expect(upstreams.planOf(accountId).planId).toBe(paidPlanId)
      })

      it("gives Free, not Suspended, when only the price of a renewal is not paid after the past-due grace", async () => {
        const accountId = randomUUID()
        const failedAt = new Date()

        await deliver("subscription.active", subscription(accountId, { currency }))
        await deliver(
          "order.created",
          renewal(accountId, { items: [{ product_price_id: fixed, amount: proPrice, tax_amount: 0, proration: false }] })
        )
        await deliver("subscription.past_due", pastDue(accountId, failedAt))

        await sweep(new Date(failedAt.getTime() + config.pastDueGraceMs + MINUTE))
        expect(upstreams.planOf(accountId).planId).toBe(freePlanId)
      })
    })

    it("counts the overage of a metered price that only the subscription lists", async () => {
      const accountId = randomUUID()
      const product = { id: polarProductId, prices: currencies.flatMap(pricesOf).filter((price) => price.amount_type === "fixed") }
      const createdAt = new Date(Date.now() - 2 * Settlement.ORDER_PAYMENT_GRACE_MS).toISOString()

      await deliver("subscription.active", subscription(accountId, { currency: "gbp", product }))
      await deliver("order.created", order(accountId, { currency: "gbp", created_at: createdAt }))

      expect(upstreams.planOf(accountId).planId).toBe(suspendedPlanId)
    })

    it("ignores a subscription and an order in a currency that the app has no prices for", async () => {
      const accountId = randomUUID()

      const created = await deliver("subscription.active", subscription(accountId, { currency: "cad" }))
      const ordered = await deliver("order.created", order(accountId, { currency: "cad" }))

      expect([created.status, ordered.status]).toEqual([200, 200])
      expect(upstreams.planChanges()).toEqual([])
      expect(upstreams.holdChanges()).toEqual([])
      const stored = await pool.query(
        `SELECT (SELECT count(*)::int FROM polar_subscriptions WHERE account_id = $1) AS subscriptions,
                (SELECT count(*)::int FROM polar_orders WHERE account_id = $1) AS orders`,
        [accountId]
      )
      expect(stored.rows).toEqual([{ subscriptions: 0, orders: 0 }])
    })
  })
})
