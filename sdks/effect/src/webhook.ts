import { Effect, Schema } from "effect"
import * as AgentHost from "golem:agent/host@1.5.0"
import { AgentsHostError, Promises } from "./agents.js"
import type { PromiseId } from "./agents.js"

/**
 * Webhook integration on top of `golem:agent/host@1.5.0.create-webhook`.
 *
 * The agent calls {@link create} to allocate a host promise and
 * mint a public POST URL bound to it (paired allocation, not a
 * single atomic host call — see {@link create} for the failure
 * semantics). The agent shares the URL with an external system (for
 * example, by passing it to an outgoing API call) and then suspends
 * on `webhook.await` until the URL is POSTed to. The host completes
 * the underlying promise with the request body, the agent resumes,
 * and the body is exposed as a {@link WebhookPayload}.
 *
 * Wire-compatible with the official `golem-ts-sdk.createWebhook()` /
 * `golem-rust.create_webhook()` factories: both call
 * `golem:api/host.create-promise` followed by
 * `golem:agent/host.create-webhook(promise-id)` and expose the
 * resulting URL string verbatim.
 *
 * Constraints (enforced by the host, surfaced as
 * {@link WebhookHostError}):
 *
 * - the agent type must be currently deployed via an HTTP API (i.e.
 *   it declares `Http.mount(...)` AND the deployment has the agent
 *   listed under `httpApi.deployments.<env>.agents`);
 * - the promise must have been created by the same component that
 *   calls `create-webhook`.
 *
 * The webhook URL contains an HMAC-SHA256-signed promise reference;
 * the SDK never forges or verifies it (entirely host responsibility).
 *
 * Authoring example:
 *
 * ```ts
 * import { Effect, Schema } from "effect"
 * import { defineAgent, Http, method, Webhook } from "effect-golem"
 *
 * const PaymentEvent = Schema.Struct({
 *   id: Schema.String,
 *   status: Schema.String,
 * })
 *
 * defineAgent({
 *   name: "PaymentWatcher",
 *   constructorParams: { name: Schema.String },
 *   http: Http.mount("/watchers/{name}", { webhookSuffix: "/payments" }),
 *   methods: {
 *     waitForPayment: method({
 *       params: {},
 *       success: PaymentEvent,
 *       http: [Http.post("/wait")],
 *     }),
 *   },
 *   impl: () =>
 *     Effect.gen(function* () {
 *       return {
 *         waitForPayment: () =>
 *           Effect.gen(function* () {
 *             const hook = yield* Webhook.create
 *             // ... share `hook.url` with the payment provider ...
 *             const payload = yield* hook.await
 *             return yield* payload.decode(PaymentEvent)
 *           }),
 *       }
 *     }),
 * })
 * ```
 */

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when the host's `create-webhook` call traps. Common causes:
 *
 * - the agent is not deployed via an HTTP API at the moment of the
 *   call;
 * - the promise was created by a different component than the one
 *   calling `create-webhook`.
 */
export class WebhookHostError {
  readonly _tag = "WebhookHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `WebhookHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// WebhookPayload — the HTTP POST body delivered to the webhook URL
// ---------------------------------------------------------------------------

/**
 * The HTTP POST body delivered to the webhook URL. Mirrors the
 * official SDKs' `WebhookRequestPayload` shape.
 */
export class WebhookPayload {
  constructor(readonly bytes: Uint8Array) {}

  /** UTF-8 decode without parsing. */
  text(): string {
    return new TextDecoder().decode(this.bytes)
  }

  /**
   * `JSON.parse` of the UTF-8 body. Throws synchronously if the body
   * is not valid JSON; wrap in `Effect.try` (or use {@link decode})
   * to surface as a typed Effect failure.
   */
  json<T = unknown>(): T {
    return JSON.parse(this.text()) as T
  }

  /**
   * Parse the body as JSON and validate the parsed value against
   * `schema`. Failures surface as either {@link WebhookDecodeError}
   * (when the body is not valid JSON) or `Schema.SchemaError` (when
   * JSON parses but does not match the schema).
   *
   * This is the recommended decoding path; prefer it over the
   * synchronous {@link json} helper, which throws.
   */
  decode<S extends Schema.Top>(
    schema: S,
  ): Effect.Effect<S["Type"], Schema.SchemaError | WebhookDecodeError, S["DecodingServices"]> {
    const bytes = this.bytes
    return Effect.gen(function* () {
      const json = yield* Effect.try({
        try: () => JSON.parse(new TextDecoder().decode(bytes)) as unknown,
        catch: (e) => new WebhookDecodeError(e),
      })
      return yield* Schema.decodeUnknownEffect(schema)(json)
    })
  }
}

/**
 * Raised when {@link WebhookPayload.decode} cannot parse the body as
 * JSON. Schema-level decode failures surface as
 * `effect/Schema.SchemaError` instead.
 */
export class WebhookDecodeError {
  readonly _tag = "WebhookDecodeError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `WebhookDecodeError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Webhook handle
// ---------------------------------------------------------------------------

/**
 * A live webhook handle. Carries the public POST URL, the underlying
 * promise id (so it can be persisted or sent across an RPC boundary),
 * and the `await` / `poll` Effects.
 *
 * Effect-equivalent of the `WebhookHandler` returned by the official
 * SDKs' `createWebhook()` / `create_webhook()` factories — except
 * `await` and `poll` are Effects rather than `PromiseLike` /
 * `IntoFuture` shims, so durability, oplog replay, and Effect-style
 * interruption all work for free.
 */
export interface WebhookHandle {
  /** The public POST URL minted by the host. */
  readonly url: string
  /** The underlying host promise id. */
  readonly promiseId: PromiseId
  /** Suspend until the URL is POSTed to. Resolves to the body. */
  readonly await: Effect.Effect<WebhookPayload, AgentsHostError>
  /** Non-blocking poll — `undefined` until the URL has been POSTed to. */
  readonly poll: Effect.Effect<WebhookPayload | undefined, AgentsHostError>
}

// ---------------------------------------------------------------------------
// Host wiring (with vitest-only swap helpers, mirroring src/agents.ts)
// ---------------------------------------------------------------------------

let createWebhookImpl: (id: PromiseId) => string = (id) => AgentHost.createWebhook(id)

/** @internal */
export const __setCreateWebhookForTest = (fn: (id: PromiseId) => string): void => {
  createWebhookImpl = fn
}
/** @internal */
export const __resetCreateWebhookForTest = (): void => {
  createWebhookImpl = (id) => AgentHost.createWebhook(id)
}

// ---------------------------------------------------------------------------
// Public factory
// ---------------------------------------------------------------------------

/**
 * Allocate a fresh host promise, then mint a public POST URL bound
 * to it. The returned {@link WebhookHandle} bundles both plus an
 * Effect that suspends until the URL is POSTed to.
 *
 * Equivalent to:
 *
 * ```
 * const id  = yield* Agents.Promises.create
 * const url = host.createWebhook(id)
 * ```
 *
 * **Failure semantics.** This is two host calls, not one. If
 * `create-webhook` traps after `create-promise` succeeded (for
 * example because the agent type is not currently deployed via an
 * HTTP API), the unused promise stays in the host's table; it has no
 * URL pointing at it and will never be completed. The returned
 * Effect surfaces the trap as {@link WebhookHostError} verbatim and
 * does NOT attempt to garbage-collect the promise (the host has no
 * "delete promise" call). This matches the failure shape of the
 * official `golem-ts-sdk` / `golem-rust` SDKs.
 *
 * Wire-compatible with `golem-ts-sdk.createWebhook()` /
 * `golem-rust.create_webhook()`.
 */
export const create: Effect.Effect<WebhookHandle, AgentsHostError | WebhookHostError> = Effect.gen(
  function* () {
    const promiseId = yield* Promises.create
    const url = yield* Effect.try({
      try: () => createWebhookImpl(promiseId),
      catch: (e) => new WebhookHostError(e),
    })
    const await_ = Promises.await(promiseId).pipe(Effect.map((bytes) => new WebhookPayload(bytes)))
    const poll_ = Promises.poll(promiseId).pipe(
      Effect.map((bytes) => (bytes === undefined ? undefined : new WebhookPayload(bytes))),
    )
    return { url, promiseId, await: await_, poll: poll_ }
  },
)
