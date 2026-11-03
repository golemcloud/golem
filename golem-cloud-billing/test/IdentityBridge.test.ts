import { symmetricDecrypt } from "better-auth/crypto"
import type pg from "pg"
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import type { BillingConfigShape } from "../src/BillingConfig.ts"
import {
  authSecret,
  baseUrl,
  CookieJar,
  dropSchema,
  FakeUpstreams,
  githubAccessToken,
  makeApp,
  queryPool,
  request,
  signInWithGitHub,
  testConfig
} from "./support.ts"

describe("GitHub sign-in", () => {
  let config: BillingConfigShape
  let pool: pg.Pool
  let app: ReturnType<typeof makeApp>
  let upstreams: FakeUpstreams

  beforeAll(() => {
    config = testConfig()
    pool = queryPool(config)
    app = makeApp(config)
    // Effect's fetch client keeps the `fetch` it sees first, so the stub delegates to the fake
    // upstreams of the running test.
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
    await pool.query(`TRUNCATE "${config.db.schema}"."user" CASCADE`)
  })

  const users = async () =>
    (
      await pool.query<{ email: string; golem_account_id: string | null }>(
        `SELECT email, golem_account_id FROM "${config.db.schema}"."user"`
      )
    ).rows

  const sessions = async () =>
    (await pool.query<{ golem_token: string | null }>(`SELECT golem_token FROM "${config.db.schema}"."session"`))
      .rows

  it("links the user to the Golem account of the GitHub identity", async () => {
    const jar = new CookieJar()
    const callback = await signInWithGitHub(app.handler, jar)

    expect(callback.status).toBe(302)
    expect(new URL(callback.headers.get("location")!, baseUrl).pathname).toBe("/")

    const [login, ...otherLogins] = upstreams.registryLogins()
    expect(otherLogins).toEqual([])
    expect(login!.url.searchParams.get("provider")).toBe("github")
    expect(login!.url.searchParams.get("access-token")).toBe(githubAccessToken)

    expect(await users()).toEqual([
      { email: "octocat@example.com", golem_account_id: upstreams.registryAccount.accountId }
    ])
  })

  it("stores the Golem API token encrypted on the session", async () => {
    await signInWithGitHub(app.handler, new CookieJar())

    const [session, ...otherSessions] = await sessions()
    expect(otherSessions).toEqual([])
    expect(session!.golem_token).not.toBeNull()
    expect(session!.golem_token).not.toContain(upstreams.registryAccount.secret)
    expect(await symmetricDecrypt({ key: authSecret, data: session!.golem_token! })).toBe(
      upstreams.registryAccount.secret
    )
  })

  it("does not expose the Golem API token through the session endpoint", async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)

    const response = await request(app.handler, jar, "GET", "/api/auth/get-session")
    const body = await response.text()

    expect(response.status).toBe(200)
    expect(body).toContain(upstreams.registryAccount.accountId)
    expect(body).not.toContain("golemToken")
    expect(body).not.toContain(upstreams.registryAccount.secret)
  })

  it("keeps the account of an earlier sign-in, as for a user who signed in with the CLI first", async () => {
    await signInWithGitHub(app.handler, new CookieJar())
    await signInWithGitHub(app.handler, new CookieJar())

    expect(upstreams.registryLogins()).toHaveLength(2)
    expect(await users()).toEqual([
      { email: "octocat@example.com", golem_account_id: upstreams.registryAccount.accountId }
    ])
    expect(await sessions()).toHaveLength(2)
  })

  it("creates no session when the registry rejects the GitHub access token", async () => {
    upstreams.registryLoginStatus = 503

    const callback = await signInWithGitHub(app.handler, new CookieJar())

    const location = new URL(callback.headers.get("location")!, baseUrl)
    expect(location.pathname).toBe("/")
    expect(location.searchParams.get("error")).toBe("unable_to_create_session")
    expect(await sessions()).toEqual([])
    expect(await users()).toEqual([{ email: "octocat@example.com", golem_account_id: null }])
  })

  it("shows the Free plan, the account and the CLI command after sign-in", async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)

    const response = await request(app.handler, jar, "GET", "/")
    const body = await response.text()

    expect(response.status).toBe(200)
    expect(body).toContain("You're on Free")
    expect(body).toContain(upstreams.registryAccount.accountId)
    expect(body).toContain("octocat@example.com")
    expect(body).toContain("golem profile new golem-cloud --url https://release.dev-api.golem.cloud --set-active")
    expect(body).toMatch(/<button[^>]*disabled>Upgrade to Paid<\/button>/)
    expect(body).toContain("Coming soon")
  })

  it("shows the sign-in form without a session", async () => {
    const response = await request(app.handler, new CookieJar(), "GET", "/")
    const body = await response.text()

    expect(response.status).toBe(200)
    expect(body).toContain('action="/sign-in/github"')
    expect(body).not.toContain("You're on Free")
  })

  it("rejects a cross-origin sign-in form post", async () => {
    const response = await request(app.handler, new CookieJar(), "POST", "/sign-in/github", {
      origin: "https://attacker.example"
    })

    expect(response.status).toBe(403)
  })

  it("signs out", async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)

    const signOut = await request(app.handler, jar, "POST", "/sign-out", { origin: baseUrl })
    const page = await request(app.handler, jar, "GET", "/")

    expect(signOut.status).toBe(303)
    expect(await page.text()).toContain('action="/sign-in/github"')
    expect(await sessions()).toEqual([])
  })
})
