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
  readonly tls: DbTls
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
  readonly db: PostgresConfig
}

export class BillingConfig extends Context.Service<BillingConfig, BillingConfigShape>()(
  "golem-cloud-billing/BillingConfig"
) {}

const schemaName = /^[a-z_][a-z0-9_]*$/

const invalid = (message: string): Effect.Effect<never, Config.ConfigError> =>
  Effect.fail(new Config.ConfigError(new ConfigProvider.SourceError({ message })))

/**
 * The connection settings use the environment variables of the registry service
 * (`GOLEM__DB__CONFIG__*`), so one secret configures both services against the same database.
 * The registry's `GOLEM__DB__CONFIG__SCHEMA` is not read: the billing app keeps its tables in
 * its own schema.
 */
const postgresConfig: Config.Config<PostgresConfig> = Config.all({
  type: Config.String("GOLEM__DB__TYPE").pipe(Config.withDefault("Postgres")),
  host: Config.String("GOLEM__DB__CONFIG__HOST"),
  port: Config.Port("GOLEM__DB__CONFIG__PORT").pipe(Config.withDefault(5432)),
  database: Config.String("GOLEM__DB__CONFIG__DATABASE"),
  username: Config.String("GOLEM__DB__CONFIG__USERNAME"),
  password: Config.Redacted("GOLEM__DB__CONFIG__PASSWORD"),
  maxConnections: Config.Int("GOLEM__DB__CONFIG__MAX_CONNECTIONS").pipe(Config.withDefault(10)),
  schema: Config.String("GOLEM_CLOUD_BILLING_DB_SCHEMA").pipe(Config.withDefault("billing")),
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
  db: postgresConfig
}).pipe(
  Config.mapEffect((config) =>
    Redacted.value(config.authSecret).length < 32
      ? invalid("BETTER_AUTH_SECRET must be at least 32 characters")
      : Effect.succeed(config)
  )
)

export const layer: Layer.Layer<BillingConfig, Config.ConfigError> = Layer.effect(
  BillingConfig,
  billingConfig
)
