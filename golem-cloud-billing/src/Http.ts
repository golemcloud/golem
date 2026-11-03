import { readFileSync } from "node:fs"
import { Effect, Option } from "effect"
import { Cookies, HttpEffect, HttpMiddleware, HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import { Auth } from "./Auth.ts"
import { BillingConfig } from "./BillingConfig.ts"
import { Database } from "./Database.ts"
import { cliBootstrapCommand, type SafeHtml, signedInPage, signedOutPage } from "./Page.ts"
import { type Account, Registry } from "./Registry.ts"

const securityHeaders = {
  // The sign-in form posts to /sign-in/github, which redirects to GitHub; `form-action` also
  // applies to that redirect.
  "content-security-policy":
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; " +
    "form-action 'self' https://github.com; frame-ancestors 'none'; base-uri 'none'",
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

const redirectWithCookies = (location: string, headers: Headers) =>
  HttpServerResponse.redirect(location, {
    status: 303,
    cookies: Cookies.fromSetCookie(headers.getSetCookie())
  })

export const routes = HttpRouter.use((router) =>
  Effect.gen(function* () {
    const { auth, getSignedIn } = yield* Auth
    const registry = yield* Registry
    const config = yield* BillingConfig
    const { pool } = yield* Database
    const cliCommand = cliBootstrapCommand(config.publicApiUrl)

    // The billing app's own form posts carry no CSRF token; a browser sends `Origin` on every
    // POST, so a cross-site post is rejected here.
    const isSameOrigin = (request: HttpServerRequest.HttpServerRequest) => {
      const origin = request.headers["origin"]
      return origin === undefined || origin === config.baseUrl.origin
    }

    yield* router.add("*", "/api/auth/*", HttpEffect.fromWebHandler(auth.handler))

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
        return page(
          signedInPage({ email: user.email, golemAccountId: user.golemAccountId, account, cliCommand })
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
        : Effect.succeed(HttpServerResponse.text("Cross-origin request", { status: 403 }))
    )

    yield* router.add("POST", "/sign-out", (request) =>
      isSameOrigin(request)
        ? Effect.tryPromise(() =>
            auth.api.signOut({ headers: toWebHeaders(request), returnHeaders: true })
          ).pipe(
            Effect.map(({ headers }) => redirectWithCookies("/", headers)),
            Effect.catch(() => Effect.succeed(HttpServerResponse.redirect("/", { status: 303 })))
          )
        : Effect.succeed(HttpServerResponse.text("Cross-origin request", { status: 403 }))
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
