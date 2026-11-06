import { Context, Data, Effect, Layer, Redacted, Schema } from "effect"
import { HttpBody, HttpClient, HttpClientError, HttpIncomingMessage } from "effect/http"
import { BillingConfig } from "./BillingConfig.ts"

const Uuid = Schema.String.check(Schema.isUUID())

/** `TokenWithSecret` of the registry service API. */
export const TokenWithSecret = Schema.Struct({
  id: Uuid,
  secret: Schema.String,
  accountId: Uuid,
  createdAt: Schema.String,
  expiresAt: Schema.String
})
export type TokenWithSecret = typeof TokenWithSecret.Type

/** The fields of the registry service's `Account` that the billing app reads. */
export const Account = Schema.Struct({
  id: Uuid,
  revision: Schema.Number,
  name: Schema.String,
  email: Schema.String,
  planId: Uuid
})
export type Account = typeof Account.Type

/**
 * `hardLimit` stops the agents of the account at its monthly amounts. `allowOverage` lets them
 * run on, and the usage above the monthly amounts is billed as paid overage.
 */
export const MonthlyUsageMode = Schema.Literals(["hardLimit", "allowOverage"])
export type MonthlyUsageMode = typeof MonthlyUsageMode.Type

/** The fields of the registry service's `AccountResourcePolicy` that the billing app reads. */
export const AccountLimits = Schema.Struct({
  monthlyUsageMode: MonthlyUsageMode,
  overageAllowedByPlan: Schema.Boolean
})
export type AccountLimits = typeof AccountLimits.Type

/**
 * The result of a change of the monthly usage mode: `changed`, `unchanged` when the account is
 * in that mode already, or `notEligible` when the plan of the account has no paid overage.
 */
export type UsageModeChange = "changed" | "unchanged" | "notEligible"

const ErrorCode = Schema.Struct({ code: Schema.String })

/** The error codes of `set_account_monthly_usage_mode` that are not failures of the call. */
const usageModeRefusals: Readonly<Record<string, UsageModeChange>> = {
  MONTHLY_USAGE_MODE_UNCHANGED: "unchanged",
  MONTHLY_USAGE_MODE_NOT_ELIGIBLE: "notEligible"
}

/**
 * A failed registry call. It carries no request URL or response body: the URL of
 * `POST /v1/login/oauth2` holds the GitHub access token.
 */
export class RegistryError extends Data.TaggedError("RegistryError")<{
  readonly operation: string
  readonly reason: string
  readonly status: number | undefined
}> {
  override get message(): string {
    return `Registry call ${this.operation} failed: ${this.reason}`
  }
}

const toRegistryError =
  (operation: string) =>
  (error: HttpClientError.HttpClientError | Schema.SchemaError): RegistryError => {
    if (HttpClientError.isHttpClientError(error)) {
      const reason = error.reason
      return reason._tag === "StatusCodeError"
        ? new RegistryError({ operation, reason: "unexpected status", status: reason.response.status })
        : new RegistryError({ operation, reason: reason._tag, status: undefined })
    }
    return new RegistryError({ operation, reason: "unexpected response body", status: undefined })
  }

export interface RegistryShape {
  /**
   * Exchanges a GitHub OAuth access token for a Golem API token, through the registry's
   * `POST /v1/login/oauth2`. The registry keeps one account per GitHub identity, so the CLI
   * and the billing app receive the same account.
   */
  readonly exchangeGitHubAccessToken: (
    accessToken: Redacted.Redacted<string>
  ) => Effect.Effect<TokenWithSecret, RegistryError>
  readonly getAccount: (
    token: Redacted.Redacted<string>,
    accountId: string
  ) => Effect.Effect<Account, RegistryError>
  /**
   * Moves the account to the plan, through `PUT /v1/accounts/{account_id}/plan`. The registry
   * answers 409 when `revision` is not the current revision of the account.
   */
  readonly setAccountPlan: (
    token: Redacted.Redacted<string>,
    account: Account,
    planId: string
  ) => Effect.Effect<Account, RegistryError>
  /** The monthly usage mode of the account, through `GET /v1/accounts/{account_id}/limits`. */
  readonly getLimits: (
    token: Redacted.Redacted<string>,
    accountId: string
  ) => Effect.Effect<AccountLimits, RegistryError>
  /**
   * Enters or leaves paid-overage mode, through `PUT /v1/accounts/{account_id}/monthly-usage-mode`.
   * Only a token of the owner of the account can enter it; the registry records the consent.
   */
  readonly setMonthlyUsageMode: (
    token: Redacted.Redacted<string>,
    accountId: string,
    mode: MonthlyUsageMode
  ) => Effect.Effect<UsageModeChange, RegistryError>
}

export class Registry extends Context.Service<Registry, RegistryShape>()(
  "golem-cloud-billing/Registry"
) {}

export const layer: Layer.Layer<Registry, never, BillingConfig | HttpClient.HttpClient> = Layer.effect(
  Registry,
  Effect.gen(function* () {
    const { registryUrl } = yield* BillingConfig
    const baseUrl = registryUrl.href.replace(/\/$/, "")
    const rawClient = yield* HttpClient.HttpClient
    const client = rawClient.pipe(HttpClient.filterStatusOk)

    const exchangeGitHubAccessToken = (accessToken: Redacted.Redacted<string>) =>
      client
        .post(`${baseUrl}/v1/login/oauth2`, {
          urlParams: { provider: "github", "access-token": Redacted.value(accessToken) },
          acceptJson: true
        })
        .pipe(
          Effect.flatMap(HttpIncomingMessage.schemaBodyJson(TokenWithSecret)),
          Effect.mapError(toRegistryError("login_oauth2")),
          // A client span records the full request URL, which holds the access token.
          Effect.provideService(HttpClient.TracerDisabledWhen, () => true)
        )

    const getAccount = (token: Redacted.Redacted<string>, accountId: string) =>
      client
        .get(`${baseUrl}/v1/accounts/${encodeURIComponent(accountId)}`, {
          headers: { authorization: `Bearer ${Redacted.value(token)}` },
          acceptJson: true
        })
        .pipe(
          Effect.flatMap(HttpIncomingMessage.schemaBodyJson(Account)),
          Effect.mapError(toRegistryError("get_account"))
        )

    const setAccountPlan = (token: Redacted.Redacted<string>, account: Account, planId: string) =>
      client
        .put(`${baseUrl}/v1/accounts/${encodeURIComponent(account.id)}/plan`, {
          headers: { authorization: `Bearer ${Redacted.value(token)}` },
          acceptJson: true,
          body: HttpBody.jsonUnsafe({ currentRevision: account.revision, plan: planId })
        })
        .pipe(
          Effect.flatMap(HttpIncomingMessage.schemaBodyJson(Account)),
          Effect.mapError(toRegistryError("set_account_plan"))
        )

    const getLimits = (token: Redacted.Redacted<string>, accountId: string) =>
      client
        .get(`${baseUrl}/v1/accounts/${encodeURIComponent(accountId)}/limits`, {
          headers: { authorization: `Bearer ${Redacted.value(token)}` },
          acceptJson: true
        })
        .pipe(
          Effect.flatMap(HttpIncomingMessage.schemaBodyJson(AccountLimits)),
          Effect.mapError(toRegistryError("get_account_limits"))
        )

    const setMonthlyUsageMode = (token: Redacted.Redacted<string>, accountId: string, mode: MonthlyUsageMode) => {
      const operation = "set_account_monthly_usage_mode"
      return rawClient
        .put(`${baseUrl}/v1/accounts/${encodeURIComponent(accountId)}/monthly-usage-mode`, {
          headers: { authorization: `Bearer ${Redacted.value(token)}` },
          acceptJson: true,
          body: HttpBody.jsonUnsafe({ mode })
        })
        .pipe(
          Effect.mapError(toRegistryError(operation)),
          Effect.flatMap((response): Effect.Effect<UsageModeChange, RegistryError> => {
            if (response.status >= 200 && response.status < 300) {
              return Effect.succeed("changed")
            }
            const unexpected = new RegistryError({ operation, reason: "unexpected status", status: response.status })
            if (response.status !== 400) {
              return Effect.fail(unexpected)
            }
            return HttpIncomingMessage.schemaBodyJson(ErrorCode)(response).pipe(
              Effect.mapError(toRegistryError(operation)),
              Effect.flatMap(({ code }) => {
                const change = usageModeRefusals[code]
                return change === undefined ? Effect.fail(unexpected) : Effect.succeed(change)
              })
            )
          })
        )
    }

    return { exchangeGitHubAccessToken, getAccount, setAccountPlan, getLimits, setMonthlyUsageMode }
  })
)
