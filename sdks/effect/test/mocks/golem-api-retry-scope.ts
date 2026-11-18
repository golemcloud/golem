/**
 * Runtime mock for `golem:api/retry-scope@1.5.0`. Each scope records when it opens and closes,
 * so tests can check that the SDK holds a scope instead of changing the agent-wide policies.
 */
import type { NamedRetryPolicy } from "./golem-api-retry.js"

const events: string[] = []

export const __reset = (): void => {
  events.length = 0
}

export const __events = (): ReadonlyArray<string> => [...events]

export class RetryPolicyScope {
  constructor(private readonly policy: NamedRetryPolicy) {
    events.push(`open ${policy.name}`)
  }

  [Symbol.dispose](): void {
    events.push(`close ${this.policy.name}`)
  }
}
