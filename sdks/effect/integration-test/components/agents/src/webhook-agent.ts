/**
 * WebhookAgent — exercises the `Webhook` namespace end-to-end against
 * a real Golem runtime.
 *
 * The agent declares an HTTP mount + `webhookSuffix`, which is what
 * makes `Webhook.create` legal on the host side (calling
 * `create-webhook` from an agent that is NOT deployed via an HTTP API
 * traps).
 *
 * Two methods drive the lifecycle in two CLI calls so the harness can
 * see the URL before the suspension blocks the `wait-for-event`
 * invocation:
 *
 * - `prime()` allocates a fresh `Webhook`, stashes the handle in a
 *   per-instance Ref, logs+returns the URL.
 * - `waitForEvent()` reads the stashed handle and `yield*`s its
 *   `await` Effect, returning the round-tripped POST body.
 *
 * Drill (run from `integration-test`):
 *
 * ```
 * URL=$(golem -L agent invoke -n 'WebhookAgent("demo")' prime | grep -oE 'http[^ "]+')
 * golem -L agent invoke -n 'WebhookAgent("demo")' wait-for-event &
 * sleep 1
 * curl -X POST -d '{"hello":"world"}' "$URL"
 * wait      # the background invoke now returns "{\"hello\":\"world\"}"
 * ```
 */
import { Effect, Ref, Schema } from "effect"
import { defineAgent, Http, method, Webhook } from "effect-golem"

interface PrimedWebhook {
  readonly url: string
  readonly handle: Webhook.WebhookHandle
}

/** Typed domain failure surfaced by `prime` / `waitForEvent`. */
const WebhookAgentError = Schema.Struct({
  code: Schema.Literals(["already-primed", "not-primed"]),
  message: Schema.String,
})

export const WebhookAgent = defineAgent({
  name: "WebhookAgent",
  description: "Exercises Webhook.create + Webhook.<handle>.await round-trips.",
  mode: "durable",
  constructorParams: { name: Schema.String },
  http: Http.mount("/webhook-agents/{name}", {
    cors: ["*"],
    webhookSuffix: "/inbox",
  }),
  methods: {
    /** Mint a webhook URL and stash the handle for `waitForEvent`. */
    prime: method({
      params: {},
      success: Schema.Struct({ url: Schema.String }),
      error: WebhookAgentError,
      description: "Allocate a host promise + signed webhook URL.",
      http: [Http.post("/prime")],
    }),
    /** Suspend on the previously primed webhook; resume when POSTed to. */
    waitForEvent: method({
      params: {},
      success: Schema.Struct({ url: Schema.String, body: Schema.String }),
      error: WebhookAgentError,
      description: "Suspend until the primed webhook is POSTed to; return the body.",
      http: [Http.post("/wait-for-event")],
    }),
    /** Diagnostic: returns whether a webhook is currently primed. */
    isPrimed: method({
      params: {},
      success: Schema.Boolean,
      http: [Http.get("/is-primed")],
    }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    yield* Effect.logInfo("WebhookAgent constructed").pipe(Effect.annotateLogs({ name }))
    const slot = yield* Ref.make<PrimedWebhook | null>(null)
    return {
      prime: () =>
        Effect.gen(function* () {
          const existing = yield* Ref.get(slot)
          if (existing !== null) {
            return yield* Effect.fail({
              code: "already-primed" as const,
              message: `WebhookAgent("${name}") already has a primed webhook awaiting POST — call waitForEvent first`,
            })
          }
          // Webhook.create + slot.set are infrastructure — escalate
          // any AgentsHostError / WebhookHostError to defects so they
          // do not pollute the typed domain error channel.
          const handle = yield* Webhook.create.pipe(Effect.orDie)
          yield* Ref.set(slot, { url: handle.url, handle })
          yield* Effect.logInfo("webhook primed").pipe(
            Effect.annotateLogs({ name, url: handle.url }),
          )
          return { url: handle.url }
        }),
      waitForEvent: () =>
        Effect.gen(function* () {
          const primed = yield* Ref.get(slot)
          if (primed === null) {
            return yield* Effect.fail({
              code: "not-primed" as const,
              message: "webhook not primed — call `prime` first to mint the URL",
            })
          }
          // `await` carries an AgentsHostError typed failure; treat
          // it as infrastructure and escalate to defect.
          const payload = yield* primed.handle.await.pipe(Effect.orDie)
          yield* Ref.set(slot, null)
          return { url: primed.url, body: payload.text() }
        }),
      isPrimed: () => Ref.get(slot).pipe(Effect.map((p) => p !== null)),
    }
  }),
)
