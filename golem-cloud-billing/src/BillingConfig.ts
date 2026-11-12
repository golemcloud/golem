import { Config, ConfigProvider, Context, Effect, Layer, Redacted } from "effect"

/**
 * TLS mode of the PostgreSQL connection.
 *
 * - `disable`: no TLS.
 * - `require`: TLS without certificate verification. The registry service connects this way.
 * - `verify-full`: TLS with certificate and host name verification against the system CA store
 *   and the certificates in `NODE_EXTRA_CA_CERTS`.
 */
export type DbTls = "disable" | "require" | "verify-full"

export interface PostgresConfig {
  readonly host: string
  readonly port: number
  readonly database: string
  readonly username: string
  readonly password: Redacted.Redacted<string>
  readonly maxConnections: number
  readonly schema: string
  /** The PostgreSQL schema of the registry tables, from which the app reads the metering tables. */
  readonly registrySchema: string
  readonly tls: DbTls
}

/**
 * The overage prices that the plan page shows, in USD, as decimal strings. Polar bills the
 * overage with the metered prices of its product; these values only show the prices, so they must
 * match the product.
 */
export interface OveragePrices {
  readonly perMillionGcu: string
  readonly perGbHour: string
  readonly perDurableGbMonth: string
  readonly perEphemeralGbMonth: string
}

export interface BillingConfigShape {
  readonly httpPort: number
  /** Public origin of the billing app, for example `https://billing.golem.cloud`. */
  readonly baseUrl: URL
  readonly authSecret: Redacted.Redacted<string>
  readonly github: {
    readonly clientId: string
    readonly clientSecret: Redacted.Redacted<string>
  }
  /** Base URL of the registry service API that the billing app calls. */
  readonly registryUrl: URL
  /** Public URL of the Golem API that the CLI connects to. */
  readonly publicApiUrl: URL
  /**
   * The registry token of the billing account. A permission share of an administrator gives
   * this account `view` and `set-plan` on every account, so the webhooks can change a plan.
   */
  readonly registryBillingToken: Redacted.Redacted<string>
  /** The plan IDs that a subscription switches between (see `Settlement.ts`). */
  readonly plans: {
    readonly free: string
    readonly paid: string
    /** No monthly amounts and no overage, so the agents of the account stop. */
    readonly suspended: string
  }
  readonly overagePrices: OveragePrices
  /**
   * How long an account keeps the paid plan after a payment of its subscription fails, in
   * milliseconds (see `Settlement.ts`). Polar retries the payment in this time.
   */
  readonly pastDueGraceMs: number
  readonly polar: {
    /** The base URL of the Polar API: `https://sandbox-api.polar.sh` or `https://api.polar.sh`. */
    readonly apiUrl: URL
    /**
     * An organization access token of Polar that can create checkouts and customer sessions
     * (`checkouts:write`, `customer_sessions:write`) and nothing else.
     */
    readonly accessToken: Redacted.Redacted<string>
    /** The Polar product of the paid plan. */
    readonly productId: string
    /** The secret that Polar signs the webhooks with (`whsec_...`). */
    readonly webhookSecret: Redacted.Redacted<string>
  }
  readonly db: PostgresConfig
}

export class BillingConfig extends Context.Service<BillingConfig, BillingConfigShape>()(
  "golem-cloud-billing/BillingConfig"
) {}

const schemaName = /^[a-z_][a-z0-9_]*$/

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

const decimal = /^(0|[1-9][0-9]*)(\.[0-9]+)?$/

const invalid = (message: string): Effect.Effect<never, Config.ConfigError> =>
  Effect.fail(new Config.ConfigError(new ConfigProvider.SourceError({ message })))

const DAY_MS = 24 * 60 * 60 * 1000

/**
 * The connection settings use the environment variables of the registry service
 * (`GOLEM__DB__CONFIG__*`), so one secret configures both services against the same database.
 * The registry's `GOLEM__DB__CONFIG__SCHEMA` is not read: the billing app keeps its tables in
 * its own schema.
 */
export const postgresConfig: Config.Config<PostgresConfig> = Config.all({
  type: Config.String("GOLEM__DB__TYPE").pipe(Config.withDefault("Postgres")),
  host: Config.String("GOLEM__DB__CONFIG__HOST"),
  port: Config.Port("GOLEM__DB__CONFIG__PORT").pipe(Config.withDefault(5432)),
  database: Config.String("GOLEM__DB__CONFIG__DATABASE"),
  username: Config.String("GOLEM__DB__CONFIG__USERNAME"),
  password: Config.Redacted("GOLEM__DB__CONFIG__PASSWORD"),
  maxConnections: Config.Int("GOLEM__DB__CONFIG__MAX_CONNECTIONS").pipe(Config.withDefault(10)),
  schema: Config.String("GOLEM_CLOUD_BILLING_DB_SCHEMA").pipe(Config.withDefault("billing")),
  registrySchema: Config.String("GOLEM_CLOUD_BILLING_REGISTRY_DB_SCHEMA").pipe(Config.withDefault("public")),
  tls: Config.Literals(["disable", "require", "verify-full"], "GOLEM_CLOUD_BILLING_DB_TLS").pipe(
    Config.withDefault("verify-full")
  )
}).pipe(
  Config.mapEffect(({ type, ...db }) => {
    if (type !== "Postgres") {
      return invalid(`GOLEM__DB__TYPE must be Postgres, got ${type}`)
    }
    if (!schemaName.test(db.schema)) {
      return invalid(`GOLEM_CLOUD_BILLING_DB_SCHEMA must match ${schemaName.source}, got ${db.schema}`)
    }
    if (!schemaName.test(db.registrySchema)) {
      return invalid(
        `GOLEM_CLOUD_BILLING_REGISTRY_DB_SCHEMA must match ${schemaName.source}, got ${db.registrySchema}`
      )
    }
    return Effect.succeed(db)
  })
)

export const billingConfig: Config.Config<BillingConfigShape> = Config.all({
  httpPort: Config.Port("GOLEM_CLOUD_BILLING_HTTP_PORT").pipe(Config.withDefault(8080)),
  baseUrl: Config.URL("BETTER_AUTH_URL"),
  authSecret: Config.Redacted("BETTER_AUTH_SECRET"),
  github: Config.all({
    clientId: Config.String("GITHUB_CLIENT_ID"),
    clientSecret: Config.Redacted("GITHUB_CLIENT_SECRET")
  }),
  registryUrl: Config.URL("GOLEM_CLOUD_BILLING_REGISTRY_URL"),
  publicApiUrl: Config.URL("GOLEM_CLOUD_BILLING_PUBLIC_API_URL"),
  registryBillingToken: Config.Redacted("GOLEM_CLOUD_BILLING_REGISTRY_TOKEN"),
  plans: Config.all({
    free: Config.String("GOLEM_CLOUD_BILLING_FREE_PLAN_ID"),
    paid: Config.String("GOLEM_CLOUD_BILLING_PAID_PLAN_ID"),
    suspended: Config.String("GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID")
  }),
  overagePrices: Config.all({
    perMillionGcu: Config.String("GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_MILLION_GCU"),
    perGbHour: Config.String("GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_GB_HOUR"),
    perDurableGbMonth: Config.String("GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_DURABLE_GB_MONTH"),
    perEphemeralGbMonth: Config.String("GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_EPHEMERAL_GB_MONTH")
  }),
  pastDueGraceMs: Config.Int("GOLEM_CLOUD_BILLING_PAST_DUE_GRACE_DAYS").pipe(
    Config.withDefault(7),
    Config.mapEffect((days) =>
      days < 0
        ? invalid(`GOLEM_CLOUD_BILLING_PAST_DUE_GRACE_DAYS must be 0 or more, got ${days}`)
        : Effect.succeed(days * DAY_MS)
    )
  ),
  polar: Config.all({
    apiUrl: Config.URL("POLAR_API_URL"),
    accessToken: Config.Redacted("POLAR_ACCESS_TOKEN"),
    productId: Config.String("POLAR_PRODUCT_ID"),
    webhookSecret: Config.Redacted("POLAR_WEBHOOK_SECRET")
  }),
  db: postgresConfig
}).pipe(
  Config.mapEffect((config) => {
    if (Redacted.value(config.authSecret).length < 32) {
      return invalid("BETTER_AUTH_SECRET must be at least 32 characters")
    }
    const plans = [
      ["GOLEM_CLOUD_BILLING_FREE_PLAN_ID", config.plans.free],
      ["GOLEM_CLOUD_BILLING_PAID_PLAN_ID", config.plans.paid],
      ["GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID", config.plans.suspended]
    ] as const
    for (const [name, value] of plans) {
      if (!uuid.test(value)) {
        return invalid(`${name} must be a UUID, got ${value}`)
      }
    }
    if (new Set(plans.map(([, value]) => value)).size !== plans.length) {
      return invalid(`${plans.map(([name]) => name).join(", ")} must differ`)
    }
    const prices = [
      ["GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_MILLION_GCU", config.overagePrices.perMillionGcu],
      ["GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_GB_HOUR", config.overagePrices.perGbHour],
      ["GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_DURABLE_GB_MONTH", config.overagePrices.perDurableGbMonth],
      ["GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_EPHEMERAL_GB_MONTH", config.overagePrices.perEphemeralGbMonth]
    ] as const
    for (const [name, value] of prices) {
      if (!decimal.test(value)) {
        return invalid(`${name} must be a decimal number of USD, such as 0.045, got ${value}`)
      }
    }
    if (!Redacted.value(config.polar.webhookSecret).startsWith("whsec_")) {
      return invalid("POLAR_WEBHOOK_SECRET must start with whsec_")
    }
    return Effect.succeed(config)
  })
)

export const layer: Layer.Layer<BillingConfig, Config.ConfigError> = Layer.effect(
  BillingConfig,
  billingConfig
)
