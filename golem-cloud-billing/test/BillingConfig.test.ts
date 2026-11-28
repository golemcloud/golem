import { ConfigProvider, Effect, Redacted } from "effect"
import { expect, it } from "vitest"
import { billingConfig } from "../src/BillingConfig.ts"

it("reads the shared PostgreSQL settings and refuses a different state store", async () => {
  const settings = {
    GOLEM__STATE_STORE: "postgres",
    GOLEM__POSTGRES__HOST: "state.internal",
    GOLEM__POSTGRES__PORT: "25432",
    GOLEM__POSTGRES__DATABASE: "installation",
    GOLEM__POSTGRES__USERNAME: "billing",
    GOLEM__POSTGRES__PASSWORD: "test-password",
    GOLEM__POSTGRES__MAX_CONNECTIONS: "37",
    GOLEM_CLOUD_BILLING_DB_SCHEMA: "billing_tables",
    BETTER_AUTH_URL: "https://billing.example.com",
    BETTER_AUTH_SECRET: "test-secret-with-at-least-32-characters",
    GITHUB_CLIENT_ID: "test-client",
    GITHUB_CLIENT_SECRET: "test-secret",
    GOLEM_CLOUD_BILLING_REGISTRY_URL: "http://registry.internal",
    GOLEM_CLOUD_BILLING_PUBLIC_API_URL: "https://api.example.com"
  }
  const config = await Effect.runPromise(billingConfig.parse(ConfigProvider.fromUnknown(settings)))
  expect(config.db).toMatchObject({
    host: "state.internal",
    port: 25432,
    database: "installation",
    username: "billing",
    maxConnections: 37,
    schema: "billing_tables"
  })
  expect(Redacted.value(config.db.password)).toBe("test-password")
  await expect(
    Effect.runPromise(
      billingConfig.parse(ConfigProvider.fromUnknown({ ...settings, GOLEM__STATE_STORE: "sqlite" }))
    )
  ).rejects.toThrow("GOLEM__STATE_STORE must be postgres")
})
