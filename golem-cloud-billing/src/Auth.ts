import { inspect } from "node:util"
import { type BetterAuthOptions, betterAuth, type GenericEndpointContext, type Session } from "better-auth"
import { symmetricDecrypt, symmetricEncrypt } from "better-auth/crypto"
import { decryptOAuthToken } from "better-auth/oauth2"
import { Context, Data, Effect, Layer, Option, Redacted } from "effect"
import type pg from "pg"
import { BillingConfig } from "./BillingConfig.ts"
import { Database, requireTables, type SchemaError } from "./Database.ts"
import { Registry, type RegistryShape } from "./Registry.ts"
import { RegistryAccounts, type RegistryAccountsShape } from "./RegistryAccounts.ts"

type SessionCreateResult = boolean | void | { data: Session & Record<string, unknown> }

export type AuthLogLevel = "debug" | "info" | "warn" | "error"

export interface AuthOptions {
  readonly pool: pg.Pool
  readonly baseUrl: URL
  readonly secret: Redacted.Redacted<string>
  readonly github: { readonly clientId: string; readonly clientSecret: Redacted.Redacted<string> }
  readonly onSessionCreate: (
    session: Session & Record<string, unknown>,
    context: GenericEndpointContext | null
  ) => Promise<SessionCreateResult>
  readonly log: (level: AuthLogLevel, message: string, args: ReadonlyArray<unknown>) => void
}

/** The tables of Better Auth, which `Migrations.migrate` creates from `schemaOptions`. */
export const tableNames: ReadonlyArray<string> = ["user", "session", "account", "verification"]

/**
 * The options of Better Auth that decide its tables: the database and the extra fields of the user
 * and the session. The web app and `Migrations.migrate` both use them, so the migration creates the
 * tables that the web app reads.
 */
export const schemaOptions = (pool: pg.Pool) =>
  ({
    // The pool sets `search_path` to the schema of the billing app, and Better Auth reads its
    // schema from there. With `database.schemaName` instead, the migrations would run
    // `CREATE SCHEMA IF NOT EXISTS`, which needs the CREATE privilege on the database even when
    // the schema exists.
    database: pool,
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
    telemetry: { enabled: false }
  }) satisfies BetterAuthOptions

/** The logger option of Better Auth, which passes each log line to `log`. */
export const loggerOptions = (log: AuthOptions["log"]) =>
  ({
    level: "info",
    log: (level, message, ...args) => log(level, message, args)
  }) satisfies BetterAuthOptions["logger"]

export const authOptions = (options: AuthOptions) =>
  ({
    ...schemaOptions(options.pool),
    appName: "Golem Cloud",
    baseURL: options.baseUrl.origin,
    secret: Redacted.value(options.secret),
    socialProviders: {
      github: {
        clientId: options.github.clientId,
        clientSecret: Redacted.value(options.github.clientSecret)
      }
    },
    account: {
      encryptOAuthTokens: true,
      // A GitHub identity signs in only to the user that it created. With linking, a GitHub
      // identity with the verified email of an existing user would join that user, and the
      // identity bridge would exchange the GitHub token of the other identity.
      accountLinking: { enabled: false }
    },
    databaseHooks: {
      session: {
        create: {
          before: options.onSessionCreate
        }
      }
    },
    logger: loggerOptions(options.log),
    // Errors that end the OAuth flow redirect to the landing page with `?error=<code>`. The
    // default error page of Better Auth is not routed.
    onAPIError: { errorURL: "/" }
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

export const describeCause = (cause: unknown): string =>
  cause instanceof Error ? cause.message : inspect(cause, { depth: 4, breakLength: Infinity })

/** Writes a log line of Better Auth to the Effect logger of `services`. */
export const logWith =
  (services: Context.Context<never>): AuthOptions["log"] =>
  (level, message, args) => {
    Effect.runForkWith(services)(
      logAt(level)(message).pipe(Effect.annotateLogs({ source: "better-auth", details: args.map(describeCause) }))
    )
  }

export class IdentityBridgeError extends Data.TaggedError("IdentityBridgeError")<{
  readonly reason: string
  readonly cause?: unknown
}> {
  override get message(): string {
    return `Cannot link the session to a Golem account: ${this.reason}`
  }
}

export class SessionError extends Data.TaggedError("SessionError")<{
  readonly reason: string
  readonly cause: unknown
}> {
  override get message(): string {
    return `Cannot read the session: ${this.reason}: ${describeCause(this.cause)}`
  }
}

const tryAuth = <A>(reason: string, f: () => Promise<A>) =>
  Effect.tryPromise({ try: f, catch: (cause) => new IdentityBridgeError({ reason, cause }) })

const trySession = <A>(reason: string, f: () => Promise<A>) =>
  Effect.tryPromise({ try: f, catch: (cause) => new SessionError({ reason, cause }) })

/**
 * The identity bridge. Before Better Auth stores a new session, it exchanges the GitHub access
 * token of the user for a Golem API token, writes the Golem account ID onto the user and puts
 * the encrypted Golem API token on the session. When any step fails, no session is created.
 *
 * Without account linking, only the GitHub identity of the user signs in to it, and Better Auth
 * stores the access token of the sign-in on that account before it creates the session. The
 * bridge therefore requires exactly one GitHub account, and it refuses a Golem account other than
 * the one that the user has already. After the registry deletes the account, the registry gives the
 * GitHub identity a new Golem account at the next login, and the bridge still refuses the user of
 * the deleted account. The cleanup deletes the Polar customer of the deleted account first and the
 * user after it, so the next sign-in creates a new user for the new account. A checkout of the new
 * account therefore never finds the Polar customer of the deleted account by its email.
 */
export const linkSessionToGolemAccount =
  (registry: RegistryShape, registryAccounts: RegistryAccountsShape) =>
  (session: Session & Record<string, unknown>, context: GenericEndpointContext | null) =>
    Effect.gen(function* () {
      if (context === null) {
        return yield* new IdentityBridgeError({ reason: "the session has no endpoint context" })
      }
      const auth = context.context
      const accounts = yield* tryAuth("cannot read the accounts of the user", () =>
        auth.internalAdapter.findAccounts(session.userId)
      )
      const githubAccounts = accounts.filter((account) => account.providerId === "github")
      if (githubAccounts.length !== 1) {
        return yield* new IdentityBridgeError({
          reason: `the user has ${githubAccounts.length} GitHub accounts, not one`
        })
      }
      const storedAccessToken = githubAccounts[0]!.accessToken
      if (!storedAccessToken) {
        return yield* new IdentityBridgeError({ reason: "the user has no GitHub access token" })
      }
      const user = yield* tryAuth("cannot read the user", () =>
        auth.internalAdapter.findUserById(session.userId)
      )
      const linkedAccountId = (user as { golemAccountId?: string | null } | null)?.golemAccountId
      const accessToken = yield* tryAuth("cannot decrypt the GitHub access token", async () =>
        Redacted.make(await decryptOAuthToken(storedAccessToken, auth))
      )
      const golemToken = yield* registry.exchangeGitHubAccessToken(accessToken)
      if (linkedAccountId && linkedAccountId !== golemToken.accountId) {
        const linkedIsDeleted = yield* registryAccounts.isDeleted(linkedAccountId).pipe(
          Effect.mapError(
            (cause) => new IdentityBridgeError({ reason: "cannot read whether the linked Golem account is deleted", cause })
          )
        )
        return yield* new IdentityBridgeError({
          reason: linkedIsDeleted
            ? `the user is linked to the deleted Golem account ${linkedAccountId}, and the cleanup has not deleted the user yet`
            : `the registry returned the Golem account ${golemToken.accountId}, but the user is linked to ${linkedAccountId}`
        })
      }
      if (!linkedAccountId) {
        yield* tryAuth("cannot store the Golem account ID", () =>
          auth.internalAdapter.updateUser(session.userId, { golemAccountId: golemToken.accountId })
        )
      }
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
  readonly getSignedIn: (headers: Headers) => Effect.Effect<Option.Option<SignedIn>, SessionError>
}

export class Auth extends Context.Service<Auth, AuthShape>()("golem-cloud-billing/Auth") {}

/**
 * Better Auth of the web app. The web app creates no table: `Migrations.migrate` has created the
 * tables of Better Auth before. The layer checks them before it creates Better Auth, which would
 * log an error for each missing table, and fails when one is missing.
 */
export const layer: Layer.Layer<Auth, SchemaError, BillingConfig | Database | Registry | RegistryAccounts> =
  Layer.effect(
    Auth,
    Effect.gen(function* () {
      yield* requireTables(tableNames)
      const config = yield* BillingConfig
      const { pool } = yield* Database
      const registry = yield* Registry
      const registryAccounts = yield* RegistryAccounts
      const services = yield* Effect.context<never>()
      const link = linkSessionToGolemAccount(registry, registryAccounts)

      const options = authOptions({
        pool,
        baseUrl: config.baseUrl,
        secret: config.authSecret,
        github: config.github,
        onSessionCreate: (session, context) => Effect.runPromiseWith(services)(link(session, context)),
        log: logWith(services)
      })
      const auth = betterAuth(options)

      const getSignedIn = (headers: Headers) =>
        Effect.gen(function* () {
          const result = yield* trySession("Better Auth failed", () => auth.api.getSession({ headers }))
          if (result === null || !result.user.golemAccountId) {
            return Option.none<SignedIn>()
          }
          const authContext = yield* trySession("Better Auth failed", () => auth.$context)
          const row = yield* trySession("cannot read the session row", () =>
            authContext.adapter.findOne<{ golemToken?: string | null }>({
              model: "session",
              where: [{ field: "id", value: result.session.id }]
            })
          )
          if (!row?.golemToken) {
            return Option.none<SignedIn>()
          }
          const encrypted = row.golemToken
          const golemToken = yield* trySession("cannot decrypt the Golem API token", async () =>
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
