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
}

export class Registry extends Context.Service<Registry, RegistryShape>()(
  "golem-cloud-billing/Registry"
) {}

export const layer: Layer.Layer<Registry, never, BillingConfig | HttpClient.HttpClient> = Layer.effect(
  Registry,
  Effect.gen(function* () {
    const { registryUrl } = yield* BillingConfig
    const baseUrl = registryUrl.href.replace(/\/$/, "")
    const client = (yield* HttpClient.HttpClient).pipe(HttpClient.filterStatusOk)

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

    return { exchangeGitHubAccessToken, getAccount, setAccountPlan }
  })
)
