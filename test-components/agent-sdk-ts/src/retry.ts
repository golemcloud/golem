// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  BaseAgent,
  agent,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  atomically,
  withRetryPolicy,
} from '@golemcloud/golem-ts-sdk';

@agent()
class RetryTest extends BaseAgent {
  constructor(private readonly _name: string) {
    super();
  }

  async withRetryPolicyTest(host: string, port: number): Promise<boolean> {
    const policy = NamedPolicy.named('retry-test', Policy.immediate().maxRetries(10));
    return withRetryPolicy(policy, () =>
      atomically(async () => {
        const response = await fetch(`http://${host}:${port}/attempt`);
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}`);
        }
        return true;
      }),
    );
  }

  async withStatusRetryPolicyTest(host: string, port: number): Promise<boolean> {
    const policy = NamedPolicy.named(
      'status-retry-test',
      Policy.immediate().maxRetries(10),
    ).appliesWhen(Predicate.eq(Props.statusCode, 500));

    return withRetryPolicy(policy, async () => {
      const response = await fetch(`http://${host}:${port}/attempt`);
      return response.ok;
    });
  }

  /**
   * Plain fetch + throw on !ok, with
   * NO `withRetryPolicy` and NO `atomically`.  All retry behaviour must
   * come from a `retryPolicyDefaults`-style policy defined at the
   * environment level (i.e. supplied to the executor through the
   * `EnvironmentStateService`).
   */
  async manifestStatusRetryTest(host: string, port: number): Promise<boolean> {
    const response = await fetch(`http://${host}:${port}/attempt`);
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    return true;
  }
}
