import { createPolarCore } from "@polar-sh/sdk/2026-10"
import { createCheckouts } from "@polar-sh/sdk/2026-10/services/checkouts"
import { createCustomerSessions } from "@polar-sh/sdk/2026-10/services/customer_sessions"
import { Config, Context, Data, Effect, Layer, Redacted } from "effect"
import { HttpBody, HttpClient, HttpClientError } from "effect/http"

export interface PolarConfigShape {
  /** The base URL of the Polar API: `https://sandbox-api.polar.sh` or `https://api.polar.sh`. */
  readonly apiUrl: URL
  /** An organization access token of Polar. */
  readonly accessToken: Redacted.Redacted<string>
}

export class PolarConfig extends Context.Service<PolarConfig, PolarConfigShape>()(
  "golem-cloud-billing/PolarConfig"
) {}

export const polarConfig: Config.Config<PolarConfigShape> = Config.all({
  apiUrl: Config.URL("POLAR_API_URL"),
  accessToken: Config.Redacted("POLAR_ACCESS_TOKEN")
})

export const configLayer: Layer.Layer<PolarConfig, Config.ConfigError> = Layer.effect(PolarConfig, polarConfig)

/** A usage event for a meter of Polar. */
export interface PolarEvent {
  readonly name: string
  /** The Golem account ID, which is the external ID of the Polar customer. */
  readonly externalCustomerId: string
  /** Polar skips an event whose external ID it has already. */
  readonly externalId: string
  readonly metadata: Readonly<Record<string, string | number>>
}

/** A failed Polar call. It carries no request body or access token. */
export class PolarError extends Data.TaggedError("PolarError")<{
  readonly operation: string
  readonly reason: string
  readonly status: number | undefined
}> {
  override get message(): string {
    return `Polar call ${this.operation} failed: ${this.reason}`
  }
}

const toPolarError =
  (operation: string) =>
  (error: HttpClientError.HttpClientError): PolarError => {
    const reason = error.reason
    return reason._tag === "StatusCodeError"
      ? new PolarError({ operation, reason: "unexpected status", status: reason.response.status })
      : new PolarError({ operation, reason: reason._tag, status: undefined })
  }

/** A checkout of the paid plan for a Golem account. */
export interface CheckoutRequest {
  readonly productId: string
  /** The Golem account ID. Polar creates the customer with this external ID, or reuses it. */
  readonly externalCustomerId: string
  readonly customerEmail: string
  readonly customerName: string | undefined
  /** The IP address of the browser, from which Polar shows prices in the local currency. */
  readonly customerIpAddress: string | undefined
  /** Polar replaces `{CHECKOUT_ID}` in the URL with the ID of the checkout. */
  readonly successUrl: string
  /** The back button of the checkout page goes here. */
  readonly returnUrl: string
}

/** The SDK errors with an HTTP status: `PolarClientError` and `PolarServerError`. */
const statusOf = (error: unknown): number | undefined =>
  typeof error === "object" && error !== null && "statusCode" in error && typeof error.statusCode === "number"
    ? error.statusCode
    : undefined

const sdkCall = <A>(operation: string, call: () => Promise<A>): Effect.Effect<A, PolarError> =>
  Effect.tryPromise({
    try: call,
    catch: (error) => {
      const status = statusOf(error)
      return new PolarError({
        operation,
        reason: status === undefined ? "request failed" : "unexpected status",
        status
      })
    }
  })

export interface PolarShape {
  /** Sends usage events to `POST /v1/events/ingest`. */
  readonly ingestEvents: (events: ReadonlyArray<PolarEvent>) => Effect.Effect<void, PolarError>
  /** Creates a checkout session and returns the URL of its checkout page. */
  readonly createCheckout: (request: CheckoutRequest) => Effect.Effect<string, PolarError>
  /**
   * Creates a customer session for the Polar customer with the external ID, and returns the
   * URL of its customer portal: payment method, invoices, cancellation.
   */
  readonly createCustomerPortalUrl: (request: {
    readonly externalCustomerId: string
    readonly returnUrl: string
  }) => Effect.Effect<string, PolarError>
}

export class Polar extends Context.Service<Polar, PolarShape>()("golem-cloud-billing/Polar") {}

export const layer: Layer.Layer<Polar, never, PolarConfig | HttpClient.HttpClient> = Layer.effect(
  Polar,
  Effect.gen(function* () {
    const { apiUrl, accessToken } = yield* PolarConfig
    const baseUrl = apiUrl.href.replace(/\/$/, "")
    const client = (yield* HttpClient.HttpClient).pipe(HttpClient.filterStatusOk)

    const ingestEvents = (events: ReadonlyArray<PolarEvent>) =>
      client
        .post(`${baseUrl}/v1/events/ingest`, {
          headers: { authorization: `Bearer ${Redacted.value(accessToken)}` },
          acceptJson: true,
          body: HttpBody.jsonUnsafe({
            events: events.map((event) => ({
              name: event.name,
              external_customer_id: event.externalCustomerId,
              external_id: event.externalId,
              metadata: event.metadata
            }))
          })
        })
        .pipe(Effect.asVoid, Effect.mapError(toPolarError("ingest_events")))

    const core = createPolarCore({ accessToken: Redacted.value(accessToken), baseUrl })
    const checkouts = createCheckouts(core)
    const customerSessions = createCustomerSessions(core)

    const createCheckout = (request: CheckoutRequest) =>
      sdkCall("create_checkout", () =>
        checkouts({
          products: [request.productId],
          external_customer_id: request.externalCustomerId,
          customer_email: request.customerEmail,
          customer_name: request.customerName ?? null,
          customer_ip_address: request.customerIpAddress ?? null,
          success_url: request.successUrl,
          return_url: request.returnUrl,
          allow_discount_codes: true
        })
      ).pipe(Effect.map((checkout) => checkout.url))

    const createCustomerPortalUrl = (request: { readonly externalCustomerId: string; readonly returnUrl: string }) =>
      sdkCall("create_customer_session", () =>
        customerSessions({ external_customer_id: request.externalCustomerId, return_url: request.returnUrl })
      ).pipe(Effect.map((session) => session.customer_portal_url))

    return { ingestEvents, createCheckout, createCustomerPortalUrl }
  })
)
