import { inspect } from "node:util"
import { type BetterAuthOptions, betterAuth, type GenericEndpointContext, type Session } from "better-auth"
import { symmetricDecrypt, symmetricEncrypt } from "better-auth/crypto"
import { getMigrations } from "better-auth/db/migration"
import { decryptOAuthToken } from "better-auth/oauth2"
import { Context, Data, Effect, Layer, Option, Redacted } from "effect"
import { PostgresDialect } from "kysely"
import type pg from "pg"
import { BillingConfig } from "./BillingConfig.ts"
import { Database, withAdvisoryLock } from "./Database.ts"
import { Registry, type RegistryShape } from "./Registry.ts"

type SessionCreateResult = boolean | void | { data: Session & Record<string, unknown> }

export type AuthLogLevel = "debug" | "info" | "warn" | "error"

export interface AuthOptions {
  readonly pool: pg.Pool
  readonly schema: string
  readonly baseUrl: URL
  readonly secret: Redacted.Redacted<string>
  readonly github: { readonly clientId: string; readonly clientSecret: Redacted.Redacted<string> }
  readonly onSessionCreate: (
    session: Session & Record<string, unknown>,
    context: GenericEndpointContext | null
  ) => Promise<SessionCreateResult>
  readonly log: (level: AuthLogLevel, message: string, args: ReadonlyArray<unknown>) => void
}

export const authOptions = (options: AuthOptions) =>
  ({
    appName: "Golem Cloud",
    baseURL: options.baseUrl.origin,
    secret: Redacted.value(options.secret),
    database: {
      dialect: new PostgresDialect({ pool: options.pool }),
      type: "postgres",
      schemaName: options.schema,
      transaction: true
    },
    socialProviders: {
      github: {
        clientId: options.github.clientId,
        clientSecret: Redacted.value(options.github.clientSecret)
      }
    },
    account: {
      encryptOAuthTokens: true
    },
    user: {
      additionalFields: {
        golemAccountId: {
          type: "string",
          required: false,
          input: false,
          unique: true,
          fieldName: "golem_account_id"
        }
      }
    },
    session: {
      additionalFields: {
        // The Golem API token of the user, encrypted with the Better Auth secret. It never
        // leaves the server.
        golemToken: {
          type: "string",
          required: false,
          input: false,
          returned: false,
          fieldName: "golem_token"
        }
      }
    },
    databaseHooks: {
      session: {
        create: {
          before: options.onSessionCreate
        }
      }
    },
    logger: {
      level: "info",
      log: (level, message, ...args) => options.log(level, message, args)
    },
    telemetry: { enabled: false }
  }) satisfies BetterAuthOptions

export const makeAuth = (options: AuthOptions) => betterAuth(authOptions(options))

export type BillingAuth = ReturnType<typeof makeAuth>

const logAt = (level: AuthLogLevel) => {
  switch (level) {
    case "debug":
      return Effect.logDebug
    case "info":
      return Effect.logInfo
    case "warn":
      return Effect.logWarning
    case "error":
      return Effect.logError
  }
}

export class IdentityBridgeError extends Data.TaggedError("IdentityBridgeError")<{
  readonly reason: string
  readonly cause?: unknown
}> {
  override get message(): string {
    return `Cannot link the session to a Golem account: ${this.reason}`
  }
}

const tryAuth = <A>(reason: string, f: () => Promise<A>) =>
  Effect.tryPromise({ try: f, catch: (cause) => new IdentityBridgeError({ reason, cause }) })

const describeCause = (cause: unknown): string =>
  cause instanceof Error ? cause.message : inspect(cause, { depth: 4, breakLength: Infinity })

/**
 * The identity bridge. Before Better Auth stores a new session, it exchanges the GitHub access
 * token of the user for a Golem API token, writes the Golem account ID onto the user and puts
 * the encrypted Golem API token on the session. When any step fails, no session is created.
 */
export const linkSessionToGolemAccount =
  (registry: RegistryShape) =>
  (session: Session & Record<string, unknown>, context: GenericEndpointContext | null) =>
    Effect.gen(function* () {
      if (context === null) {
        return yield* new IdentityBridgeError({ reason: "the session has no endpoint context" })
      }
      const auth = context.context
      const accounts = yield* tryAuth("cannot read the accounts of the user", () =>
        auth.internalAdapter.findAccounts(session.userId)
      )
      const storedAccessToken = accounts.find((account) => account.providerId === "github")?.accessToken
      if (!storedAccessToken) {
        return yield* new IdentityBridgeError({ reason: "the user has no GitHub access token" })
      }
      const accessToken = yield* tryAuth("cannot decrypt the GitHub access token", async () =>
        Redacted.make(await decryptOAuthToken(storedAccessToken, auth))
      )
      const golemToken = yield* registry.exchangeGitHubAccessToken(accessToken)
      yield* tryAuth("cannot store the Golem account ID", () =>
        auth.internalAdapter.updateUser(session.userId, { golemAccountId: golemToken.accountId })
      )
      const encryptedSecret = yield* tryAuth("cannot encrypt the Golem API token", () =>
        symmetricEncrypt({ key: auth.secretConfig, data: golemToken.secret })
      )
      yield* Effect.logInfo("Linked the session to a Golem account").pipe(
        Effect.annotateLogs({ golemAccountId: golemToken.accountId })
      )
      return { data: { ...session, golemToken: encryptedSecret } }
    }).pipe(
      Effect.catch((error) =>
        Effect.logError("Refused to create a session").pipe(
          Effect.annotateLogs({
            error: error.message,
            ...(error._tag === "IdentityBridgeError" && error.cause !== undefined
              ? { cause: describeCause(error.cause) }
              : {})
          }),
          Effect.as(false)
        )
      )
    )

export interface SignedIn {
  readonly userId: string
  readonly name: string
  readonly email: string
  readonly golemAccountId: string
  readonly golemToken: Redacted.Redacted<string>
}

export interface AuthShape {
  readonly auth: BillingAuth
  /** Returns the signed-in user of the request, if the session is linked to a Golem account. */
  readonly getSignedIn: (headers: Headers) => Effect.Effect<Option.Option<SignedIn>, IdentityBridgeError>
}

export class Auth extends Context.Service<Auth, AuthShape>()("golem-cloud-billing/Auth") {}

/**
 * Applies the Better Auth migrations, then turns `user.golem_account_id` into a `uuid` column.
 * Better Auth has no `uuid` field type, so it creates the column as `text`; its migration check
 * accepts `uuid` for a string field, so later runs leave the column alone.
 *
 * Call it before `betterAuth`: a new Better Auth instance checks the schema at once and logs
 * an error for every missing table.
 */
export const migrate = (options: BetterAuthOptions, pool: pg.Pool, schema: string): Promise<void> =>
  withAdvisoryLock(pool, "golem-cloud-billing/migrations", async () => {
    const { runMigrations } = await getMigrations(options)
    await runMigrations()
    const column = await pool.query<{ data_type: string }>(
      `SELECT data_type FROM information_schema.columns
       WHERE table_schema = $1 AND table_name = 'user' AND column_name = 'golem_account_id'`,
      [schema]
    )
    if (column.rows[0]?.data_type !== "uuid") {
      await pool.query(
        `ALTER TABLE "${schema}"."user"
         ALTER COLUMN golem_account_id TYPE uuid USING golem_account_id::uuid`
      )
    }
  })

export const layer: Layer.Layer<Auth, IdentityBridgeError, BillingConfig | Database | Registry> =
  Layer.effect(
    Auth,
    Effect.gen(function* () {
      const config = yield* BillingConfig
      const { pool, schema } = yield* Database
      const registry = yield* Registry
      const services = yield* Effect.context<never>()
      const link = linkSessionToGolemAccount(registry)

      const options = authOptions({
        pool,
        schema,
        baseUrl: config.baseUrl,
        secret: config.authSecret,
        github: config.github,
        onSessionCreate: (session, context) => Effect.runPromiseWith(services)(link(session, context)),
        log: (level, message, args) => {
          Effect.runForkWith(services)(
            logAt(level)(message).pipe(
              Effect.annotateLogs({ source: "better-auth", details: args.map(describeCause) })
            )
          )
        }
      })

      yield* tryAuth("cannot apply the database migrations", () => migrate(options, pool, schema))
      yield* Effect.logInfo("Applied the database migrations").pipe(Effect.annotateLogs({ schema }))
      const auth = betterAuth(options)

      const getSignedIn = (headers: Headers) =>
        Effect.gen(function* () {
          const result = yield* tryAuth("cannot read the session", () => auth.api.getSession({ headers }))
          if (result === null || !result.user.golemAccountId) {
            return Option.none<SignedIn>()
          }
          const authContext = yield* tryAuth("cannot read the auth context", () => auth.$context)
          const row = yield* tryAuth("cannot read the session", () =>
            authContext.adapter.findOne<{ golemToken?: string | null }>({
              model: "session",
              where: [{ field: "id", value: result.session.id }]
            })
          )
          if (!row?.golemToken) {
            return Option.none<SignedIn>()
          }
          const encrypted = row.golemToken
          const golemToken = yield* tryAuth("cannot decrypt the Golem API token", async () =>
            Redacted.make(await symmetricDecrypt({ key: authContext.secretConfig, data: encrypted }))
          )
          return Option.some<SignedIn>({
            userId: result.user.id,
            name: result.user.name,
            email: result.user.email,
            golemAccountId: result.user.golemAccountId,
            golemToken
          })
        })

      return { auth, getSignedIn }
    })
  )
