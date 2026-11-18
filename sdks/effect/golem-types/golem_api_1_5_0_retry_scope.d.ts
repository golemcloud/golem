declare module 'golem:api/retry-scope@1.5.0' {
  import * as golemApi150Retry from 'golem:api/retry@1.5.0';
  export class RetryPolicyScope {
    constructor(policy: NamedRetryPolicy);
  }
  export type NamedRetryPolicy = golemApi150Retry.NamedRetryPolicy;
}
