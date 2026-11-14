import { readFileSync } from "node:fs"
import { ByteSize, Effect, Option } from "effect"
import {
  Cookies,
  HttpEffect,
  HttpIncomingMessage,
  HttpMiddleware,
  HttpRouter,
  HttpServerError,
  HttpServerRequest,
  HttpServerResponse
} from "effect/http"
import { Auth, type SignedIn } from "./Auth.ts"
import { BillingConfig } from "./BillingConfig.ts"
import { Database } from "./Database.ts"
import {
  canCheckOut,
  cliBootstrapCommand,
  type OverageNotice,
  type PlanState,
  type SafeHtml,
  signedInPage,
  signedOutPage
} from "./Page.ts"
import { Polar } from "./Polar.ts"
import { type Account, type AccountLimits, Registry } from "./Registry.ts"
import { pastDueGraceEnd } from "./Settlement.ts"
import { type Subscription, Subscriptions } from "./Subscriptions.ts"
import * as Webhooks from "./Webhooks.ts"

const securityHeaders = {
  // The sign-in form posts to /sign-in/github, which redirects to GitHub, and the billing forms
  // redirect to the checkout and the customer portal of Polar; `form-action` also applies to
  // these redirects.
  "content-security-policy":
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; " +
    "form-action 'self' https://github.com https://polar.sh https://*.polar.sh; frame-ancestors 'none'; " +
    "base-uri 'none'",
  "x-content-type-options": "nosniff",
  "referrer-policy": "same-origin",
  "cache-control": "no-store"
}

const page = (body: SafeHtml, status = 200) =>
  HttpServerResponse.text(body.value, {
    status,
    contentType: "text/html; charset=utf-8",
    headers: securityHeaders
  })

const errorPage = HttpServerResponse.text("An error occurred. Reload the page.", {
  status: 500,
  headers: securityHeaders
})

const assetsDirectory = new URL("./assets/", import.meta.url)

const asset = (name: string, contentType: string) =>
  HttpServerResponse.uint8Array(readFileSync(new URL(name, assetsDirectory)), {
    contentType,
    headers: { "cache-control": "public, max-age=3600", "x-content-type-options": "nosniff" }
  })

const toWebHeaders = (request: HttpServerRequest.HttpServerRequest): Headers =>
  new globalThis.Headers(Object.entries(request.headers))

const crossOrigin = () => HttpServerResponse.text("Cross-origin request", { status: 403 })

/** The Standard Webhooks headers that Polar sends with every delivery. */
const webhookSignatureHeaders = ["webhook-id", "webhook-timestamp", "webhook-signature"] as const

/**
 * The largest webhook body that the app reads. A delivery of Polar is a few kilobytes: one
 * subscription or order with its product and customer.
 */
export const webhookBodyLimit = ByteSize.mebibytes(1)

const isBodyTooLarge = (error: unknown): boolean =>
  error instanceof HttpServerError.HttpServerError &&
  error.reason._tag === "RequestParseError" &&
  error.reason.reason === "BodyTooLarge"

const overageNoticeOf = (value: string | null): OverageNotice | undefined =>
  value === "on" || value === "off" || value === "not-eligible" ? value : undefined

/**
 * The IP address of the client: the last address of `x-forwarded-for`, or the address of the
 * connection when the request has no such header. The app has one proxy hop in front of it, the
 * gateway, which appends the address that it sees to `x-forwarded-for`. So the last address is the
 * client, and every address to the left of it comes from the client, which can send any value.
 * The gateway sees the address of the client only while its load balancer keeps that address (on
 * AWS, an NLB with instance targets and `externalTrafficPolicy: Local`).
 */
export const clientAddress = (request: HttpServerRequest.HttpServerRequest): string | undefined => {
  const last = request.headers["x-forwarded-for"]?.split(",").at(-1)?.trim()
  return last !== undefined && last !== "" ? last : Option.getOrUndefined(request.remoteAddress)
}

/**
 * Sets `x-forwarded-for` to the client address alone, for a request to `auth.handler`. Better
 * Auth trusts the header only when it has one address, and else counts the request in one rate
 * limit that all such requests share, so a client could fill that limit for the others.
 */
const withClientAddress = (headers: Headers, address: string | undefined): Headers => {
  headers.delete("x-forwarded-for")
  if (address !== undefined) {
    headers.set("x-forwarded-for", address)
  }
  return headers
}

const redirectWithCookies = (location: string, headers: Headers) =>
  HttpServerResponse.redirect(location, {
    status: 303,
    cookies: Cookies.fromSetCookie(headers.getSetCookie())
  })

export const routes = HttpRouter.use((router) =>
  Effect.gen(function* () {
    const { auth, getSignedIn } = yield* Auth
    const registry = yield* Registry
    const polar = yield* Polar
    const subscriptions = yield* Subscriptions
    const config = yield* BillingConfig
    const { pool } = yield* Database
    const handleWebhook = yield* Webhooks.makeHandler
    const cliCommand = cliBootstrapCommand(config.publicApiUrl)
    const home = new URL("/", config.baseUrl).href

    const planOf = (account: Account | undefined): PlanState => {
      if (account === undefined) return "unknown"
      if (account.planId === config.plans.paid) return "paid"
      if (account.planId === config.plans.suspended) return "suspended"
      if (account.planId === config.plans.free) return "free"
      return "other"
    }

    // The billing app's own form posts carry no CSRF token. A browser sends `Origin` on every
    // form post, so a post without `Origin`, with `Origin: null`, or from another origin is
    // rejected here, also when the browser sends the session cookie.
    const isSameOrigin = (request: HttpServerRequest.HttpServerRequest) =>
      request.headers["origin"] === config.baseUrl.origin

    // The browser reaches Better Auth only for the OAuth callback. The pages call the other
    // endpoints on the server, through `auth.api` or, for the sign-in, `auth.handler`. The rest
    // of /api/auth stays unrouted: it includes get-access-token, which returns the decrypted
    // GitHub access token, and link-social and update-user, which change the user.
    yield* router.add("GET", "/api/auth/callback/github", (request) =>
      HttpEffect.fromWebHandler((web) => {
        const headers = withClientAddress(new Headers(web.headers), clientAddress(request))
        return auth.handler(new Request(web, { headers }))
      })
    )

    // Polar delivers the webhooks here. The handler is the app's own, not a Better Auth endpoint;
    // the path is the one that the Polar organization has configured. Polar signs every
    // delivery, so a request without the signature headers is rejected before the app reads its
    // body, and the app reads at most `webhookBodyLimit` of a body.
    yield* router.add("POST", "/api/auth/polar/webhooks", (request) =>
      webhookSignatureHeaders.every((name) => request.headers[name] !== undefined)
        ? request.text.pipe(
            Effect.provideService(HttpIncomingMessage.MaxBodySize, webhookBodyLimit),
            Effect.flatMap((body) => handleWebhook(body, request.headers)),
            Effect.map((response) => HttpServerResponse.text(response.body, { status: response.status })),
            Effect.catch((error) =>
              Effect.succeed(
                isBodyTooLarge(error)
                  ? HttpServerResponse.text("the body is too large", { status: 413 })
                  : HttpServerResponse.text("cannot read the body", { status: 400 })
              )
            )
          )
        : Effect.logWarning("Rejected a webhook").pipe(
            Effect.annotateLogs({ status: 403, reason: "no signature headers" }),
            Effect.as(HttpServerResponse.text("rejected", { status: 403 }))
          )
    )

    yield* router.add("GET", "/", (request) =>
      Effect.gen(function* () {
        const signedIn = yield* getSignedIn(toWebHeaders(request))
        if (Option.isNone(signedIn)) {
          const error = new URL(request.url, config.baseUrl).searchParams.get("error") ?? undefined
          return page(signedOutPage({ error }))
        }
        const user = signedIn.value
        const account: Account | undefined = yield* registry.getAccount(user.golemToken, user.golemAccountId).pipe(
          Effect.catch((error) =>
            Effect.logWarning("Cannot read the Golem account").pipe(
              Effect.annotateLogs({ golemAccountId: user.golemAccountId, error: error.message }),
              Effect.as(undefined)
            )
          )
        )
        const subscription: Subscription | undefined = yield* subscriptions
          .latest(user.golemAccountId, config.polar.productId)
          .pipe(
            Effect.catch((error) =>
              Effect.logWarning("Cannot read the subscription").pipe(
                Effect.annotateLogs({ golemAccountId: user.golemAccountId, error: error.message }),
                Effect.as(undefined)
              )
            )
          )
        const plan = planOf(account)
        const params = new URL(request.url, config.baseUrl).searchParams
        // Polar redirects here with the checkout ID after a payment; the webhook that changes the
        // plan can arrive a few seconds later.
        const checkoutPending = params.has("checkout_id") && (plan === "free" || plan === "suspended")
        const limits: AccountLimits | undefined =
          plan === "paid"
            ? yield* registry.getLimits(user.golemToken, user.golemAccountId).pipe(
                Effect.catch((error) =>
                  Effect.logWarning("Cannot read the monthly usage mode").pipe(
                    Effect.annotateLogs({ golemAccountId: user.golemAccountId, error: error.message }),
                    Effect.as(undefined)
                  )
                )
              )
            : undefined
        return page(
          signedInPage({
            email: user.email,
            golemAccountId: user.golemAccountId,
            account,
            plan,
            subscription,
            pastDueGraceEnd:
              plan === "paid" && subscription !== undefined
                ? pastDueGraceEnd(subscription, config.pastDueGraceMs)
                : undefined,
            checkoutPending,
            cliCommand,
            limits,
            overagePrices: config.overagePrices,
            overageNotice: overageNoticeOf(params.get("overage"))
          })
        )
      }).pipe(
        Effect.catch((error) =>
          Effect.logError("Cannot render the landing page").pipe(
            Effect.annotateLogs({ error: error.message }),
            Effect.as(errorPage)
          )
        )
      )
    )

    // The form calls the sign-in endpoint through `auth.handler`, not `auth.api`, so that the rate
    // limit of Better Auth applies to it (`rateLimit` in `authOptions`).
    const signInSocial = (request: HttpServerRequest.HttpServerRequest) => {
      const headers = withClientAddress(toWebHeaders(request), clientAddress(request))
      headers.delete("content-length")
      headers.delete("transfer-encoding")
      headers.set("content-type", "application/json")
      return auth.handler(
        new Request(new URL("/api/auth/sign-in/social", config.baseUrl), {
          method: "POST",
          headers,
          body: JSON.stringify({ provider: "github", callbackURL: "/", errorCallbackURL: "/" })
        })
      )
    }

    yield* router.add("POST", "/sign-in/github", (request) =>
      isSameOrigin(request)
        ? Effect.tryPromise(() => signInSocial(request)).pipe(
            Effect.flatMap((response) => {
              if (response.status === 429) {
                return Effect.logWarning("Refused a GitHub sign-in over the rate limit").pipe(
                  Effect.as(HttpServerResponse.redirect("/?error=too_many_requests", { status: 303 }))
                )
              }
              if (!response.ok) {
                return Effect.fail(new Error(`Better Auth answered ${response.status}`))
              }
              return Effect.tryPromise(() => response.json() as Promise<{ readonly url?: unknown }>).pipe(
                Effect.flatMap((body) =>
                  typeof body.url === "string"
                    ? Effect.succeed(redirectWithCookies(body.url, response.headers))
                    : Effect.fail(new Error("Better Auth returned no GitHub authorization URL"))
                )
              )
            }),
            Effect.catch((error) =>
              Effect.logError("Cannot start the GitHub sign-in").pipe(
                Effect.annotateLogs({ error: String(error) }),
                Effect.as(errorPage)
              )
            )
          )
        : Effect.succeed(crossOrigin())
    )

    yield* router.add("POST", "/billing/checkout", (request) =>
      isSameOrigin(request)
        ? Effect.gen(function* () {
            // The route accepts only a checkout that the page offers. When the registry or the
            // database fails, the session or the plan is not known, so the route refuses the
            // checkout, as it refuses a request without a sign-in.
            const signedIn = yield* getSignedIn(toWebHeaders(request)).pipe(
              Effect.catch((error) =>
                Effect.logWarning("Cannot read the session for the checkout").pipe(
                  Effect.annotateLogs({ error: error.message }),
                  Effect.as(Option.none<SignedIn>())
                )
              )
            )
            if (Option.isNone(signedIn)) {
              return HttpServerResponse.redirect("/", { status: 303 })
            }
            const user = signedIn.value
            const offered = yield* Effect.gen(function* () {
              const account = yield* registry.getAccount(user.golemToken, user.golemAccountId)
              const subscription = yield* subscriptions.latest(user.golemAccountId, config.polar.productId)
              const plan = planOf(account)
              if (!canCheckOut(plan, subscription)) {
                yield* Effect.logWarning("Refused a checkout that the page does not offer").pipe(
                  Effect.annotateLogs({
                    golemAccountId: user.golemAccountId,
                    plan,
                    subscriptionStatus: subscription?.status
                  })
                )
                return false
              }
              return true
            }).pipe(
              Effect.catch((error) =>
                Effect.logWarning("Cannot read the plan for the checkout").pipe(
                  Effect.annotateLogs({ golemAccountId: user.golemAccountId, error: error.message }),
                  Effect.as(false)
                )
              )
            )
            if (!offered) {
              return HttpServerResponse.redirect("/", { status: 303 })
            }
            const url = yield* polar.createCheckout({
              productId: config.polar.productId,
              externalCustomerId: user.golemAccountId,
              customerEmail: user.email,
              customerName: user.name === "" ? undefined : user.name,
              customerIpAddress: clientAddress(request),
              successUrl: `${home}?checkout_id={CHECKOUT_ID}`,
              returnUrl: home
            })
            return HttpServerResponse.redirect(url, { status: 303 })
          }).pipe(
            Effect.catch((error) =>
              Effect.logError("Cannot start the checkout").pipe(
                Effect.annotateLogs({ error: error.message }),
                Effect.as(errorPage)
              )
            )
          )
        : Effect.succeed(crossOrigin())
    )

    // The registry accepts the change of the monthly usage mode only with the token of the owner,
    // and records it as the consent of the owner, so the form uses the token of the session.
    yield* router.add("POST", "/billing/overage", (request) =>
      isSameOrigin(request)
        ? Effect.gen(function* () {
            const signedIn = yield* getSignedIn(toWebHeaders(request))
            if (Option.isNone(signedIn)) {
              return HttpServerResponse.redirect("/", { status: 303 })
            }
            const mode = new URLSearchParams(yield* request.text).get("mode")
            if (mode !== "allowOverage" && mode !== "hardLimit") {
              return HttpServerResponse.text("Unknown mode", { status: 400, headers: securityHeaders })
            }
            const user = signedIn.value
            const change = yield* registry.setMonthlyUsageMode(user.golemToken, user.golemAccountId, mode)
            yield* Effect.logInfo("Changed the monthly usage mode").pipe(
              Effect.annotateLogs({ golemAccountId: user.golemAccountId, mode, change })
            )
            const notice: OverageNotice =
              change === "notEligible" ? "not-eligible" : mode === "allowOverage" ? "on" : "off"
            return HttpServerResponse.redirect(`/?overage=${notice}`, { status: 303 })
          }).pipe(
            Effect.catch((error) =>
              Effect.logError("Cannot change the monthly usage mode").pipe(
                Effect.annotateLogs({ error: error.message }),
                Effect.as(errorPage)
              )
            )
          )
        : Effect.succeed(crossOrigin())
    )

    yield* router.add("POST", "/billing/portal", (request) =>
      isSameOrigin(request)
        ? Effect.gen(function* () {
            const signedIn = yield* getSignedIn(toWebHeaders(request))
            if (Option.isNone(signedIn)) {
              return HttpServerResponse.redirect("/", { status: 303 })
            }
            const url = yield* polar.createCustomerPortalUrl({
              externalCustomerId: signedIn.value.golemAccountId,
              returnUrl: home
            })
            return HttpServerResponse.redirect(url, { status: 303 })
          }).pipe(
            Effect.catch((error) =>
              Effect.logError("Cannot open the customer portal").pipe(
                Effect.annotateLogs({ error: error.message }),
                Effect.as(errorPage)
              )
            )
          )
        : Effect.succeed(crossOrigin())
    )

    yield* router.add("POST", "/sign-out", (request) =>
      isSameOrigin(request)
        ? Effect.tryPromise(() =>
            auth.api.signOut({ headers: toWebHeaders(request), returnHeaders: true })
          ).pipe(
            Effect.map(({ headers }) => redirectWithCookies("/", headers)),
            Effect.catch(() => Effect.succeed(HttpServerResponse.redirect("/", { status: 303 })))
          )
        : Effect.succeed(crossOrigin())
    )

    yield* router.add("GET", "/assets/app.css", asset("app.css", "text/css; charset=utf-8"))
    yield* router.add("GET", "/assets/app.js", asset("app.js", "text/javascript; charset=utf-8"))
    yield* router.add("GET", "/assets/golem-logo.png", asset("golem-logo.png", "image/png"))
    yield* router.add("GET", "/assets/favicon.png", asset("favicon.png", "image/png"))

    // Kubernetes probes call these every few seconds; they are not logged.
    yield* router.add(
      "GET",
      "/healthz",
      HttpMiddleware.withLoggerDisabled(Effect.succeed(HttpServerResponse.text("ok")))
    )
    yield* router.add(
      "GET",
      "/readyz",
      Effect.tryPromise(() => pool.query("SELECT 1")).pipe(
        Effect.as(HttpServerResponse.text("ok")),
        Effect.catch(() => Effect.succeed(HttpServerResponse.text("database unavailable", { status: 503 }))),
        HttpMiddleware.withLoggerDisabled
      )
    )
  })
)
