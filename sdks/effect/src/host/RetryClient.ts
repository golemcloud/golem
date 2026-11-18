/**
 * Host service for `golem:api/retry@1.5.0`. Wraps the synchronous host
 * functions (`get-retry-policies`, `get-retry-policy-by-name`,
 * `resolve-retry-policy`, `set-retry-policy`, `remove-retry-policy`) and
 * the `golem:api/retry-scope@1.5.0` resource into an Effect-typed surface so SDK code can reach them via DI rather
 * than importing the WIT specifier directly.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as RetryHost from "golem:api/retry@1.5.0"
import * as RetryScopeHost from "golem:api/retry-scope@1.5.0"
import process from "node:process"

export interface RetryClientShape {
  readonly nextRetryDelay: typeof RetryHost.nextRetryDelay
  readonly now: () => bigint
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
  /**
   * Opens a `golem:api/retry-scope.retry-policy-scope` and returns the function that drops it.
   */
  readonly openRetryPolicyScope: (policy: RetryHost.NamedRetryPolicy) => () => void
}

export class RetryClient extends Context.Service<RetryClient, RetryClientShape>()(
  "effect-golem/host/Retry",
) {}

export const RetryLive: Layer.Layer<RetryClient> = Layer.succeed(
  RetryClient,
  RetryClient.of({
    nextRetryDelay: (...args) => RetryHost.nextRetryDelay(...args),
    now: () => process.hrtime.bigint(),
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
    openRetryPolicyScope: (policy) => {
      const scope = new RetryScopeHost.RetryPolicyScope(policy) as unknown as {
        [Symbol.dispose](): void
      }
      return () => scope[Symbol.dispose]()
    },
  }),
)
