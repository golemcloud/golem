import { randomUUID } from "node:crypto"
import { Layer, ManagedRuntime, Redacted } from "effect"
import { FetchHttpClient, HttpRouter } from "effect/http"
import type pg from "pg"
import * as App from "../src/App.ts"
import { BillingConfig, type BillingConfigShape } from "../src/BillingConfig.ts"
import { makePool } from "../src/Database.ts"
import * as Http from "../src/Http.ts"

export const baseUrl = "http://localhost:3000"
export const registryUrl = "http://registry.test"
export const authSecret = "test-secret-that-is-at-least-32-characters-long"

/**
 * The registry token of the billing account, which has `view`, `set-plan` and `hold-deletion` on
 * every account.
 */
export const registryBillingToken = "golem_billing_token"
/** The account of the billing token: the holder of every deletion hold of the app. */
export const billingAccountId = "5f0c2e7a-91d4-4b3c-8a6e-2d7b1c9e0f34"
export const freePlanId = "6d3b6a42-2d71-4b0e-9b1a-1f4e7b3b2c10"
export const paidPlanId = "9a1f3c2e-5b7d-4e8f-a6c4-2d1e0b9f8a73"
export const suspendedPlanId = "3e8b1d47-0c2a-4f96-b5e1-7a9d2c4f6b08"
export const overagePrices = {
  perMillionGcu: "0.0112",
  perGbHour: "0.045",
  perDurableGbMonth: "0.27",
  perEphemeralGbMonth: "0.24"
}
/** A plan that an administrator gave the account; subscriptions do not change it. */
export const internalPlanId = "c4e2a9b1-7d3f-4a6e-8b5c-1f0e9d2c3b4a"
export const polarApiUrl = "https://sandbox-api.polar.test"
export const polarWebToken = "polar_oat_web_token_for_the_tests"
export const polarProductId = "115e800a-2b62-42e0-97bf-9e8a5d6022b3"
export const webhookSecret = `whsec_${Buffer.from("a webhook secret of 32 bytes!!!!").toString("base64")}`

const requireEnv = (name: string): string => {
  const value = process.env[name]
  if (value === undefined || value === "") {
    throw new Error(`${name} is not set; the tests need a PostgreSQL server (see README.md)`)
  }
  return value
}

/** Connection settings of the PostgreSQL server of the tests, from the libpq variables. */
export const postgres = () => ({
  host: requireEnv("PGHOST"),
  port: Number(process.env["PGPORT"] ?? "5432"),
  database: requireEnv("PGDATABASE"),
  username: requireEnv("PGUSER"),
  password: requireEnv("PGPASSWORD")
})

/** A configuration whose tables live in a new schema, so test files do not share rows. */
export const testConfig = (): BillingConfigShape => {
  const db = postgres()
  return {
    httpPort: 0,
    baseUrl: new URL(baseUrl),
    authSecret: Redacted.make(authSecret),
    github: { clientId: "test-client-id", clientSecret: Redacted.make("test-client-secret") },
    registryUrl: new URL(registryUrl),
    publicApiUrl: new URL("https://release.dev-api.golem.cloud"),
    registryBillingToken: Redacted.make(registryBillingToken),
    plans: { free: freePlanId, paid: paidPlanId, suspended: suspendedPlanId },
    overagePrices,
    pastDueGraceMs: 7 * 24 * 60 * 60 * 1000,
    polar: {
      apiUrl: new URL(polarApiUrl),
      accessToken: Redacted.make(polarWebToken),
      productId: polarProductId,
      webhookSecret: Redacted.make(webhookSecret)
    },
    db: {
      ...db,
      password: Redacted.make(db.password),
      maxConnections: 5,
      schema: `test_${randomUUID().replaceAll("-", "")}`,
      registrySchema: "public",
      tls: "disable"
    }
  }
}

/** A pool with the settings of the app, for the queries of the tests. */
export const queryPool = (config: BillingConfigShape) => makePool({ ...config.db, maxConnections: 2 })

export const dropSchema = async (pool: pg.Pool, schema: string) => {
  await pool.query(`DROP SCHEMA IF EXISTS "${schema}" CASCADE`)
}

/** The billing app as a Fetch handler, built from the same layers as the server. */
export const makeApp = (config: BillingConfigShape) =>
  HttpRouter.toWebHandler(
    Http.routes.pipe(
      Layer.provide(App.services),
      Layer.provide(FetchHttpClient.layer),
      Layer.provide(Layer.succeed(BillingConfig, config))
    ),
    { disableLogger: true }
  )

/** The services of the app with `config` and its configuration, for calls outside a request. */
export const makeServices = (config: BillingConfigShape) =>
  ManagedRuntime.make(
    App.services.pipe(Layer.provide(FetchHttpClient.layer), Layer.provideMerge(Layer.succeed(BillingConfig, config)))
  )

export interface RecordedRequest {
  readonly method: string
  readonly url: URL
  readonly headers: Headers
  /** The JSON body of a POST or PUT, if it has one. */
  readonly body: unknown
}

/** The plan of a fake registry account. */
export interface RegistryPlan {
  planId: string
  revision: number
}

export interface RegistryAccount {
  readonly accountId: string
  readonly secret: string
}

export interface GitHubIdentity {
  readonly id: number
  readonly login: string
  readonly email: string
  /** The access token that the fake GitHub issues for this identity. */
  readonly accessToken: string
}

export const newGitHubIdentity = (id: number, email = "octocat@example.com"): GitHubIdentity => ({
  id,
  login: `octocat-${id}`,
  email,
  accessToken: `gho_${randomUUID().replaceAll("-", "")}`
})

const newRegistryAccount = (): RegistryAccount => ({
  accountId: randomUUID(),
  secret: `golem_${randomUUID()}`
})

/**
 * Replaces `fetch` for the GitHub OAuth endpoints and the registry service. `github` is the
 * identity that the next GitHub authorization signs in. Like the registry, the fake resolves each
 * GitHub access token to one Golem account (`registryAccounts`).
 */
export class FakeUpstreams {
  readonly requests: Array<RecordedRequest> = []
  github: GitHubIdentity = newGitHubIdentity(4242)
  readonly registryAccounts = new Map<string, RegistryAccount>()
  registryLoginStatus = 200
  /** The status of `GET /v1/accounts/{account_id}`; with another status than 200, the read fails. */
  accountReadStatus = 200
  /** The plan of each account ID; an account starts on the free plan. */
  readonly registryPlans = new Map<string, RegistryPlan>()
  /** Whether the permission share of the billing account exists; without it the registry answers 404. */
  billingShare = true
  /** The number of plan changes that fail with 409 before one succeeds. */
  planConflicts = 0
  /** The monthly usage mode of each account ID; an account starts in `hardLimit`. */
  readonly usageModes = new Map<string, "hardLimit" | "allowOverage">()
  /** `monthly` of the limits of every account, as the registry reports it; `undefined` leaves it out. */
  monthly: unknown = {
    computeGcu: {
      metering: "enabled",
      planAmount: 100_000_000,
      resolvedMonthlyAmount: 100_000_000,
      usage: 2_500_000,
      remaining: 97_500_000,
      allowOverageUsage: 0,
      unit: "GCU",
      behavior: "hardLimit"
    },
    memoryGbSeconds: {
      metering: "enabled",
      planAmount: 3_285_000,
      resolvedMonthlyAmount: 3_285_000,
      usage: 900_000,
      remaining: 2_385_000,
      allowOverageUsage: 0,
      unit: "GB-seconds",
      behavior: "hardLimit"
    },
    durableStorageGbMonth: { metering: "disabled", planAmount: 5, unit: "GB-month" },
    ephemeralStorageGbMonth: { metering: "unknown", planAmount: 5, unit: "GB-month" }
  }
  /** The reason of the deletion hold of the billing account on each account that has one. */
  readonly deletionHolds = new Map<string, string>()
  /** The accounts that the registry deleted; it answers 404 for them, as for an account it hides. */
  readonly deletedAccounts = new Set<string>()

  /** The Golem account of the access token of `identity`, created on first use. */
  accountOf(identity: GitHubIdentity = this.github): RegistryAccount {
    let account = this.registryAccounts.get(identity.accessToken)
    if (account === undefined) {
      account = newRegistryAccount()
      this.registryAccounts.set(identity.accessToken, account)
    }
    return account
  }

  get registryAccount(): RegistryAccount {
    return this.accountOf()
  }

  planOf(accountId: string): RegistryPlan {
    let plan = this.registryPlans.get(accountId)
    if (plan === undefined) {
      plan = { planId: freePlanId, revision: 0 }
      this.registryPlans.set(accountId, plan)
    }
    return plan
  }

  private accountJson(accountId: string) {
    const plan = this.planOf(accountId)
    return {
      id: accountId,
      revision: plan.revision,
      name: "Octo Cat",
      email: this.github.email,
      planId: plan.planId,
      roles: [],
      accountRootCardId: "0b7a4b6e-8c2b-4f63-bb0e-6a4cf7c1b9d2"
    }
  }

  readonly fetch = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init)
    const url = new URL(request.url)
    const text = request.method === "POST" || request.method === "PUT" ? await request.text() : ""
    let body: unknown = undefined
    try {
      body = text === "" ? undefined : JSON.parse(text)
    } catch {
      body = undefined
    }
    this.requests.push({ method: request.method, url, headers: request.headers, body })
    const route = `${request.method} ${url.origin}${url.pathname}`
    const bearer = request.headers.get("authorization")?.replace(/^Bearer /, "")
    switch (route) {
      case `POST ${polarApiUrl}/v1/checkouts/`:
        return bearer === polarWebToken
          ? Response.json({ id: "polar_c_test", url: "https://sandbox.polar.sh/checkout/polar_c_test" }, { status: 201 })
          : Response.json({ error: "invalid token" }, { status: 401 })
      case `POST ${polarApiUrl}/v1/customer-sessions/`:
        return bearer === polarWebToken
          ? Response.json(
              {
                id: "polar_cs_test",
                token: "polar_cst_test",
                customer_portal_url: "https://sandbox.polar.sh/golem/portal?customer_session_token=polar_cst_test"
              },
              { status: 201 }
            )
          : Response.json({ error: "invalid token" }, { status: 401 })
      case "POST https://github.com/login/oauth/access_token":
        this.accountOf(this.github)
        return Response.json({
          access_token: this.github.accessToken,
          token_type: "bearer",
          scope: "read:user,user:email"
        })
      case "GET https://api.github.com/user":
        return bearer === this.github.accessToken
          ? Response.json({
              id: this.github.id,
              login: this.github.login,
              name: "Octo Cat",
              email: null,
              avatar_url: "https://avatars.example/octocat"
            })
          : new Response(null, { status: 401 })
      case "GET https://api.github.com/user/emails":
        return Response.json([{ email: this.github.email, primary: true, verified: true }])
      case `POST ${registryUrl}/v1/login/oauth2`: {
        if (this.registryLoginStatus !== 200) {
          return Response.json({ error: "unavailable" }, { status: this.registryLoginStatus })
        }
        const token = url.searchParams.get("access-token")
        const account = token === null ? undefined : this.registryAccounts.get(token)
        if (url.searchParams.get("provider") !== "github" || account === undefined) {
          return Response.json({ error: "invalid token" }, { status: 401 })
        }
        return Response.json({
          id: randomUUID(),
          secret: account.secret,
          accountId: account.accountId,
          createdAt: "2026-11-03T10:00:00Z",
          expiresAt: "2036-11-03T10:00:00Z"
        })
      }
      case `GET ${registryUrl}/v1/me/token`:
        return bearer === registryBillingToken
          ? Response.json({
              id: "1d6f0e3a-7c2b-4f5e-9a8d-3b4c5d6e7f80",
              accountId: billingAccountId,
              createdAt: "2026-11-03T10:00:00Z",
              expiresAt: "2027-11-03T10:00:00Z"
            })
          : new Response(null, { status: 401 })
      default: {
        const match = /^\/v1\/accounts\/([0-9a-f-]{36})(\/plan|\/limits|\/monthly-usage-mode|\/deletion-holds\/([0-9a-f-]{36}))?$/.exec(url.pathname)
        if (url.origin !== registryUrl || match === null) {
          return new Response(`No fake for ${route}`, { status: 599 })
        }
        const accountId = match[1]!
        const account = [...this.registryAccounts.values()].find((candidate) => candidate.accountId === accountId)
        const isOwner = account !== undefined && bearer === account.secret
        const isBilling = bearer === registryBillingToken && this.billingShare
        if (this.deletedAccounts.has(accountId)) {
          return new Response(null, { status: 404 })
        }
        if (match[3] !== undefined) {
          if (!isBilling) {
            return new Response(null, { status: 404 })
          }
          if (match[3] !== billingAccountId) {
            return Response.json({ code: "FORBIDDEN" }, { status: 403 })
          }
          if (request.method === "PUT") {
            const reason = (body as { reason: string }).reason
            this.deletionHolds.set(accountId, reason)
            return Response.json({
              accountId,
              holderAccountId: billingAccountId,
              reason,
              createdAt: "2026-11-04T10:00:00Z"
            })
          }
          if (request.method === "DELETE") {
            this.deletionHolds.delete(accountId)
            return Response.json({})
          }
        }
        if (match[2] === undefined && request.method === "GET") {
          if (this.accountReadStatus !== 200) {
            return Response.json({ error: "unavailable" }, { status: this.accountReadStatus })
          }
          // The billing token sees every account, also one that never signed in to the app.
          return isOwner || isBilling ? Response.json(this.accountJson(accountId)) : new Response(null, { status: 404 })
        }
        if (match[2] === "/limits" && request.method === "GET") {
          return isOwner
            ? Response.json({
                accountId,
                monthlyUsageMode: this.usageModes.get(accountId) ?? "hardLimit",
                overageAllowedByPlan: this.planOf(accountId).planId === paidPlanId,
                ...(this.monthly === undefined ? {} : { monthly: this.monthly })
              })
            : new Response(null, { status: 404 })
        }
        if (match[2] === "/monthly-usage-mode" && request.method === "PUT") {
          // Only the owner can enter paid-overage mode, and only on a plan with paid overage.
          if (!isOwner) {
            return Response.json({ code: "FORBIDDEN" }, { status: 403 })
          }
          const { mode } = body as { mode: "hardLimit" | "allowOverage" }
          const current = this.usageModes.get(accountId) ?? "hardLimit"
          if (mode === current) {
            return Response.json({ errors: ["unchanged"], code: "MONTHLY_USAGE_MODE_UNCHANGED" }, { status: 400 })
          }
          if (mode === "allowOverage" && this.planOf(accountId).planId !== paidPlanId) {
            return Response.json({ errors: ["not eligible"], code: "MONTHLY_USAGE_MODE_NOT_ELIGIBLE" }, { status: 400 })
          }
          this.usageModes.set(accountId, mode)
          return Response.json({
            actorAccountId: accountId,
            changedAt: new Date().toISOString(),
            source: "owner",
            previousMode: current,
            newMode: mode
          })
        }
        if (match[2] === "/plan" && request.method === "PUT") {
          if (isOwner) {
            return Response.json({ code: "FORBIDDEN" }, { status: 403 })
          }
          if (!isBilling) {
            return new Response(null, { status: 404 })
          }
          const update = body as { currentRevision: number; plan: string }
          const plan = this.planOf(accountId)
          if (this.planConflicts > 0) {
            this.planConflicts--
            plan.revision++
            return Response.json({ code: "CONCURRENT_UPDATE" }, { status: 409 })
          }
          if (update.currentRevision !== plan.revision) {
            return Response.json({ code: "CONCURRENT_UPDATE" }, { status: 409 })
          }
          plan.planId = update.plan
          plan.revision++
          // The registry sets an account on a plan without paid overage to hardLimit.
          if (update.plan !== paidPlanId) {
            this.usageModes.set(accountId, "hardLimit")
          }
          return Response.json(this.accountJson(accountId))
        }
        return new Response(`No fake for ${route}`, { status: 599 })
      }
    }
  }

  registryLogins(): Array<RecordedRequest> {
    return this.requests.filter((request) => request.url.href.startsWith(`${registryUrl}/v1/login/oauth2`))
  }

  usageModeChanges(): Array<RecordedRequest> {
    return this.requests.filter(
      (request) => request.method === "PUT" && request.url.pathname.endsWith("/monthly-usage-mode")
    )
  }

  /** The calls that place or remove a deletion hold. */
  holdChanges(): Array<RecordedRequest> {
    return this.requests.filter((request) => request.url.pathname.includes("/deletion-holds/"))
  }

  planChanges(): Array<RecordedRequest> {
    return this.requests.filter((request) => request.method === "PUT" && request.url.pathname.endsWith("/plan"))
  }

  polarRequests(path: string): Array<RecordedRequest> {
    return this.requests.filter((request) => request.url.href === `${polarApiUrl}${path}`)
  }
}

/** Keeps the cookies that the billing app sets, like a browser. */
export class CookieJar {
  private readonly cookies = new Map<string, string>()

  update(response: Response): void {
    for (const header of response.headers.getSetCookie()) {
      const [pair, ...attributes] = header.split(";")
      const separator = pair!.indexOf("=")
      const name = pair!.slice(0, separator).trim()
      const value = pair!.slice(separator + 1).trim()
      const expired = attributes.some((attribute) => /^\s*max-age=0\s*$/i.test(attribute))
      if (expired || value === "") {
        this.cookies.delete(name)
      } else {
        this.cookies.set(name, value)
      }
    }
  }

  header(): string {
    return [...this.cookies].map(([name, value]) => `${name}=${value}`).join("; ")
  }
}

export const request = async (
  handler: (request: Request) => Promise<Response>,
  jar: CookieJar,
  method: string,
  path: string,
  headers: Record<string, string> = {},
  body?: unknown
): Promise<Response> => {
  const response = await handler(
    new Request(new URL(path, baseUrl), {
      method,
      headers: { cookie: jar.header(), ...headers },
      redirect: "manual",
      ...(body === undefined ? {} : { body: typeof body === "string" ? body : JSON.stringify(body) })
    })
  )
  jar.update(response)
  return response
}

/**
 * Runs the browser side of the GitHub sign-in: posts the sign-in form, lets the fake GitHub
 * authorize, and follows the callback. Returns the callback response.
 */
export const signInWithGitHub = async (
  handler: (request: Request) => Promise<Response>,
  jar: CookieJar
): Promise<Response> => {
  const start = await request(handler, jar, "POST", "/sign-in/github", { origin: baseUrl })
  if (start.status !== 303) {
    throw new Error(`The sign-in form returned ${start.status}: ${await start.text()}`)
  }
  const authorize = new URL(start.headers.get("location")!)
  if (authorize.origin + authorize.pathname !== "https://github.com/login/oauth/authorize") {
    throw new Error(`The sign-in form redirected to ${authorize.href}`)
  }
  const state = authorize.searchParams.get("state")!
  return request(handler, jar, "GET", `/api/auth/callback/github?code=test-code&state=${state}`)
}
