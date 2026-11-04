import { readFileSync } from "node:fs"
import { Effect, Option } from "effect"
import { Cookies, HttpEffect, HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import { Auth } from "./Auth.ts"
import { BillingConfig } from "./BillingConfig.ts"
import { Database } from "./Database.ts"
import { cliBootstrapCommand, type PlanState, type SafeHtml, signedInPage, signedOutPage } from "./Page.ts"
import { Polar } from "./Polar.ts"
import { type Account, Registry } from "./Registry.ts"
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

const errorPage = HttpServerResponse.text("Something went wrong. Reload the page to try again.", {
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

/**
 * The IP address of the browser, for the local currency of the checkout: the first address of
 * `x-forwarded-for`, which the gateway sets, or the address of the connection.
 */
const clientIp = (request: HttpServerRequest.HttpServerRequest): string | undefined => {
  const forwarded = request.headers["x-forwarded-for"]?.split(",")[0]?.trim()
  return forwarded !== undefined && forwarded !== "" ? forwarded : Option.getOrUndefined(request.remoteAddress)
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
      if (account.planId === config.plans.free) return "free"
      return "other"
    }

    // The billing app's own form posts carry no CSRF token; a browser sends `Origin` on every
    // POST, so a cross-site post is rejected here.
    const isSameOrigin = (request: HttpServerRequest.HttpServerRequest) => {
      const origin = request.headers["origin"]
      return origin === undefined || origin === config.baseUrl.origin
    }

    // The browser reaches Better Auth only for the OAuth callback. The pages call the other
    // endpoints on the server through `auth.api`. The rest of /api/auth stays unrouted: it
    // includes get-access-token, which returns the decrypted GitHub access token, and
    // link-social and update-user, which change the user.
    yield* router.add("GET", "/api/auth/callback/github", HttpEffect.fromWebHandler(auth.handler))

    // Polar delivers the webhooks here. The handler is the app's own, not a Better Auth endpoint;
    // the path is the one that the Polar organization has configured.
    yield* router.add("POST", "/api/auth/polar/webhooks", (request) =>
      request.text.pipe(
        Effect.flatMap((body) => handleWebhook(body, request.headers)),
        Effect.map((response) => HttpServerResponse.text(response.body, { status: response.status })),
        Effect.catch(() => Effect.succeed(HttpServerResponse.text("cannot read the body", { status: 400 })))
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
        // Polar redirects here with the checkout ID after a payment; the webhook that changes the
        // plan can arrive a few seconds later.
        const checkoutPending = new URL(request.url, config.baseUrl).searchParams.has("checkout_id") && plan === "free"
        return page(
          signedInPage({
            email: user.email,
            golemAccountId: user.golemAccountId,
            account,
            plan,
            subscription,
            checkoutPending,
            cliCommand
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

    yield* router.add("POST", "/sign-in/github", (request) =>
      isSameOrigin(request)
        ? Effect.tryPromise(() =>
            auth.api.signInSocial({
              body: { provider: "github", callbackURL: "/", errorCallbackURL: "/" },
              headers: toWebHeaders(request),
              returnHeaders: true
            })
          ).pipe(
            Effect.flatMap(({ headers, response }) =>
              response.url === undefined
                ? Effect.fail(new Error("Better Auth returned no GitHub authorization URL"))
                : Effect.succeed(redirectWithCookies(response.url, headers))
            ),
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
            const signedIn = yield* getSignedIn(toWebHeaders(request))
            if (Option.isNone(signedIn)) {
              return HttpServerResponse.redirect("/", { status: 303 })
            }
            const user = signedIn.value
            const url = yield* polar.createCheckout({
              productId: config.polar.productId,
              externalCustomerId: user.golemAccountId,
              customerEmail: user.email,
              customerName: user.name === "" ? undefined : user.name,
              customerIpAddress: clientIp(request),
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
