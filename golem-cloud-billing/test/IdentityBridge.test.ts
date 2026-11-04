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
  makeApp,
  newGitHubIdentity,
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
    expect(login!.url.searchParams.get("access-token")).toBe(upstreams.github.accessToken)

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

  it("keeps every Better Auth endpoint but the GitHub callback out of reach of a signed-in browser", async () => {
    const jar = new CookieJar()
    await signInWithGitHub(app.handler, jar)
    const json = { "content-type": "application/json", origin: baseUrl }
    const [account] = (
      await pool.query<{ id: string }>(`SELECT id FROM "${config.db.schema}"."account" WHERE "providerId" = 'github'`)
    ).rows

    // get-access-token returns the decrypted GitHub access token, list-accounts the account IDs
    // that it takes, and link-social adds another GitHub identity to the user.
    const responses = [
      await request(app.handler, jar, "POST", "/api/auth/get-access-token", json, { accountId: account!.id }),
      await request(app.handler, jar, "POST", "/api/auth/refresh-token", json, { accountId: account!.id }),
      await request(app.handler, jar, "GET", "/api/auth/get-session"),
      await request(app.handler, jar, "GET", "/api/auth/list-accounts"),
      await request(app.handler, jar, "POST", "/api/auth/link-social", json, { provider: "github" }),
      await request(app.handler, jar, "POST", "/api/auth/update-user", json, { name: "Mallory" })
    ]

    for (const response of responses) {
      const body = await response.text()
      expect(response.status).toBe(404)
      expect(body).not.toContain(upstreams.github.accessToken)
      expect(body).not.toContain(upstreams.registryAccount.secret)
    }
    const page = await (await request(app.handler, jar, "GET", "/")).text()
    expect(page).not.toContain(upstreams.github.accessToken)
    expect(page).not.toContain(upstreams.registryAccount.secret)
  })

  it("refuses a second GitHub identity with the email of an existing user", async () => {
    // The email moved from the first GitHub identity to another one, and the first identity's
    // access token is still valid.
    await signInWithGitHub(app.handler, new CookieJar())
    const first = upstreams.github
    upstreams.github = newGitHubIdentity(5555, first.email)

    const callback = await signInWithGitHub(app.handler, new CookieJar())

    expect(new URL(callback.headers.get("location")!, baseUrl).searchParams.get("error")).toBe("account_not_linked")
    expect(upstreams.registryLogins().map((login) => login.url.searchParams.get("access-token"))).toEqual([
      first.accessToken
    ])
    expect(await sessions()).toHaveLength(1)
    expect(await users()).toEqual([
      { email: first.email, golem_account_id: upstreams.accountOf(first).accountId }
    ])
  })

  it("refuses a session when the registry returns another account than the user has", async () => {
    await signInWithGitHub(app.handler, new CookieJar())
    const linked = upstreams.registryAccount.accountId
    upstreams.registryAccounts.set(upstreams.github.accessToken, {
      accountId: "0e9f2f4a-4f7c-4c55-9a3e-3b6a8f0d2c11",
      secret: "golem_other"
    })

    const callback = await signInWithGitHub(app.handler, new CookieJar())

    expect(new URL(callback.headers.get("location")!, baseUrl).searchParams.get("error")).toBe(
      "unable_to_create_session"
    )
    expect(await sessions()).toHaveLength(1)
    expect(await users()).toEqual([{ email: upstreams.github.email, golem_account_id: linked }])
  })

  it("refuses a session for a user with more than one GitHub account", async () => {
    await signInWithGitHub(app.handler, new CookieJar())
    await pool.query(
      `INSERT INTO "${config.db.schema}"."account" (id, "accountId", "providerId", "userId", "createdAt", "updatedAt")
       SELECT 'second-account', '9999', 'github', id, now(), now() FROM "${config.db.schema}"."user"`
    )

    const callback = await signInWithGitHub(app.handler, new CookieJar())

    expect(new URL(callback.headers.get("location")!, baseUrl).searchParams.get("error")).toBe(
      "unable_to_create_session"
    )
    expect(await sessions()).toHaveLength(1)
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
    expect(body).toMatch(/<form method="post" action="\/billing\/checkout">\s*<button type="submit"[^>]*>Upgrade to Paid<\/button>/)
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
