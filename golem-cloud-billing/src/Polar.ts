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

export interface PolarShape {
  /** Sends usage events to `POST /v1/events/ingest`. */
  readonly ingestEvents: (events: ReadonlyArray<PolarEvent>) => Effect.Effect<void, PolarError>
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

    return { ingestEvents }
  })
)
