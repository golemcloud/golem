import { randomUUID } from "node:crypto"
import { Layer, Redacted } from "effect"
import { FetchHttpClient, HttpRouter } from "effect/http"
import type pg from "pg"
import * as App from "../src/App.ts"
import { BillingConfig, type BillingConfigShape } from "../src/BillingConfig.ts"
import { makePool } from "../src/Database.ts"
import * as Http from "../src/Http.ts"

export const baseUrl = "http://localhost:3000"
export const registryUrl = "http://registry.test"
export const authSecret = "test-secret-that-is-at-least-32-characters-long"

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

export interface RecordedRequest {
  readonly method: string
  readonly url: URL
  readonly headers: Headers
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

  readonly fetch = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init)
    const url = new URL(request.url)
    this.requests.push({ method: request.method, url, headers: request.headers })
    const route = `${request.method} ${url.origin}${url.pathname}`
    const bearer = request.headers.get("authorization")?.replace(/^Bearer /, "")
    switch (route) {
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
      default: {
        const account = [...this.registryAccounts.values()].find(
          (candidate) => route === `GET ${registryUrl}/v1/accounts/${candidate.accountId}`
        )
        if (account === undefined) {
          return new Response(`No fake for ${route}`, { status: 599 })
        }
        return bearer === account.secret
          ? Response.json({
              id: account.accountId,
              revision: 0,
              name: "Octo Cat",
              email: this.github.email,
              planId: "6d3b6a42-2d71-4b0e-9b1a-1f4e7b3b2c10",
              roles: [],
              accountRootCardId: "0b7a4b6e-8c2b-4f63-bb0e-6a4cf7c1b9d2"
            })
          : new Response(null, { status: 401 })
      }
    }
  }

  registryLogins(): Array<RecordedRequest> {
    return this.requests.filter((request) => request.url.href.startsWith(`${registryUrl}/v1/login/oauth2`))
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
      ...(body === undefined ? {} : { body: JSON.stringify(body) })
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
