/**
 * Host service for the promise-rendezvous subset of
 * `golem:api/host@1.5.0`:
 *
 * - `createPromise` — allocate a new host promise.
 * - `getPromise` — open a `GetPromiseResult` handle for a promise id.
 * - `completePromise` — complete a promise with a payload.
 *
 * Used by `src/agents.ts` (`Promises.create` / `poll` / `await` /
 * `complete`) and by `src/webhook.ts` (which composes
 * `createPromise` with `golem:agent/host.createWebhook`).
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"

export interface PromiseClientShape {
  /** Mirrors `golem:api/host.createPromise`. */
  readonly createPromise: () => ApiHost.PromiseId
  /** Mirrors `golem:api/host.getPromise`. */
  readonly getPromise: (promiseId: ApiHost.PromiseId) => ApiHost.GetPromiseResult
  /** Mirrors `golem:api/host.completePromise`. */
  readonly completePromise: (promiseId: ApiHost.PromiseId, payload: Uint8Array) => boolean
}

export class PromiseClient extends Context.Service<PromiseClient, PromiseClientShape>()(
  "effect-golem/host/Promise",
) {}

export const PromiseLive: Layer.Layer<PromiseClient> = Layer.succeed(
  PromiseClient,
  PromiseClient.of({
    createPromise: () => ApiHost.createPromise(),
    getPromise: (id) => ApiHost.getPromise(id),
    completePromise: (id, payload) => ApiHost.completePromise(id, payload),
  }),
)
