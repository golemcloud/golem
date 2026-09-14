/**
 * Host service for `golem:api/retry@1.5.0`. Wraps the synchronous host
 * functions (`get-retry-policies`, `get-retry-policy-by-name`,
 * `resolve-retry-policy`, `set-retry-policy`, `remove-retry-policy`)
 * into an Effect-typed surface so SDK code can reach them via DI rather
 * than importing the WIT specifier directly.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as RetryHost from "golem:api/retry@1.5.0"

export interface RetryClientShape {
  /** Mirrors `golem:api/retry.get-retry-policies`. */
  readonly getRetryPolicies: () => ReadonlyArray<RetryHost.NamedRetryPolicy>
  /** Mirrors `golem:api/retry.get-retry-policy-by-name`. */
  readonly getRetryPolicyByName: (name: string) => RetryHost.NamedRetryPolicy | undefined
  /** Mirrors `golem:api/retry.resolve-retry-policy`. */
  readonly resolveRetryPolicy: (
    verb: string,
    nounUri: string,
    properties: ReadonlyArray<readonly [string, RetryHost.PredicateValue]>,
  ) => RetryHost.RetryPolicy | undefined
  /** Mirrors `golem:api/retry.set-retry-policy`. */
  readonly setRetryPolicy: (policy: RetryHost.NamedRetryPolicy) => void
  /** Mirrors `golem:api/retry.remove-retry-policy`. */
  readonly removeRetryPolicy: (name: string) => void
}

export class RetryClient extends Context.Service<RetryClient, RetryClientShape>()(
  "effect-golem/host/Retry",
) {}

export const RetryLive: Layer.Layer<RetryClient> = Layer.succeed(
  RetryClient,
  RetryClient.of({
    getRetryPolicies: () => RetryHost.getRetryPolicies(),
    getRetryPolicyByName: (name) => RetryHost.getRetryPolicyByName(name),
    resolveRetryPolicy: (verb, nounUri, properties) =>
      RetryHost.resolveRetryPolicy(
        verb,
        nounUri,
        properties.map(([k, v]) => [k, v]) as Array<[string, RetryHost.PredicateValue]>,
      ),
    setRetryPolicy: (policy) => RetryHost.setRetryPolicy(policy),
    removeRetryPolicy: (name) => RetryHost.removeRetryPolicy(name),
  }),
)
